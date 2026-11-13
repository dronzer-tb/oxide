//! `minecraft:seagrass`: one seagrass plant near the origin, short or tall.
//!
//! Port of `SeagrassFeature`: the offset is `nextInt(8) - nextInt(8)` on x then z, the floor is
//! the `OCEAN_FLOOR` height there, and only a water cell is tried. A `nextDouble` below the
//! configured `float` probability makes the plant tall. Vanilla checks survival of the lower
//! half only; a tall plant is then written only when the cell above is water too, yet the
//! feature still reports success when it is not.

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, HeightmapType, RandomSource};
use oxide_datapack::water_feature::SeagrassConfig;
use oxide_datapack::BlockFacts;

use super::support::{default_state, is_water, relative, survives, Dir};
use crate::decorator::context::FeatureContext;

#[derive(Debug)]
pub struct SeagrassFeature {
    probability: f32,
    short: BlockState,
    tall_lower: BlockState,
    tall_upper: BlockState,
}

impl SeagrassFeature {
    pub(super) fn compile(config: &SeagrassConfig, facts: &BlockFacts) -> Result<Self> {
        let tall_lower = default_state(facts, "tall_seagrass")?;
        Ok(Self {
            probability: config.probability,
            short: default_state(facts, "seagrass")?,
            tall_upper: tall_lower.clone().with_property("half", "upper"),
            tall_lower,
        })
    }

    pub(super) fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let x = cx.random.next_int_bounded(8) - cx.random.next_int_bounded(8);
        let z = cx.random.next_int_bounded(8) - cx.random.next_int_bounded(8);
        let Some(y) = cx.region.height(HeightmapType::OceanFloor, origin.x + x, origin.z + z) else {
            return false;
        };
        let pos = BlockPos::new(origin.x + x, y, origin.z + z);
        if !is_water(cx, pos) {
            return false;
        }
        let tall = cx.random.next_double() < f64::from(self.probability);
        let state = if tall { &self.tall_lower } else { &self.short };
        if !survives(cx, state, pos) {
            return false;
        }
        if tall {
            let above = relative(pos, Dir::Up, 1);
            if is_water(cx, above) {
                cx.region.set_block(pos, self.tall_lower.clone());
                cx.region.set_block(above, self.tall_upper.clone());
            }
        } else {
            cx.region.set_block(pos, self.short.clone());
        }
        true
    }
}
