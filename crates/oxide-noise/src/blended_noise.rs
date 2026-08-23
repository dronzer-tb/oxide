//! `BlendedNoise`: the pre-1.18 terrain noise the modern generator still uses as its
//! `minecraft:old_blended_noise` node -- three legacy-initialised [`PerlinNoise`] stacks
//! (a min limit, a max limit, and a main selector) blended per position.
//!
//! Ported 2026-08-23 from decompiled 26.2
//! `net.minecraft.world.level.levelgen.synth.BlendedNoise`: octave counts (16/16/8), the
//! `684.412` multipliers, the `/10 + 1) / 2` selector, and the final
//! `Mth.clampedLerp(factor, min / 512, max / 512) / 128`. Note 26.2's `Mth.clampedLerp` takes
//! the factor *first* -- an older signature had it last, which would silently swap the
//! arguments here.

use crate::perlin_noise::PerlinNoise;
use crate::random::WorldRandom;

/// The per-node scaling constants from a `minecraft:old_blended_noise` density function.
/// Vanilla bakes these into each `BlendedNoise` instance, but every instance in a router is
/// seeded from the same `"minecraft:terrain"` random, so one set of Perlin stacks serves them
/// all -- see [`BlendedNoise::compute`].
#[derive(Debug, Clone, Copy)]
pub struct BlendedNoiseParams {
    pub xz_scale: f64,
    pub y_scale: f64,
    pub xz_factor: f64,
    pub y_factor: f64,
    pub smear_scale_multiplier: f64,
}

#[derive(Debug, Clone)]
pub struct BlendedNoise {
    min_limit: PerlinNoise,
    max_limit: PerlinNoise,
    main: PerlinNoise,
}

impl BlendedNoise {
    /// `random` must be the router's terrain random: `positional.from_hash_of("minecraft:terrain")`
    /// for a Xoroshiro world, or a legacy source seeded with the world seed. The three stacks
    /// are drawn from it in order (min, max, main), so the order here is part of the seeding.
    pub fn new(random: &mut WorldRandom) -> Self {
        Self {
            min_limit: PerlinNoise::create_legacy_for_blended_noise(random, -15, vec![1.0; 16]),
            max_limit: PerlinNoise::create_legacy_for_blended_noise(random, -15, vec![1.0; 16]),
            main: PerlinNoise::create_legacy_for_blended_noise(random, -7, vec![1.0; 8]),
        }
    }

    pub fn compute(&self, params: &BlendedNoiseParams, x: f64, y: f64, z: f64) -> f64 {
        let xz_multiplier = 684.412 * params.xz_scale;
        let y_multiplier = 684.412 * params.y_scale;

        let limit_x = x * xz_multiplier;
        let limit_y = y * y_multiplier;
        let limit_z = z * xz_multiplier;
        let main_x = limit_x / params.xz_factor;
        let main_y = limit_y / params.y_factor;
        let main_z = limit_z / params.xz_factor;
        let limit_smear = y_multiplier * params.smear_scale_multiplier;
        let main_smear = limit_smear / params.y_factor;

        // The main stack selects how far to blend between the two limit stacks. Its 8 octaves
        // are walked highest-frequency-first via `get_octave_noise`, halving `pow` each step.
        let mut main_value = 0.0;
        let mut pow = 1.0;
        for i in 0..8 {
            if let Some(noise) = self.main.get_octave_noise(i) {
                main_value += noise.noise_with_scale(
                    wrap(main_x * pow),
                    wrap(main_y * pow),
                    wrap(main_z * pow),
                    main_smear * pow,
                    main_y * pow,
                ) / pow;
            }
            pow /= 2.0;
        }

        let factor = (main_value / 10.0 + 1.0) / 2.0;
        // Vanilla skips a limit stack entirely once the selector saturates -- not just an
        // optimisation, since a skipped stack contributes nothing to the lerp it would lose.
        let is_max = factor >= 1.0;
        let is_min = factor <= 0.0;

        let mut blend_min = 0.0;
        let mut blend_max = 0.0;
        pow = 1.0;
        for i in 0..16 {
            let wx = wrap(limit_x * pow);
            let wy = wrap(limit_y * pow);
            let wz = wrap(limit_z * pow);
            let y_scale_pow = limit_smear * pow;
            if !is_max {
                if let Some(noise) = self.min_limit.get_octave_noise(i) {
                    blend_min +=
                        noise.noise_with_scale(wx, wy, wz, y_scale_pow, limit_y * pow) / pow;
                }
            }
            if !is_min {
                if let Some(noise) = self.max_limit.get_octave_noise(i) {
                    blend_max +=
                        noise.noise_with_scale(wx, wy, wz, y_scale_pow, limit_y * pow) / pow;
                }
            }
            pow /= 2.0;
        }

        clamped_lerp(factor, blend_min / 512.0, blend_max / 512.0) / 128.0
    }
}

/// `Mth.clampedLerp(factor, min, max)` as of 26.2 -- factor first.
fn clamped_lerp(factor: f64, min: f64, max: f64) -> f64 {
    if factor < 0.0 {
        min
    } else if factor > 1.0 {
        max
    } else {
        min + factor * (max - min)
    }
}

fn wrap(value: f64) -> f64 {
    crate::improved_noise::wrap_coordinate(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overworld() -> BlendedNoiseParams {
        // `NoiseRouterData.BASE_3D_NOISE_OVERWORLD`.
        BlendedNoiseParams {
            xz_scale: 0.25,
            y_scale: 0.125,
            xz_factor: 80.0,
            y_factor: 160.0,
            smear_scale_multiplier: 8.0,
        }
    }

    #[test]
    fn varies_horizontally() {
        // The bug this replaced returned a constant 0.0, which flattened the nether into a
        // slab: its terrain shape is *only* this noise.
        let mut r = WorldRandom::new(42, false);
        let n = BlendedNoise::new(&mut r);
        let a = n.compute(&overworld(), 0.0, 64.0, 0.0);
        let b = n.compute(&overworld(), 100.0, 64.0, -60.0);
        assert_ne!(a, b);
        assert!(a.is_finite() && b.is_finite());
    }

    #[test]
    fn deterministic_for_same_seed() {
        let mut r1 = WorldRandom::new(7, false);
        let mut r2 = WorldRandom::new(7, false);
        let a = BlendedNoise::new(&mut r1).compute(&overworld(), 12.0, 40.0, -8.0);
        let b = BlendedNoise::new(&mut r2).compute(&overworld(), 12.0, 40.0, -8.0);
        assert_eq!(a, b);
    }

    #[test]
    fn stays_in_a_terrain_sized_range() {
        let mut r = WorldRandom::new(1, false);
        let n = BlendedNoise::new(&mut r);
        for x in (-400..400).step_by(37) {
            for y in (0..256).step_by(31) {
                let v = n.compute(&overworld(), x as f64, y as f64, (x / 2) as f64);
                assert!(
                    v.abs() < 10.0,
                    "blended noise {v} at ({x}, {y}) out of range"
                );
            }
        }
    }
}
