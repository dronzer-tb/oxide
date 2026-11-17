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

use crate::improved_noise::{HashKind, NOISE_BLOCK};
use crate::normal_noise::LANE_CHUNK;
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

    /// [`Self::compute`] at every `(xs[i], ys[i], zs[i])`, bit-identical per lane: the same
    /// per-lane expressions in the same octave order, with each octave's lanes in an inner loop
    /// that reuses the octave's lattice hashes while consecutive lanes stay in one cell.
    pub fn compute_lanes(
        &self,
        params: &BlendedNoiseParams,
        xs: &[f64],
        ys: &[f64],
        zs: &[f64],
        out: &mut [f64],
    ) {
        for start in (0..out.len()).step_by(LANE_CHUNK) {
            let end = (start + LANE_CHUNK).min(out.len());
            self.compute_chunk(
                params,
                &xs[start..end],
                &ys[start..end],
                &zs[start..end],
                &mut out[start..end],
            );
        }
    }

    /// [`Self::compute_lanes`] for at most [`LANE_CHUNK`] lanes.
    fn compute_chunk(
        &self,
        params: &BlendedNoiseParams,
        xs: &[f64],
        ys: &[f64],
        zs: &[f64],
        out: &mut [f64],
    ) {
        let k = out.len();
        debug_assert!(k <= LANE_CHUNK);
        let xz_multiplier = 684.412 * params.xz_scale;
        let y_multiplier = 684.412 * params.y_scale;
        let mut limit_x = [0.0f64; LANE_CHUNK];
        let mut limit_y = [0.0f64; LANE_CHUNK];
        let mut limit_z = [0.0f64; LANE_CHUNK];
        let mut main_x = [0.0f64; LANE_CHUNK];
        let mut main_y = [0.0f64; LANE_CHUNK];
        let mut main_z = [0.0f64; LANE_CHUNK];
        for j in 0..k {
            limit_x[j] = xs[j] * xz_multiplier;
            limit_y[j] = ys[j] * y_multiplier;
            limit_z[j] = zs[j] * xz_multiplier;
            main_x[j] = limit_x[j] / params.xz_factor;
            main_y[j] = limit_y[j] / params.y_factor;
            main_z[j] = limit_z[j] / params.xz_factor;
        }
        let limit_smear = y_multiplier * params.smear_scale_multiplier;
        let main_smear = limit_smear / params.y_factor;

        let mut main_value = [0.0f64; LANE_CHUNK];
        let mut samples = [0.0f64; NOISE_BLOCK];
        let mut pow = 1.0;
        for i in 0..8 {
            if let Some(noise) = self.main.get_octave_noise(i) {
                let mut memo = noise.cell_hashes(HashKind::Blended);
                noise.noise_block_reusing(
                    &main_x[..k],
                    &main_y[..k],
                    &main_z[..k],
                    pow,
                    main_smear * pow,
                    None,
                    &mut memo,
                    &mut samples,
                );
                for j in 0..k {
                    main_value[j] += samples[j] / pow;
                }
            }
            pow /= 2.0;
        }

        let mut factor = [0.0f64; LANE_CHUNK];
        for j in 0..k {
            factor[j] = (main_value[j] / 10.0 + 1.0) / 2.0;
        }
        // Vanilla skips a limit stack for a lane whose selector saturates; gather each stack's
        // lanes (in lane order) so the octave kernel runs over exactly those.
        let mut min_lanes = Compact::new();
        let mut max_lanes = Compact::new();
        for j in 0..k {
            // Negated as the scalar path tests them, so a NaN factor behaves the same.
            let is_max = factor[j] >= 1.0;
            let is_min = factor[j] <= 0.0;
            if !is_max {
                min_lanes.push(j, limit_x[j], limit_y[j], limit_z[j]);
            }
            if !is_min {
                max_lanes.push(j, limit_x[j], limit_y[j], limit_z[j]);
            }
        }
        let mut blend_min = [0.0f64; LANE_CHUNK];
        let mut blend_max = [0.0f64; LANE_CHUNK];
        for (stack, lanes, blend) in [
            (&self.min_limit, &min_lanes, &mut blend_min),
            (&self.max_limit, &max_lanes, &mut blend_max),
        ] {
            let m = lanes.len;
            if m == 0 {
                continue;
            }
            let mut sums = [0.0f64; LANE_CHUNK];
            pow = 1.0;
            for i in 0..16 {
                if let Some(noise) = stack.get_octave_noise(i) {
                    let mut memo = noise.cell_hashes(HashKind::Blended);
                    noise.noise_block_reusing(
                        &lanes.x[..m],
                        &lanes.y[..m],
                        &lanes.z[..m],
                        pow,
                        limit_smear * pow,
                        None,
                        &mut memo,
                        &mut samples,
                    );
                    for c in 0..m {
                        sums[c] += samples[c] / pow;
                    }
                }
                pow /= 2.0;
            }
            for c in 0..m {
                blend[lanes.lane[c] as usize] = sums[c];
            }
        }

        for j in 0..k {
            out[j] = clamped_lerp(factor[j], blend_min[j] / 512.0, blend_max[j] / 512.0) / 128.0;
        }
    }
}

