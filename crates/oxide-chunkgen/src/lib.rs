//! oxide-chunkgen — see `docs/ARCHITECTURE.md`.
//!
//! Noise-based terrain fill and heightmap computation — wave 3 per `docs/ROADMAP.md`.
//!
//! Scope cut, not fabricated: aquifers, ore veins, and carvers are each a distinct, genuinely
//! large vanilla subsystem and are not built yet — see `fill.rs`'s module doc. Surface rules
//! (grass/dirt/sand/bedrock) *are* evaluated now, with two named gaps — see `surface.rs`.

mod biome_grid;
mod fill;
mod surface;

pub use fill::fill_chunk;
pub use surface::{BiomeTemperatures, SurfaceSystem};

use oxide_biome::BiomeSearchTree;
use oxide_core::{ChunkData, ChunkPos, ChunkStatus};
use oxide_datapack::NoiseGeneratorSettings;
use oxide_noise::NoiseRouterEvaluator;

/// Noise fill followed by surface-rule evaluation: the full terrain pass this crate offers.
///
/// Heightmaps are the ones [`fill_chunk`] computed. Surface rules only ever replace one solid
/// block with another, so every heightmap predicate this crate models sees the same column
/// profile before and after -- recomputing them here would be work with no output difference.
pub fn generate_chunk(
    pos: ChunkPos,
    settings: &NoiseGeneratorSettings,
    router: &NoiseRouterEvaluator,
    biomes: Option<&BiomeSearchTree>,
    biome_temperatures: &BiomeTemperatures,
) -> ChunkData {
    let caches = router.chunk_caches(pos.x, pos.z);
    let mut chunk = fill_chunk(pos, settings, router, biomes);
    SurfaceSystem::new(settings, router, biome_temperatures).apply(&mut chunk, pos, &caches);
    chunk.status = ChunkStatus::Surface;
    chunk
}
