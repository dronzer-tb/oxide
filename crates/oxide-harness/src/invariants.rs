//! Invariant checks that run without a vanilla reference — `docs/ARCHITECTURE.md`'s "Invariant
//! checks ... run without a Java reference."
//!
//! Only the checks meaningful against what `oxide-chunkgen` actually produces today are
//! implemented. `docs/ARCHITECTURE.md`'s example list also names "no bedrock breach" and "no
//! floating trees" — both are **not applicable yet**, not silently passing: `oxide-chunkgen`
//! doesn't place bedrock or generate tree features at all (wave 3 scope cut, see
//! `oxide_chunkgen::fill`'s module doc), so there is nothing for those checks to verify. Add
//! them once the generation they check for exists, rather than shipping a check that's
//! vacuously green.

use std::collections::HashSet;

use oxide_core::{BlockState, ChunkData, HeightmapType};
use oxide_datapack::{Biome, Registry};

#[derive(Debug, Clone)]
pub struct InvariantFailure {
    pub check: &'static str,
    pub detail: String,
}

fn local_index(x: usize, y: usize, z: usize) -> usize {
    (y * 16 + z) * 16 + x
}

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

fn actual_column_top_y(
    chunk: &ChunkData,
    x: usize,
    z: usize,
    ty: HeightmapType,
    air: &BlockState,
    default_fluid: &BlockState,
) -> Option<i32> {
    for section in chunk.sections.iter().rev() {
        for local_y in (0..16usize).rev() {
            let block = section.block_states.get(local_index(x, local_y, z));
            if counts_for_heightmap(ty, block, air, default_fluid) {
                return Some((section.y as i32) * 16 + local_y as i32);
            }
        }
    }
    None
}

/// Recomputes every stored heightmap from the blocks actually placed and reports any column
/// that disagrees with the value `oxide-chunkgen` stored. Deliberately reimplements the
/// predicate independently of `oxide_chunkgen::fill` (rather than sharing a function) — a
/// stored-vs-recomputed mismatch should mean one of the two implementations drifted, and
/// sharing the code would hide that class of bug behind an invariant that always trivially
/// agrees with itself.
pub fn heightmaps_match_surface(
    chunk: &ChunkData,
    air: &BlockState,
    default_fluid: &BlockState,
) -> Vec<InvariantFailure> {
    let mut out = Vec::new();
    for (&ty, hm) in &chunk.heightmaps {
        for x in 0..16usize {
            for z in 0..16usize {
                let expected_relative =
                    match actual_column_top_y(chunk, x, z, ty, air, default_fluid) {
                        Some(y) => y + 1 - chunk.min_y,
                        None => 0,
                    };
                let stored = hm.get(x, z);
                if stored != expected_relative {
                    out.push(InvariantFailure {
                        check: "heightmaps_match_surface",
                        detail: format!(
                            "{ty:?} at ({x},{z}): stored={stored} recomputed={expected_relative}"
                        ),
                    });
                }
            }
        }
    }
    out
}

/// Every biome id referenced by a chunk's biome palettes must resolve against the loaded
/// `worldgen/biome` registry.
pub fn biome_ids_are_registered(
    chunk: &ChunkData,
    biomes: &Registry<Biome>,
) -> Vec<InvariantFailure> {
    let mut out = Vec::new();
    let mut checked = HashSet::new();
    for section in &chunk.sections {
        for id in section.biomes.palette() {
            if checked.insert(id.clone()) && !biomes.contains(id) {
                out.push(InvariantFailure {
                    check: "biome_ids_are_registered",
                    detail: format!("section y={} references unregistered biome {id}", section.y),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_chunkgen::fill_chunk;
    use oxide_core::{ChunkPos, ResourceLocation};
    use oxide_datapack::{
        DensityFunction, DensityFunctionObject, NoiseDimensionSettings, NoiseGeneratorSettings,
        NoiseRouter, SurfaceRule,
    };
    use oxide_noise::NoiseRouterEvaluator;

    fn settings() -> NoiseGeneratorSettings {
        let df = DensityFunction::Object(Box::new(DensityFunctionObject::YClampedGradient {
            from_y: 0,
            to_y: 1,
            from_value: 1.0,
            to_value: -1.0,
        }));
        NoiseGeneratorSettings {
            sea_level: 63,
            disable_mob_generation: false,
            aquifers_enabled: false,
            ore_veins_enabled: false,
            legacy_random_source: false,
            default_block: BlockState::new(ResourceLocation::new("minecraft", "stone")),
            default_fluid: BlockState::new(ResourceLocation::new("minecraft", "water")),
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
                final_density: df,
                vein_toggle: DensityFunction::Constant(0.0),
                vein_ridged: DensityFunction::Constant(0.0),
                vein_gap: DensityFunction::Constant(0.0),
            },
            surface_rule: SurfaceRule::Sequence { sequence: vec![] },
            spawn_target: vec![],
        }
    }

    #[test]
    fn a_freshly_filled_chunk_has_no_heightmap_mismatches() {
        let settings = settings();
        let df_registry = Registry::default();
        let noise_registry = Registry::default();
        let router = NoiseRouterEvaluator::new(1, &settings, &df_registry, &noise_registry);
        let chunk = fill_chunk(ChunkPos::new(0, 0), &settings, &router, None);
        let air = BlockState::new(ResourceLocation::minecraft("air"));
        let failures = heightmaps_match_surface(&chunk, &air, &settings.default_fluid);
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn tampered_heightmap_is_caught() {
        let settings = settings();
        let df_registry = Registry::default();
        let noise_registry = Registry::default();
        let router = NoiseRouterEvaluator::new(1, &settings, &df_registry, &noise_registry);
        let mut chunk = fill_chunk(ChunkPos::new(0, 0), &settings, &router, None);
        let hm = chunk
            .heightmaps
            .get_mut(&HeightmapType::WorldSurface)
            .unwrap();
        let original = hm.get(0, 0);
        hm.set(0, 0, original + 5);

        let air = BlockState::new(ResourceLocation::minecraft("air"));
        let failures = heightmaps_match_surface(&chunk, &air, &settings.default_fluid);
        assert!(failures
            .iter()
            .any(|f| f.check == "heightmaps_match_surface"));
    }

    #[test]
    fn unregistered_biome_is_caught() {
        let settings = settings();
        let df_registry = Registry::default();
        let noise_registry = Registry::default();
        let router = NoiseRouterEvaluator::new(1, &settings, &df_registry, &noise_registry);
        let chunk = fill_chunk(ChunkPos::new(0, 0), &settings, &router, None);
        // fill_chunk's default biome ("minecraft:plains") is never in an empty registry.
        let empty_biomes = Registry::default();
        let failures = biome_ids_are_registered(&chunk, &empty_biomes);
        assert!(!failures.is_empty());
    }
}
