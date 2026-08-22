//! `LegacyRandom` = `java.util.Random`'s 48-bit LCG, bit-for-bit.

use super::{RandomSource, DOUBLE_ULP, FLOAT_DIVISOR};

const MULTIPLIER: i64 = 0x5DEECE66D;
const ADDEND: i64 = 0xB;
const MASK: i64 = (1i64 << 48) - 1;

/// Java's `java.util.Random`: a 48-bit linear congruential generator.
#[derive(Debug, Clone)]
pub struct LegacyRandom {
    seed: i64,
    next_next_gaussian: Option<f64>,
}

impl LegacyRandom {
    pub fn new(seed: i64) -> Self {
        let mut r = Self {
            seed: 0,
            next_next_gaussian: None,
        };
        r.set_seed(seed);
        r
    }
}

impl RandomSource for LegacyRandom {
    fn next_bits(&mut self, bits: u32) -> i32 {
        debug_assert!(bits <= 32);
        self.seed = self.seed.wrapping_mul(MULTIPLIER).wrapping_add(ADDEND) & MASK;
        // `>>>` in Java: seed is always non-negative here (masked to 48 bits) so a plain
        // signed shift is already equivalent, but go via u64 to make the "unsigned shift"
        // intent explicit and safe if that invariant is ever broken.
        ((self.seed as u64) >> (48 - bits)) as i32
    }

    fn next_int(&mut self) -> i32 {
        self.next_bits(32)
    }

    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "bound must be positive");
        if (bound & bound.wrapping_neg()) == bound {
            // Power-of-two fast path: `(bound * (long) next(31)) >> 31`.
            return ((bound as i64).wrapping_mul(self.next_bits(31) as i64) >> 31) as i32;
        }
        loop {
            let bits = self.next_bits(31);
            let val = bits % bound;
            // Java: `while (bits - val + (bound - 1) < 0)` — wrapping i32 arithmetic,
            // rejects on overflow to keep the distribution unbiased.
            if bits.wrapping_sub(val).wrapping_add(bound - 1) >= 0 {
                return val;
            }
        }
    }

    fn next_int_between(&mut self, min: i32, max: i32) -> i32 {
        min + self.next_int_bounded(max - min + 1)
    }

    fn next_long(&mut self) -> i64 {
        let hi = self.next_bits(32) as i64;
        let lo = self.next_bits(32) as i64;
        (hi << 32).wrapping_add(lo)
    }

    fn next_boolean(&mut self) -> bool {
        self.next_bits(1) != 0
    }

    fn next_float(&mut self) -> f32 {
        self.next_bits(24) as f32 / FLOAT_DIVISOR
    }

    fn next_double(&mut self) -> f64 {
        let hi = self.next_bits(26) as i64;
        let lo = self.next_bits(27) as i64;
        (((hi << 27) + lo) as f64) * DOUBLE_ULP
    }

    fn next_gaussian(&mut self) -> f64 {
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
        self.seed = (seed ^ MULTIPLIER) & MASK;
        self.next_next_gaussian = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // All expected values below were captured from real `java.util.Random` (OpenJDK 25) —
    // see the session's scratchpad `javaref/Ref.java`. `nextInt`/`nextInt(bound)`/`nextLong`/
    // `nextBoolean`/`nextFloat`/`nextDouble` are exact-integer/bit operations so equality is
    // exact; `nextGaussian` involves `ln`/`sqrt` so it's asserted with an epsilon (see
    // // PARITY-CHECK in rng/mod.rs).

    #[test]
    fn next_int_sequence_seed_42() {
        let mut r = LegacyRandom::new(42);
        let expected = [-1170105035, 234785527, -1360544799, 205897768, 1325939940];
        for e in expected {
            assert_eq!(r.next_int(), e);
        }
    }

    #[test]
    fn next_int_sequence_seed_0() {
        let mut r = LegacyRandom::new(0);
        let expected = [-1155484576, -723955400, 1033096058];
        for e in expected {
            assert_eq!(r.next_int(), e);
        }
    }

    #[test]
    fn next_int_bounded_power_of_two() {
        let mut r = LegacyRandom::new(42);
        let expected = [11, 0, 10, 0, 4];
        for e in expected {
            assert_eq!(r.next_int_bounded(16), e);
        }
    }

    #[test]
    fn next_int_bounded_non_power_of_two() {
        let mut r = LegacyRandom::new(42);
        let expected = [0, 3, 8, 4, 0];
        for e in expected {
            assert_eq!(r.next_int_bounded(10), e);
        }
    }

    /// Exercises the rejection loop repeatedly (bound=7 is not a power of two).
    #[test]
    fn next_int_bounded_rejection_seed_1() {
        let mut r = LegacyRandom::new(1);
        let expected = [4, 4, 1, 0, 6, 6, 0, 1, 3, 6];
        for e in expected {
            assert_eq!(r.next_int_bounded(7), e);
        }
    }

    #[test]
    fn next_long_sequence_seed_42() {
        let mut r = LegacyRandom::new(42);
        let expected: [i64; 3] = [
            -5025562857975149833,
            -5843495416241995736,
            5694868678511409995,
        ];
        for e in expected {
            assert_eq!(r.next_long(), e);
        }
    }

    #[test]
    fn next_boolean_sequence_seed_42() {
        let mut r = LegacyRandom::new(42);
        let expected = [true, false, true, false, false];
        for e in expected {
            assert_eq!(r.next_boolean(), e);
        }
    }

    #[test]
    fn next_float_sequence_seed_42() {
        let mut r = LegacyRandom::new(42);
        let expected: [f32; 3] = [0.7275637, 0.054665208, 0.6832234];
        for e in expected {
            assert_eq!(r.next_float(), e);
        }
    }

    #[test]
    fn next_double_sequence_seed_42() {
        let mut r = LegacyRandom::new(42);
        let expected = [0.7275636800328681, 0.6832234717598454, 0.30871945533265976];
        for e in expected {
            assert_eq!(r.next_double(), e);
        }
    }

    #[test]
    fn next_gaussian_first_two_seed_42() {
        let mut r = LegacyRandom::new(42);
        let expected = [1.1419053154730547, 0.9194079489827879];
        for e in expected {
            let got = r.next_gaussian();
            assert!((got - e).abs() < 1e-12, "got {got}, expected {e}");
        }
    }

    #[test]
    fn next_int_bounded_rejects_non_positive() {
        let mut r = LegacyRandom::new(1);
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.next_int_bounded(0)));
        assert!(result.is_err());
    }
}
