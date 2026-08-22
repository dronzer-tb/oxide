//! `NormalNoise`: two decorrelated [`PerlinNoise`] instances combined to reduce the axis-aligned
//! directional artifacts plain Perlin noise shows, matching vanilla's
//! `net.minecraft.world.level.levelgen.synth.NormalNoise`. This is what `minecraft:noise`
//! density-function nodes and `worldgen/noise/*.json` (`NormalNoiseParameters`) sample.

use crate::perlin_noise::PerlinNoise;
use crate::random::WorldRandom;

/// Irrational scale applied to the second octave set so it doesn't sample the same lattice
/// points as the first at any integer coordinate.
const INPUT_FACTOR: f64 = 1.018_126_888_217_522_7;

#[derive(Debug, Clone)]
pub struct NormalNoise {
    value_factor: f64,
    first: PerlinNoise,
    second: PerlinNoise,
}

impl NormalNoise {
    pub fn create(random: &mut WorldRandom, first_octave: i32, amplitudes: Vec<f64>) -> Self {
        let first = PerlinNoise::create(random, first_octave, amplitudes.clone());
        let second = PerlinNoise::create(random, first_octave, amplitudes.clone());

        let mut min_octave: Option<i32> = None;
        let mut max_octave: Option<i32> = None;
        for (i, &amplitude) in amplitudes.iter().enumerate() {
            if amplitude != 0.0 {
                let i = i as i32;
                min_octave = Some(min_octave.map_or(i, |m| m.min(i)));
                max_octave = Some(max_octave.map_or(i, |m| m.max(i)));
            }
        }
        // `octave_span` is the 0-based index gap between the lowest and highest nonzero
        // amplitude (not a count) — matches vanilla's `maxOctave - minOctave` exactly. An
        // all-zero amplitudes list (min/max never set) has no vanilla-defined behavior either;
        // this falls back to span 0 rather than replicating a Java `Integer.MAX - Integer.MIN`
        // overflow.
        let octave_span = match (min_octave, max_octave) {
            (Some(lo), Some(hi)) => hi - lo,
            _ => 0,
        };
        // Verified 2026-08-22 against decompiled `NormalNoise`'s constructor:
        // `valueFactor = (1.0/6.0) / expectedDeviation(octaveSpan)`,
        // `expectedDeviation(span) = 0.1 * (1.0 + 1.0 / (span + 1.0))`.
        let expected_deviation = 0.1 * (1.0 + 1.0 / (octave_span as f64 + 1.0));
        let value_factor = (1.0 / 6.0) / expected_deviation;

        Self {
            value_factor,
            first,
            second,
        }
    }

    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        let x2 = x * INPUT_FACTOR;
        let y2 = y * INPUT_FACTOR;
        let z2 = z * INPUT_FACTOR;
        (self.first.get_value(x, y, z) + self.second.get_value(x2, y2, z2)) * self.value_factor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured 2026-08-22 from a standalone Java transcription of the confirmed-correct
    // algorithm chain run on real OpenJDK 25 — see the session's scratchpad
    // `decomp/NoiseRef.java`. This is wave 2 -> 3's gate per `docs/ROADMAP.md` ("the
    // density-function interpreter reproduces sampled values from a vanilla reference dump"),
    // now actually met for the noise primitives themselves.
    #[test]
    fn get_value_matches_real_java_seed_1() {
        let mut r = WorldRandom::new(1, false);
        let n = NormalNoise::create(&mut r, -7, vec![1.0, 1.0]);
        assert_eq!(n.get_value(5.0, 6.0, 7.0), 0.007315295452405016);
    }

    #[test]
    fn get_value_matches_real_java_seed_2026_grid() {
        let mut r = WorldRandom::new(2026, false);
        let n = NormalNoise::create(&mut r, -6, vec![1.0, 1.0, 1.0, 1.0]);
        let expected = [
            -0.4683983585486846,
            -0.583222461440265,
            -0.7240688784328928,
            -0.48924289290342937,
            -0.5706813829179779,
            -0.6110434355775295,
            -0.48426498917716077,
            -0.509567788502729,
            -0.49782437894002185,
        ];
        let mut i = 0;
        for x in -1..=1 {
            for z in -1..=1 {
                let v = n.get_value(x as f64 * 4.0, 64.0, z as f64 * 4.0);
                assert_eq!(v, expected[i], "x={x} z={z}");
                i += 1;
            }
        }
    }

    #[test]
    fn deterministic_for_same_seed() {
        let mut r1 = WorldRandom::new(1, false);
        let n1 = NormalNoise::create(&mut r1, -7, vec![1.0, 1.0]);
        let mut r2 = WorldRandom::new(1, false);
        let n2 = NormalNoise::create(&mut r2, -7, vec![1.0, 1.0]);
        assert_eq!(n1.get_value(5.0, 6.0, 7.0), n2.get_value(5.0, 6.0, 7.0));
    }

    #[test]
    fn finite_over_a_grid() {
        let mut r = WorldRandom::new(2026, false);
        let n = NormalNoise::create(&mut r, -6, vec![1.0, 1.0, 1.0, 1.0]);
        for x in -3..3 {
            for z in -3..3 {
                let v = n.get_value(x as f64 * 4.0, 64.0, z as f64 * 4.0);
                assert!(v.is_finite());
            }
        }
    }
}
