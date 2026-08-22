//! oxide-chunkgen — see `docs/ARCHITECTURE.md`.
//!
//! Noise-based terrain fill and heightmap computation — wave 3 per `docs/ROADMAP.md`.
//!
//! Scope cut, not fabricated: aquifers, ore veins, carvers, and surface-rule evaluation
//! (grass/dirt/sand block variety) are each a distinct, genuinely large vanilla subsystem and
//! are not built yet — see `fill.rs`'s module doc. What's here is enough to satisfy the wave
//! 3 -> 4 gate (a generated chunk opens in a vanilla client without corruption), not full
//! generation quality.

mod biome_grid;
mod fill;

pub use fill::fill_chunk;
