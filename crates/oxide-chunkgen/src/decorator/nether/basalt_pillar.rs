//! `basalt_pillar`: a basalt pillar hanging from a ceiling down to the floor.
//!
//! From an air origin under a solid block the pillar fills air downward. Each layer rolls a
//! hang-off on each side, and a side stops rolling at its first miss. At the bottom four base
//! hang-offs are tossed on coins, and a 7x7 skirt of basalt, sparser towards the corners, drops
//! up to three blocks to settle on the floor.

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, RandomSource};
use oxide_datapack::BlockFacts;

use super::super::context::FeatureContext;
use super::{default_state, is_empty, offset};

/// North, south, west, east: the order the hang-offs roll in.
const SIDES: [[i32; 2]; 4] = [[0, -1], [0, 1], [-1, 0], [1, 0]];

#[derive(Debug)]
pub struct BasaltPillarFeature {
    basalt: BlockState,
}

impl BasaltPillarFeature {
    pub fn compile(facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            basalt: default_state(facts, "basalt")?,
        })
    }

    /// `BasaltPillarFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if !is_empty(cx, origin) || is_empty(cx, offset(origin, 0, 1, 0)) {
            return false;
        }
        let mut pos = origin;
        let mut hang_off = [true; 4];
        while is_empty(cx, pos) {
            if cx.outside_build_height(pos.y) {
                return true;
            }
            cx.region.set_block(pos, self.basalt.clone());
            // `a && placeHangOff(..)`: a side that missed once draws no more.
            for (side, [dx, dz]) in hang_off.iter_mut().zip(SIDES) {
                *side = *side && self.place_hang_off(cx, offset(pos, dx, 0, dz));
            }
            pos = offset(pos, 0, -1, 0);
        }
        pos = offset(pos, 0, 1, 0);
        for [dx, dz] in SIDES {
            if cx.random.next_boolean() {
                cx.region
                    .set_block(offset(pos, dx, 0, dz), self.basalt.clone());
            }
        }
        pos = offset(pos, 0, -1, 0);
        for dx in -3i32..4 {
            for dz in -3i32..4 {
                let probability = dx.abs() * dz.abs();
                if cx.random.next_int_bounded(10) < 10 - probability {
                    let mut base = offset(pos, dx, 0, dz);
                    let mut max_drop = 3;
                    while is_empty(cx, offset(base, 0, -1, 0)) {
                        base = offset(base, 0, -1, 0);
                        max_drop -= 1;
                        if max_drop <= 0 {
                            break;
                        }
                    }
                    if !is_empty(cx, offset(base, 0, -1, 0)) {
                        cx.region.set_block(base, self.basalt.clone());
                    }
                }
            }
        }
        true
    }

    /// `placeHangOff`: basalt at `pos` unless the one-in-ten miss comes up.
    fn place_hang_off(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        if cx.random.next_int_bounded(10) != 0 {
            cx.region.set_block(pos, self.basalt.clone());
            true
        } else {
            false
        }
    }
}
