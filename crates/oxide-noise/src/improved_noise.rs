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

#[inline]
fn wrap(value: f64) -> f64 {
    value - ((value / WRAP + 0.5).floor() * WRAP)
}

#[inline(always)]
fn smoothstep(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline(always)]
fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

#[inline(always)]
fn grad_dot(gradient_index: u8, x: f64, y: f64, z: f64) -> f64 {
    let g = GRADIENT[(gradient_index & 15) as usize];
    g[0] * x + g[1] * y + g[2] * z
}

#[derive(Debug, Clone)]
pub struct ImprovedNoise {
    /// The 256-entry shuffled permutation, stored twice over (`permutation[i + 256] ==
    /// permutation[i]`), so that `p[(a + b) & 255]` with `a, b` in `0..=255` can be read as
    /// `p[a + b]` (and `p[a + b + 1]`) with no masking and no bounds check: every index the hash
    /// forms is at most `255 + 255 + 1 = 511`.
    permutation: [u8; 512],
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

        let mut doubled = [0u8; 512];
        doubled[..256].copy_from_slice(&permutation);
        doubled[256..].copy_from_slice(&permutation);

        Self {
            permutation: doubled,
            xo,
            yo,
            zo,
        }
    }

    pub fn noise(&self, x: f64, y: f64, z: f64) -> f64 {
        self.noise_with_scale(x, y, z, 0.0, 0.0)
    }

    /// `y_scale`/`y_max` implement vanilla's Y-axis "smear" used by terrain-shaping density
    /// functions to flatten noise variation below a threshold height; pass `0.0`/`0.0` for
    /// plain 3D noise.
    #[inline]
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

    /// [`Self::noise_with_scale`], reusing the lattice hashes `memo` holds when this sample
    /// falls in the same cell as the one before it.
    ///
    /// Bit-identical to `noise_with_scale`: the eight corner hash bytes are a pure function of
    /// the grid cell `(i, j, k)` and this instance's permutation, so a remembered set for the
    /// same cell is exactly what recomputing would give. Every float operation is the one
    /// `sample_and_lerp` performs, on the same operands, in the same order.
    ///
    /// `memo` must only ever be passed with this same instance; start it with
    /// [`Self::cell_hashes`].
    #[inline]
    pub(crate) fn noise_with_scale_reusing(
        &self,
        x: f64,
        y: f64,
        z: f64,
        y_scale: f64,
        y_max: f64,
        memo: &mut CellHashes,
    ) -> f64 {
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

        let cell = [i, j, k];
        let reused = memo.cell == cell;
        if !reused {
            memo.cell = cell;
            memo.hashes = self.corner_hashes(i, j, k);
        }
        #[cfg(any(test, feature = "hash-stats"))]
        hash_stats::record(memo.kind, reused);
        lerp_corners(&memo.hashes, dx, dy - dy_faded, dz, dy)
    }

    /// A [`CellHashes`] memo for this instance, primed with a real cell's hashes so that it is
    /// always consistent (no "empty" state for a sample to be wrongly matched against).
    #[inline]
    pub(crate) fn cell_hashes(&self, kind: HashKind) -> CellHashes {
        let cell = [i32::MIN; 3];
        let _ = kind;
        CellHashes {
            cell,
            hashes: self.corner_hashes(cell[0], cell[1], cell[2]),
            #[cfg(any(test, feature = "hash-stats"))]
            kind,
        }
    }

    /// The eight corner hash bytes of cell `(grid_x, grid_y, grid_z)`, in the order
    /// [`lerp_corners`] reads them: `p[k], p[i1], p[l], p[j1], p[k+1], p[i1+1], p[l+1],
    /// p[j1+1]` as `sample_and_lerp` names them.
    #[inline(always)]
    fn corner_hashes(&self, grid_x: i32, grid_y: i32, grid_z: i32) -> [u8; 8] {
        let p = &self.permutation;
        let xm = (grid_x & 255) as usize;
        let ym = (grid_y & 255) as usize;
        let zm = (grid_z & 255) as usize;
        let a = p[xm] as usize + ym;
        let b = p[xm + 1] as usize + ym;
        let k = p[a] as usize + zm;
        let l = p[a + 1] as usize + zm;
        let i1 = p[b] as usize + zm;
        let j1 = p[b + 1] as usize + zm;
        [
            p[k],
            p[i1],
            p[l],
            p[j1],
            p[k + 1],
            p[i1 + 1],
            p[l + 1],
            p[j1 + 1],
        ]
    }

    /// The lattice hash is vanilla's `p(p(p(x) + y) + z)` (each `p` masking its argument with
    /// `& 255`), read off the doubled table. Because `(a + y) & 255 == (a + (y & 255)) & 255` in
    /// wrapping `i32` arithmetic (masking commutes with addition mod 2^8), and every `p` value is
    /// already in `0..=255`, masking each grid coordinate once up front and indexing the doubled
    /// table at the unmasked sum picks exactly the same entry. The float work is unchanged.
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
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
        let p = &self.permutation;
        let xm = (grid_x & 255) as usize;
        let ym = (grid_y & 255) as usize;
        let zm = (grid_z & 255) as usize;
        let a = p[xm] as usize + ym;
        let b = p[xm + 1] as usize + ym;
        let k = p[a] as usize + zm;
        let l = p[a + 1] as usize + zm;
        let i1 = p[b] as usize + zm;
        let j1 = p[b + 1] as usize + zm;

        let d0 = grad_dot(p[k], dx, dy, dz);
        let d1 = grad_dot(p[i1], dx - 1.0, dy, dz);
        let d2 = grad_dot(p[l], dx, dy - 1.0, dz);
        let d3 = grad_dot(p[j1], dx - 1.0, dy - 1.0, dz);
        let d4 = grad_dot(p[k + 1], dx, dy, dz - 1.0);
        let d5 = grad_dot(p[i1 + 1], dx - 1.0, dy, dz - 1.0);
        let d6 = grad_dot(p[l + 1], dx, dy - 1.0, dz - 1.0);
        let d7 = grad_dot(p[j1 + 1], dx - 1.0, dy - 1.0, dz - 1.0);

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

