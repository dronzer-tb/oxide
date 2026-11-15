//! `blue_ice`: blue ice growing out of packed ice under the sea, in two hundred attempts.
//!
//! It starts only below sea level, in or on water, beside packed ice. The seed block becomes blue
//! ice; then each attempt picks a nearby position (a skewed offset that stays within a few blocks
//! and a little more upward) and turns it to blue ice if it is air, water, ice or packed ice and
//! already touches blue ice.

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, RandomSource};
use oxide_datapack::BlockFacts;

use super::super::context::FeatureContext;
use super::{default_state, is_air, is_block_at, offset, DIRECTIONS};

#[derive(Debug)]
pub struct BlueIceFeature {
    blue_ice: BlockState,
}

impl BlueIceFeature {
    pub fn compile(facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            blue_ice: default_state(facts, "blue_ice")?,
        })
    }

    /// `BlueIceFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if origin.y > cx.env.sea_level - 1 {
            return false;
        }
        if !is_block_at(cx, origin, "water") && !is_block_at(cx, offset(origin, 0, -1, 0), "water")
        {
            return false;
        }
        // Every direction but down.
        let beside_packed_ice = DIRECTIONS
            .iter()
            .skip(1)
            .any(|d| is_block_at(cx, offset(origin, d[0], d[1], d[2]), "packed_ice"));
        if !beside_packed_ice {
            return false;
        }
        cx.region.set_block_ref(origin, &self.blue_ice);
        for _ in 0..200 {
            let dy = cx.random.next_int_bounded(5) - cx.random.next_int_bounded(6);
            let mut radius = 3;
            if dy < 2 {
                radius += dy / 2;
            }
            if radius < 1 {
                continue;
            }
            let dx = cx.random.next_int_bounded(radius) - cx.random.next_int_bounded(radius);
            let dz = cx.random.next_int_bounded(radius) - cx.random.next_int_bounded(radius);
            let pos = offset(origin, dx, dy, dz);
            let replaceable = is_air(cx, pos)
                || ["water", "packed_ice", "ice"]
                    .iter()
                    .any(|path| is_block_at(cx, pos, path));
            if !replaceable {
                continue;
            }
            let touches_blue_ice = DIRECTIONS.iter().any(|d| {
                is_block_at(
                    cx,
                    BlockPos::new(pos.x + d[0], pos.y + d[1], pos.z + d[2]),
                    "blue_ice",
                )
            });
            if touches_blue_ice {
                cx.region.set_block_ref(pos, &self.blue_ice);
            }
        }
        true
    }
}
