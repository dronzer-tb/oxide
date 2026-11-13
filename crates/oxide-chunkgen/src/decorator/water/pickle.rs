//! `minecraft:sea_pickle`: clusters of sea pickles scattered on the ocean floor.
//!
//! Port of `SeaPickleFeature`: `count` is sampled once, then each try draws its offset
//! (`nextInt(8) - nextInt(8)` on x then z) and its pickle count (`nextInt(4) + 1`) before the
//! cell is checked, so a failed try still consumes all three draws.

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, HeightmapType, RandomSource};
use oxide_datapack::placement::IntProvider;
use oxide_datapack::water_feature::SeaPickleConfig;
use oxide_datapack::BlockFacts;

use super::support::{default_state, is_water, survives};
use crate::decorator::context::FeatureContext;
use crate::decorator::provider::sample_int;

#[derive(Debug)]
pub struct SeaPickleFeature {
    count: IntProvider,
    pickle: BlockState,
}

impl SeaPickleFeature {
    pub(super) fn compile(config: &SeaPickleConfig, facts: &BlockFacts) -> Result<Self> {
        Ok(Self { count: config.count.clone(), pickle: default_state(facts, "sea_pickle")? })
    }

    pub(super) fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let count = sample_int(&self.count, &mut *cx.random);
        let mut placed = 0;
        for _ in 0..count {
            let x = cx.random.next_int_bounded(8) - cx.random.next_int_bounded(8);
            let z = cx.random.next_int_bounded(8) - cx.random.next_int_bounded(8);
            let height = cx.region.height(HeightmapType::OceanFloor, origin.x + x, origin.z + z);
            let pickles = cx.random.next_int_bounded(4) + 1;
            let Some(y) = height else {
                continue;
            };
            let pos = BlockPos::new(origin.x + x, y, origin.z + z);
            let state = self.pickle.clone().with_property("pickles", pickles.to_string());
            if is_water(cx, pos) && survives(cx, &state, pos) {
                cx.region.set_block(pos, state);
                placed += 1;
            }
        }
        placed > 0
    }
}
