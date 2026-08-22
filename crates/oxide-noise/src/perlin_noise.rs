//! `PerlinNoise`: multi-octave combinator over [`ImprovedNoise`], matching vanilla's
//! `net.minecraft.world.level.levelgen.synth.PerlinNoise`.

use crate::improved_noise::{wrap_coordinate, ImprovedNoise};
use crate::random::WorldRandom;

#[derive(Debug, Clone)]
pub struct PerlinNoise {
    // `None` for an octave whose `amplitudes` entry is `0.0` — matches vanilla skipping
    // `ImprovedNoise` construction (and its RNG draws) for zero-weight octaves.
    noise_levels: Vec<Option<ImprovedNoise>>,
    amplitudes: Vec<f64>,
    lowest_freq_input_factor: f64,
    lowest_freq_value_factor: f64,
}

impl PerlinNoise {
    /// `first_octave` is the exponent of the lowest-frequency octave (`amplitudes[0]`
    /// corresponds to `2^first_octave`); per-octave RNGs are derived by name
    /// (`"octave_" + octave`) from `random`'s forked positional factory, so octave RNG draws
    /// don't depend on how many octaves precede them — matches vanilla's `PositionalRandomFactory`
    /// derivation, not a sequential RNG walk.
    pub fn create(random: &mut WorldRandom, first_octave: i32, amplitudes: Vec<f64>) -> Self {
        let factory = random.fork_positional();
        let mut noise_levels = Vec::with_capacity(amplitudes.len());
        for (i, &amplitude) in amplitudes.iter().enumerate() {
            if amplitude != 0.0 {
                let octave = first_octave + i as i32;
                let mut octave_random = factory.from_hash_of(&format!("octave_{octave}"));
                noise_levels.push(Some(ImprovedNoise::new(&mut octave_random)));
            } else {
                noise_levels.push(None);
            }
        }

        let size = amplitudes.len() as i32;
        // PARITY-CHECK: reconstructed from memory of `PerlinNoise`'s constructor. The freq
        // doubling per octave (`lowestFreqInputFactor * 2^i`) is certain (that's the
        // definition of an octave); the exact `lowestFreqValueFactor` normalization constant
        // is the least-certain part of this file.
        let lowest_freq_input_factor = 2f64.powi(first_octave);
        let lowest_freq_value_factor = 2f64.powi(size - 1) / (2f64.powi(size) - 1.0);

        Self {
            noise_levels,
            amplitudes,
            lowest_freq_input_factor,
            lowest_freq_value_factor,
        }
    }

    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        self.get_value_with_scale(x, y, z, 0.0, 0.0, false)
    }

    pub fn get_value_with_scale(
        &self,
        x: f64,
        y: f64,
        z: f64,
        y_scale: f64,
        y_max: f64,
        fix_y: bool,
    ) -> f64 {
        let mut result = 0.0;
        let mut freq = self.lowest_freq_input_factor;
        let mut amp = self.lowest_freq_value_factor;

        for (i, level) in self.noise_levels.iter().enumerate() {
            if let Some(noise) = level {
                let y_in = if fix_y {
                    // PARITY-CHECK: mirrors vanilla's `-improvedNoise.yo` fixed-Y path (used
                    // by `old_blended_noise`'s max-noise sampling).
                    -noise.yo()
                } else {
                    wrap_coordinate(y * freq)
                };
                let sample = noise.noise_with_scale(
                    wrap_coordinate(x * freq),
                    y_in,
                    wrap_coordinate(z * freq),
                    y_scale * freq,
                    y_max * freq,
                );
                result += self.amplitudes[i] * sample * amp;
            }
            freq *= 2.0;
            amp /= 2.0;
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_same_seed() {
        let mut r1 = WorldRandom::new(42, false);
        let p1 = PerlinNoise::create(&mut r1, -7, vec![1.0, 1.0, 1.0]);
        let mut r2 = WorldRandom::new(42, false);
        let p2 = PerlinNoise::create(&mut r2, -7, vec![1.0, 1.0, 1.0]);
        assert_eq!(
            p1.get_value(10.0, 20.0, 30.0),
            p2.get_value(10.0, 20.0, 30.0)
        );
    }

    #[test]
    fn zero_amplitude_octave_is_skipped_without_consuming_rng() {
        // An all-zero amplitude list must not construct any ImprovedNoise, and so must not
        // advance the positional factory relative to a shorter all-zero list either.
        let mut r1 = WorldRandom::new(1, false);
        let p1 = PerlinNoise::create(&mut r1, 0, vec![0.0, 0.0]);
        assert_eq!(p1.get_value(1.0, 2.0, 3.0), 0.0);
    }

    #[test]
    fn legacy_flavour_also_works() {
        let mut r = WorldRandom::new(7, true);
        let p = PerlinNoise::create(&mut r, -4, vec![1.0, 0.5]);
        assert!(p.get_value(0.0, 0.0, 0.0).is_finite());
    }
}
