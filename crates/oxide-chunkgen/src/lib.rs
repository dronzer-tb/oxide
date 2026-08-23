//! oxide-chunkgen — see `docs/ARCHITECTURE.md`.
//!
//! Noise-based terrain fill and heightmap computation — wave 3 per `docs/ROADMAP.md`.
//!
//! Scope cut, not fabricated: aquifers, ore veins, and carvers are each a distinct, genuinely
//! large vanilla subsystem and are not built yet — see `fill.rs`'s module doc. Surface rules
//! (grass/dirt/sand/bedrock) *are* evaluated now, with two named gaps — see `surface.rs`.

mod aquifer;
mod biome_grid;
mod carver;
mod fill;
mod ore_veins;
mod surface;

pub use carver::{apply_carvers, CarverWorld};
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
    carvers: Option<&CarverSetup<'_>>,
) -> ChunkData {
    let caches = router.chunk_caches(pos.x, pos.z);
    // One aquifer for the whole chunk, shared by the fill and the carvers -- vanilla's carvers
    // consult the same aquifer the fill used, which is what makes a cave that breaks into a
    // flooded aquifer fill with water instead of staying dry.
    let mut aquifer = aquifer::for_settings(pos, settings, router, &caches);
    let mut chunk = fill::fill_chunk_with(pos, settings, router, biomes, &caches, aquifer.as_mut());
    // Before the surface pass, as vanilla orders it: veins run inside the noise fill, so the
    // surface rules see whatever the veins left.
    ore_veins::apply_ore_veins(&mut chunk, pos, settings, router, &caches);
    SurfaceSystem::new(settings, router, biome_temperatures).apply(&mut chunk, pos, &caches);
    chunk.status = ChunkStatus::Surface;

    // Vanilla's order: noise, then surface, then carvers. Carving after the surface pass is
    // what lets a cave cut through the grass and dirt the surface rules just placed.
    if let Some(setup) = carvers {
        let world = CarverWorld {
            settings,
            seed: setup.seed,
        };
        apply_carvers(&mut chunk, pos, &world, setup.carvers_at, aquifer.as_mut());
        // Carving removes blocks, so the heightmaps the fill pass computed are stale -- a
        // column whose surface was carved away now reports a surface that is not there.
        // Vanilla keeps its heightmaps updated as each block changes; recomputing once after
        // carving reaches the same end state.
        chunk.heightmaps = crate::fill::compute_heightmaps(&chunk, settings);
        chunk.status = ChunkStatus::Carvers;
    }
    chunk
}

/// What carving needs beyond the chunk: the world seed, and which carvers a source chunk's
/// biome configures.
pub struct CarverSetup<'a> {
    pub seed: i64,
    pub carvers_at: &'a dyn Fn(ChunkPos) -> Vec<oxide_datapack::ConfiguredCarver>,
}
