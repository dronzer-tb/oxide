//! `ImprovedNoise`: single-octave 3D gradient (Perlin) noise, matching vanilla's
//! `net.minecraft.world.level.levelgen.synth.ImprovedNoise`.
//!
//! This exact algorithm (permutation-table shuffle + 16-entry gradient table + smoothstep
//! trilinear interpolation) is Ken Perlin's public "improved noise" scheme and has been stable
//! across Minecraft versions since its 1.13 worldgen rewrite — high confidence, but still
//! unverified bit-for-bit against a real 26.2 run (see `docs/ARCHITECTURE.md` § RNG is
//! load-bearing). `oxide-harness`'s Merkle diff is what actually confirms this.

use oxide_core::RandomSource;

/// Fixed 16-entry gradient table indexed by the low 4 bits of a permuted value.
const GRADIENT: [[f64; 3]; 16] = [
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [1.0, -1.0, 0.0],
    [-1.0, -1.0, 0.0],
    [1.0, 0.0, 1.0],
    [-1.0, 0.0, 1.0],
    [1.0, 0.0, -1.0],
    [-1.0, 0.0, -1.0],
    [0.0, 1.0, 1.0],
    [0.0, -1.0, 1.0],
    [0.0, 1.0, -1.0],
    [0.0, -1.0, -1.0],
    [1.0, 1.0, 0.0],
    [0.0, -1.0, 1.0],
    [-1.0, 1.0, 0.0],
    [0.0, -1.0, -1.0],
];

/// The classic "avoid float precision loss over huge world coordinates" wrap, `2^25`.
const WRAP: f64 = 3.355_443_2e7;

fn wrap(value: f64) -> f64 {
    value - ((value / WRAP + 0.5).floor() * WRAP)
}

