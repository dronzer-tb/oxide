//! `block_pile`: a low heap of blocks, two layers high, such as a pumpkin or hay patch.
//!
//! The box around the origin is two blocks tall and two or three blocks out in each horizontal
//! direction. Each cell is rolled against a threshold that falls off with its distance from the
//! centre, plus a small flat chance, and a chosen cell takes a block only where the one below can
//! carry it (a dirt path carries it half the time).

use anyhow::{Context, Result};
use oxide_core::{BlockPos, RandomSource};
use oxide_datapack::ground_feature::BlockPileConfig;
use oxide_datapack::{BlockFact, BlockFacts};

use super::super::context::{block_in, FeatureContext};
use super::super::state_provider::StateProvider;
use super::{between_closed, is_air, is_block, provide};

#[derive(Debug)]
pub struct BlockPileFeature {
    provider: StateProvider,
}

impl BlockPileFeature {
    pub fn compile(config: &BlockPileConfig, facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            provider: StateProvider::compile(&config.state_provider, facts)
                .context("block pile provider")?,
        })
    }

    /// `BlockPileFeature.place`.
    ///
    /// PARITY-CHECK: "can carry it" is the exported `SturdyTop` fact, standing for
    /// `isFaceSturdy(level, pos, Direction.UP)`; confirm on blocks with partial shapes.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if origin.y < cx.region.min_y() + 5 {
            return false;
        }
        let rx = 2 + cx.random.next_int_bounded(2);
        let rz = 2 + cx.random.next_int_bounded(2);
        let from = BlockPos::new(origin.x - rx, origin.y, origin.z - rz);
        let to = BlockPos::new(origin.x + rx, origin.y + 1, origin.z + rz);
        for pos in between_closed(from, to) {
            let dx = origin.x - pos.x;
            let dz = origin.z - pos.z;
            let distance = (dx * dx + dz * dz) as f32;
            let threshold = cx.random.next_float() * 10.0 - cx.random.next_float() * 6.0;
            if distance <= threshold || f64::from(cx.random.next_float()) < 0.031 {
                self.try_place(cx, pos);
            }
        }
        true
    }

    /// `BlockPileFeature.tryPlaceBlock`.
    fn try_place(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos) {
        if !is_air(cx, pos) {
            return;
        }
        let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
        let support = block_in(cx.region, below);
        let allowed = if is_block(support, "dirt_path") {
            cx.random.next_boolean()
        } else {
            cx.env.facts.is(support, BlockFact::SturdyTop)
        };
        if allowed {
            if let Some(state) = provide(cx, &self.provider, pos) {
                cx.region.set_block(pos, state);
            }
        }
    }
}
