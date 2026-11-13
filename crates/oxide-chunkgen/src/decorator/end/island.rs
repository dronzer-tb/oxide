//! `end_island`: a small end-stone island, a stack of shrinking discs hanging downward.
//!
//! The top disc has a radius of four to six; each lower disc is smaller by between one half and
//! one and a half, until the radius drops to one half. A disc covers the columns whose squared
//! distance from the origin is at most `(radius + 1)^2`. All maths is `f32`, as in vanilla.

use anyhow::Result;
use oxide_core::{BlockState, RandomSource};
use oxide_datapack::BlockFacts;

use super::super::context::FeatureContext;
use super::{default_state, offset};

#[derive(Debug)]
pub struct EndIslandFeature {
    end_stone: BlockState,
}

impl EndIslandFeature {
    pub fn compile(facts: &BlockFacts) -> Result<Self> {
        Ok(Self { end_stone: default_state(facts, "end_stone")? })
    }

    /// `EndIslandFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let mut radius = cx.random.next_int_bounded(3) as f32 + 4.0;
        let mut y = 0;
        while radius > 0.5 {
            let low = (-radius).floor() as i32;
            let high = radius.ceil() as i32;
            for x in low..=high {
                for z in low..=high {
                    if (x * x + z * z) as f32 <= (radius + 1.0) * (radius + 1.0) {
                        cx.region.set_block(offset(origin, x, y, z), self.end_stone.clone());
                    }
                }
            }
            radius -= cx.random.next_int_bounded(2) as f32 + 0.5;
            y -= 1;
        }
        true
    }
}
