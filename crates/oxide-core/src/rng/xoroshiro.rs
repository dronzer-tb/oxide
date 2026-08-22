//! `Xoroshiro128PlusPlus`: Minecraft's modern world-seed RNG.
//!
//! Verified 2026-08-22 against `RandomSupport`/`Xoroshiro128PlusPlus`/`XoroshiroRandomSource`
//! decompiled from the real Minecraft 26.2 server jar (`net.minecraft.server.Main`): the step
//! function, the seed-upgrade path (constants included — vanilla names them differently
//! internally, but the values and formula match bit-for-bit), and the bounded-`nextInt`
//! algorithm below are all confirmed exact, not reconstructed from memory. See
//! `docs/REFERENCE_DATA.md` for how to re-derive this if a future version changes it.

use super::{RandomSource, FLOAT_DIVISOR};

/// `0x6A09E667F3BCC909` — the literal vanilla XORs a legacy seed with in
/// `RandomSupport.upgradeSeedTo128bitUnmixed` (unnamed there; kept as a named constant here).
const SILVER_RATIO_64: u64 = 0x6A09_E667_F3BC_C909;
/// `0x9E3779B97F4A7C15` (`= -7046029254386353131L`) — vanilla's `RandomSupport.GOLDEN_RATIO_64`,
/// added to the XORed seed to derive the second 64-bit half before mixing.
const GOLDEN_RATIO_64: u64 = 0x9E37_79B9_7F4A_7C15;

/// Fallback state substituted when a seed upgrades to an all-zero 128-bit state (xoroshiro's
/// state must never be all-zero — it's a fixed point of the step function). Confirmed exact
/// against vanilla's `Xoroshiro128PlusPlus` constructor.
const FALLBACK_LO: u64 = -7046029254386353131i64 as u64;
const FALLBACK_HI: u64 = 7640891576956012809u64;

