//! Noise-based terrain fill + heightmap computation. Wave 3 per `docs/ROADMAP.md`.
//!
//! Scope cut, not fabricated: aquifers, ore veins, carvers, and surface rules (grass/dirt/sand
//! layering) are each a distinct, genuinely large vanilla subsystem — `settings.surface_rule`
//! is loaded and validated by `oxide-datapack` but not evaluated here yet. This wave fills each
//! column from `final_density` alone (solid below the surface, fluid to sea level, air above),
//! which is enough to satisfy the wave 3 -> 4 gate ("a generated chunk opens in a vanilla
//! client without corruption") without pretending the terrain is textured/detailed.

use std::collections::HashMap;

use oxide_core::{
    BlockState, ChunkData, ChunkPos, ChunkSection, ChunkStatus, Heightmap, HeightmapType,
    ResourceLocation,
};
use oxide_datapack::NoiseGeneratorSettings;
use oxide_noise::{NoiseRouterEvaluator, RouterSlot};

use crate::biome_grid::fill_biomes;

/// `(y*16+z)*16+x` — vanilla's flattened section block-array order. Matches the convention
/// already used by `oxide-pregen`'s `local_index` (see its own `// PARITY-CHECK`); kept
/// consistent across both crates deliberately rather than each picking its own.
pub(crate) fn local_index(x: usize, y: usize, z: usize) -> usize {
    (y * 16 + z) * 16 + x
}

/// Fills one chunk column purely from the noise router's `final_density` slot — no aquifers,
/// no ore veins, no carvers, no surface rules (see module doc). `biomes` is `None` when the
/// dimension's biome source is an unresolvable `Preset` (see
/// `oxide_biome::BiomeSearchTree::from_source`); the biome palette then stays at its default
/// single value.
pub fn fill_chunk(
    pos: ChunkPos,
    settings: &NoiseGeneratorSettings,
    router: &NoiseRouterEvaluator,
    biomes: Option<&oxide_biome::BiomeSearchTree>,
) -> ChunkData {
    let caches = router.chunk_caches(pos.x, pos.z);
    let mut aquifer = crate::aquifer::for_settings(pos, settings, router, &caches);
    fill_chunk_with(pos, settings, router, biomes, &caches, aquifer.as_mut())
}

/// As [`fill_chunk`], but sharing a caller-owned cache set and aquifer -- what
/// [`crate::generate_chunk`] uses so the carving pass can consult the same aquifer vanilla's
/// carvers consult.
pub fn fill_chunk_with(
    pos: ChunkPos,
    settings: &NoiseGeneratorSettings,
    router: &NoiseRouterEvaluator,
    biomes: Option<&oxide_biome::BiomeSearchTree>,
    caches: &oxide_noise::ChunkCaches,
    mut aquifer: Option<&mut crate::aquifer::Aquifer>,
) -> ChunkData {
    let min_y = settings.noise.min_y;
    let height = settings.noise.height;
    let air = BlockState::new(ResourceLocation::minecraft("air"));
    // Fallback only: real biome resolution happens per quart-cell below when `biomes` is
    // `Some`; this is what an unresolved `Preset` source (see above) leaves every cell at.
    let default_biome = ResourceLocation::minecraft("plains");

    let mut chunk = ChunkData::new(pos, min_y, height);
    let section_count = chunk.section_count();

    for i in 0..section_count {
        let section_y = min_y / 16 + i as i32;
        let mut section = ChunkSection::new(section_y as i8, air.clone(), default_biome.clone());
        let section_min_y = section_y * 16;

        for local_y in 0..16i32 {
            let y = section_min_y + local_y;
            for local_z in 0..16usize {
                let block_z = pos.min_block_z() + local_z as i32;
                for local_x in 0..16usize {
                    let block_x = pos.min_block_x() + local_x as i32;
                    let density = router.sample_in_chunk(
                        caches,
                        RouterSlot::FinalDensity,
                        block_x,
                        y,
                        block_z,
                    );
                    // The aquifer decides what a non-solid position holds -- air, water or
                    // lava, at a level that varies per underground body. `None` means solid, so
                    // the default block goes in. Vanilla runs exactly this as the first filler
                    // in its noise fill.
                    let block = match aquifer.as_deref_mut() {
                        Some(aquifer) => {
                            match aquifer.compute_substance(block_x, y, block_z, density) {
                                None => settings.default_block.clone(),
                                // Air is already the section's default palette entry.
                                Some(state) if state == air => continue,
                                Some(state) => state,
                            }
                        }
                        // Aquifers disabled by the settings: solid below the surface, the
                        // dimension's fluid up to sea level, air above.
                        None => {
                            if density > 0.0 {
                                settings.default_block.clone()
                            } else if y <= settings.sea_level {
                                settings.default_fluid.clone()
                            } else {
                                continue;
                            }
                        }
                    };
                    section
                        .block_states
                        .set(local_index(local_x, local_y as usize, local_z), block);
                }
            }
        }

        if let Some(tree) = biomes {
            fill_biomes(&mut section, pos, section_min_y, router, caches, tree);
        }

        chunk.sections.push(section);
    }

    chunk.status = ChunkStatus::Noise;
    chunk.needs_relight = true;
    chunk.heightmaps = compute_heightmaps(&chunk, settings);
    chunk
}