/// `sample_and_lerp`'s float work over already-hashed corners: the same expressions, operands
/// and order, so the result is bit-identical to it for the same cell.
#[inline(always)]
fn lerp_corners(h: &[u8; 8], dx: f64, dy: f64, dz: f64, fade_dy: f64) -> f64 {
    let d0 = grad_dot(h[0], dx, dy, dz);
    let d1 = grad_dot(h[1], dx - 1.0, dy, dz);
    let d2 = grad_dot(h[2], dx, dy - 1.0, dz);
    let d3 = grad_dot(h[3], dx - 1.0, dy - 1.0, dz);
    let d4 = grad_dot(h[4], dx, dy, dz - 1.0);
    let d5 = grad_dot(h[5], dx - 1.0, dy, dz - 1.0);
    let d6 = grad_dot(h[6], dx, dy - 1.0, dz - 1.0);
    let d7 = grad_dot(h[7], dx - 1.0, dy - 1.0, dz - 1.0);

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

/// The last lattice cell one [`ImprovedNoise`] instance sampled, and its eight corner hashes.
/// See [`ImprovedNoise::noise_with_scale_reusing`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct CellHashes {
    cell: [i32; 3],
    hashes: [u8; 8],
    #[cfg(any(test, feature = "hash-stats"))]
    kind: HashKind,
}

/// Which caller a hash-reuse count belongs to, for [`hash_stats`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HashKind {
    /// An octave of a `PerlinNoise` (inside a `NormalNoise`).
    Perlin,
    /// An octave of one of `BlendedNoise`'s three stacks.
    Blended,
}

/// Counts of samples that reused their cell's hashes versus recomputed them, so the reuse rate
/// is measured on the code path that runs rather than assumed. Compiled only into tests and
/// the `hash-stats` feature; the plain build carries no counter.
#[cfg(any(test, feature = "hash-stats"))]
pub mod hash_stats {
    use super::HashKind;
    use std::cell::Cell;
    #[cfg(feature = "hash-stats")]
    use std::sync::atomic::{AtomicU64, Ordering};

    /// `[perlin reused, perlin computed, blended reused, blended computed]`.
    pub type Counts = [u64; 4];

    thread_local! {
        static LOCAL: Cell<Counts> = const { Cell::new([0; 4]) };
    }
    #[cfg(feature = "hash-stats")]
    static GLOBAL: [AtomicU64; 4] = [
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
    ];

    #[inline]
    pub(crate) fn record(kind: HashKind, reused: bool) {
        let index = match kind {
            HashKind::Perlin => 0,
            HashKind::Blended => 2,
        } + usize::from(!reused);
        LOCAL.with(|c| {
            let mut v = c.get();
            v[index] += 1;
            c.set(v);
        });
        #[cfg(feature = "hash-stats")]
        GLOBAL[index].fetch_add(1, Ordering::Relaxed);
    }

    /// This thread's counts so far.
    pub fn local() -> Counts {
        LOCAL.with(|c| c.get())
    }

