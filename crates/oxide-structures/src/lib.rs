//! oxide-structures — see `docs/ARCHITECTURE.md`.
//!
//! Four layers, at very different levels of confidence. Do not collapse them in a status
//! report:
//!
//! 1. **Placement — verified.** `random_spread` start-chunk selection (`placement.rs`, both
//!    `linear` and `triangular`) and frequency reduction (`frequency.rs`, all four methods) are
//!    bit-exact against real Minecraft 26.2, decompiled and run on real OpenJDK 25. Exclusion
//!    zones (`set.rs`) are ported from the same decompile. All of it uses `LegacyRandom` only,
//!    whatever RNG flavor the world's noise generator is set to.
//! 2. **Placement — unverified.** `concentric_rings` (strongholds) omits vanilla's
//!    `findBiomeHorizontal` search entirely and uses the `Mth` sine table where vanilla uses
//!    `Math.sin`/`Math.cos` on doubles. It answers "roughly where", not "where".
//! 3. **Start bookkeeping (`starts.rs`) — built and tested, order-independent by construction.**
//!    Which starts can reach a chunk, memoised across chunks and threads, and stamping them.
//!    Correct as *plumbing*; it is only as faithful as the assembler handed to it.
//! 4. **Jigsaw assembly (`jigsaw.rs`) — a stub, not a port.** No rotation (the jigsaw block's
//!    `orientation` is never read), no connector `name`/`target` matching, no child-origin
//!    offset, no processors, no `VoxelShape` collision, `Single` pool elements only. It produces
//!    a plausible pile of pieces, not vanilla's pile. Nothing outside this crate calls it, and
//!    nothing should until those are built.

mod frequency;
pub mod jigsaw;
mod placement;
pub mod pool;
mod select;
mod set;
pub mod starts;
pub mod template;

pub use frequency::{passes_frequency, should_generate};
pub use jigsaw::{assemble_jigsaw, AssembledPiece, AssembledStructure, BoundingBox};
pub use placement::{is_random_spread_chunk, potential_structure_chunk};
pub use pool::{PoolElement, PoolElementEntry, TemplatePool};
pub use select::pick_weighted;
pub use starts::{
    candidate_starts_in_area, ChunkArea, StartAssembler, StartContext, StructureStart,
    StructureStartCache,
};
pub use set::{is_structure_chunk, is_structure_chunk_supported, StructureSetLookup};
pub use template::{JigsawConnector, JigsawJoint, PlacedTemplateBlock, StructureTemplate};
