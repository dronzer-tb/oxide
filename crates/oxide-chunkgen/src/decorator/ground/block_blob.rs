//! `block_blob`: a forest rock, three overlapping lumps of one block.
//!
//! The origin first sinks until the block below it is one the config allows the blob on, then
//! three small ellipsoid-ish lumps are stamped, each drifting a block sideways and down from the
//! last.

use anyhow::{Context, Result};
use oxide_core::{BlockPos, BlockState, RandomSource};
use oxide_datapack::ground_feature::BlockBlobConfig;
use oxide_datapack::BlockFacts;

use super::super::context::FeatureContext;
use super::super::predicate::Predicate;
use super::super::state_provider::state_from_spec;
use super::between_closed;

#[derive(Debug)]
pub struct BlockBlobFeature {
    state: BlockState,
    can_place_on: Predicate,
}

impl BlockBlobFeature {
    pub fn compile(config: &BlockBlobConfig, facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            state: state_from_spec(&config.state, facts).context("block blob state")?,
            can_place_on: Predicate::compile(&config.can_place_on, facts)
                .context("block blob can_place_on")?,
        })
    }

    /// `BlockBlobFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let min_y = cx.region.min_y();
        let mut origin = cx.origin;
        while origin.y > min_y + 3 {
            let below = BlockPos::new(origin.x, origin.y - 1, origin.z);
            if self.can_place_on.test(cx, below) {
                break;
            }
            origin = below;
        }
        if origin.y <= min_y + 3 {
            return false;
        }
        for _ in 0..3 {
            let rx = cx.random.next_int_bounded(2);
            let ry = cx.random.next_int_bounded(2);
            let rz = cx.random.next_int_bounded(2);
            let radius = (rx + ry + rz) as f32 * 0.333 + 0.5;
            let from = BlockPos::new(origin.x - rx, origin.y - ry, origin.z - rz);
            let to = BlockPos::new(origin.x + rx, origin.y + ry, origin.z + rz);
            for pos in between_closed(from, to) {
                let dx = f64::from(pos.x - origin.x);
                let dy = f64::from(pos.y - origin.y);
                let dz = f64::from(pos.z - origin.z);
                if dx * dx + dy * dy + dz * dz <= f64::from(radius * radius) {
                    cx.region.set_block_ref(pos, &self.state);
                }
            }
            origin = BlockPos::new(
                origin.x + (-1 + cx.random.next_int_bounded(2)),
                origin.y - cx.random.next_int_bounded(2),
                origin.z + (-1 + cx.random.next_int_bounded(2)),
            );
        }
        true
    }
}
