//! Positional random factories: derive a per-position or per-name RNG from a base seed.

use super::xoroshiro::upgrade_seed_to_128bit;
use super::{LegacyRandom, RandomSource, Xoroshiro128PlusPlus};
use md5::{Digest, Md5};

/// Derives a per-`(x, y, z)` or per-name RNG from a base seed. Two flavours exist because the
/// legacy and Xoroshiro RNGs mix position/name into their seed differently.
pub trait PositionalRandomFactory {
    type Rng: RandomSource;
    fn at(&self, x: i32, y: i32, z: i32) -> Self::Rng;
    // Name is fixed by the crate's public API contract (docs/ARCHITECTURE.md); it takes
    // `&self` deliberately (derives an RNG from the factory's base seed), not `Self`.
    #[allow(clippy::wrong_self_convention)]
    fn from_hash_of(&self, name: &str) -> Self::Rng;
}

/// Mojang's `Mth.getSeed(x, y, z)`: mixes a block position into a single 64-bit value. Shared
/// by both factory flavours — the legacy XORs it directly into a 64-bit seed, Xoroshiro XORs
/// it into the low 128-bit state half (see `// PARITY-CHECK` on the Xoroshiro factory below).
fn mth_get_seed(x: i32, y: i32, z: i32) -> i64 {
    let mut l =
        (x.wrapping_mul(3_129_871) as i64) ^ ((z as i64).wrapping_mul(116_129_781)) ^ (y as i64);
    l = l
        .wrapping_mul(l)
        .wrapping_mul(42_317_861)
        .wrapping_add(l.wrapping_mul(11));
    l >> 16
}

/// MD5-digests `name` and returns its first two 8-byte big-endian halves as signed 64-bit
/// values (Guava's `Longs.fromBytes`, big-endian with the first byte most significant).
/// Verified 2026-08-22 against decompiled `RandomSupport.seedFromHashOf` — used by
/// `XoroshiroPositionalRandomFactory::from_hash_of` only; `LegacyPositionalRandomFactory`
/// doesn't hash at all, see `java_string_hash_code` below.
fn md5_halves(name: &str) -> (i64, i64) {
    let digest = Md5::digest(name.as_bytes());
    let hi = i64::from_be_bytes(digest[0..8].try_into().expect("md5 digest is 16 bytes"));
    let lo = i64::from_be_bytes(digest[8..16].try_into().expect("md5 digest is 16 bytes"));
    (hi, lo)
}

/// Java's `String.hashCode()`: `s[0]*31^(n-1) + s[1]*31^(n-2) + ... + s[n-1]`, computed over
/// UTF-16 code units with wrapping 32-bit arithmetic. Verified 2026-08-22 against decompiled
/// `LegacyRandomSource.LegacyPositionalRandomFactory.fromHashOf` — it uses `name.hashCode()`
/// directly, not MD5 (MD5 is Xoroshiro's `fromHashOf` only, via `RandomSupport.seedFromHashOf`).
fn java_string_hash_code(name: &str) -> i32 {
    name.encode_utf16()
        .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
}

/// Positional factory backed by `LegacyRandom` (`java.util.Random`-style).
#[derive(Debug, Clone, Copy)]
pub struct LegacyPositionalRandomFactory {
    seed: i64,
}

impl LegacyPositionalRandomFactory {
    pub fn new(seed: i64) -> Self {
        Self { seed }
    }
}

impl PositionalRandomFactory for LegacyPositionalRandomFactory {
    type Rng = LegacyRandom;

    fn at(&self, x: i32, y: i32, z: i32) -> LegacyRandom {
        LegacyRandom::new(mth_get_seed(x, y, z) ^ self.seed)
    }

    // Verified 2026-08-22: vanilla's `LegacyPositionalRandomFactory.fromHashOf` is
    // `(long) name.hashCode() ^ this.seed` — Java's `String.hashCode()`, not MD5. MD5 is
    // Xoroshiro's `fromHashOf` only (see below).
    fn from_hash_of(&self, name: &str) -> LegacyRandom {
        LegacyRandom::new(java_string_hash_code(name) as i64 ^ self.seed)
    }
}

