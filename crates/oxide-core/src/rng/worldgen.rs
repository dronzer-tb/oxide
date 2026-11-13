//! `net.minecraft.world.level.levelgen.WorldgenRandom` over a Xoroshiro source: the random
//! chunk decoration draws from.
//!
//! It is not a plain Xoroshiro. `WorldgenRandom` extends `LegacyRandomSource`, so every draw
//! goes through `BitRandomSource`'s `java.util.Random`-shaped recipes (31-bit bounded ints,
//! two draws per long, 24-bit floats), and only the raw bits come from the wrapped source:
//! `next(bits) = (int)(source.nextLong() >>> (64 - bits))`. Seeding the same Xoroshiro and
//! calling its own `nextInt(bound)` gives a different stream.
//!
//! Verified 2026-11-14 against decompiled 26.2 `WorldgenRandom` and `BitRandomSource`.

use super::{RandomSource, Xoroshiro128PlusPlus, DOUBLE_ULP, FLOAT_DIVISOR};

#[derive(Debug, Clone)]
pub struct WorldgenRandom {
    source: Xoroshiro128PlusPlus,
    /// `LegacyRandomSource.gaussianSource`'s cached second value. `WorldgenRandom.setSeed`
    /// overrides the legacy one without resetting it, so it survives a reseed.
    next_next_gaussian: Option<f64>,
}

impl WorldgenRandom {
    pub fn new(seed: i64) -> Self {
        Self { source: Xoroshiro128PlusPlus::new(seed), next_next_gaussian: None }
    }
}

impl RandomSource for WorldgenRandom {
    fn next_bits(&mut self, bits: u32) -> i32 {
        debug_assert!((1..=32).contains(&bits));
        ((self.source.next_long() as u64) >> (64 - bits)) as i32
    }

    fn next_int(&mut self) -> i32 {
        self.next_bits(32)
    }

    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "bound must be positive");
        if bound & (bound - 1) == 0 {
            return ((bound as i64).wrapping_mul(self.next_bits(31) as i64) >> 31) as i32;
        }
        loop {
            let sample = self.next_bits(31);
            let modulo = sample % bound;
            if sample.wrapping_sub(modulo).wrapping_add(bound - 1) >= 0 {
                return modulo;
            }
        }
    }

    fn next_int_between(&mut self, min: i32, max: i32) -> i32 {
        self.next_int_bounded(max - min + 1) + min
    }

    fn next_long(&mut self) -> i64 {
        let upper = self.next_bits(32) as i64;
        let lower = self.next_bits(32) as i64;
        (upper << 32).wrapping_add(lower)
    }

    fn next_boolean(&mut self) -> bool {
        self.next_bits(1) != 0
    }

    fn next_float(&mut self) -> f32 {
        self.next_bits(24) as f32 / FLOAT_DIVISOR
    }

    fn next_double(&mut self) -> f64 {
        let upper = self.next_bits(26) as i64;
        let lower = self.next_bits(27) as i64;
        ((upper << 27) + lower) as f64 * DOUBLE_ULP
    }

    fn next_gaussian(&mut self) -> f64 {
        if let Some(value) = self.next_next_gaussian.take() {
            return value;
        }
        loop {
            let x = 2.0 * self.next_double() - 1.0;
            let y = 2.0 * self.next_double() - 1.0;
            let d = x * x + y * y;
            if d < 1.0 && d != 0.0 {
                let multiplier = (-2.0 * d.ln() / d).sqrt();
                self.next_next_gaussian = Some(y * multiplier);
                return x * multiplier;
            }
        }
    }

    fn consume(&mut self, count: i32) {
        for _ in 0..count {
            self.next_int();
        }
    }

    fn set_seed(&mut self, seed: i64) {
        self.source.set_seed(seed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_are_bit_recipes_over_the_xoroshiro_stream() {
        let mut random = WorldgenRandom::new(42);
        let mut raw = Xoroshiro128PlusPlus::new(42);
        let mut top = |bits: u32| ((raw.next_long() as u64) >> (64 - bits)) as i32;

        assert_eq!(random.next_int_bounded(16), ((16i64 * top(31) as i64) >> 31) as i32);
        let (upper, lower) = (top(32) as i64, top(32) as i64);
        assert_eq!(random.next_long(), (upper << 32).wrapping_add(lower));
        assert_eq!(random.next_float(), top(24) as f32 / FLOAT_DIVISOR);
        assert_eq!(random.next_boolean(), top(1) != 0);
    }

    #[test]
    fn a_reseed_keeps_the_cached_gaussian() {
        let mut random = WorldgenRandom::new(1);
        random.next_gaussian();
        let cached = random.next_next_gaussian;
        assert!(cached.is_some());
        random.set_seed(2);
        assert_eq!(random.next_gaussian(), cached.unwrap());
    }
}
