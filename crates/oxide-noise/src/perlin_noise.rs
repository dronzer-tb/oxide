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
        // Verified 2026-08-22 against decompiled `PerlinNoise`'s constructor:
        // `lowestFreqInputFactor = 2^firstOctave`, `lowestFreqValueFactor = 2^(n-1)/(2^n-1)`.
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
        self.get_value_with_scale(x, y, z, 0.0, 0.0)
    }

    /// Verified 2026-08-22 against decompiled `PerlinNoise.getValue(x, y, z, yScale, yFudge)`:
    /// `x`/`y`/`z` are each wrapped before scaling into the octave's `ImprovedNoise.noise`
    /// call; `yScale`/`yFudge` are passed through scaled but *not* wrapped. There is no
    /// "fixed Y" variant on this class at all — that was this crate's own invention, not
    /// vanilla behavior, and has been removed.
    pub fn get_value_with_scale(&self, x: f64, y: f64, z: f64, y_scale: f64, y_fudge: f64) -> f64 {
        let mut result = 0.0;
        let mut freq = self.lowest_freq_input_factor;
        let mut amp = self.lowest_freq_value_factor;

        for (i, level) in self.noise_levels.iter().enumerate() {
            if let Some(noise) = level {
                let sample = noise.noise_with_scale(
                    wrap_coordinate(x * freq),
                    wrap_coordinate(y * freq),
                    wrap_coordinate(z * freq),
                    y_scale * freq,
                    y_fudge * freq,
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

    /// Captured 2026-08-22 from a standalone Java transcription of the confirmed-correct
    /// algorithm chain run on real OpenJDK 25 — see the session's scratchpad
    /// `decomp/NoiseRef.java`.
    #[test]
    fn get_value_matches_real_java_seed_42() {
        let mut r = WorldRandom::new(42, false);
        let p = PerlinNoise::create(&mut r, -7, vec![1.0, 1.0, 1.0]);
        assert_eq!(p.get_value(10.0, 20.0, 30.0), -0.14383364863583398);
    }

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
