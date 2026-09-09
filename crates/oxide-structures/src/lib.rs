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
//! 4. **Jigsaw assembly (`jigsaw.rs`) — ported, exercised on real data, not yet parity-checked.**
//!    A transcription of 26.2's `JigsawPlacement.addPieces`/`Placer.tryPlacingChildren`:
//!    rotations, `canAttach` connector matching, child-origin offsets, the free-space region,
//!    `terrain_matching` projection, the expansion hack, and priority-ordered expansion. It
//!    builds 60-100-piece plains villages out of Mojang's own pools and `.nbt` templates
//!    (`tests/village_assembly.rs`). Its primitives — `transform`, `Util.shuffle`,
//!    `Rotation.getShuffled`/`getRandom` — are bit-exact against the game
//!    (`tests/jigsaw_math_parity.rs`). What is *not* established is that a given seed produces
//!    vanilla's village: that needs a generated world to diff against, and until then the piece
//!    layout is "correct by construction", not "verified".
//!
//! Still not ported, each a divergence rather than a shortcut: structure processors, `list` and
//! `feature` pool elements, pool aliases, the `start_jigsaw_name` anchor, dimension padding, and
//! liquid settings. Block-state rotation covers the standard directional properties; a rail's
//! `shape` and a few similar per-block cases come out unturned.

mod datapack;
mod frequency;
pub mod jigsaw;
mod placement;
pub mod pool;
pub mod rotation;
mod select;
mod set;
pub mod starts;
pub mod template;

pub use datapack::JigsawData;
pub use frequency::{passes_frequency, should_generate};
pub use jigsaw::{
    assemble_jigsaw, AssembledPiece, AssembledStructure, BoundingBox, FreeRegion, JigsawSettings,
    SurfaceHeights, TemplateSource,
};
pub use placement::{is_random_spread_chunk, potential_structure_chunk};
pub use pool::{PoolElement, PoolElementEntry, Projection, TemplatePool};
pub use select::pick_weighted;
pub use starts::{
    candidate_starts_in_area, ChunkArea, StartAssembler, StartContext, StructureStart,
    StructureStartCache,
};
pub use set::{is_structure_chunk, is_structure_chunk_supported, StructureSetLookup};
pub use rotation::{Direction, Mirror, Rotation};
pub use template::{JigsawConnector, JigsawJoint, PlacedTemplateBlock, StructureTemplate};
