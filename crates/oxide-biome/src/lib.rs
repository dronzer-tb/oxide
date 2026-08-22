//! oxide-biome — see `docs/ARCHITECTURE.md`.
//!
//! Climate-parameter sampling ([`ClimateSample`]) and the multi-noise nearest-biome search
//! ([`BiomeSearchTree`]) — wave 3 per `docs/ROADMAP.md`. Consumed by `oxide-chunkgen` to
//! assign a biome per quart-cell while filling a chunk.

mod climate;
mod search;

pub use climate::ClimateSample;
pub use search::BiomeSearchTree;
