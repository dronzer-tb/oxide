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
        // Most slots a fill writes hold the dimension's default block, and resolving its
        // palette index once per section spares a `BlockState` clone, an `AHashMap` probe over
        // two `String`s and a `BTreeMap`, and a drop, at every one of them. Resolved lazily
        // because resolving *inserts*: a section that stays all air must keep a one-entry
        // palette, which is what makes it serialize to zero data longs.
        let mut default_block_index: Option<u32> = None;

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
                                None => None,
                                // Air is already the section's default palette entry.
                                Some(state) if state == air => continue,
                                Some(state) => Some(state),
                            }
                        }
                        // Aquifers disabled by the settings: solid below the surface, the
                        // dimension's fluid up to sea level, air above.
                        None => {
                            if density > 0.0 {
                                None
                            } else if y <= settings.sea_level {
                                Some(settings.default_fluid.clone())
                            } else {
                                continue;
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
/// `NoiseBasedChunkGenerator.getBaseHeight` reports it: scan the column from the top down and
/// return one above the first block the heightmap type counts, or the world floor if the
/// column has none.
///
/// Vanilla scans noise + aquifer only -- no surface rules and no carvers -- so this does the
/// same rather than generating the finished chunk. Two consequences, both deliberate: it is
/// far cheaper than a full chunk (one shared corner grid, then 385 interpolated samples), and
/// it reports the pre-carve surface, so a structure placed over a cave mouth sits where
/// vanilla would put it.
///
/// The "one shared corner grid" is what the thread-local below buys. A caller asking about a
/// structure asks column by column, and building a chunk's caches means evaluating five
/// `interpolated` nodes at 1225 cell corners each -- so building them per call made a single
/// chunk's worth of `base_height` cost eighteen times more than generating that chunk. The
/// caches depend only on the router and the chunk, so the last chunk's are kept and reused.
pub fn base_height(
    x: i32,
    z: i32,
    settings: &NoiseGeneratorSettings,
    router: &NoiseRouterEvaluator,
    ty: HeightmapType,
) -> i32 {
    let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
    COLUMN_CACHES.with(|slot| {
        let mut slot = slot.borrow_mut();
        // Keyed by the router too: one process can hold generators for several worlds, and a
        // chunk position means nothing without knowing whose.
        let key = (router as *const NoiseRouterEvaluator as usize, chunk_pos);
        match slot.as_ref() {
            Some((cached, _)) if *cached == key => {}
            _ => *slot = Some((key, router.chunk_caches(chunk_pos.x, chunk_pos.z))),
        }
        let caches = &slot.as_ref().expect("just populated").1;
        base_height_with(x, z, chunk_pos, settings, router, caches, ty)
    })
}

thread_local! {
    /// The last chunk `base_height` was asked about, and its density caches. One entry is
    /// enough: callers walk a chunk's columns together.
    static COLUMN_CACHES: std::cell::RefCell<
        Option<((usize, ChunkPos), oxide_noise::ChunkCaches)>,
    > = const { std::cell::RefCell::new(None) };
}

#[allow(clippy::too_many_arguments)]
fn base_height_with(
    x: i32,
    z: i32,
    chunk_pos: ChunkPos,
    settings: &NoiseGeneratorSettings,
    router: &NoiseRouterEvaluator,
    caches: &oxide_noise::ChunkCaches,
    ty: HeightmapType,
) -> i32 {
    let min_y = settings.noise.min_y;
    let air = BlockState::new(ResourceLocation::minecraft("air"));
    let mut aquifer = crate::aquifer::for_settings(chunk_pos, settings, router, caches);

    for y in (min_y..min_y + settings.noise.height).rev() {
        let density = router.sample_in_chunk(caches, RouterSlot::FinalDensity, x, y, z);
        let block = match aquifer.as_mut() {
            Some(aquifer) => match aquifer.compute_substance(x, y, z, density) {
                None => settings.default_block.clone(),
                Some(state) => state,
            },
            None => {
                if density > 0.0 {
                    settings.default_block.clone()
                } else if y <= settings.sea_level {
                    settings.default_fluid.clone()
                } else {
                    air.clone()
                }
            }
        };
        if counts_for_heightmap(ty, &block, &air, &settings.default_fluid) {
            return y + 1;
        }
    }
    min_y
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

/// Value stored is one above the highest counting block (vanilla convention: the height you'd
/// stand on), `0` (chunk floor) if a column has no counting block.
///
/// The six heightmap types this crate models collapse to two predicates -- `!air`, which
/// `WORLD_SURFACE*` and `MOTION_BLOCKING*` share, and `!air && !fluid` for `OCEAN_FLOOR*` (see
/// [`counts_for_heightmap`]) -- so both are resolved in one downward pass and the results are
/// copied into the six slots. Classification happens once per section palette rather than once
/// per block: a section holds 4096 blocks and a handful of distinct states, and comparing
/// `BlockState` means comparing two `String`s and a `BTreeMap`.
pub(crate) fn compute_heightmaps(
    chunk: &ChunkData,
    settings: &NoiseGeneratorSettings,
) -> HashMap<HeightmapType, Heightmap> {
    const COLUMNS: usize = 256;
    /// Palette-entry flags: counts for `!air`, and for `!air && !fluid`.
    const COUNTS_SURFACE: u8 = 1;
    const COUNTS_FLOOR: u8 = 2;

    let air = BlockState::new(ResourceLocation::minecraft("air"));

    // Relative y (vanilla's convention: one above the block) per column, 0 meaning "nothing
    // found", plus how many columns are still unresolved so the scan can stop early.
    let mut surface = [0i32; COLUMNS];
    let mut floor = [0i32; COLUMNS];
    let mut surface_left = COLUMNS;
    let mut floor_left = COLUMNS;

    for section in chunk.sections.iter().rev() {
        if surface_left == 0 && floor_left == 0 {
            break;
        }
        let flags: Vec<u8> = section
            .block_states
            .palette()
            .iter()
            .map(|state| {
                if *state == air {
                    0
                } else if *state == settings.default_fluid {
                    COUNTS_SURFACE
                } else {
                    COUNTS_SURFACE | COUNTS_FLOOR
                }
            })
            .collect();
        // A section made only of blocks no heightmap counts -- which is every section above
        // the terrain, holding nothing but air -- cannot resolve a column, so skip its 4096
        // slots outright.
        if flags.iter().all(|f| *f == 0) {
            continue;
        }
        let indices = section.block_states.indices();
        let section_min_y = (section.y as i32) * 16;

        for local_y in (0..16usize).rev() {
            let relative = section_min_y + local_y as i32 + 1 - chunk.min_y;
            let plane = local_y * 256;
            for column in 0..COLUMNS {
                if surface[column] != 0 && floor[column] != 0 {
                    continue;
                }
                // `local_index` is `(y * 16 + z) * 16 + x`, so one y-plane is 256 contiguous
                // slots in (z, x) order -- the same order `column` counts in.
                let flag = flags[indices[plane + column] as usize];
                if flag & COUNTS_SURFACE != 0 && surface[column] == 0 {
                    surface[column] = relative;
                    surface_left -= 1;
                }
                if flag & COUNTS_FLOOR != 0 && floor[column] == 0 {
                    floor[column] = relative;
                    floor_left -= 1;
                }
            }
        }
    }

    let to_heightmap = |values: &[i32; COLUMNS]| {
        let mut hm = Heightmap::new(chunk.height);
        for (column, value) in values.iter().enumerate() {
            hm.set(column % 16, column / 16, *value);
        }
        hm
    };
    let surface = to_heightmap(&surface);
    let floor = to_heightmap(&floor);

    let mut out = HashMap::new();
    out.insert(HeightmapType::WorldSurface, surface.clone());
    out.insert(HeightmapType::WorldSurfaceWg, surface.clone());
    out.insert(HeightmapType::MotionBlocking, surface.clone());
    out.insert(HeightmapType::MotionBlockingNoLeaves, surface);
    out.insert(HeightmapType::OceanFloor, floor.clone());
    out.insert(HeightmapType::OceanFloorWg, floor);
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
    fn base_height_agrees_with_the_filled_chunk_heightmap() {
        // `base_height` scans one column straight from the router instead of filling a chunk,
        // so it has to land on exactly the same surface the fill's own heightmap reports --
        // that agreement is the whole point of it standing in for vanilla's getBaseHeight.
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
        let hm = &chunk.heightmaps[&HeightmapType::WorldSurface];

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
        // Aquifers are off in this fixture, so an all-negative density leaves water up to sea
        // level -- which WORLD_SURFACE counts but OCEAN_FLOOR does not.
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