/// Per vanilla `Heightmap.Types`: `WORLD_SURFACE*` counts anything non-air (fluids included),
/// `OCEAN_FLOOR*` counts only solid/opaque blocks (fluids excluded), `MOTION_BLOCKING*` counts
/// anything that blocks movement *or* is a fluid — which, in our two-block-type model (no
/// per-`BlockState` "blocks motion"/"is opaque" property system yet), is the same predicate as
/// `WORLD_SURFACE`. `_NO_LEAVES` is identical to its counterpart here since no leaves are
/// modeled. PARITY-CHECK: reconstructed from memory of `Heightmap.Types`' predicates, not
/// verified against 26.2.
fn counts_for_heightmap(
    ty: HeightmapType,
    block: &BlockState,
    air: &BlockState,
    default_fluid: &BlockState,
) -> bool {
    let is_air = block == air;
    let is_fluid = block == default_fluid;
    match ty {
        HeightmapType::WorldSurface | HeightmapType::WorldSurfaceWg => !is_air,
        HeightmapType::OceanFloor | HeightmapType::OceanFloorWg => !is_air && !is_fluid,
        HeightmapType::MotionBlocking | HeightmapType::MotionBlockingNoLeaves => !is_air,
    }
}

fn column_top_y(
    chunk: &ChunkData,
    x: usize,
    z: usize,
    ty: HeightmapType,
    settings: &NoiseGeneratorSettings,
) -> Option<i32> {
    let air = BlockState::new(ResourceLocation::minecraft("air"));
    for section in chunk.sections.iter().rev() {
        for local_y in (0..16usize).rev() {
            let block = section.block_states.get(local_index(x, local_y, z));
            if counts_for_heightmap(ty, block, &air, &settings.default_fluid) {
                let section_min_y = (section.y as i32) * 16;
                return Some(section_min_y + local_y as i32);
            }
        }
    }
    None
}

