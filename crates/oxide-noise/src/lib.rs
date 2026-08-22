//! oxide-noise — see `docs/ARCHITECTURE.md`.
//!
//! Perlin / improved-noise / normal-noise primitives, the density-function interpreter, and the
//! noise router: turns an `oxide-datapack`-loaded `NoiseGeneratorSettings` plus a world seed
//! into `f64` samples at a block position. Consumed by `oxide-biome` (climate sampling) and
//! `oxide-chunkgen` (terrain fill) — wave 3, not built yet.
//!
//! Bit-exactness with vanilla Java is unverified in this crate — every reconstructed constant
//! is marked `// PARITY-CHECK` per `docs/ARCHITECTURE.md` § RNG is load-bearing. The gate for
//! wave 2 → 3 (`docs/ROADMAP.md`) is the density-function interpreter reproducing sampled values
//! from a real vanilla reference dump; nothing here is trusted until that's run.

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
