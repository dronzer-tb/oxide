//! `netherrack_replace_blobs`: swaps the target block for another inside a diamond.
//!
//! The origin first sinks to the highest target block at or below it (a column that never meets
//! one gives up), then every target block within the radii, visited by Manhattan distance, is
//! replaced.

use anyhow::{Context, Result};
use oxide_core::{BlockPos, BlockState, ResourceLocation};
use oxide_datapack::nether_feature::ReplaceBlobsConfig;
use oxide_datapack::placement::IntProvider;
use oxide_datapack::BlockFacts;

use super::super::context::FeatureContext;
use super::super::provider::sample_int;
use super::super::state_provider::state_from_spec;
use super::{is_block_at, offset};

#[derive(Debug)]
pub struct ReplaceBlobsFeature {
    target: ResourceLocation,
    state: BlockState,
    radius: IntProvider,
}

impl ReplaceBlobsFeature {
    pub fn compile(config: &ReplaceBlobsConfig, facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            target: state_from_spec(&config.target, facts).context("replace blobs target")?.name,
            state: state_from_spec(&config.state, facts).context("replace blobs state")?,
            radius: config.radius.clone(),
        })
    }

    /// `NetherrackReplaceBlobsFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let min_y = cx.region.min_y();
        // `Level.getMaxY` is inclusive; the region's `max_y` is one past the top.
        let max_y = cx.region.max_y() - 1;
        let mut origin = cx.origin;
        origin = BlockPos::new(origin.x, origin.y.max(min_y + 1).min(max_y), origin.z);
        loop {
            if origin.y <= min_y + 1 {
                return false;
            }
            if is_block_at(cx, origin, &self.target) {
                break;
            }
            origin = offset(origin, 0, -1, 0);
        }
        let radius_x = sample_int(&self.radius, cx.random);
        let radius_y = sample_int(&self.radius, cx.random);
        let radius_z = sample_int(&self.radius, cx.random);
        let mut placed = false;
        // `BlockPos.withinManhattan(origin, rx, ry, rz)`, stopping past the largest radius: by
        // distance, then x ascending, then y ascending, then +z before -z.
        for depth in 0..=radius_x.max(radius_y).max(radius_z) {
            let max_x = radius_x.min(depth);
            for x in -max_x..=max_x {
                let max_y_step = radius_y.min(depth - x.abs());
                for y in -max_y_step..=max_y_step {
                    let z = depth - x.abs() - y.abs();
                    if z > radius_z {
                        continue;
                    }
                    self.replace(cx, offset(origin, x, y, z), &mut placed);
                    if z != 0 {
                        self.replace(cx, offset(origin, x, y, -z), &mut placed);
                    }
                }
            }
        }
        placed
    }

    fn replace(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos, placed: &mut bool) {
        if is_block_at(cx, pos, &self.target) {
            cx.region.set_block(pos, self.state.clone());
            *placed = true;
        }
    }
}
