//! Evaluates placed-feature modifier chains and places configured features into chunk sections.
//!
//! Vanilla's feature pipeline processes features in strict step order:
//! RawGeneration -> Lakes -> LocalModifications -> UndergroundStructures -> SurfaceStructures
//! -> Strongholds -> UndergroundOres -> UndergroundDecoration -> FluidSprings -> VegetalDecoration
//! -> TopLayerModification.
//!
//! Placing features in native Rust completely avoids Java ChunkAccess wrapper allocations and
//! eliminates GC pauses during world generation.

use std::str::FromStr;

use oxide_core::{
    BlockPos, BlockState, ChunkData, ChunkPos, HeightmapType, PositionalRandomFactory,
    RandomSource, ResourceLocation,
};
use oxide_datapack::feature::{
    BlockStateProvider, ConfiguredFeature, Holder, OreConfig, OreTarget, SimpleBlockConfig,
    SpringConfig,
};
use oxide_datapack::placement::{
    BlockStateSpec, HeightmapKind, IntProvider, PlacementModifier, PlacedFeature, TaggedIntProvider,
};
use oxide_datapack::surface_rule::VerticalAnchor;
use oxide_datapack::Registry;

use crate::fill::local_index;

/// Context provided to placement modifiers and feature placing logic.
pub struct PlacementContext<'a, F: PositionalRandomFactory> {
    pub pos: ChunkPos,
    pub min_y: i32,
    pub height: i32,
    pub sea_level: i32,
    pub rng_factory: &'a F,
}

impl<'a, F: PositionalRandomFactory> PlacementContext<'a, F> {
    pub fn new(
        pos: ChunkPos,
        min_y: i32,
        height: i32,
        sea_level: i32,
        rng_factory: &'a F,
    ) -> Self {
        Self {
            pos,
            min_y,
            height,
            sea_level,
            rng_factory,
        }
    }
}

/// Evaluates a placed feature's modifier chain and places all produced positions into `chunk`.
pub fn place_feature<F: PositionalRandomFactory>(
    placed: &PlacedFeature,
    ctx: &PlacementContext<'_, F>,
    chunk: &mut ChunkData,
    configured_registry: &Registry<ConfiguredFeature>,
) {
    let mut positions = vec![BlockPos::new(
        ctx.pos.min_block_x(),
        ctx.min_y,
        ctx.pos.min_block_z(),
    )];

    let mut random = ctx.rng_factory.at(ctx.pos.x, 0, ctx.pos.z);

    // Evaluate placement modifiers
    for modifier in &placed.placement {
        let mut next_positions = Vec::with_capacity(positions.len());
        for pos in positions {
            match modifier {
                PlacementModifier::InSquare => {
                    let ox = random.next_int_bounded(16);
                    let oz = random.next_int_bounded(16);
                    next_positions.push(BlockPos::new(
                        ctx.pos.min_block_x() + ox,
                        pos.y,
                        ctx.pos.min_block_z() + oz,
                    ));
                }
                PlacementModifier::Count { count } => {
                    let n = sample_int(count, &mut random);
                    for _ in 0..n {
                        next_positions.push(pos);
                    }
                }
                PlacementModifier::RarityFilter { chance } => {
                    let c = *chance;
                    if c <= 1 || random.next_int_bounded(c) == 0 {
                        next_positions.push(pos);
                    }
                }
                PlacementModifier::HeightRange { height } => {
                    let resolve = |anchor: &VerticalAnchor| -> i32 {
                        match anchor {
                            VerticalAnchor::Absolute { absolute } => *absolute,
                            VerticalAnchor::AboveBottom { above_bottom } => ctx.min_y + *above_bottom,
                            VerticalAnchor::BelowTop { below_top } => ctx.min_y + ctx.height - *below_top,
                        }
                    };
                    let y = height.sample(&mut random, &resolve);
                    next_positions.push(BlockPos::new(pos.x, y, pos.z));
                }
                PlacementModifier::Heightmap { heightmap } => {
                    let hm_type = match heightmap {
                        HeightmapKind::WorldSurface | HeightmapKind::WorldSurfaceWg => {
                            HeightmapType::WorldSurface
                        }
                        HeightmapKind::OceanFloor | HeightmapKind::OceanFloorWg => {
                            HeightmapType::OceanFloor
                        }
                        HeightmapKind::MotionBlocking | HeightmapKind::MotionBlockingNoLeaves => {
                            HeightmapType::MotionBlocking
                        }
                    };
                    let local_x = (pos.x - ctx.pos.min_block_x()).rem_euclid(16) as usize;
                    let local_z = (pos.z - ctx.pos.min_block_z()).rem_euclid(16) as usize;
                    if let Some(hm) = chunk.heightmaps.get(&hm_type) {
                        let y = hm.get(local_x, local_z) + ctx.min_y;
                        next_positions.push(BlockPos::new(pos.x, y, pos.z));
                    }
                }
                PlacementModifier::Biome => {
                    next_positions.push(pos);
                }
                PlacementModifier::SurfaceWaterDepthFilter { max_water_depth } => {
                    if pos.y <= ctx.sea_level + *max_water_depth {
                        next_positions.push(pos);
                    }
                }
                _ => {
                    next_positions.push(pos);
                }
            }
        }
        positions = next_positions;
        if positions.is_empty() {
            break;
        }
    }

    // Resolve configured feature
    let configured = match &placed.feature {
        Holder::Inline(inline) => Some(inline.as_ref()),
        Holder::Reference(id) => configured_registry.get(id),
    };

    let Some(feature) = configured else {
        return;
    };

    // Place configured feature at each resolved position
    for pos in positions {
        apply_configured_feature(feature, pos, ctx.min_y, ctx.height, ctx.pos, chunk, &mut random);
    }
}