    /// Every thread's counts so far.
    #[cfg(feature = "hash-stats")]
    pub fn global() -> Counts {
        std::array::from_fn(|i| GLOBAL[i].load(Ordering::Relaxed))
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
        for &v in &noise.permutation[..256] {
            assert!(!seen[v as usize], "value {v} appeared twice");
            seen[v as usize] = true;
        }
        assert_eq!(noise.permutation[..256], noise.permutation[256..]);
    }

    /// The pre-doubling implementation, kept verbatim as the reference the fast path must
    /// match bit for bit.
    mod reference {
        use super::super::{lerp, smoothstep, GRADIENT};

        fn grad_dot(gradient_index: i32, x: f64, y: f64, z: f64) -> f64 {
            let g = GRADIENT[(gradient_index & 15) as usize];
            g[0] * x + g[1] * y + g[2] * z
        }

        pub struct Ref<'a> {
            pub permutation: &'a [u8],
            pub xo: f64,
            pub yo: f64,
            pub zo: f64,
        }

        impl Ref<'_> {
            fn p(&self, index: i32) -> i32 {
                self.permutation[(index & 255) as usize] as i32
            }

            pub fn noise_with_scale(
                &self,
                x: f64,
                y: f64,
                z: f64,
                y_scale: f64,
                y_max: f64,
            ) -> f64 {
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
                let b = self.p(grid_x.wrapping_add(1));
                let k = self.p(a.wrapping_add(grid_y));
                let l = self.p(a.wrapping_add(grid_y).wrapping_add(1));
                let i1 = self.p(b.wrapping_add(grid_y));
                let j1 = self.p(b.wrapping_add(grid_y).wrapping_add(1));
                let d0 = grad_dot(self.p(k.wrapping_add(grid_z)), dx, dy, dz);
                let d1 = grad_dot(self.p(i1.wrapping_add(grid_z)), dx - 1.0, dy, dz);
                let d2 = grad_dot(self.p(l.wrapping_add(grid_z)), dx, dy - 1.0, dz);
                let d3 = grad_dot(self.p(j1.wrapping_add(grid_z)), dx - 1.0, dy - 1.0, dz);
                let d4 = grad_dot(
                    self.p(k.wrapping_add(grid_z).wrapping_add(1)),
                    dx,
                    dy,
                    dz - 1.0,
                );
                let d5 = grad_dot(
                    self.p(i1.wrapping_add(grid_z).wrapping_add(1)),
                    dx - 1.0,
                    dy,
                    dz - 1.0,
                );
                let d6 = grad_dot(
                    self.p(l.wrapping_add(grid_z).wrapping_add(1)),
                    dx,
                    dy - 1.0,
                    dz - 1.0,
                );
                let d7 = grad_dot(
                    self.p(j1.wrapping_add(grid_z).wrapping_add(1)),
                    dx - 1.0,
                    dy - 1.0,
                    dz - 1.0,
                );
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
        }
    }

    #[test]
    fn doubled_table_matches_reference_bit_for_bit() {
        use oxide_core::Xoroshiro128PlusPlus;
        let mut rng = LegacyRandom::new(0x5eed);
        for seed in 0..64u64 {
            let mut r = Xoroshiro128PlusPlus::new(seed as i64);
            let noise = ImprovedNoise::new(&mut r);
            let reference = reference::Ref {
                permutation: &noise.permutation[..256],
                xo: noise.xo,
                yo: noise.yo,
                zo: noise.zo,
            };
            let scales = [1.0, 1.0e3, 1.0e6, 3.0e7, 1.0e9, 3.0e9];
            for n in 0..4000 {
                let s = scales[n % scales.len()];
                let x = (rng.next_double() * 2.0 - 1.0) * s;
                let y = (rng.next_double() * 2.0 - 1.0) * s;
                let z = (rng.next_double() * 2.0 - 1.0) * s;
                let (y_scale, y_max) = match n % 4 {
                    0 => (0.0, 0.0),
                    1 => (rng.next_double() * 2.0, rng.next_double() * 2.0),
                    2 => (rng.next_double() * 0.5, -1.0),
                    _ => (-(rng.next_double() * 2.0), rng.next_double()),
                };
                let fast = noise.noise_with_scale(x, y, z, y_scale, y_max);
                let slow = reference.noise_with_scale(x, y, z, y_scale, y_max);
                assert_eq!(
                    fast.to_bits(),
                    slow.to_bits(),
                    "seed {seed} at ({x}, {y}, {z}) y_scale {y_scale} y_max {y_max}"
                );
            }
            // Integer lattice points, negative zero, and exact cell boundaries.
            for &x in &[
                -0.0,
                0.0,
                -1.0,
                1.0,
                -256.0,
                255.999_999_999,
                -1.0e-300,
                2.0e9,
                -2.0e9,
            ] {
                for &y in &[-0.0, 0.0, -17.0, 0.5, 1.0e-20] {
                    for &z in &[-0.0, 0.0, -255.0, 4096.0] {
                        let a = noise.noise_with_scale(x, y, z, 0.0, 0.0).to_bits();
                        let b = reference.noise_with_scale(x, y, z, 0.0, 0.0).to_bits();
                        assert_eq!(a, b, "seed {seed} at ({x}, {y}, {z})");
                    }
                }
            }
        }
    }

    /// The hash-reusing sample must give `noise_with_scale`'s exact bits along columns: steps
    /// that stay in one cell (reuse), steps that cross cells (recompute), negative coordinates,
    /// exact lattice points and signed zeros, with and without y-smear.
    #[test]
    fn reusing_hashes_is_bit_identical_along_columns() {
        use oxide_core::Xoroshiro128PlusPlus;
        let mut rng = LegacyRandom::new(0xc01);
        let before = hash_stats::local();
        for seed in 0..32u64 {
            let mut r = Xoroshiro128PlusPlus::new(seed as i64);
            let noise = ImprovedNoise::new(&mut r);
            // Per-step y deltas from far below one cell to several cells.
            for &step in &[0.0, 1.0e-3, 0.06, 0.25, 0.53, 1.0, 1.7, 5.3, 684.412] {
                let x = (rng.next_double() * 2.0 - 1.0) * 1.0e4;
                let z = (rng.next_double() * 2.0 - 1.0) * 1.0e4;
                let y0 = (rng.next_double() * 2.0 - 1.0) * 300.0 - 64.0;
                let smear = match seed % 4 {
                    0 => 0.0,
                    1 => rng.next_double() * 2.0,
                    2 => -(rng.next_double() * 2.0),
                    _ => rng.next_double() * 0.01,
                };
                let mut memo = noise.cell_hashes(HashKind::Perlin);
                for lane in 0..49 {
                    let y = y0 + lane as f64 * step;
                    // y_max is the sample's own y, as `BlendedNoise` passes it, or a fixed one.
                    let y_max = if lane % 2 == 0 { y } else { -1.0 };
                    let fast = noise.noise_with_scale_reusing(x, y, z, smear, y_max, &mut memo);
                    let slow = noise.noise_with_scale(x, y, z, smear, y_max);
                    assert_eq!(
                        fast.to_bits(),
                        slow.to_bits(),
                        "seed {seed} step {step} lane {lane} at ({x}, {y}, {z}) smear {smear}"
                    );
                }
            }
            // Columns that walk across zero and through exact lattice points and signed zeros,
            // and a memo carried over unrelated positions (every cell change must recompute).
            let mut memo = noise.cell_hashes(HashKind::Perlin);
            for &x in &[-0.0, 0.0, -1.0, -256.0, 255.999_999_999, -1.0e-300] {
                for &z in &[-0.0, 0.0, -255.0, 4096.0] {
                    for lane in -20..20 {
                        let y = lane as f64 * 0.25 - noise.yo;
                        for &y in &[y, -y, y + 1.0e-20] {
                            let fast = noise.noise_with_scale_reusing(x, y, z, 0.0, 0.0, &mut memo);
                            let slow = noise.noise_with_scale(x, y, z, 0.0, 0.0);
                            assert_eq!(fast.to_bits(), slow.to_bits(), "at ({x}, {y}, {z})");
                        }
                    }
                }
            }
        }
        let after = hash_stats::local();
        let (reused, computed) = (after[0] - before[0], after[1] - before[1]);
        assert!(reused > 1000 && computed > 1000, "{reused} reused, {computed} computed");
    }

    /// Captured 2026-08-22 from a standalone Java transcription of the confirmed-correct
    /// algorithm chain (decompiled from the real Minecraft 26.2 server jar's `ImprovedNoise`/
    /// `XoroshiroRandomSource`/`RandomSupport`) run on real OpenJDK 25 — see the session's
    /// scratchpad `decomp/NoiseRef.java`.
    #[test]
    fn noise_matches_real_java_xoroshiro_seed_1234() {
        use oxide_core::Xoroshiro128PlusPlus;
        let mut r = Xoroshiro128PlusPlus::new(1234);
        let noise = ImprovedNoise::new(&mut r);
        assert_eq!(noise.noise(1.5, 2.5, 3.5), -0.2825922132396466);
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
