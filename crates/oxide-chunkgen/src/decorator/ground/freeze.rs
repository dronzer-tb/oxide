//! `freeze_top_layer`: ice on cold water and snow on cold ground across one chunk.
//!
//! The feature draws no random numbers. For each of the chunk's 256 columns it takes the first
//! free block above the motion-blocking surface and asks the biome there whether the block under
//! it should ice over and whether the block itself should take a snow layer; the climate rules
//! are in `climate`.

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, HeightmapType};
use oxide_datapack::BlockFacts;

use super::super::context::{block_in, FeatureContext};
use super::climate::{should_freeze, should_snow};
use super::default_state;

#[derive(Debug)]
pub struct FreezeTopLayerFeature {
    ice: BlockState,
    snow: BlockState,
}

impl FreezeTopLayerFeature {
    pub fn compile(facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            ice: default_state(facts, "ice")?,
            snow: default_state(facts, "snow")?,
        })
    }

    /// `FreezeTopLayerFeature.place`. A snowy-capable block under new snow (grass, podzol,
    /// mycelium: the blocks with a `snowy` property) is switched to its snowy state.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        for dx in 0..16 {
            for dz in 0..16 {
                let (x, z) = (origin.x + dx, origin.z + dz);
                let Some(y) = cx.region.height(HeightmapType::MotionBlocking, x, z) else {
                    continue;
                };
                let top = BlockPos::new(x, y, z);
                let below = BlockPos::new(x, y - 1, z);
                if should_freeze(cx, top, below, false) {
                    cx.region.set_block_ref(below, &self.ice);
                }
                if should_snow(cx, top, &self.snow) {
                    cx.region.set_block_ref(top, &self.snow);
                    let under = block_in(cx.region, below);
                    if under.properties.contains_key("snowy") {
                        let mut under = under.clone();
                        under
                            .properties
                            .insert("snowy".to_string(), "true".to_string());
                        cx.region.set_block(below, under);
                    }
                }
            }
        }
        true
    }
}
