//! oxide-structures — see `docs/ARCHITECTURE.md`.
//!
//! Structure-set placement (`minecraft:random_spread`) and weighted start selection — wave 4
//! per `docs/ROADMAP.md`. This is the crate's highest-risk wave per the roadmap (it depends on
//! RNG exactness, biome resolution, and chunkgen all being correct first, and the Xoroshiro
//! positional RNG it doesn't even need is still `// PARITY-CHECK`-unverified upstream).
//!
//! Scope cuts, documented not fabricated: `minecraft:concentric_rings` placement, frequency
//! reduction, exclusion zones, and jigsaw piece layout (pool walking, connector matching,
//! actual NBT piece placement) are not built. What's here answers "does this chunk get a
//! structure-start attempt, and which structure" — not "what gets placed there".

mod placement;
mod select;

pub use placement::{is_random_spread_chunk, potential_structure_chunk};
pub use select::pick_weighted;
