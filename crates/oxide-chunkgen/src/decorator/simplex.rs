//! The fixed biome-information noise two count modifiers read.
//!
//! `noise_threshold_count` and `noise_based_count` ask vanilla's `Biome.BIOME_INFO_NOISE`: a
//! `PerlinSimplexNoise` over a single octave, seeded from a legacy random source with the
//! constant 2345. With one octave and no offsets, that class reduces to one `SimplexNoise`
//! sampled at the given coordinates, which is all that is ported here.
//!
//! PARITY-CHECK: nothing in `oxide-noise` covers simplex noise and the reference implementation
//! keeps its own, so this is ported from the decompiled algorithm rather than from a copy that
//! could be diffed. Compare `info_noise(x, z)` against a real 26.2 `BIOME_INFO_NOISE` at a few
//! dozen coordinates before trusting `noise_threshold_count` or `noise_based_count`; a wrong
//! gradient or skew constant moves every grass and flower density threshold.

use std::sync::LazyLock;

use oxide_core::{LegacyRandom, RandomSource};

/// The twelve edge gradients, then four repeats, indexed by `permutation % 12`.
const GRADIENT: [[i32; 3]; 16] = [
    [1, 1, 0],
    [-1, 1, 0],
    [1, -1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [-1, 0, 1],
    [1, 0, -1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, -1, 1],
    [0, 1, -1],
    [0, -1, -1],
    [1, 1, 0],
    [0, -1, 1],
    [-1, 1, 0],
    [0, -1, -1],
];

/// `SimplexNoise`: a 256-entry shuffled permutation and the two-dimensional skewed lattice.
pub struct SimplexNoise {
    permutation: [i32; 256],
}

impl SimplexNoise {
    /// Draws the three unused offsets first and then shuffles, in vanilla's order: the offsets
    /// are never read here but they consume three `nextDouble` draws the shuffle follows.
    pub fn new(random: &mut impl RandomSource) -> Self {
        random.next_double();
        random.next_double();
        random.next_double();
        let mut permutation = [0i32; 256];
        for (slot, value) in permutation.iter_mut().zip(0..) {
            *slot = value;
        }
        for index in 0..256usize {
            let offset = random.next_int_bounded((256 - index) as i32) as usize;
            permutation.swap(index, index + offset);
        }
        Self { permutation }
    }

    fn permute(&self, index: i32) -> i32 {
        self.permutation[(index & 255) as usize]
    }

    fn corner(gradient: usize, x: f64, y: f64) -> f64 {
        let falloff = 0.5 - x * x - y * y;
        if falloff < 0.0 {
            return 0.0;
        }
        let squared = falloff * falloff;
        let g = GRADIENT[gradient];
        squared * squared * (f64::from(g[0]) * x + f64::from(g[1]) * y)
    }

    /// `SimplexNoise.getValue(double, double)`.
    pub fn value(&self, x: f64, y: f64) -> f64 {
        let sqrt3 = 3.0f64.sqrt();
        let skew = 0.5 * (sqrt3 - 1.0);
        let unskew = (3.0 - sqrt3) / 6.0;

        let s = (x + y) * skew;
        let i = (x + s).floor() as i32;
        let j = (y + s).floor() as i32;
        let t = f64::from(i.wrapping_add(j)) * unskew;
        let x0 = x - (f64::from(i) - t);
        let y0 = y - (f64::from(j) - t);
        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
        let x1 = x0 - f64::from(i1) + unskew;
        let y1 = y0 - f64::from(j1) + unskew;
        let x2 = x0 - 1.0 + 2.0 * unskew;
        let y2 = y0 - 1.0 + 2.0 * unskew;

        let ii = i & 255;
        let jj = j & 255;
        let g0 = (self.permute(ii.wrapping_add(self.permute(jj))) % 12) as usize;
        let g1 = (self.permute(ii.wrapping_add(i1).wrapping_add(self.permute(jj.wrapping_add(j1))))
            % 12) as usize;
        let g2 = (self.permute(ii.wrapping_add(1).wrapping_add(self.permute(jj.wrapping_add(1))))
            % 12) as usize;

        70.0 * (Self::corner(g0, x0, y0) + Self::corner(g1, x1, y1) + Self::corner(g2, x2, y2))
    }
}

static BIOME_INFO_NOISE: LazyLock<SimplexNoise> =
    LazyLock::new(|| SimplexNoise::new(&mut LegacyRandom::new(2345)));

/// `Biome.BIOME_INFO_NOISE.getValue(x, z, false)`.
pub fn info_noise(x: f64, z: f64) -> f64 {
    BIOME_INFO_NOISE.value(x, z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_permutation_is_a_shuffle_of_zero_to_255() {
        let noise = SimplexNoise::new(&mut LegacyRandom::new(2345));
        let mut seen = [false; 256];
        for value in noise.permutation {
            seen[value as usize] = true;
        }
        assert!(seen.iter().all(|&present| present));
    }

    #[test]
    fn values_are_deterministic_bounded_and_vary() {
        let mut samples = Vec::new();
        for step in 0..400 {
            let x = f64::from(step) * 0.37 - 70.0;
            let z = f64::from(step) * 0.91 + 13.0;
            let value = info_noise(x, z);
            assert_eq!(value, info_noise(x, z));
            assert!(value.abs() <= 1.0 + 1e-9, "{value} out of range");
            samples.push(value);
        }
        let (min, max) = samples
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        assert!(max - min > 0.5, "noise barely varies: {min}..{max}");
    }
}