/// The lanes of a [`BlendedNoise::compute_chunk`] that sample one limit stack, compacted.
struct Compact {
    len: usize,
    lane: [u8; LANE_CHUNK],
    x: [f64; LANE_CHUNK],
    y: [f64; LANE_CHUNK],
    z: [f64; LANE_CHUNK],
}

impl Compact {
    fn new() -> Self {
        Self {
            len: 0,
            lane: [0; LANE_CHUNK],
            x: [0.0; LANE_CHUNK],
            y: [0.0; LANE_CHUNK],
            z: [0.0; LANE_CHUNK],
        }
    }

    #[inline(always)]
    fn push(&mut self, lane: usize, x: f64, y: f64, z: f64) {
        let c = self.len;
        self.lane[c] = lane as u8;
        self.x[c] = x;
        self.y[c] = y;
        self.z[c] = z;
        self.len = c + 1;
    }
}

/// An upper bound on `|BlendedNoise::compute(params, ..)|` at any block position (|x|, |y|,
/// |z| < 2^31), or `None` when the parameters could overflow or divide by zero.
///
/// One limit-stack octave with smear `y_scale` is at most `4 + |y_scale|` in magnitude: the y
/// offset fed to the dot products is `dy - dy_faded` with `dy` in `[0, 1)` and
/// `|dy_faded| <= |clamped| + |y_scale|`, `|clamped| <= 1`, while the y fade still uses `dy`,
/// so the lerps stay convex. The limit sums divide each octave by `pow = 2^-i`, and the final
/// `clamped_lerp` either picks one sum or blends the two, so it never exceeds the larger.
pub(crate) fn blended_max_abs(params: &BlendedNoiseParams) -> Option<f64> {
    let all = [
        params.xz_scale,
        params.y_scale,
        params.xz_factor,
        params.y_factor,
        params.smear_scale_multiplier,
    ];
    if all.iter().any(|v| !v.is_finite() || v.abs() > 1e100)
        || params.xz_factor.abs() < 1e-100
        || params.y_factor.abs() < 1e-100
    {
        return None;
    }
    let limit_smear = (684.412 * params.y_scale * params.smear_scale_multiplier).abs();
    let main_smear = limit_smear / params.y_factor.abs();
    // A smear that is nonzero but tiny at the deepest octave would let `clamped / y_scale`
    // overflow, and infinity times a tiny `y_scale` is not finite.
    let deepest = 2f64.powi(-15);
    for smear in [limit_smear, main_smear] {
        if smear != 0.0 && smear * deepest < 1e-200 {
            return None;
        }
    }
    let mut sum = 0.0;
    let mut pow = 1.0f64;
    for _ in 0..16 {
        sum += (4.0 + limit_smear * pow) / pow;
        pow /= 2.0;
    }
    Some(sum / 512.0 / 128.0 * 1.001)
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

    impl BlendedNoise {
        /// The main-stack selector `compute` derives, for checking test coverage.
        fn selector_for_test(&self, params: &BlendedNoiseParams, x: f64, y: f64, z: f64) -> f64 {
            let xz_multiplier = 684.412 * params.xz_scale;
            let y_multiplier = 684.412 * params.y_scale;
            let main_x = x * xz_multiplier / params.xz_factor;
            let main_y = y * y_multiplier / params.y_factor;
            let main_z = z * xz_multiplier / params.xz_factor;
            let main_smear = y_multiplier * params.smear_scale_multiplier / params.y_factor;
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
            (main_value / 10.0 + 1.0) / 2.0
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

    /// The lane-batched noises over real corner columns (49 corners 8 blocks apart, at chunk
    /// corner x/z, overworld parameters) must equal the scalar ones bit for bit, and this is
    /// where the hash-reuse rate is measured rather than assumed.
    #[test]
    fn lanes_are_bit_identical_and_reuse_hashes_on_corner_columns() {
        use crate::improved_noise::hash_stats;
        use crate::normal_noise::NormalNoise;
        let mut r = WorldRandom::new(12345, false);
        let blended = BlendedNoise::new(&mut r);
        let positional = WorldRandom::new(12345, false).fork_positional();
        // (first octave, amplitudes, xz scale, y scale) of overworld noises a corner grid reads.
        let specs: [(i32, Vec<f64>, f64, f64); 6] = [
            (
                -8,
                vec![0.5, 1.0, 2.0, 1.0, 2.0, 1.0, 0.0, 2.0, 0.0],
                1.0,
                2.0 / 3.0,
            ), // cave_cheese
            (-8, vec![1.0], 1.0, 8.0),            // cave_layer
            (-7, vec![0.4, 0.5, 1.0], 0.75, 0.5), // cave_entrance
            (-7, vec![1.0, 1.0], 25.0, 0.3),      // pillar
            (-8, vec![1.0], 1.0, 1.0),            // noodle
            (-7, vec![1.0], 2.666_666_666_666_667, 2.666_666_666_666_667), // noodle_ridge
        ];
        let normals: Vec<(NormalNoise, f64, f64)> = specs
            .iter()
            .enumerate()
            .map(|(i, (first, amps, xz, y))| {
                let mut nr = positional.from_hash_of(&format!("test:{i}"));
                (NormalNoise::create(&mut nr, *first, amps.clone()), *xz, *y)
            })
            .collect();

        let before = hash_stats::local();
        let mut columns = 0;
        for chunk in [(0, 0), (-3, 5), (40, -17), (-200, -90)] {
            for (ix, iz) in [(0, 0), (1, 2), (3, 3), (4, 1)] {
                let x = (chunk.0 * 16 + ix * 4) as f64;
                let z = (chunk.1 * 16 + iz * 4) as f64;
                let ys: Vec<f64> = (0..49).map(|cy| (-64 + cy * 8) as f64).collect();
                let xs = vec![x; 49];
                let zs = vec![z; 49];
                let mut out = vec![0.0; 49];
                blended.compute_lanes(&overworld(), &xs, &ys, &zs, &mut out);
                for (j, v) in out.iter().enumerate() {
                    let expected = blended.compute(&overworld(), x, ys[j], z);
                    assert_eq!(
                        v.to_bits(),
                        expected.to_bits(),
                        "blended at {x} {} {z}",
                        ys[j]
                    );
                }
                for (n, xz, y) in &normals {
                    let sx: Vec<f64> = xs.iter().map(|v| v * xz).collect();
                    let sy: Vec<f64> = ys.iter().map(|v| v * y).collect();
                    let sz: Vec<f64> = zs.iter().map(|v| v * xz).collect();
                    n.get_values(&sx, &sy, &sz, &mut out);
                    for j in 0..49 {
                        let expected = n.get_value(sx[j], sy[j], sz[j]);
                        assert_eq!(out[j].to_bits(), expected.to_bits(), "normal at lane {j}");
                    }
                }
                columns += 1;
            }
        }
        let after = hash_stats::local();
        let d: Vec<u64> = (0..4).map(|i| after[i] - before[i]).collect();
        let rate = |r: u64, c: u64| r as f64 / (r + c) as f64;
        let (perlin, blended_rate) = (rate(d[0], d[1]), rate(d[2], d[3]));
        eprintln!(
            "HASH-REUSE columns={columns} perlin {}/{} = {perlin:.3}, blended {}/{} = {blended_rate:.3}",
            d[0],
            d[0] + d[1],
            d[2],
            d[2] + d[3]
        );
        assert!(perlin > 0.5, "perlin reuse {perlin}");
        assert!(blended_rate > 0.2, "blended reuse {blended_rate}");
    }

    /// Lanes with random (also huge and negative) positions, and parameter sets whose
    /// selector saturates for some lanes and not others, so the compacted limit stacks skip
    /// lanes exactly as the scalar path does.
    #[test]
    fn lanes_are_bit_identical_with_saturating_selectors() {
        use oxide_core::{LegacyRandom, RandomSource};
        let mut r = WorldRandom::new(99, false);
        let blended = BlendedNoise::new(&mut r);
        let mut rng = LegacyRandom::new(0xb1e);
        let param_sets = [
            overworld(),
            BlendedNoiseParams {
                xz_scale: 1.0,
                y_scale: 1.0,
                xz_factor: 1.0,
                y_factor: 1.0,
                smear_scale_multiplier: 8.0,
            },
            BlendedNoiseParams {
                xz_scale: 0.25,
                y_scale: 0.375,
                xz_factor: 80.0,
                y_factor: 60.0,
                smear_scale_multiplier: 0.0,
            },
            BlendedNoiseParams {
                xz_scale: 3.0,
                y_scale: 0.01,
                xz_factor: 0.5,
                y_factor: 0.7,
                smear_scale_multiplier: -4.0,
            },
        ];
        let (mut saw_min, mut saw_max, mut saw_mid) = (false, false, false);
        for params in &param_sets {
            for round in 0..24 {
                let n = 1 + (round * 13) % 130;
                let mag = [64.0, 3.0e4, 3.0e7, 2.0e9][round % 4];
                let xs: Vec<f64> = (0..n)
                    .map(|_| ((rng.next_double() * 2.0 - 1.0) * mag).floor())
                    .collect();
                let ys: Vec<f64> = (0..n)
                    .map(|_| ((rng.next_double() * 2.0 - 1.0) * 400.0).floor())
                    .collect();
                let zs: Vec<f64> = (0..n)
                    .map(|_| ((rng.next_double() * 2.0 - 1.0) * mag).floor())
                    .collect();
                let mut out = vec![0.0; n];
                blended.compute_lanes(params, &xs, &ys, &zs, &mut out);
                for j in 0..n {
                    let expected = blended.compute(params, xs[j], ys[j], zs[j]);
                    assert_eq!(
                        out[j].to_bits(),
                        expected.to_bits(),
                        "lane {j} at {} {} {}",
                        xs[j],
                        ys[j],
                        zs[j]
                    );
                }
                for j in 0..n {
                    let probe =
                        BlendedNoise::selector_for_test(&blended, params, xs[j], ys[j], zs[j]);
                    saw_min |= probe <= 0.0;
                    saw_max |= probe >= 1.0;
                    saw_mid |= probe > 0.0 && probe < 1.0;
                }
            }
        }
        assert!(
            saw_min && saw_max && saw_mid,
            "{saw_min} {saw_max} {saw_mid}"
        );
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