fn smoothstep(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

fn grad_dot(gradient_index: i32, x: f64, y: f64, z: f64) -> f64 {
    let g = GRADIENT[(gradient_index & 15) as usize];
    g[0] * x + g[1] * y + g[2] * z
}

#[derive(Debug, Clone)]
pub struct ImprovedNoise {
    permutation: [u8; 256],
    xo: f64,
    yo: f64,
    zo: f64,
}

impl ImprovedNoise {
    /// Consumes 3 `next_double` calls (origin offsets) then shuffles a 0..255 permutation
    /// table with a forward Fisher-Yates driven by `next_int_bounded`, matching vanilla's
    /// `ImprovedNoise(RandomSource)` constructor exactly (loop order and bound matter for
    /// parity — this is not "a" valid shuffle, it must be *this* shuffle).
    pub fn new(random: &mut impl RandomSource) -> Self {
        let xo = random.next_double() * 256.0;
        let yo = random.next_double() * 256.0;
        let zo = random.next_double() * 256.0;

        let mut permutation: [u8; 256] = [0; 256];
        for (i, slot) in permutation.iter_mut().enumerate() {
            *slot = i as u8;
        }
        for i in 0..256i32 {
            let j = random.next_int_bounded(256 - i);
            permutation.swap(i as usize, (i + j) as usize);
        }

        Self {
            permutation,
            xo,
            yo,
            zo,
        }
    }

    fn p(&self, index: i32) -> i32 {
        self.permutation[(index & 255) as usize] as i32
    }

    pub fn noise(&self, x: f64, y: f64, z: f64) -> f64 {
        self.noise_with_scale(x, y, z, 0.0, 0.0)
    }

    /// `y_scale`/`y_max` implement vanilla's Y-axis "smear" used by terrain-shaping density
    /// functions to flatten noise variation below a threshold height; pass `0.0`/`0.0` for
    /// plain 3D noise.
    pub fn noise_with_scale(&self, x: f64, y: f64, z: f64, y_scale: f64, y_max: f64) -> f64 {
        let d0 = x + self.xo;
        let d1 = y + self.yo;
        let d2 = z + self.zo;
        let i = d0.floor() as i32;
        let j = d1.floor() as i32;
        let k = d2.floor() as i32;
        let dx = d0 - i as f64;
        let dy = d1 - j as f64;
        let dz = d2 - k as f64;

        let dy_faded = if y_scale != 0.0 {
            let clamped = if y_max >= 0.0 && y_max < dy {
                y_max
            } else {
                dy
            };
            (clamped / y_scale + 1.0e-7).floor() * y_scale
        } else {
            0.0
        };

        self.sample_and_lerp(i, j, k, dx, dy - dy_faded, dz, dy)
    }

    #[allow(clippy::too_many_arguments)]
    fn sample_and_lerp(
        &self,
        grid_x: i32,
        grid_y: i32,
        grid_z: i32,
        dx: f64,
        dy: f64,
        dz: f64,
        fade_dy: f64,
    ) -> f64 {
        let a = self.p(grid_x);
        let b = self.p(grid_x + 1);
        let k = self.p(a + grid_y);
        let l = self.p(a + grid_y + 1);
        let i1 = self.p(b + grid_y);
        let j1 = self.p(b + grid_y + 1);

        let d0 = grad_dot(self.p(k + grid_z), dx, dy, dz);
        let d1 = grad_dot(self.p(i1 + grid_z), dx - 1.0, dy, dz);
        let d2 = grad_dot(self.p(l + grid_z), dx, dy - 1.0, dz);
        let d3 = grad_dot(self.p(j1 + grid_z), dx - 1.0, dy - 1.0, dz);
        let d4 = grad_dot(self.p(k + grid_z + 1), dx, dy, dz - 1.0);
        let d5 = grad_dot(self.p(i1 + grid_z + 1), dx - 1.0, dy, dz - 1.0);
        let d6 = grad_dot(self.p(l + grid_z + 1), dx, dy - 1.0, dz - 1.0);
        let d7 = grad_dot(self.p(j1 + grid_z + 1), dx - 1.0, dy - 1.0, dz - 1.0);

        let t = smoothstep(dx);
        let u = smoothstep(fade_dy);
        let v = smoothstep(dz);

        let e0 = lerp(t, d0, d1);
        let e1 = lerp(t, d2, d3);
        let e2 = lerp(t, d4, d5);
        let e3 = lerp(t, d6, d7);
        let f0 = lerp(u, e0, e1);
        let f1 = lerp(u, e2, e3);
        lerp(v, f0, f1)
    }

    pub fn yo(&self) -> f64 {
        self.yo
    }
}

pub(crate) fn wrap_coordinate(value: f64) -> f64 {
    wrap(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::LegacyRandom;

    #[test]
    fn permutation_table_is_a_bijection_of_0_255() {
        let mut r = LegacyRandom::new(42);
        let noise = ImprovedNoise::new(&mut r);
        let mut seen = [false; 256];
        for &v in &noise.permutation {
            assert!(!seen[v as usize], "value {v} appeared twice");
            seen[v as usize] = true;
        }
    }

    #[test]
    fn same_seed_gives_deterministic_noise() {
        let mut r1 = LegacyRandom::new(1234);
        let n1 = ImprovedNoise::new(&mut r1);
        let mut r2 = LegacyRandom::new(1234);
        let n2 = ImprovedNoise::new(&mut r2);
        assert_eq!(n1.noise(1.5, 2.5, 3.5), n2.noise(1.5, 2.5, 3.5));
    }

    #[test]
    fn different_seed_gives_different_noise() {
        let mut r1 = LegacyRandom::new(1);
        let n1 = ImprovedNoise::new(&mut r1);
        let mut r2 = LegacyRandom::new(2);
        let n2 = ImprovedNoise::new(&mut r2);
        assert_ne!(n1.noise(1.5, 2.5, 3.5), n2.noise(1.5, 2.5, 3.5));
    }

    #[test]
    fn noise_is_finite_over_a_grid() {
        let mut r = LegacyRandom::new(99);
        let noise = ImprovedNoise::new(&mut r);
        for x in -4..4 {
            for y in -4..4 {
                for z in -4..4 {
                    let v = noise.noise(x as f64 * 0.3, y as f64 * 0.3, z as f64 * 0.3);
                    assert!(v.is_finite());
                }
            }
        }
    }

    #[test]
    fn wrap_is_identity_near_origin() {
        assert!((wrap_coordinate(10.0) - 10.0).abs() < 1e-9);
    }
}
