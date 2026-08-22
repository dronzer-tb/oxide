//! oxide-noise — see `docs/ARCHITECTURE.md`.
//!
//! Perlin / improved-noise / normal-noise primitives, the density-function interpreter, and the
//! noise router: turns an `oxide-datapack`-loaded `NoiseGeneratorSettings` plus a world seed
//! into `f64` samples at a block position. Consumed by `oxide-biome` (climate sampling) and
//! `oxide-chunkgen` (terrain fill).
//!
//! The noise primitives (`ImprovedNoise`, `PerlinNoise`, `NormalNoise`) are verified bit-exact
//! against real Minecraft 26.2 output as of 2026-08-22: decompiled from the real server jar,
//! transcribed standalone, and run on real OpenJDK 25 to produce the exact-value test vectors
//! in each module's tests (see `docs/REFERENCE_DATA.md`). The density-function interpreter's
//! well-known ops (arithmetic, clamp, spline, shift) are individually verified the same way;
//! the handful that are genuinely still reconstructed from memory (or that don't exist in a
//! real 26.2 datapack at all, like `weird_scaled_sampler`) stay `// PARITY-CHECK`-flagged
//! per-node in `density.rs`.

mod density;
mod improved_noise;
mod normal_noise;
mod perlin_noise;
mod random;
mod router;

pub use density::{evaluate, EvalCtx, FunctionContext};
pub use improved_noise::ImprovedNoise;
pub use normal_noise::NormalNoise;
pub use perlin_noise::PerlinNoise;
pub use random::{WorldPositionalFactory, WorldRandom};
pub use router::{NoiseRouterEvaluator, RouterSlot};
