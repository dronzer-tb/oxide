//! RNG primitives. Bit-exactness with vanilla Java is the whole point of this module — see
//! `docs/ARCHITECTURE.md` § "RNG is load-bearing". Every method here is implemented
//! independently per RNG type (no shared default trait methods) because the exact bit
//! manipulation differs between `LegacyRandom` and `Xoroshiro128PlusPlus`, and silently
//! sharing an implementation is exactly the kind of bug this crate exists to avoid.

mod legacy;
mod positional;
mod xoroshiro;

pub use legacy::LegacyRandom;
pub use positional::{
    LegacyPositionalRandomFactory, PositionalRandomFactory, XoroshiroPositionalRandomFactory,
};
pub use xoroshiro::Xoroshiro128PlusPlus;

/// Common interface implemented by every RNG source, matching Minecraft's `RandomSource`.
pub trait RandomSource {
    /// Returns the next `bits` high-order bits of the underlying generator as a
    /// non-negative `i32` (Java's `protected int next(int bits)`).
    fn next_bits(&mut self, bits: u32) -> i32;
    fn next_int(&mut self) -> i32;
    /// `bound` must be positive (matches Java's `IllegalArgumentException` contract);
    /// panics otherwise.
    fn next_int_bounded(&mut self, bound: i32) -> i32;
    /// Inclusive on both ends: `[min, max]`.
    fn next_int_between(&mut self, min: i32, max: i32) -> i32;
    fn next_long(&mut self) -> i64;
    fn next_boolean(&mut self) -> bool;
    fn next_float(&mut self) -> f32;
    fn next_double(&mut self) -> f64;
    fn next_gaussian(&mut self) -> f64;
    /// Advances the generator `count` times, discarding output (`RandomSource.consumeCount`).
    fn consume(&mut self, count: i32);
    fn set_seed(&mut self, seed: i64);
}

/// `1.0 / 2^53`, the ULP used by `nextDouble`'s bit layout (`0x1.0p-53` in Java). Written as
/// an exact division by a power of two so the constant is bit-exact, not `powi`-approximated.
pub(crate) const DOUBLE_ULP: f64 = 1.0 / 9_007_199_254_740_992.0;

/// Java's `float` ULP for `nextFloat`: `1 / 2^24`.
pub(crate) const FLOAT_DIVISOR: f32 = 16_777_216.0;

// Note: the polar (Marsaglia) Gaussian method — matching `java.util.Random.nextGaussian` /
// Minecraft's `MarsagliaPolarGaussian` — is implemented separately in `legacy.rs` and
// `xoroshiro.rs` rather than shared here. Each needs to call its own `next_double` while also
// holding `&mut self.next_next_gaussian`; going through a shared free function taking a
// `FnMut() -> f64` closure would force the closure to capture all of `self` (method calls
// can't do disjoint field capture), which conflicts with the separate cache-field borrow. The
// ~12-line duplication is cheaper than fighting the borrow checker over it.
//
// // PARITY-CHECK: Rust's `f64::ln`/`f64::sqrt` are not guaranteed bit-identical to Java's
// // `Math.log`/`Math.sqrt` on every platform (sqrt is IEEE-754 correctly-rounded on both and
// // should match; `ln` uses different libm implementations and can differ by ~1 ULP). Tests
// // assert `next_gaussian` against real `java.util.Random` output with an epsilon, not
// // bit-for-bit equality.