/// Helper to sample an `IntProvider`.
fn sample_int(provider: &IntProvider, random: &mut impl RandomSource) -> i32 {
    match provider {
        IntProvider::Constant(v) => *v,
        IntProvider::Tagged(tagged) => match tagged.as_ref() {
            TaggedIntProvider::Constant { value } => *value,
            TaggedIntProvider::Uniform {
                min_inclusive,
                max_inclusive,
            } => {
                let span = (max_inclusive - min_inclusive + 1).max(1);
                min_inclusive + random.next_int_bounded(span)
            }
            TaggedIntProvider::BiasedToBottom {
                min_inclusive,
                max_inclusive,
            } => {
                let span = (max_inclusive - min_inclusive + 1).max(1);
                let r1 = random.next_int_bounded(span);
                let r2 = random.next_int_bounded(span);
                min_inclusive + r1.min(r2)
            }
            TaggedIntProvider::Clamped {
                source,
                min_inclusive,
                max_inclusive,
            } => sample_int(source, random).clamp(*min_inclusive, *max_inclusive),
            TaggedIntProvider::ClampedNormal {
                mean,
                deviation,
                min_inclusive,
                max_inclusive,
            } => {
                let val = *mean + random.next_gaussian() as f32 * *deviation;
                (val.round() as i32).clamp(*min_inclusive, *max_inclusive)
            }
            TaggedIntProvider::Trapezoid { min, max, plateau } => {
                let span = max - min;
                if span <= 0 {
                    *min
                } else {
                    let half = (span - plateau) / 2;
                    let r1 = random.next_int_bounded((half + 1).max(1));
                    let r2 = random.next_int_bounded((half + 1).max(1));
                    min + r1 + r2
                }
            }
            TaggedIntProvider::WeightedList { distribution } => {
                let total_weight: i32 = distribution.iter().map(|w| w.weight).sum();
                if total_weight <= 0 {
                    0
                } else {
                    let mut pick = random.next_int_bounded(total_weight);
                    for item in distribution {
                        if pick < item.weight {
                            return sample_int(&item.data, random);
                        }
                        pick -= item.weight;
                    }
                    0
                }
            }
        },
    }
}

/// Applies a configured feature definition (Ore, SimpleBlock, Spring, Disk) at `origin`.
fn apply_configured_feature(
    feature: &ConfiguredFeature,
    origin: BlockPos,
    min_y: i32,
    height: i32,
    chunk_pos: ChunkPos,
    chunk: &mut ChunkData,
    random: &mut impl RandomSource,
) {
    match feature {
        ConfiguredFeature::Ore(config) | ConfiguredFeature::ScatteredOre(config) => {
            place_ore_blob(config, origin, min_y, height, chunk_pos, chunk, random);
        }
        ConfiguredFeature::SimpleBlock(config) => {
            place_simple_block(config, origin, min_y, height, chunk_pos, chunk);
        }
        ConfiguredFeature::SpringFeature(config) => {
            place_spring(config, origin, min_y, height, chunk_pos, chunk);
        }
        _ => {}
    }
}

/// Converts a `BlockStateSpec` into a `BlockState`.
fn spec_to_block_state(spec: &BlockStateSpec) -> BlockState {
    let rl = ResourceLocation::from_str(&spec.name)
        .unwrap_or_else(|_| ResourceLocation::minecraft("stone"));
    BlockState {
        name: rl,
        properties: spec.properties.clone(),
    }
}