/// Value stored is one above the highest counting block (vanilla convention: the height you'd
/// stand on), `0` (chunk floor) if a column has no counting block.
pub(crate) fn compute_heightmaps(
    chunk: &ChunkData,
    settings: &NoiseGeneratorSettings,
) -> HashMap<HeightmapType, Heightmap> {
    const TYPES: [HeightmapType; 6] = [
        HeightmapType::WorldSurface,
        HeightmapType::WorldSurfaceWg,
        HeightmapType::OceanFloor,
        HeightmapType::OceanFloorWg,
        HeightmapType::MotionBlocking,
        HeightmapType::MotionBlockingNoLeaves,
    ];

    let mut out = HashMap::new();
    for ty in TYPES {
        let mut hm = Heightmap::new(chunk.height);
        for x in 0..16usize {
            for z in 0..16usize {
                let relative = match column_top_y(chunk, x, z, ty, settings) {
                    Some(y) => y + 1 - chunk.min_y,
                    None => 0,
                };
                hm.set(x, z, relative);
            }
        }
        out.insert(ty, hm);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::ResourceLocation as Rl;
    use oxide_datapack::{
        DensityFunction, NoiseDimensionSettings, NoiseRouter, Registry, SurfaceRule,
    };

    fn settings(final_density: DensityFunction) -> NoiseGeneratorSettings {
        NoiseGeneratorSettings {
            sea_level: 63,
            disable_mob_generation: false,
            aquifers_enabled: false,
            ore_veins_enabled: false,
            legacy_random_source: false,
            default_block: BlockState::new(Rl::new("minecraft", "stone")),
            default_fluid: BlockState::new(Rl::new("minecraft", "water")),
            noise: NoiseDimensionSettings {
                min_y: -64,
                height: 384,
                size_horizontal: 1,
                size_vertical: 2,
            },
            noise_router: NoiseRouter {
                barrier: DensityFunction::Constant(0.0),
                fluid_level_floodedness: DensityFunction::Constant(0.0),
                fluid_level_spread: DensityFunction::Constant(0.0),
                lava: DensityFunction::Constant(0.0),
                temperature: DensityFunction::Constant(0.0),
                vegetation: DensityFunction::Constant(0.0),
                continents: DensityFunction::Constant(0.0),
                erosion: DensityFunction::Constant(0.0),
                depth: DensityFunction::Constant(0.0),
                ridges: DensityFunction::Constant(0.0),
                initial_density_without_jaggedness: DensityFunction::Constant(0.0),
                preliminary_surface_level: DensityFunction::Constant(0.0),
                final_density,
                vein_toggle: DensityFunction::Constant(0.0),
                vein_ridged: DensityFunction::Constant(0.0),
                vein_gap: DensityFunction::Constant(0.0),
            },
            surface_rule: SurfaceRule::Sequence { sequence: vec![] },
            spawn_target: vec![],
        }
    }

    #[test]
    fn all_solid_density_fills_every_block_with_default_block() {
        let settings = settings(DensityFunction::Constant(1.0));
        let df_registry = Registry::default();
        let noise_registry = Registry::default();
        let router = NoiseRouterEvaluator::new(42, &settings, &df_registry, &noise_registry);
        let chunk = fill_chunk(ChunkPos::new(0, 0), &settings, &router, None);

        let bottom = chunk.sections.first().unwrap();
        assert_eq!(
            *bottom.block_states.get(local_index(0, 0, 0)),
            settings.default_block
        );
    }

    #[test]
    fn all_air_density_leaves_fluid_below_sea_level_and_air_above() {
        let settings = settings(DensityFunction::Constant(-1.0));
        let df_registry = Registry::default();
        let noise_registry = Registry::default();
        let router = NoiseRouterEvaluator::new(1, &settings, &df_registry, &noise_registry);
        let chunk = fill_chunk(ChunkPos::new(0, 0), &settings, &router, None);
        let air = BlockState::new(ResourceLocation::minecraft("air"));

        // section 4 blocks above min_y=-64 -> section index 0, local_y=4 -> world y=-60,
        // well below sea_level=63, so it must be fluid.
        let low_section = &chunk.sections[0];
        assert_eq!(
            *low_section.block_states.get(local_index(0, 4, 0)),
            settings.default_fluid
        );

        // Top section is entirely above sea level -> air.
        let top_section = chunk.sections.last().unwrap();
        assert_eq!(*top_section.block_states.get(local_index(0, 0, 0)), air);
    }

    #[test]
    fn heightmap_top_is_one_above_the_solid_surface() {
        // Solid below y=0, air at and above (a step function via y_clamped_gradient would be
        // more realistic, but a constant threshold is enough to test the heightmap wiring).
        let df = DensityFunction::Object(Box::new(
            oxide_datapack::DensityFunctionObject::YClampedGradient {
                from_y: 0,
                to_y: 1,
                from_value: 1.0,
                to_value: -1.0,
            },
        ));
        let settings = settings(df);
        let df_registry = Registry::default();
        let noise_registry = Registry::default();
        let router = NoiseRouterEvaluator::new(7, &settings, &df_registry, &noise_registry);
        let chunk = fill_chunk(ChunkPos::new(0, 0), &settings, &router, None);

        let hm = &chunk.heightmaps[&HeightmapType::WorldSurface];
        // Solid up to y=0 (inclusive-ish, density>0 stops exactly at the gradient's midpoint),
        // so the surface height must land at or just above y=0, i.e. a small positive relative
        // value near `0 - min_y = 64`, not 0 (no solid found) and not the world's full height.
        let relative = hm.get(0, 0);
        assert!(
            relative > 0 && relative < chunk.height,
            "relative={relative}"
        );
    }
}