/// Confirmed exact against `RandomSupport.mixStafford13` — these are also the well-known
/// MurmurHash3/SplitMix64 finalizer constants, unchanged from that public origin.
fn stafford_mix13(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Mojang's `RandomSupport.upgradeSeedTo128bit`: expands a 64-bit legacy-style seed into a
/// 128-bit xoroshiro state, returning `(lo, hi)`.
pub(crate) fn upgrade_seed_to_128bit(seed: i64) -> (u64, u64) {
    let l = (seed as u64) ^ SILVER_RATIO_64;
    let m = l.wrapping_add(GOLDEN_RATIO_64);
    (stafford_mix13(l), stafford_mix13(m))
}

/// Minecraft's `Xoroshiro128PlusPlus` RNG (two `u64` state words).
#[derive(Debug, Clone)]
pub struct Xoroshiro128PlusPlus {
    lo: u64,
    hi: u64,
    next_next_gaussian: Option<f64>,
}

impl Xoroshiro128PlusPlus {
    /// Seeds from a single 64-bit seed via the standard upgrade path.
    pub fn new(seed: i64) -> Self {
        let (lo, hi) = upgrade_seed_to_128bit(seed);
        Self::from_state(lo, hi)
    }

    /// Constructs directly from a raw 128-bit state (`lo`, `hi`), substituting the fixed
    /// fallback state if both halves are zero.
    pub fn from_state(lo: u64, hi: u64) -> Self {
        let (lo, hi) = if lo == 0 && hi == 0 {
            (FALLBACK_LO, FALLBACK_HI)
        } else {
            (lo, hi)
        };
        Self {
            lo,
            hi,
            next_next_gaussian: None,
        }
    }

    /// One xoroshiro128++ step: returns the raw 64-bit output and advances the state.
    /// Reference algorithm (Blackman & Vigna): `rotl(s0 + s1, 17) + s0`, then
    /// `s1 ^= s0; s0 = rotl(s0, 49) ^ s1 ^ (s1 << 21); s1 = rotl(s1, 28)`.
    fn next_raw(&mut self) -> u64 {
        let s0 = self.lo;
        let mut s1 = self.hi;
        let result = s0.wrapping_add(s1).rotate_left(17).wrapping_add(s0);
        s1 ^= s0;
        self.lo = s0.rotate_left(49) ^ s1 ^ (s1 << 21);
        self.hi = s1.rotate_left(28);
        result
    }
}

impl RandomSource for Xoroshiro128PlusPlus {
    fn next_bits(&mut self, bits: u32) -> i32 {
        debug_assert!(bits <= 32);
        (self.next_raw() >> (64 - bits)) as i32
    }

    fn next_int(&mut self) -> i32 {
        // Truncates to the low 32 bits directly rather than going through `next_bits(32)`
        // (which would take the *high* 32 bits) — matches Mojang's `(int) nextLong()`.
        self.next_raw() as i32
    }

    // Verified 2026-08-22 against decompiled `XoroshiroRandomSource.nextInt(int)` (Minecraft
    // 26.2, `net.minecraft.server.Main`): a 32-bit Lemire-style multiply-and-shift over
    // `nextInt()` (the low 32 bits of `nextLong()`), not the 31-bit `next(bits)`-based scheme
    // `LegacyRandom`/`java.util.Random` use. Bit-exact with vanilla.
    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "bound must be positive");
        let bound_u = bound as u32 as u64;
        let mut random_bits = self.next_int() as u32 as u64;
        let mut m = random_bits.wrapping_mul(bound_u);
        let mut frac = m & 0xFFFF_FFFF;
        if frac < bound_u {
            // `Integer.remainderUnsigned(-bound, bound)`: unsigned 32-bit remainder of
            // `-bound` (`bound.wrapping_neg()` as u32) by `bound`.
            let threshold = ((bound as u32).wrapping_neg() % (bound as u32)) as u64;
            while frac < threshold {
                random_bits = self.next_int() as u32 as u64;
                m = random_bits.wrapping_mul(bound_u);
                frac = m & 0xFFFF_FFFF;
            }
        }
        (m >> 32) as i32
    }

    fn next_int_between(&mut self, min: i32, max: i32) -> i32 {
        min + self.next_int_bounded(max - min + 1)
    }

    fn next_long(&mut self) -> i64 {
        self.next_raw() as i64
    }

    // Verified 2026-08-22: vanilla's `nextBoolean` is `(randomNumberGenerator.nextLong() & 1L)
    // != 0L` — the *low* bit of a fresh `nextLong()` draw, not the high bit `next_bits(1)`
    // extracts (that's `LegacyRandom`'s `next(bits)`-based shape, which Xoroshiro doesn't use
    // here despite sharing the `RandomSource` trait method).
    fn next_boolean(&mut self) -> bool {
        (self.next_raw() & 1) != 0
    }

    fn next_float(&mut self) -> f32 {
        self.next_bits(24) as f32 / FLOAT_DIVISOR
    }

    // Verified 2026-08-22: vanilla's `nextDouble` is `(double) nextBits(53) * DOUBLE_UNIT`
    // where `nextBits` is Xoroshiro's own private single-draw high-bit extractor
    // (`nextLong() >>> (64 - bits)`) — one `next_raw()` call, not `LegacyRandom`'s two-call
    // 26+27 split (that split exists only because `java.util.Random.next(int)` is capped at
    // 32 bits; Xoroshiro has no such cap). `DOUBLE_UNIT` itself is also Xoroshiro-specific:
    // vanilla defines it as `(double) 1.110223E-16f` — a *float* literal widened to double,
    // not the exact `2^-53` `LegacyRandom` uses — so this crate's shared `DOUBLE_ULP` constant
    // (exact `2^-53`, correct for `LegacyRandom`) does not apply here.
    fn next_double(&mut self) -> f64 {
        const DOUBLE_UNIT: f64 = 1.110223E-16_f32 as f64;
        let bits53 = self.next_raw() >> 11; // top 53 bits of one nextLong() draw
        (bits53 as f64) * DOUBLE_UNIT
    }

    fn next_gaussian(&mut self) -> f64 {
        // Duplicated from `LegacyRandom::next_gaussian` rather than shared — see the note in
        // `rng/mod.rs` on why a shared free function fights the borrow checker here.
        if let Some(v) = self.next_next_gaussian.take() {
            return v;
        }
        loop {
            let v1 = 2.0 * self.next_double() - 1.0;
            let v2 = 2.0 * self.next_double() - 1.0;
            let s = v1 * v1 + v2 * v2;
            if s < 1.0 && s != 0.0 {
                let multiplier = (-2.0 * s.ln() / s).sqrt();
                self.next_next_gaussian = Some(v2 * multiplier);
                return v1 * multiplier;
            }
        }
    }

    fn consume(&mut self, count: i32) {
        for _ in 0..count {
            self.next_int();
        }
    }

    fn set_seed(&mut self, seed: i64) {
        let (lo, hi) = upgrade_seed_to_128bit(seed);
        let (lo, hi) = if lo == 0 && hi == 0 {
            (FALLBACK_LO, FALLBACK_HI)
        } else {
            (lo, hi)
        };
        self.lo = lo;
        self.hi = hi;
        self.next_next_gaussian = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // All expected values below were captured 2026-08-22 from a standalone Java transcription
    // of the confirmed-correct algorithm (decompiled from the real Minecraft 26.2 server jar's
    // `Xoroshiro128PlusPlus`/`XoroshiroRandomSource`/`RandomSupport`, Mojang serialization
    // stripped) run on real OpenJDK 25 — see the session's scratchpad `decomp/XoroRef.java`.
    // Same convention as `LegacyRandom`'s tests: exact equality for integer/bit operations,
    // epsilon for anything routed through `ln`/`sqrt`.

    #[test]
    fn raw_step_matches_real_java_from_known_state() {
        let mut r = Xoroshiro128PlusPlus::from_state(1, 2);
        assert_eq!(r.next_raw(), 393217);
        assert_eq!(r.next_raw(), 669327710093319);
    }

    #[test]
    fn all_zero_state_substitutes_fallback() {
        let r = Xoroshiro128PlusPlus::from_state(0, 0);
        assert_eq!(r.lo, FALLBACK_LO);
        assert_eq!(r.hi, FALLBACK_HI);
    }

    #[test]
    fn seed_upgrade_avoids_all_zero_state_in_practice() {
        let r = Xoroshiro128PlusPlus::new(0);
        assert!(r.lo != 0 || r.hi != 0);
    }

    #[test]
    fn next_int_bounded_non_power_of_two_seed_42() {
        let mut r = Xoroshiro128PlusPlus::new(42);
        let expected = [15, 11, 31, 17, 24];
        for e in expected {
            assert_eq!(r.next_int_bounded(37), e);
        }
    }

    #[test]
    fn next_int_bounded_power_of_two_seed_999() {
        let mut r = Xoroshiro128PlusPlus::new(999);
        let expected = [2, 2, 11, 10, 15];
        for e in expected {
            assert_eq!(r.next_int_bounded(16), e);
        }
    }

    #[test]
    fn next_double_sequence_seed_7() {
        let mut r = Xoroshiro128PlusPlus::new(7);
        let expected = [0.97682267280947, 0.6426700847971978, 0.22714610973813298];
        for e in expected {
            assert_eq!(r.next_double(), e);
        }
    }

    #[test]
    fn next_boolean_sequence_seed_42() {
        let mut r = Xoroshiro128PlusPlus::new(42);
        let expected = [true, true, true, false, true];
        for e in expected {
            assert_eq!(r.next_boolean(), e);
        }
    }

    #[test]
    fn next_float_sequence_seed_7() {
        let mut r = Xoroshiro128PlusPlus::new(7);
        let expected: [f32; 3] = [0.9768226, 0.64267004, 0.22714609];
        for e in expected {
            assert_eq!(r.next_float(), e);
        }
    }

    #[test]
    fn next_int_bounded_always_in_range() {
        let mut r = Xoroshiro128PlusPlus::new(12345);
        for bound in [1, 2, 3, 7, 16, 1000, i32::MAX] {
            for _ in 0..200 {
                let v = r.next_int_bounded(bound);
                assert!((0..bound).contains(&v), "bound={bound} got={v}");
            }
        }
    }

    #[test]
    fn set_seed_resets_gaussian_cache() {
        let mut r = Xoroshiro128PlusPlus::new(1);
        let _ = r.next_gaussian();
        assert!(r.next_next_gaussian.is_some());
        r.set_seed(2);
        assert!(r.next_next_gaussian.is_none());
    }
}
