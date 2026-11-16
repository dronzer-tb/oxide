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

    let cell_width = caches.cell_width();
    let cell_height = caches.cell_height();
    let cells_xz = (16 / cell_width).max(1) as usize;
    let cells_y = (height / cell_height).max(0) as usize;
    let y_lo = band_min.max(min_y);
    let y_hi = band_max.min(min_y + height - 1);

    // Per y, the `edge_roundoff` each vein type would use there, or `None` where y is outside
    // that type's band. These are `vein_state`'s own first steps, which depend on y alone:
    // `vein_toggle > 0` picks copper, else iron; a y outside the picked band gives up; and so
    // does `|vein_toggle| + edge_roundoff < VEININESS_THRESHOLD`. Each value is computed by the
    // same `edge_roundoff` call `vein_state` makes, so a test against it is bit-for-bit that
    // test. Between the bands (y -7..=-1) both are `None` and the whole row is skipped.
    let band_len = (y_hi - y_lo + 1).max(0) as usize;
    let copper_roundoff: Vec<Option<f64>> =
        (0..band_len).map(|i| edge_roundoff(&COPPER, y_lo + i as i32)).collect();
    let iron_roundoff: Vec<Option<f64>> =
        (0..band_len).map(|i| edge_roundoff(&IRON, y_lo + i as i32)).collect();

    // Cells that provably hold no vein. A block can only place something when its type's
    // `|vein_toggle| + edge_roundoff` reaches `VEININESS_THRESHOLD`: a positive toggle is copper
    // and needs `toggle + copper_roundoff >= threshold` at a y in the copper band; a
    // non-positive one is iron and needs `-toggle + iron_roundoff >= threshold` in the iron
    // band. So a cell is veinless when, over its layer's y, neither side can get there:
    // `hi + max(copper_roundoff) < threshold` (or no y of the layer is in the copper band), and
    // the same for `-lo` and iron. Float addition rounds monotonically, so the block's own sum
    // is at most the bound's sum; the bounds must clear the threshold by
    // [`CELL_BOUND_MARGIN`] (see there for why). This contains the old test
    // (`-threshold < lo`, `hi < threshold`), since a roundoff is never positive. The bounds
    // come from the slot's cell-corner grid, which exists once the slot has been sampled in
    // this chunk; where none can be proven the cell is evaluated block by block.
    let _ = router.sample_in_chunk(
        caches,
        RouterSlot::VeinToggle,
        pos.min_block_x(),
        y_lo,
        pos.min_block_z(),
    );
    let layer_max = |table: &[Option<f64>], cell_y: usize| -> Option<f64> {
        let layer_min_y = min_y + cell_y as i32 * cell_height;
        (layer_min_y..layer_min_y + cell_height)
            .filter(|y| (y_lo..=y_hi).contains(y))
            .filter_map(|y| table[(y - y_lo) as usize])
            .reduce(f64::max)
    };
    let mut veinless = vec![false; cells_xz * cells_xz * cells_y];
    for cell_y in 0..cells_y {
        let cell_min_y = min_y + cell_y as i32 * cell_height;
        if cell_min_y > band_max || cell_min_y + cell_height <= band_min {
            continue;
        }
        let copper_max = layer_max(&copper_roundoff, cell_y);
        let iron_max = layer_max(&iron_roundoff, cell_y);
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
                        copper_max.map_or(true, |er| {
                            hi + er < VEININESS_THRESHOLD - CELL_BOUND_MARGIN
                        }) && iron_max.map_or(true, |er| {
                            -lo + er < VEININESS_THRESHOLD - CELL_BOUND_MARGIN
                        })
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

    // Per-column cell coordinates, resolved once instead of a `div_euclid` per block.
    let cell_of: [usize; 16] =
        std::array::from_fn(|local| (local as i32).div_euclid(cell_width) as usize);
    let in_cell: [usize; 16] = std::array::from_fn(|local| {
        local.wrapping_sub(cell_of[local].wrapping_mul(cell_width as usize))
    });

    // `vein_toggle` a whole cell at a time through `sample_cell` (bit-identical to
    // `sample_in_chunk` at each block), into a buffer for one layer of cells, evaluated on a
    // cell's first use. `toggle_state` per cell: 0 not yet asked, 1 in `toggle_values`, 2
    // declined (that cell's blocks go through `sample_in_chunk`).
    let cell_w = cell_width as usize;
    let cell_len = router.cell_len(caches);
    let mut toggle_values = vec![0.0f64; cells_xz * cells_xz * cell_len];
    let mut toggle_state = vec![0u8; cells_xz * cells_xz];
    let mut loaded_cell_y = usize::MAX;

    for y in y_lo..=y_hi {
        let Some(section_index) = section_of(settings, y) else {
            continue;
        };
        let band_row = (y - y_lo) as usize;
        let copper_er = copper_roundoff[band_row];
        let iron_er = iron_roundoff[band_row];
        if copper_er.is_none() && iron_er.is_none() {
            continue;
        }
        let cell_y = ((y - min_y).div_euclid(cell_height)) as usize;
        let Some(Some(default_index)) = default_index.get(section_index).copied() else {
            continue;
        };
        if cell_y != loaded_cell_y {
            toggle_state.fill(0);
            loaded_cell_y = cell_y;
        }
        let cell_local_y = ((y - min_y) - cell_y as i32 * cell_height) as usize;
        let local_y = (y - min_y).rem_euclid(16) as usize;
        for local_z in 0..16usize {
            let block_z = pos.min_block_z() + local_z as i32;
            let cell_z = cell_of[local_z];
            let row_index = local_index(0, local_y, local_z);
            // Which blocks of this row hold the default block. Read once per row: a vein
            // write only changes its own position, and each position is read before its own
            // write, so this is what the per-block read saw.
            let mut row_default = 0u32;
            {
                let indices = &chunk.sections[section_index].block_states.indices()
                    [row_index..row_index + 16];
                for (local_x, &index) in indices.iter().enumerate() {
                    row_default |= ((index == default_index) as u32) << local_x;
                }
            }
            if row_default == 0 {
                continue;
            }
            let lane_row = (cell_local_y * cell_w + in_cell[local_z]) * cell_w;
            for local_x in 0..16usize {
                if row_default & (1 << local_x) == 0 {
                    continue;
                }
                let cell_x = cell_of[local_x];
                if veinless
                    .get((cell_y * cells_xz + cell_z) * cells_xz + cell_x)
                    .copied()
                    .unwrap_or(false)
                {
                    continue;
                }
                let block_x = pos.min_block_x() + local_x as i32;
                let cell_index = cell_z * cells_xz + cell_x;
                let in_grid = cell_x < cells_xz && cell_z < cells_xz && cell_y < cells_y;
                let veininess = if in_grid {
                    if toggle_state[cell_index] == 0 {
                        let values =
                            &mut toggle_values[cell_index * cell_len..(cell_index + 1) * cell_len];
                        toggle_state[cell_index] = if router.sample_cell(
                            caches,
                            RouterSlot::VeinToggle,
                            cell_x as i32,
                            cell_y as i32,
                            cell_z as i32,
                            values,
                        ) {
                            1
                        } else {
                            2
                        };
                    }
                    if toggle_state[cell_index] == 1 {
                        toggle_values[cell_index * cell_len + lane_row + in_cell[local_x]]
                    } else {
                        router.sample_in_chunk(caches, RouterSlot::VeinToggle, block_x, y, block_z)
                    }
                } else {
                    router.sample_in_chunk(caches, RouterSlot::VeinToggle, block_x, y, block_z)
                };
                // `vein_state`'s own early outs, against the per-y tables.
                let Some(er) = (if veininess > 0.0 { copper_er } else { iron_er }) else {
                    continue;
                };
                if veininess.abs() + er < VEININESS_THRESHOLD {
                    continue;
                }
                if let Some(name) =
                    vein_state(router, caches, &ore_factory, block_x, y, block_z, veininess)
                {
                    let id = vein_block_id(name);
                    let cached = &mut palette_index[section_index * VEIN_BLOCKS.len() + id];
                    let block_states = &mut chunk.sections[section_index].block_states;
                    // Interned on this section's first write of the state, exactly where the
                    // old per-block `set` would have added it, so palette order is unchanged.
                    if *cached == u32::MAX {
                        *cached = block_states.index_of_or_insert(states[id].clone());
                    }
                    block_states.set_index(row_index + local_x, *cached);
                }
            }
        }
    }
}

