//! oxide-structures — see `docs/ARCHITECTURE.md`.
//!
//! Structure-set placement (`minecraft:random_spread`, both `linear` and `triangular` spread)
//! and weighted start selection — wave 4 per `docs/ROADMAP.md`. `potential_structure_chunk` is
//! verified bit-exact against real Minecraft 26.2 output (decompiled + run on real OpenJDK 25,
//! see `placement.rs`'s tests) — it only ever uses `LegacyRandom`, regardless of a world's
//! noise-generator RNG flavor, so it doesn't depend on the Xoroshiro positional RNG this crate
//! doesn't otherwise need.
//!
//! Scope cuts, documented not fabricated: `minecraft:concentric_rings` placement, frequency
//! reduction, exclusion zones, and jigsaw piece layout (pool walking, connector matching,
//! actual NBT piece placement) are not built. What's here answers "does this chunk get a
//! structure-start attempt, and which structure" — not "what gets placed there".

mod frequency;
pub mod jigsaw;
mod placement;
pub mod pool;
mod select;
mod set;
pub mod template;

pub use frequency::{passes_frequency, should_generate};
pub use jigsaw::{assemble_jigsaw, AssembledPiece, AssembledStructure, BoundingBox};
pub use placement::{is_random_spread_chunk, potential_structure_chunk};
pub use pool::{PoolElement, PoolElementEntry, TemplatePool};
pub use select::pick_weighted;
pub use set::{is_structure_chunk, is_structure_chunk_supported, StructureSetLookup};
pub use template::{JigsawConnector, JigsawJoint, PlacedTemplateBlock, StructureTemplate};
