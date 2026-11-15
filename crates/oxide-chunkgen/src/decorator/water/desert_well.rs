//! `minecraft:desert_well`: the sandstone well of deserts.
//!
//! Port of `DesertWellFeature`. Starting one above the origin, it sinks through air (while above
//! `minY + 2`) to the first block, which must be `sand`; a 5x5 footprint with air two deep under
//! any column aborts. The layout is then written in vanilla's order, and the only random draws are
//! the two `Util.getRandom` picks among the five water cells (centre, east, south, west, north)
//! whose cells one and two below become suspicious sand.
//!
//! Suspicious sand gets its `archaeology/desert_well` loot table through a block entity, which
//! the region cannot carry: each one placed is tallied as `block_entity:minecraft:brushable_block`.

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, RandomSource};
use oxide_datapack::BlockFacts;

use super::support::{default_state, is_air, is_block_at, offset, relative, Dir, HORIZONTAL};
use crate::decorator::context::FeatureContext;

#[derive(Debug)]
pub struct DesertWellFeature {
    sand: BlockState,
    slab: BlockState,
    sandstone: BlockState,
    water: BlockState,
    suspicious_sand: BlockState,
}

impl DesertWellFeature {
    pub(super) fn compile(facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            sand: default_state(facts, "sand")?,
            slab: default_state(facts, "sandstone_slab")?,
            sandstone: default_state(facts, "sandstone")?,
            water: default_state(facts, "water")?,
            suspicious_sand: default_state(facts, "suspicious_sand")?,
        })
    }

    pub(super) fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let mut origin = relative(cx.origin, Dir::Up, 1);
        let min_y = cx.region.min_y();
        while is_air(cx, origin) && origin.y > min_y + 2 {
            origin = relative(origin, Dir::Down, 1);
        }
        if !is_block_at(cx, origin, "sand") {
            return false;
        }
        for ox in -2..=2 {
            for oz in -2..=2 {
                if is_air(cx, offset(origin, ox, -1, oz)) && is_air(cx, offset(origin, ox, -2, oz))
                {
                    return false;
                }
            }
        }

        for oy in -2..=0 {
            for ox in -2..=2 {
                for oz in -2..=2 {
                    self.set(cx, offset(origin, ox, oy, oz), &self.sandstone);
                }
            }
        }
        self.set(cx, origin, &self.water);
        for direction in HORIZONTAL {
            self.set(cx, relative(origin, direction, 1), &self.water);
        }
        let sand_center = relative(origin, Dir::Down, 1);
        self.set(cx, sand_center, &self.sand);
        for direction in HORIZONTAL {
            self.set(cx, relative(sand_center, direction, 1), &self.sand);
        }
        for ox in -2..=2 {
            for oz in -2..=2 {
                if ox == -2 || ox == 2 || oz == -2 || oz == 2 {
                    self.set(cx, offset(origin, ox, 1, oz), &self.sandstone);
                }
            }
        }
        for (ox, oz) in [(2, 0), (-2, 0), (0, 2), (0, -2)] {
            self.set(cx, offset(origin, ox, 1, oz), &self.slab);
        }
        for ox in -1..=1 {
            for oz in -1..=1 {
                let state = if ox == 0 && oz == 0 {
                    &self.sandstone
                } else {
                    &self.slab
                };
                self.set(cx, offset(origin, ox, 4, oz), state);
            }
        }
        for oy in 1..=3 {
            for (ox, oz) in [(-1, -1), (-1, 1), (1, -1), (1, 1)] {
                self.set(cx, offset(origin, ox, oy, oz), &self.sandstone);
            }
        }

        let water_cells = [
            origin,
            relative(origin, Dir::East, 1),
            relative(origin, Dir::South, 1),
            relative(origin, Dir::West, 1),
            relative(origin, Dir::North, 1),
        ];
        for depth in [1, 2] {
            let cell = water_cells[cx.random.next_int_bounded(water_cells.len() as i32) as usize];
            let pos = relative(cell, Dir::Down, depth);
            if cx.region.set_block(pos, self.suspicious_sand.clone()) {
                cx.unsupported
                    .record("block_entity", "minecraft:brushable_block");
            }
        }
        true
    }

    fn set(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos, state: &BlockState) {
        cx.region.set_block(pos, state.clone());
    }
}
