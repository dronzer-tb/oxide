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
#[inline(always)]
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
    aquifer: Option<&mut crate::aquifer::Aquifer>,
) -> ChunkData {
    fill_chunk_culling(pos, settings, router, biomes, caches, aquifer, true)
}

/// [`fill_chunk_with`] with cell culling switchable.
///
/// `cull = false` evaluates density at every position, which is what this did before culling
/// existed. Exposed so a test can assert the two produce byte-identical chunks -- a shortcut
/// that silently changed terrain would be far worse than a slow one.
pub fn fill_chunk_culling(
    pos: ChunkPos,
    settings: &NoiseGeneratorSettings,
    router: &NoiseRouterEvaluator,
    biomes: Option<&oxide_biome::BiomeSearchTree>,
    caches: &oxide_noise::ChunkCaches,
    mut aquifer: Option<&mut crate::aquifer::Aquifer>,
    cull: bool,
) -> ChunkData {
    let min_y = settings.noise.min_y;
    let height = settings.noise.height;
    let air = BlockState::new(ResourceLocation::minecraft("air"));
    // Fallback only: real biome resolution happens per quart-cell below when `biomes` is
    // `Some`; this is what an unresolved `Preset` source (see above) leaves every cell at.
    let default_biome = ResourceLocation::minecraft("plains");

    let mut chunk = ChunkData::new(pos, min_y, height);
    let section_count = chunk.section_count();

    // Cell-level culling setup. `final_density` resolves through an `interpolated` cache node,
    // whose corner grid this asks for the min/max of per cell. A trilinear blend never leaves
    // the range of its eight corners, so `min > 0` proves a whole cell is solid and `max < 0`
    // proves none of it is -- either way the cell's interior needs no per-block density
    // evaluation at all. In real terrain most cells are one or the other, which is what takes
    // the ~98k `Program::run` calls per chunk down to the boundary cells only.
    //
    // The grid is built lazily on first sample, so take one now to force it; without this the
    // bounds are unavailable and every cell falls through to the exact path (still correct,
    // just not faster).
    if cull {
        // The corner grids are built lazily on first sample; take one so the bounds below have
        // something to read. Without it every cell falls through to the exact path.
        let _ = router.sample_in_chunk(
            caches,
            RouterSlot::FinalDensity,
            pos.min_block_x(),
            min_y,
            pos.min_block_z(),
        );
    }
    let cell_width = caches.cell_width();
    let cell_height = caches.cell_height();

    for i in 0..section_count {
        let section_y = min_y / 16 + i as i32;
        let mut section = ChunkSection::new(section_y as i8, air.clone(), default_biome.clone());
        let section_min_y = section_y * 16;
        let mut default_block_index: Option<u32> = None;

        for local_y in 0..16i32 {
            let y = section_min_y + local_y;
            // Which cell this row of blocks sits in, and what the density is known to do
            // across it. `None` = no shortcut available, evaluate exactly.
            let cell_y = (y - min_y).div_euclid(cell_height);
            for local_z in 0..16usize {
                let block_z = pos.min_block_z() + local_z as i32;
                let cell_z = (local_z as i32).div_euclid(cell_width);
                // Bounds are a property of the CELL, so they are resolved once per cell here
                // rather than once per block. Computing them per position cost a 5-op interval
                // walk 98k times a chunk and ate the entire saving.
                let mut cached_cell_x = i32::MIN;
                let mut cached_all_air = false;
                for local_x in 0..16usize {
                    let block_x = pos.min_block_x() + local_x as i32;
                    let cell_x = (local_x as i32).div_euclid(cell_width);

                    // Uniform-cell shortcut, taken before any density evaluation.
                    //
                    // Only the "all air" direction is claimed. `final_density` is
                    // `min(squeeze(interpolated(..)), noodle_caves)` in a real overworld router:
                    // the `min` means an unbounded cave term can pull any position down, so a
                    // positive lower bound on the interpolated part would NOT prove the cell is
                    // solid. An upper bound of <= 0 is safe in spite of the `min`, because `min`
                    // only ever lowers the value further.
                    #[cfg(feature = "cull-stats")]
                    {
                        crate::CULL_CONSIDERED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if cull {
                            if let Some((_, hi)) = router.cell_bounds(
                                caches, RouterSlot::FinalDensity, cell_x, cell_y, cell_z) {
                                crate::CULL_BOUNDED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                if hi <= 0.0 {
                                    crate::CULL_HI_NEG.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    if cell_x != cached_cell_x {
                        cached_cell_x = cell_x;
                        cached_all_air = cull
                            && router
                                .cell_bounds(
                                    caches,
                                    RouterSlot::FinalDensity,
                                    cell_x,
                                    cell_y,
                                    cell_z,
                                )
                                .is_some_and(|(_, hi)| hi <= 0.0);
                    }
                    // Per block, because a cell spans several y and the answer changes with it.
                    //
                    // With aquifers off: above sea level a non-solid position is air.
                    // With aquifers on: above `skip_sampling_above_y` the aquifer answers from
                    // its global fluid alone -- no grid sampling and no mutation of its memo
                    // state -- so the result is predictable here and skipping the call cannot
                    // change what any later call returns.
                    let all_air = cached_all_air
                        && match aquifer.as_deref() {
                            None => y > settings.sea_level,
                            Some(aq) => aq.air_above_skip(y),
                        };

                    let block = if all_air {
                        // Above sea level with aquifers off, a non-solid position is air, which
                        // is already the section's palette default -- nothing to write.
                        continue;
                    } else {
                        {
                            let density = router.sample_in_chunk(
                                caches,
                                RouterSlot::FinalDensity,
                                block_x,
                                y,
                                block_z,
                            );
                            // The aquifer decides what a non-solid position holds -- air, water
                            // or lava, at a level that varies per underground body. `None` means
                            // solid, so the default block goes in. Vanilla runs exactly this as
                            // the first filler in its noise fill.
                            match aquifer.as_deref_mut() {
                                Some(aquifer) => {
                                    match aquifer.compute_substance(block_x, y, block_z, density) {
                                        None => None,
                                        // Air is already the section's default palette entry.
                                        Some(state) if state == air => continue,
                                        Some(state) => Some(state),
                                    }
                                }
                                // Aquifers disabled by the settings: solid below the surface,
                                // the dimension's fluid up to sea level, air above.
                                None => {
                                    if density > 0.0 {
                                        None
                                    } else if y <= settings.sea_level {
                                        Some(settings.default_fluid.clone())
                                    } else {
                                        continue;
                                    }
                                }
                            }
                        }
                    };
                    let slot = local_index(local_x, local_y as usize, local_z);
                    match block {
                        // `None` is the default block, the common case.
                        None => {
                            let index = match default_block_index {
                                Some(index) => index,
                                None => {
                                    let index = section
                                        .block_states
                                        .index_of_or_insert(settings.default_block.clone());
                                    default_block_index = Some(index);
                                    index
                                }
                            };
                            section.block_states.set_index(slot, index);
                        }
                        Some(state) => section.block_states.set(slot, state),
                    }
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

/// The height a structure would sit at in the column `(x, z)`, as vanilla's
/// `ChunkGenerator#getBaseHeight`.
///
/// Vanilla samples terrain density top-down at `(x, z)` in cell-height steps (typically 8
/// blocks), and when it crosses from negative to positive density it switches to single-block
/// steps inside that one cell to find the exact top solid block. What arrives back is the block
/// *above* that surface (an empty position to stand on).
pub fn base_height(
    x: i32,
    z: i32,
    settings: &NoiseGeneratorSettings,
    router: &NoiseRouterEvaluator,
    heightmap_type: HeightmapType,
) -> i32 {
    let caches = router.chunk_caches(x.div_euclid(16), z.div_euclid(16));
    base_height_in(x, z, settings, router, &caches, heightmap_type)
}

/// [`base_height`] against caches the caller already holds.
///
/// Building a `ChunkCaches` means evaluating every `interpolated` node over the chunk's corner
/// grid, so a caller asking about many columns of one chunk -- which is exactly what Bukkit does
/// during structure placement, 256 columns at a time -- must build it once and pass it here.
/// `caches` must belong to `(x, z)`'s own chunk: cell lookups are relative to its origin and
/// clamp to the grid, so caches from another chunk do not fail, they quietly return the wrong
/// column's terrain.
pub fn base_height_in(
    x: i32,
    z: i32,
    settings: &NoiseGeneratorSettings,
    router: &NoiseRouterEvaluator,
    caches: &oxide_noise::ChunkCaches,
    heightmap_type: HeightmapType,
) -> i32 {
    let min_y = settings.noise.min_y;
    let height = settings.noise.height;
    let cell_height = (settings.noise.size_vertical * 4).max(1);
    let top_y = min_y + height;

    let is_solid = |density: f64| density > 0.0;
    let is_fluid = |y: i32| y <= settings.sea_level;

    let stops = |y: i32, density: f64| -> bool {
        match heightmap_type {
            HeightmapType::WorldSurface | HeightmapType::WorldSurfaceWg => {
                is_solid(density) || is_fluid(y)
            }
            HeightmapType::OceanFloor | HeightmapType::OceanFloorWg => is_solid(density),
            HeightmapType::MotionBlocking | HeightmapType::MotionBlockingNoLeaves => {
                is_solid(density) || is_fluid(y)
            }
        }
    };

    let mut coarse_y = top_y - cell_height;
    while coarse_y >= min_y {
        let density = router.sample_in_chunk(caches, RouterSlot::FinalDensity, x, coarse_y, z);
        if stops(coarse_y, density) {
            let scan_top = (coarse_y + cell_height).min(top_y);
            for y in (coarse_y..scan_top).rev() {
                let d = router.sample_in_chunk(caches, RouterSlot::FinalDensity, x, y, z);
                if stops(y, d) {
                    return y + 1;
                }
            }
            return coarse_y + 1;
        }
        coarse_y -= cell_height;
    }

    min_y
}

/// Heightmaps vanilla calculates immediately after noise fill: `OCEAN_FLOOR_WG` and
/// `WORLD_SURFACE_WG`.
pub(crate) fn compute_heightmaps(
    chunk: &ChunkData,
    settings: &NoiseGeneratorSettings,
) -> HashMap<HeightmapType, Heightmap> {
    let min_y = chunk.min_y;
    let height = chunk.height;
    let mut ocean_floor = Heightmap::new(height);
    let mut world_surface = Heightmap::new(height);

    let default_fluid = &settings.default_fluid;

    for local_z in 0..16usize {
        for local_x in 0..16usize {
            let mut found_ocean_floor = false;
            let mut found_world_surface = false;

            for section_index in (0..chunk.section_count()).rev() {
                let section = &chunk.sections[section_index];
                let section_base_y = min_y + (section_index as i32 * 16);

                for local_y in (0..16usize).rev() {
                    let y = section_base_y + local_y as i32;
                    let slot = local_index(local_x, local_y, local_z);
                    let state = section.block_states.get(slot);

                    let is_air = state.name.namespace() == "minecraft" && state.name.path() == "air";
                    let is_fluid = state == default_fluid;

                    if !is_air && !found_world_surface {
                        world_surface.set(local_x, local_z, y - min_y + 1);
                        found_world_surface = true;
                    }

                    if !is_air && !is_fluid && !found_ocean_floor {
                        ocean_floor.set(local_x, local_z, y - min_y + 1);
                        found_ocean_floor = true;
                    }

                    if found_ocean_floor && found_world_surface {
                        break;
                    }
                }

                if found_ocean_floor && found_world_surface {
                    break;
                }
            }
        }
    }

    let mut map = HashMap::new();
    map.insert(HeightmapType::OceanFloor, ocean_floor.clone());
    map.insert(HeightmapType::WorldSurface, world_surface.clone());
    map.insert(HeightmapType::OceanFloorWg, ocean_floor.clone());
    map.insert(HeightmapType::WorldSurfaceWg, world_surface.clone());
    map.insert(HeightmapType::MotionBlocking, world_surface.clone());
    map.insert(HeightmapType::MotionBlockingNoLeaves, world_surface);
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_datapack::NoiseGeneratorSettings;
    use oxide_datapack::{DensityFunction, Registry};

    fn settings(final_density: DensityFunction) -> NoiseGeneratorSettings {
        let json = serde_json::json!({
            "default_block": {"Name": "minecraft:stone"},
            "default_fluid": {"Name": "minecraft:water"},
            "sea_level": 63,
            "disable_mob_generation": false,
            "aquifers_enabled": false,
            "ore_veins_enabled": false,
            "legacy_random_source": false,
            "noise": {
                "min_y": -64,
                "height": 384,
                "size_horizontal": 1,
                "size_vertical": 2
            },
            "noise_router": {
                "temperature": 0.0,
                "vegetation": 0.0,
                "continents": 0.0,
                "erosion": 0.0,
                "depth": 0.0,
                "ridges": 0.0,
                "final_density": final_density
            },
            "surface_rule": {"type": "minecraft:block", "result_state": {"Name": "minecraft:stone"}},
            "spawn_target": []
        });
        serde_json::from_value(json).expect("valid fixture")
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

        let low_section = &chunk.sections[0];
        assert_eq!(
            *low_section.block_states.get(local_index(0, 4, 0)),
            settings.default_fluid
        );

        let top_section = chunk.sections.last().unwrap();
        assert_eq!(*top_section.block_states.get(local_index(0, 0, 0)), air);
    }

    #[test]
    fn base_height_agrees_with_the_filled_chunk_heightmap() {
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
        let router = NoiseRouterEvaluator::new(11, &settings, &df_registry, &noise_registry);
        let chunk = fill_chunk(ChunkPos::new(0, 0), &settings, &router, None);
        let hm = &chunk.heightmaps[&HeightmapType::WorldSurfaceWg];

        for (x, z) in [(0usize, 0usize), (5, 11), (15, 15)] {
            let from_fill = hm.get(x, z) + settings.noise.min_y;
            let from_scan = base_height(
                x as i32,
                z as i32,
                &settings,
                &router,
                HeightmapType::WorldSurface,
            );
            assert_eq!(from_fill, from_scan, "column ({x}, {z})");
        }
    }

    #[test]
    fn base_height_reports_the_world_floor_for_an_empty_column() {
        let settings = settings(DensityFunction::Constant(-1.0));
        let df_registry = Registry::default();
        let noise_registry = Registry::default();
        let router = NoiseRouterEvaluator::new(3, &settings, &df_registry, &noise_registry);
        assert_eq!(
            base_height(0, 0, &settings, &router, HeightmapType::OceanFloor),
            settings.noise.min_y
        );
        assert_eq!(
            base_height(0, 0, &settings, &router, HeightmapType::WorldSurface),
            settings.sea_level + 1
        );
    }

    #[test]
    fn heightmap_top_is_one_above_the_solid_surface() {
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

        let hm = &chunk.heightmaps[&HeightmapType::WorldSurfaceWg];
        let relative = hm.get(0, 0);
        assert!(
            relative > 0 && relative < chunk.height,
            "relative={relative}"
        );
    }
}
