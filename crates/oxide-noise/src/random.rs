//! Flavour-erasing wrapper so `PerlinNoise`/`NormalNoise` construction doesn't need to be
//! generic over which RNG a `noise_settings` entry picked (`legacy_random_source`).
//!
//! Vanilla's `RandomSource.forkPositional()` derives a *new* positional factory by consuming
//! output from an existing RNG instance — that's what turns a single world-seed RNG into the
//! per-named-noise and per-octave RNGs used throughout the noise router. `oxide_core::rng`
//! doesn't declare `forkPositional` on its `RandomSource` trait (it's routing/wiring, not a
//! primitive), so it's reconstructed here per flavour instead of touching that crate.

use oxide_core::{
    LegacyPositionalRandomFactory, LegacyRandom, PositionalRandomFactory, RandomSource,
    Xoroshiro128PlusPlus, XoroshiroPositionalRandomFactory,
};

/// Either RNG flavour a `noise_settings` entry can select (`legacy_random_source`).
#[derive(Debug, Clone)]
pub enum WorldRandom {
    Xoroshiro(Xoroshiro128PlusPlus),
    Legacy(LegacyRandom),
}

impl WorldRandom {
    /// Seeds a world-flavour RNG directly from the world seed, matching vanilla's
    /// `RandomSupplier` (`XoroshiroRandomSource::new` / `LegacyRandomSource::new`).
    pub fn new(seed: i64, legacy: bool) -> Self {
        if legacy {
            Self::Legacy(LegacyRandom::new(seed))
        } else {
            Self::Xoroshiro(Xoroshiro128PlusPlus::new(seed))
        }
    }

    // PARITY-CHECK: `forkPositional`'s exact consumption (nextLong x2 for Xoroshiro's
    // (lo, hi) pair vs nextLong x1 for legacy) is reconstructed from memory of
    // `XoroshiroRandomSource`/`LegacyRandomSource`, not verified against decompiled 26.2.
    pub fn fork_positional(&mut self) -> WorldPositionalFactory {
        match self {
            Self::Xoroshiro(r) => {
                let lo = r.next_long() as u64;
                let hi = r.next_long() as u64;
                WorldPositionalFactory::Xoroshiro(XoroshiroPositionalRandomFactory::new(lo, hi))
            }
            Self::Legacy(r) => {
                WorldPositionalFactory::Legacy(LegacyPositionalRandomFactory::new(r.next_long()))
            }
        }
    }
}

impl RandomSource for WorldRandom {
    fn next_bits(&mut self, bits: u32) -> i32 {
        match self {
            Self::Xoroshiro(r) => r.next_bits(bits),
            Self::Legacy(r) => r.next_bits(bits),
        }
    }
    fn next_int(&mut self) -> i32 {
        match self {
            Self::Xoroshiro(r) => r.next_int(),
            Self::Legacy(r) => r.next_int(),
        }
    }
    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        match self {
            Self::Xoroshiro(r) => r.next_int_bounded(bound),
            Self::Legacy(r) => r.next_int_bounded(bound),
        }
    }
    fn next_int_between(&mut self, min: i32, max: i32) -> i32 {
        match self {
            Self::Xoroshiro(r) => r.next_int_between(min, max),
            Self::Legacy(r) => r.next_int_between(min, max),
        }
    }
    fn next_long(&mut self) -> i64 {
        match self {
            Self::Xoroshiro(r) => r.next_long(),
            Self::Legacy(r) => r.next_long(),
        }
    }
    fn next_boolean(&mut self) -> bool {
        match self {
            Self::Xoroshiro(r) => r.next_boolean(),
            Self::Legacy(r) => r.next_boolean(),
        }
    }
    fn next_float(&mut self) -> f32 {
        match self {
            Self::Xoroshiro(r) => r.next_float(),
            Self::Legacy(r) => r.next_float(),
        }
    }
    fn next_double(&mut self) -> f64 {
        match self {
            Self::Xoroshiro(r) => r.next_double(),
            Self::Legacy(r) => r.next_double(),
        }
    }
    fn next_gaussian(&mut self) -> f64 {
        match self {
            Self::Xoroshiro(r) => r.next_gaussian(),
            Self::Legacy(r) => r.next_gaussian(),
        }
    }
    fn consume(&mut self, count: i32) {
        match self {
            Self::Xoroshiro(r) => r.consume(count),
            Self::Legacy(r) => r.consume(count),
        }
    }
    fn set_seed(&mut self, seed: i64) {
        match self {
            Self::Xoroshiro(r) => r.set_seed(seed),
            Self::Legacy(r) => r.set_seed(seed),
        }
    }
}

/// Either positional-factory flavour, produced by [`WorldRandom::fork_positional`].
#[derive(Debug, Clone)]
pub enum WorldPositionalFactory {
    Xoroshiro(XoroshiroPositionalRandomFactory),
    Legacy(LegacyPositionalRandomFactory),
}

impl WorldPositionalFactory {
    pub fn from_hash_of(&self, name: &str) -> WorldRandom {
        match self {
            Self::Xoroshiro(f) => WorldRandom::Xoroshiro(f.from_hash_of(name)),
            Self::Legacy(f) => WorldRandom::Legacy(f.from_hash_of(name)),
        }
    }

    pub fn at(&self, x: i32, y: i32, z: i32) -> WorldRandom {
        match self {
            Self::Xoroshiro(f) => WorldRandom::Xoroshiro(f.at(x, y, z)),
            Self::Legacy(f) => WorldRandom::Legacy(f.at(x, y, z)),
        }
    }
}
