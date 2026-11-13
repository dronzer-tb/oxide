//! `end_platform` and `void_start_platform`: the two fixed platforms placed in an otherwise empty
//! dimension. Neither draws from the random stream.
//!
//! The End platform is the 5x5 obsidian square players arrive on, cleared three blocks upward.
//! The void-start platform is a stone square in the chunks next to the world origin, with one
//! cobblestone block at (8, 8) of the origin chunk.

use anyhow::Result;
use oxide_core::BlockState;
use oxide_datapack::BlockFacts;

use super::super::context::{block_in, FeatureContext};
use super::{default_state, offset};

#[derive(Debug)]
pub struct EndPlatformFeature {
    obsidian: BlockState,
    air: BlockState,
}

impl EndPlatformFeature {
    pub fn compile(facts: &BlockFacts) -> Result<Self> {
        Ok(Self { obsidian: default_state(facts, "obsidian")?, air: default_state(facts, "air")? })
    }

    /// `EndPlatformFeature.createEndPlatform` with `dropBlocks = false`: a block already in its
    /// target state is left alone.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        for dz in -2..=2 {
            for dx in -2..=2 {
                for dy in -1..3 {
                    let pos = offset(origin, dx, dy, dz);
                    let target = if dy == -1 { &self.obsidian } else { &self.air };
                    if block_in(cx.region, pos) != target {
                        cx.region.set_block(pos, target.clone());
                    }
                }
            }
        }
        true
    }
}

#[derive(Debug)]
pub struct VoidStartPlatformFeature {
    stone: BlockState,
    cobblestone: BlockState,
}

impl VoidStartPlatformFeature {
    pub fn compile(facts: &BlockFacts) -> Result<Self> {
        Ok(Self { stone: default_state(facts, "stone")?, cobblestone: default_state(facts, "cobblestone")? })
    }

    /// `VoidStartPlatformFeature.place`: only the chunk ring within one chunk of the origin is
    /// touched, and only columns within 16 blocks (checkerboard) of (8, 8).
    ///
    /// PARITY-CHECK: the platform's Y is the origin's plus three, as the reference implements it;
    /// verify against a real 26.2 void-start world.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let (chunk_x, chunk_z) = (cx.origin.x >> 4, cx.origin.z >> 4);
        if chunk_x.abs().max(chunk_z.abs()) > 1 {
            return true;
        }
        let y = cx.origin.y + 3;
        for z in (chunk_z << 4)..=(chunk_z << 4) + 15 {
            for x in (chunk_x << 4)..=(chunk_x << 4) + 15 {
                if (8 - x).abs().max((8 - z).abs()) <= 16 {
                    let state = if x == 8 && z == 8 { &self.cobblestone } else { &self.stone };
                    cx.region.set_block(oxide_core::BlockPos::new(x, y, z), state.clone());
                }
            }
        }
        true
    }
}
