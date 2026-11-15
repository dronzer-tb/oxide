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

/// Slack applied to a cell's corner bounds before a skip trusts them.
///
/// The bounds are the min and max of a cell's eight corners pushed through interval
/// arithmetic, while the value at a block is a trilinear blend of those corners and then the
/// same ops in floating point. In exact arithmetic a blend never leaves its corners' range; in
/// floating point the rounding of the blend and of each op can land a few ulp outside it. A
/// skip that compares against the threshold with no slack would then rest on that never
/// happening at a value on the edge. Terrain values are O(1), so 1e-9 is far above any
/// rounding these few ops can accumulate and far below anything that changes which cells skip.
pub(crate) const CELL_BOUND_MARGIN: f64 = 1e-9;

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

    // "Is this whole cell provably non-solid?", resolved once per cell for the chunk rather
    // than once per row of blocks crossing it: the interval walk behind `cell_bounds`
    // allocates, and asking per (y, z) row called it ~24k times a chunk for 768 cells.
    //
    // Only the "all air" direction is claimed. `final_density` is
    // `min(squeeze(interpolated(..)), noodle_caves)` in a real overworld router: the `min`
    // means an unbounded cave term can pull any position down, so a positive lower bound on
    // the interpolated part would NOT prove the cell is solid. An upper bound of <= 0 is safe
    // in spite of the `min`, because `min` only ever lowers the value further. The bound must
    // clear zero by [`CELL_BOUND_MARGIN`].
    let cells_xz = (16 / cell_width).max(1) as usize;
    let cells_y = (height / cell_height).max(0) as usize;
    let mut air_cells = vec![false; cells_xz * cells_xz * cells_y];
    if cull {
        for cell_y in 0..cells_y {
            for cell_z in 0..cells_xz {
                for cell_x in 0..cells_xz {
                    #[cfg(feature = "cull-stats")]
                    crate::CULL_CONSIDERED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let bounds = router.cell_bounds(
                        caches,
                        RouterSlot::FinalDensity,
                        cell_x as i32,
                        cell_y as i32,
                        cell_z as i32,
                    );
                    #[cfg(feature = "cull-stats")]
                    if let Some((_, hi)) = bounds {
                        crate::CULL_BOUNDED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if hi <= 0.0 {
                            crate::CULL_HI_NEG.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    air_cells[(cell_y * cells_xz + cell_z) * cells_xz + cell_x] =
                        bounds.is_some_and(|(_, hi)| hi <= -CELL_BOUND_MARGIN);
                }
            }
        }
    }
    // The row of `air_cells` a block row `(cell_y, cell_z)` reads, or `None` when no cell of
    // that row is in the grid. Rows are `cells_xz` long and start at a multiple of it, so a
    // row is either wholly inside `air_cells` or wholly outside it, and `row[cell_x]` for
    // `cell_x < cells_xz` is exactly the per-block lookup this replaces.
    let air_row = |cell_y: i32, cell_z: usize| -> Option<&[bool]> {
        if cell_y < 0 || cell_z >= cells_xz {
            return None;
        }
        let start = (cell_y as usize * cells_xz + cell_z) * cells_xz;
        air_cells.get(start..start + cells_xz)
    };

    // Per-column cell coordinates, resolved once instead of a `div_euclid` per block: the cell
    // a local x/z falls in, and its offset inside that cell.
    let cell_of: [usize; 16] =
        std::array::from_fn(|local| (local as i32).div_euclid(cell_width) as usize);
    let in_cell: [usize; 16] = std::array::from_fn(|local| {
        let cell = cell_of[local];
        local.wrapping_sub(cell.wrapping_mul(cell_width as usize))
    });

    // The three states the fill can place besides air. Each is interned into a section's
    // palette on first use (so palettes hold exactly what the section contains, in the order
    // it was first written) and written by index after that -- no `BlockState` is built,
    // hashed or dropped per block.
    const DEFAULT: usize = 0;
    const SEA: usize = 1;
    const LAVA: usize = 2;
    let lava = BlockState::new(ResourceLocation::minecraft("lava"));
    let placeable: [&BlockState; 3] = [&settings.default_block, &settings.default_fluid, &lava];
    // A fluid that *is* air needs no write: air is every section's palette default.
    let skip: [bool; 3] = [false, settings.default_fluid == air, false];

    // Densities come a whole cell at a time through `sample_cell`, into a buffer holding one
    // horizontal layer of cells. A cell is evaluated the first time a block in it needs a
    // density, so cells the shortcut below skips are never evaluated, and the visit/write
    // order (and every `compute_kind` call) is exactly the per-block loop's. `cell_state` per
    // cell of the loaded layer: 0 not yet asked, 1 in `cell_values`, 2 `sample_cell` declined
    // (that cell's blocks go through `sample_in_chunk`).
    let cell_len = router.cell_len(caches);
    let cell_w = cell_width as usize;
    let mut cell_values = vec![0.0f64; cells_xz * cells_xz * cell_len];
    let mut cell_state = vec![0u8; cells_xz * cells_xz];
    let mut loaded_cell_y = i32::MIN;

    for i in 0..section_count {
        let section_y = min_y / 16 + i as i32;
        let mut section = ChunkSection::new(section_y as i8, air.clone(), default_biome.clone());
        let section_min_y = section_y * 16;
        let mut palette_index: [Option<u32>; 3] = [None; 3];

        for local_y in 0..16i32 {
            let y = section_min_y + local_y;
            let cell_y = (y - min_y).div_euclid(cell_height);
            // Per row, because a cell spans several y and the answer changes with it.
            //
            // With aquifers off: above sea level a non-solid position is air.
            // With aquifers on: above `skip_sampling_above_y` the aquifer answers from its
            // global fluid alone -- no grid sampling and no mutation of its memo state -- so
            // the result is predictable here and skipping the call cannot change what any
            // later call returns.
            let air_if_not_solid = match aquifer.as_deref() {
                None => y > settings.sea_level,
                Some(aq) => aq.air_above_skip(y),
            };
            let row_in_cells = cell_y >= 0 && (cell_y as usize) < cells_y;
            if row_in_cells && cell_y != loaded_cell_y {
                cell_state.fill(0);
                loaded_cell_y = cell_y;
            }
            // Only meaningful (and only read) when `row_in_cells`.
            let cell_local_y = if row_in_cells {
                ((y - min_y) - cell_y * cell_height) as usize
            } else {
                0
            };
            for local_z in 0..16usize {
                let block_z = pos.min_block_z() + local_z as i32;
                let cell_z = cell_of[local_z];
                let lane_row = (cell_local_y * cell_w + in_cell[local_z]) * cell_w;
                // Only consulted for the uniform-cell shortcut, so only when it can apply.
                let air_row = if air_if_not_solid {
                    air_row(cell_y, cell_z)
                } else {
                    None
                };
                let row_cells = row_in_cells && cell_z < cells_xz;
                let row_index = local_index(0, local_y as usize, local_z);
                for local_x in 0..16usize {
                    let block_x = pos.min_block_x() + local_x as i32;
                    let cell_x = cell_of[local_x];

                    // Uniform-cell shortcut, taken before any density evaluation: air is
                    // already the section's palette default, so there is nothing to write.
                    if let Some(row) = air_row {
                        if cell_x < cells_xz && row[cell_x] {
                            continue;
                        }
                    }

                    let in_cells = row_cells && cell_x < cells_xz;
                    let cell_index = cell_z * cells_xz + cell_x;
                    let density = if in_cells {
                        if cell_state[cell_index] == 0 {
                            let values = &mut cell_values
                                [cell_index * cell_len..(cell_index + 1) * cell_len];
                            cell_state[cell_index] = if router.sample_cell(
                                caches,
                                RouterSlot::FinalDensity,
                                cell_x as i32,
                                cell_y,
                                cell_z as i32,
                                values,
                            ) {
                                1
                            } else {
                                2
                            };
                        }
                        if cell_state[cell_index] == 1 {
                            cell_values[cell_index * cell_len + lane_row + in_cell[local_x]]
                        } else {
                            router.sample_in_chunk(
                                caches,
                                RouterSlot::FinalDensity,
                                block_x,
                                y,
                                block_z,
                            )
                        }
                    } else {
                        router.sample_in_chunk(
                            caches,
                            RouterSlot::FinalDensity,
                            block_x,
                            y,
                            block_z,
                        )
                    };
                    // The aquifer decides what a non-solid position holds -- air, water or
                    // lava, at a level that varies per underground body. Vanilla runs exactly
                    // this as the first filler in its noise fill.
                    //
                    // A solid density is answered here: `compute_kind` returns `None` for
                    // `density > 0` before touching any state, and ~3 in 4 calls were that.
                    let kind = match aquifer.as_deref_mut() {
                        Some(_) if density > 0.0 => DEFAULT,
                        Some(aquifer) => match aquifer.compute_kind(block_x, y, block_z, density) {
                            None => DEFAULT,
                            Some(crate::aquifer::FluidKind::Air) => continue,
                            Some(crate::aquifer::FluidKind::Sea) => SEA,
                            Some(crate::aquifer::FluidKind::Lava) => LAVA,
                        },
                        // Aquifers disabled by the settings: solid below the surface, the
                        // dimension's fluid up to sea level, air above.
                        None => {
                            if density > 0.0 {
                                DEFAULT
                            } else if y <= settings.sea_level {
                                SEA
                            } else {
                                continue;
                            }
                        }
                    };
                    if skip[kind] {
                        continue;
                    }
                    let index = match palette_index[kind] {
                        Some(index) => index,
                        None => {
                            let index = section
                                .block_states
                                .index_of_or_insert(placeable[kind].clone());
                            palette_index[kind] = Some(index);
                            index
                        }
                    };
                    section.block_states.set_index(row_index + local_x, index);
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

    // Each section's palette classified once -- a handful of entries -- so the column scan
    // below reads a palette index and a table byte per block instead of comparing strings.
    const AIR: u8 = 0;
    const FLUID: u8 = 1;
    const SOLID: u8 = 2;
    let classes: Vec<Vec<u8>> = chunk
        .sections
        .iter()
        .map(|section| {
            section
                .block_states
                .palette()
                .iter()
                .map(|state| {
                    if state.name.namespace() == "minecraft" && state.name.path() == "air" {
                        AIR
                    } else if state == default_fluid {
                        FLUID
                    } else {
                        SOLID
                    }
                })
                .collect()
        })
        .collect();

    for local_z in 0..16usize {
        for local_x in 0..16usize {
            let mut found_world_surface = false;

            'column: for section_index in (0..chunk.section_count()).rev() {
                let section = &chunk.sections[section_index];
                let class = &classes[section_index];
                // A section holding only air cannot end either scan.
                if class.len() == 1 && class[0] == AIR {
                    continue;
                }
                let indices = section.block_states.indices();
                let section_base_y = min_y + (section_index as i32 * 16);

                for local_y in (0..16usize).rev() {
                    let y = section_base_y + local_y as i32;
                    let kind = class[indices[local_index(local_x, local_y, local_z)] as usize];
                    if kind == AIR {
                        continue;
                    }
                    if !found_world_surface {
                        world_surface.set(local_x, local_z, y - min_y + 1);
                        found_world_surface = true;
                    }
                    if kind == SOLID {
                        ocean_floor.set(local_x, local_z, y - min_y + 1);
                        break 'column;
                    }
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