/// `vein`'s `edge_roundoff` at `y`, or `None` when y is outside its band -- exactly the values
/// `vein_state` computes and gives up on.
fn edge_roundoff(vein: &VeinType, y: i32) -> Option<f64> {
    let distance_from_top = vein.max_y - y;
    let distance_from_bottom = y - vein.min_y;
    if distance_from_bottom < 0 || distance_from_top < 0 {
        return None;
    }
    let distance_from_edge = distance_from_top.min(distance_from_bottom);
    // Fades veins out over the last 20 blocks before a band's edge.
    Some(clamped_map(
        distance_from_edge as f64,
        0.0,
        EDGE_ROUNDOFF_BEGIN,
        MAX_EDGE_ROUNDOFF,
        0.0,
    ))
}

/// Vanilla's `OreVeinifier` filler at one position, given `vein_toggle` there.
fn vein_state(
    router: &NoiseRouterEvaluator,
    caches: &ChunkCaches,
    ore_factory: &oxide_noise::WorldPositionalFactory,
    x: i32,
    y: i32,
    z: i32,
    veininess: f64,
) -> Option<&'static str> {
    let vein = if veininess > 0.0 { &COPPER } else { &IRON };
    let edge_roundoff = edge_roundoff(vein, y)?;
    let veininess_ridged = veininess.abs();
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