/// Places a randomized ellipsoid ore blob (e.g. Iron, Coal, Diamond, Copper).
fn place_ore_blob(
    config: &OreConfig,
    origin: BlockPos,
    min_y: i32,
    height: i32,
    chunk_pos: ChunkPos,
    chunk: &mut ChunkData,
    _random: &mut impl RandomSource,
) {
    let size = config.size;
    if size <= 0 {
        return;
    }

    let radius = (size as f32) / 4.0;
    let radius_sq = radius * radius;
    let r_i32 = radius.ceil() as i32;

    for dy in -r_i32..=r_i32 {
        let y = origin.y + dy;
        if y < min_y || y >= min_y + height {
            continue;
        }

        let section_idx = ((y - min_y) / 16) as usize;
        if section_idx >= chunk.sections.len() {
            continue;
        }

        let local_y = (y - min_y).rem_euclid(16) as usize;

        for dz in -r_i32..=r_i32 {
            let z = origin.z + dz;
            let local_z = z - chunk_pos.min_block_z();
            if !(0..16).contains(&local_z) {
                continue;
            }

            for dx in -r_i32..=r_i32 {
                let x = origin.x + dx;
                let local_x = x - chunk_pos.min_block_x();
                if !(0..16).contains(&local_x) {
                    continue;
                }

                let dist_sq = (dx * dx + dy * dy + dz * dz) as f32;
                if dist_sq <= radius_sq {
                    let slot = local_index(local_x as usize, local_y, local_z as usize);
                    let section = &mut chunk.sections[section_idx];
                    let current_state = section.block_states.get(slot);

                    // Check if current block matches any target rule
                    for target in &config.targets {
                        if matches_target(target, current_state) {
                            let new_state = spec_to_block_state(&target.state);
                            section.block_states.set(slot, new_state);
                            break;
                        }
                    }
                }
            }
        }
    }
}

/// Checks if a block state matches an ore target rule (e.g. stone or deepslate ore replacement).
fn matches_target(target: &OreTarget, state: &BlockState) -> bool {
    match &target.target {
        oxide_datapack::feature::RuleTest::AlwaysTrue => true,
        oxide_datapack::feature::RuleTest::BlockMatch { block } => state.name.path() == block,
        oxide_datapack::feature::RuleTest::TagMatch { tag } => {
            if tag == "stone_ore_replaceables" || tag == "minecraft:stone_ore_replaceables" {
                state.name.path() == "stone"
                    || state.name.path() == "granite"
                    || state.name.path() == "diorite"
                    || state.name.path() == "andesite"
            } else if tag == "deepslate_ore_replaceables"
                || tag == "minecraft:deepslate_ore_replaceables"
            {
                state.name.path() == "deepslate" || state.name.path() == "tuff"
            } else {
                state.name.path() == tag.as_str()
            }
        }
        _ => false,
    }
}

/// Places a single decorative block (e.g. grass, flower, mushroom) if the space is air.
fn place_simple_block(
    config: &SimpleBlockConfig,
    origin: BlockPos,
    min_y: i32,
    height: i32,
    chunk_pos: ChunkPos,
    chunk: &mut ChunkData,
) {
    let y = origin.y;
    if y < min_y || y >= min_y + height {
        return;
    }

    let local_x = origin.x - chunk_pos.min_block_x();
    let local_z = origin.z - chunk_pos.min_block_z();
    if !(0..16).contains(&local_x) || !(0..16).contains(&local_z) {
        return;
    }

    let section_idx = ((y - min_y) / 16) as usize;
    if section_idx >= chunk.sections.len() {
        return;
    }

    let local_y = (y - min_y).rem_euclid(16) as usize;
    let slot = local_index(local_x as usize, local_y, local_z as usize);
    let section = &mut chunk.sections[section_idx];

    let current = section.block_states.get(slot);
    if current.name.path() == "air" {
        let state = match &config.to_place {
            BlockStateProvider::Simple { state } => spec_to_block_state(state),
            BlockStateProvider::RotatedBlock { state } => spec_to_block_state(state),
            _ => BlockState::new(ResourceLocation::minecraft("dandelion")),
        };
        section.block_states.set(slot, state);
    }
}

/// Places a spring fluid source block (water/lava spring emerging from stone).
fn place_spring(
    config: &SpringConfig,
    origin: BlockPos,
    min_y: i32,
    height: i32,
    chunk_pos: ChunkPos,
    chunk: &mut ChunkData,
) {
    let y = origin.y;
    if y < min_y || y >= min_y + height {
        return;
    }

    let local_x = origin.x - chunk_pos.min_block_x();
    let local_z = origin.z - chunk_pos.min_block_z();
    if !(0..16).contains(&local_x) || !(0..16).contains(&local_z) {
        return;
    }

    let section_idx = ((y - min_y) / 16) as usize;
    if section_idx >= chunk.sections.len() {
        return;
    }

    let local_y = (y - min_y).rem_euclid(16) as usize;
    let slot = local_index(local_x as usize, local_y, local_z as usize);
    let section = &mut chunk.sections[section_idx];

    let state = spec_to_block_state(&config.state);
    section.block_states.set(slot, state);
}
