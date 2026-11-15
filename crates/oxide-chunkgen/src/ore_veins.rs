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

use crate::fill::{local_index, CELL_BOUND_MARGIN};

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

    // Where the default block sits in each section's palette, resolved once. A vein write can
    // only append palette entries, never move one, so these stay valid for the whole pass --
    // and "is this the default block?" becomes an integer compare instead of a `BlockState`
    // clone and comparison per position.
    let default_index: Vec<Option<u32>> = chunk
        .sections
        .iter()
        .map(|section| {
            section
                .block_states
                .palette()
                .iter()
                .position(|state| *state == settings.default_block)
                .map(|index| index as u32)
        })
        .collect();

    // Cells that provably hold no vein. `vein_state` gives up when
    // `|vein_toggle| + edge_roundoff < VEININESS_THRESHOLD`, and `edge_roundoff` is never
    // positive, so a cell whose `vein_toggle` stays strictly inside +-threshold cannot place
    // anything -- and veins are rare, so that is most cells. The bounds must clear the
    // threshold by [`CELL_BOUND_MARGIN`]; see there for why. The bounds come from the
    // slot's cell-corner grid, which exists once the slot has been sampled in this chunk;
    // where none can be proven the cell is evaluated block by block as before.
    let cell_width = caches.cell_width();
    let cell_height = caches.cell_height();
    let cells_xz = (16 / cell_width).max(1) as usize;
    let cells_y = (height / cell_height).max(0) as usize;
    let _ = router.sample_in_chunk(
        caches,
        RouterSlot::VeinToggle,
        pos.min_block_x(),
        band_min.max(min_y),
        pos.min_block_z(),
    );
    let mut veinless = vec![false; cells_xz * cells_xz * cells_y];
    for cell_y in 0..cells_y {
        let cell_min_y = min_y + cell_y as i32 * cell_height;
        if cell_min_y > band_max || cell_min_y + cell_height <= band_min {
            continue;
        }
        for cell_z in 0..cells_xz {
            for cell_x in 0..cells_xz {
                veinless[(cell_y * cells_xz + cell_z) * cells_xz + cell_x] = router
                    .cell_bounds(
                        caches,
                        RouterSlot::VeinToggle,
                        cell_x as i32,
                        cell_y as i32,
                        cell_z as i32,
                    )
                    .is_some_and(|(lo, hi)| {
                        lo > -VEININESS_THRESHOLD + CELL_BOUND_MARGIN
                            && hi < VEININESS_THRESHOLD - CELL_BOUND_MARGIN
                    });
            }
        }
    }

    // The six states a vein can place, built once per chunk, and per section the palette
    // index each has once written there (`u32::MAX` = not yet). A vein block used to build a
    // fresh `BlockState` and hash it into the palette on every write.
    let states: Vec<BlockState> = VEIN_BLOCKS
        .iter()
        .map(|name| BlockState::new(ResourceLocation::minecraft(*name)))
        .collect();
    let mut palette_index = vec![u32::MAX; chunk.sections.len() * VEIN_BLOCKS.len()];

    for y in band_min.max(min_y)..=band_max.min(min_y + height - 1) {
        let Some(section_index) = section_of(settings, y) else {
            continue;
        };
        let cell_y = ((y - min_y).div_euclid(cell_height)) as usize;
        let Some(Some(default_index)) = default_index.get(section_index).copied() else {
            continue;
        };
        let local_y = (y - min_y).rem_euclid(16) as usize;
        for local_z in 0..16usize {
            let block_z = pos.min_block_z() + local_z as i32;
            let cell_z = (local_z as i32).div_euclid(cell_width) as usize;
            for local_x in 0..16usize {
                let block_x = pos.min_block_x() + local_x as i32;
                let cell_x = (local_x as i32).div_euclid(cell_width) as usize;
                if veinless
                    .get((cell_y * cells_xz + cell_z) * cells_xz + cell_x)
                    .copied()
                    .unwrap_or(false)
                {
                    continue;
                }
                if chunk.sections[section_index].block_states.indices()
                    [local_index(local_x, local_y, local_z)]
                    != default_index
                {
                    continue;
                }
                if let Some(name) = vein_state(router, caches, &ore_factory, block_x, y, block_z) {
                    let id = vein_block_id(name);
                    let cached = &mut palette_index[section_index * VEIN_BLOCKS.len() + id];
                    let block_states = &mut chunk.sections[section_index].block_states;
                    // Interned on this section's first write of the state, exactly where the
                    // old per-block `set` would have added it, so palette order is unchanged.
                    if *cached == u32::MAX {
                        *cached = block_states.index_of_or_insert(states[id].clone());
                    }
                    block_states.set_index(local_index(local_x, local_y, local_z), *cached);
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
) -> Option<&'static str> {
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
        return Some(name);
    }
    Some(vein.filler)
}

/// Every block name a vein can place: the index into this is the id `apply_ore_veins` keys
/// its per-section palette cache on.
const VEIN_BLOCKS: [&str; 6] = [
    COPPER.ore,
    COPPER.raw_ore_block,
    COPPER.filler,
    IRON.ore,
    IRON.raw_ore_block,
    IRON.filler,
];

fn vein_block_id(name: &'static str) -> usize {
    VEIN_BLOCKS
        .iter()
        .position(|known| *known == name)
        .expect("vein_state only returns names from VEIN_BLOCKS")
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
