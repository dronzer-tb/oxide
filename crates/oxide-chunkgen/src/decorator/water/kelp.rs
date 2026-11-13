//! `minecraft:kelp`: a kelp column grown up from the ocean floor.
//!
//! Port of `KelpFeature`: one draw for the column height (`1 + nextInt(10)`), then the column is
//! walked upward from the `OCEAN_FLOOR` height. A cell that is water with water above and where
//! `kelp_plant` survives takes a plant; the last cell takes the `kelp` head with `age` in
//! `20..=23`. When the column runs out of water early, the cell below the stop becomes the head
//! instead, if a head survives there and the cell below that is not already a head.

use anyhow::Result;
use oxide_core::{BlockState, HeightmapType, RandomSource};
use oxide_datapack::BlockFacts;

use super::support::{default_state, is_block_at, is_water, relative, survives, Dir};
use crate::decorator::context::FeatureContext;

#[derive(Debug)]
pub struct KelpFeature {
    head: BlockState,
    plant: BlockState,
}

impl KelpFeature {
    pub(super) fn compile(facts: &BlockFacts) -> Result<Self> {
        Ok(Self { head: default_state(facts, "kelp")?, plant: default_state(facts, "kelp_plant")? })
    }

    /// The head with vanilla's generated age, `nextInt(4) + 20`.
    fn aged_head(&self, cx: &mut FeatureContext<'_, '_, '_>) -> BlockState {
        let age = cx.random.next_int_bounded(4) + 20;
        self.head.clone().with_property("age", age.to_string())
    }

    pub(super) fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let Some(y) = cx.region.height(HeightmapType::OceanFloor, origin.x, origin.z) else {
            return false;
        };
        let mut pos = oxide_core::BlockPos::new(origin.x, y, origin.z);
        let mut placed = 0;
        if is_water(cx, pos) {
            let height = 1 + cx.random.next_int_bounded(10);
            for h in 0..=height {
                if is_water(cx, pos) && is_water(cx, relative(pos, Dir::Up, 1)) && survives(cx, &self.plant, pos) {
                    if h == height {
                        let head = self.aged_head(cx);
                        cx.region.set_block(pos, head);
                        placed += 1;
                    } else {
                        cx.region.set_block(pos, self.plant.clone());
                    }
                } else if h > 0 {
                    let below = relative(pos, Dir::Down, 1);
                    if survives(cx, &self.head, below) && !is_block_at(cx, relative(below, Dir::Down, 1), "kelp") {
                        let head = self.aged_head(cx);
                        cx.region.set_block(below, head);
                        placed += 1;
                    }
                    break;
                }
                pos = relative(pos, Dir::Up, 1);
            }
        }
        placed > 0
    }
}