/// Positional factory backed by `Xoroshiro128PlusPlus`.
#[derive(Debug, Clone, Copy)]
pub struct XoroshiroPositionalRandomFactory {
    seed_lo: u64,
    seed_hi: u64,
}

impl XoroshiroPositionalRandomFactory {
    pub fn new(seed_lo: u64, seed_hi: u64) -> Self {
        Self { seed_lo, seed_hi }
    }

    /// Builds from a single 64-bit legacy-style seed via the standard 128-bit upgrade path.
    pub fn from_seed(seed: i64) -> Self {
        let (lo, hi) = upgrade_seed_to_128bit(seed);
        Self {
            seed_lo: lo,
            seed_hi: hi,
        }
    }
}

// Verified 2026-08-22 against decompiled `XoroshiroRandomSource.XoroshiroPositionalRandomFactory`:
// `at` XORs `Mth.getSeed(x,y,z)` into `seedLo` only, leaving `seedHi` unchanged — exactly what
// `at` below does. `fromHashOf` XORs each MD5 half into the corresponding state half via
// `RandomSupport.seedFromHashOf(name).xor(seedLo, seedHi)` — matches `from_hash_of` below.
impl PositionalRandomFactory for XoroshiroPositionalRandomFactory {
    type Rng = Xoroshiro128PlusPlus;

    fn at(&self, x: i32, y: i32, z: i32) -> Xoroshiro128PlusPlus {
        let pos_seed = mth_get_seed(x, y, z) as u64;
        Xoroshiro128PlusPlus::from_state(pos_seed ^ self.seed_lo, self.seed_hi)
    }

    fn from_hash_of(&self, name: &str) -> Xoroshiro128PlusPlus {
        let (hi, lo) = md5_halves(name);
        Xoroshiro128PlusPlus::from_state(hi as u64 ^ self.seed_lo, lo as u64 ^ self.seed_hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // MD5("minecraft:test") captured from real Java `MessageDigest.getInstance("MD5")` —
    // see the session's scratchpad `javaref/Ref.java`. This part (raw MD5 + big-endian byte
    // packing) has no Mojang-specific ambiguity, so it's asserted exactly.
    #[test]
    fn md5_halves_known_vector() {
        let (hi, lo) = md5_halves("minecraft:test");
        assert_eq!(hi, 1885711875966874721);
        assert_eq!(lo, 3357099975631595687);
    }

    #[test]
    fn legacy_at_is_deterministic_and_position_dependent() {
        let f = LegacyPositionalRandomFactory::new(42);
        let mut a = f.at(1, 2, 3);
        let mut b = f.at(1, 2, 3);
        let mut c = f.at(1, 2, 4);
        assert_eq!(a.next_int(), b.next_int());
        assert_ne!(f.at(1, 2, 3).next_int(), c.next_int());
    }

    #[test]
    fn legacy_from_hash_of_is_deterministic() {
        let f = LegacyPositionalRandomFactory::new(0);
        let mut a = f.from_hash_of("minecraft:village");
        let mut b = f.from_hash_of("minecraft:village");
        assert_eq!(a.next_long(), b.next_long());
    }

    #[test]
    fn xoroshiro_at_is_deterministic_and_position_dependent() {
        let f = XoroshiroPositionalRandomFactory::from_seed(42);
        let mut a = f.at(5, 6, 7);
        let mut b = f.at(5, 6, 7);
        assert_eq!(a.next_int(), b.next_int());
        assert_ne!(f.at(5, 6, 7).next_int(), f.at(5, 6, 8).next_int());
    }

    #[test]
    fn xoroshiro_from_hash_of_is_deterministic() {
        let f = XoroshiroPositionalRandomFactory::from_seed(0);
        let mut a = f.from_hash_of("minecraft:desert_pyramid");
        let mut b = f.from_hash_of("minecraft:desert_pyramid");
        assert_eq!(a.next_long(), b.next_long());
    }
}
