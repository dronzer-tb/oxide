//! Ore veins: the copper/granite and iron/tuff ribbons that run through deep stone.
//!
//! Ported from the decompiled 26.2 `OreVeinifier`. Unlike ore *blobs* (which are features, and
//! are not this), veins are driven by the noise router's `vein_toggle`, `vein_ridged` and
//! `vein_gap` slots -- data this crate already loaded and, until now, never read.
//!
//! Vanilla applies this as a block-state filler during the noise fill, over positions the
//! aquifer left as stone. This runs as a pass over the filled chunk instead, which reaches the
//! same result because a vein only ever replaces the default block.

use oxide_core::{BlockState, ChunkData, ChunkPos, RandomSource, ResourceLocation};
use oxide_datapack::NoiseGeneratorSettings;
use oxide_noise::{ChunkCaches, NoiseRouterEvaluator, RouterSlot};

use crate::fill::local_index;

const VEININESS_THRESHOLD: f64 = 0.4;
const EDGE_ROUNDOFF_BEGIN: f64 = 20.0;
const MAX_EDGE_ROUNDOFF: f64 = -0.2;
const VEIN_SOLIDNESS: f32 = 0.7;
const MIN_RICHNESS: f64 = 0.1;
const MAX_RICHNESS: f64 = 0.3;
const MAX_RICHNESS_THRESHOLD: f64 = 0.6;
const CHANCE_OF_RAW_ORE_BLOCK: f32 = 0.02;
const SKIP_ORE_IF_GAP_NOISE_IS_BELOW: f64 = -0.3;

/// `OreVeinifier.VeinType`. The y bounds are vanilla's own constants, not datapack-driven.
struct VeinType {
    ore: &'static str,
    raw_ore_block: &'static str,
    filler: &'static str,
    min_y: i32,
    max_y: i32,
}

const COPPER: VeinType = VeinType {
    ore: "copper_ore",
    raw_ore_block: "raw_copper_block",
    filler: "granite",
    min_y: 0,
    max_y: 50,
};

const IRON: VeinType = VeinType {
    ore: "deepslate_iron_ore",
    raw_ore_block: "raw_iron_block",
    filler: "tuff",
    min_y: -60,
    max_y: -8,
};

/// Replaces default-block positions inside a vein with its ore or filler stone.
pub fn apply_ore_veins(
    chunk: &mut ChunkData,
    pos: ChunkPos,
    settings: &NoiseGeneratorSettings,
    router: &NoiseRouterEvaluator,
    caches: &ChunkCaches,
) {
    if !settings.ore_veins_enabled {
        return;
    }

    // Vanilla's oreRandom: the RandomState positional factory, hashed with "minecraft:ore".
    let ore_factory = router
        .positional_factory()
        .from_hash_of("minecraft:ore")
        .fork_positional();

    let min_y = settings.noise.min_y;
    let height = settings.noise.height;
    // Outside both vein types' bands vanilla returns "no opinion" for every position, so the
    // whole band-less part of the column is skipped rather than sampled.
    let band_min = IRON.min_y.min(COPPER.min_y);
    let band_max = IRON.max_y.max(COPPER.max_y);

    for y in band_min.max(min_y)..=band_max.min(min_y + height - 1) {
        for local_z in 0..16usize {
            let block_z = pos.min_block_z() + local_z as i32;
            for local_x in 0..16usize {
                let block_x = pos.min_block_x() + local_x as i32;
                if get_block(chunk, settings, local_x, y, local_z).as_ref()
                    != Some(&settings.default_block)
                {
                    continue;
                }
                if let Some(state) = vein_state(router, caches, &ore_factory, block_x, y, block_z) {
                    set_block(chunk, settings, local_x, y, local_z, state);
                }
            }
        }
    }
}

fn vein_state(
    router: &NoiseRouterEvaluator,
    caches: &ChunkCaches,
    ore_factory: &oxide_noise::WorldPositionalFactory,
    x: i32,
    y: i32,
    z: i32,
) -> Option<BlockState> {
    let veininess = router.sample_in_chunk(caches, RouterSlot::VeinToggle, x, y, z);
    let vein = if veininess > 0.0 { &COPPER } else { &IRON };

    let distance_from_top = vein.max_y - y;
    let distance_from_bottom = y - vein.min_y;
    if distance_from_bottom < 0 || distance_from_top < 0 {
        return None;
    }

    let veininess_ridged = veininess.abs();
    let distance_from_edge = distance_from_top.min(distance_from_bottom);
    // Fades veins out over the last 20 blocks before a band's edge.
    let edge_roundoff = clamped_map(
        distance_from_edge as f64,
        0.0,
        EDGE_ROUNDOFF_BEGIN,
        MAX_EDGE_ROUNDOFF,
        0.0,
    );
    if veininess_ridged + edge_roundoff < VEININESS_THRESHOLD {
        return None;
    }

    let mut random = ore_factory.at(x, y, z);
    if random.next_float() > VEIN_SOLIDNESS {
        return None;
    }
    if router.sample_in_chunk(caches, RouterSlot::VeinRidged, x, y, z) >= 0.0 {
        return None;
    }

    let richness = clamped_map(
        veininess_ridged,
        VEININESS_THRESHOLD,
        MAX_RICHNESS_THRESHOLD,
        MIN_RICHNESS,
        MAX_RICHNESS,
    );
    if (random.next_float() as f64) < richness
        && router.sample_in_chunk(caches, RouterSlot::VeinGap, x, y, z)
            > SKIP_ORE_IF_GAP_NOISE_IS_BELOW
    {
        let name = if random.next_float() < CHANCE_OF_RAW_ORE_BLOCK {
            vein.raw_ore_block
        } else {
            vein.ore
        };
        return Some(BlockState::new(ResourceLocation::minecraft(name)));
    }
    Some(BlockState::new(ResourceLocation::minecraft(vein.filler)))
}

/// Vanilla `Mth.clampedMap`.
fn clamped_map(value: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    let factor = (value - from_min) / (from_max - from_min);
    if factor < 0.0 {
        to_min
    } else if factor > 1.0 {
        to_max
    } else {
        to_min + factor * (to_max - to_min)
    }
}

fn section_of(settings: &NoiseGeneratorSettings, y: i32) -> Option<usize> {
    let min_y = settings.noise.min_y;
    if y < min_y || y >= min_y + settings.noise.height {
        return None;
    }
    Some(((y - min_y) / 16) as usize)
}

fn get_block(
    chunk: &ChunkData,
    settings: &NoiseGeneratorSettings,
    x: usize,
    y: i32,
    z: usize,
) -> Option<BlockState> {
    let section = chunk.sections.get(section_of(settings, y)?)?;
    let local_y = (y - settings.noise.min_y).rem_euclid(16) as usize;
    Some(section.block_states.get(local_index(x, local_y, z)).clone())
}

fn set_block(
    chunk: &mut ChunkData,
    settings: &NoiseGeneratorSettings,
    x: usize,
    y: i32,
    z: usize,
    state: BlockState,
) {
    let Some(index) = section_of(settings, y) else {
        return;
    };
    let Some(section) = chunk.sections.get_mut(index) else {
        return;
    };
    let local_y = (y - settings.noise.min_y).rem_euclid(16) as usize;
    section.block_states.set(local_index(x, local_y, z), state);
}
