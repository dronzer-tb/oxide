//! `glowstone_blob`: a cluster of glowstone hanging from a ceiling.
//!
//! The origin must be air under netherrack, basalt or blackstone; it becomes glowstone, then 1500
//! random cells within 7 blocks across and 11 below grow the cluster, each only when it is air
//! touching exactly one glowstone face. Cells are tried in draw order against the blob as it
//! grows, so the shape depends on every earlier placement.

use anyhow::Result;
use oxide_core::{BlockState, RandomSource, ResourceLocation};
use oxide_datapack::BlockFacts;

use super::super::context::FeatureContext;
use super::{default_state, is_block_at, is_empty, offset, DIRECTIONS};

#[derive(Debug)]
pub struct GlowstoneFeature {
    glowstone: BlockState,
    /// The blocks a blob may hang from.
    ceilings: [ResourceLocation; 3],
}

impl GlowstoneFeature {
    pub fn compile(facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            glowstone: default_state(facts, "glowstone")?,
            ceilings: ["netherrack", "basalt", "blackstone"].map(ResourceLocation::minecraft),
        })
    }

    /// `GlowstoneFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if !is_empty(cx, origin) {
            return false;
        }
        let above = offset(origin, 0, 1, 0);
        if !self.ceilings.iter().any(|name| is_block_at(cx, above, name)) {
            return false;
        }
        cx.region.set_block(origin, self.glowstone.clone());
        for _ in 0..1500 {
            let dx = cx.random.next_int_bounded(8) - cx.random.next_int_bounded(8);
            let dy = -cx.random.next_int_bounded(12);
            let dz = cx.random.next_int_bounded(8) - cx.random.next_int_bounded(8);
            let pos = offset(origin, dx, dy, dz);
            if !is_empty(cx, pos) {
                continue;
            }
            let mut neighbours = 0;
            for [nx, ny, nz] in DIRECTIONS {
                if is_block_at(cx, offset(pos, nx, ny, nz), &self.glowstone.name) {
                    neighbours += 1;
                }
                if neighbours > 1 {
                    break;
                }
            }
            if neighbours == 1 {
                cx.region.set_block(pos, self.glowstone.clone());
            }
        }
        true
    }
}
