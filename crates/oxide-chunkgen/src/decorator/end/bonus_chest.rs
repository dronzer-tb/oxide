//! `bonus_chest`: the spawn chest, found by scanning the origin's chunk in a shuffled order.
//!
//! The columns are visited in a random order (x and z each shuffled), and the first whose top
//! block is air or has no collision gets a chest, then a torch on each side where one would
//! survive. The chest's loot table (`minecraft:chests/spawn_bonus_chest`) lives in a block
//! entity the region cannot carry, so it is tallied as `block_entity:minecraft:chest`; the
//! `nextLong` vanilla draws for the loot seed is still drawn so later features see the same
//! stream.
//!
//! PARITY-CHECK: the loot-seed draw happens once per placed chest, after the chest and before the
//! torches, as in the reference; verify the stream position against a real 26.2 spawn.

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, HeightmapType, RandomSource};
use oxide_datapack::{BlockFact, BlockFacts};

use super::super::context::{block_in, FeatureContext};
use super::{default_state, is_air, offset};

/// `Direction.Plane.HORIZONTAL`: north, east, south, west.
const HORIZONTAL: [[i32; 2]; 4] = [[0, -1], [1, 0], [0, 1], [-1, 0]];

#[derive(Debug)]
pub struct BonusChestFeature {
    chest: BlockState,
    torch: BlockState,
}

impl BonusChestFeature {
    pub fn compile(facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            chest: default_state(facts, "chest")?,
            torch: default_state(facts, "torch")?,
        })
    }

    /// `BonusChestFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let mut xs: [i32; 16] = std::array::from_fn(|i| (cx.origin.x & !15) + i as i32);
        let mut zs: [i32; 16] = std::array::from_fn(|i| (cx.origin.z & !15) + i as i32);
        // `Util.toShuffledList`.
        for values in [&mut xs, &mut zs] {
            for end in (2..=16usize).rev() {
                let other = cx.random.next_int_bounded(end as i32) as usize;
                values.swap(end - 1, other);
            }
        }
        for x in xs {
            for z in zs {
                let Some(y) = cx
                    .region
                    .height(HeightmapType::MotionBlockingNoLeaves, x, z)
                else {
                    return false;
                };
                let pos = BlockPos::new(x, y, z);
                let open = is_air(cx, pos)
                    || cx
                        .env
                        .facts
                        .is(block_in(cx.region, pos), BlockFact::NoCollision);
                if !open {
                    continue;
                }
                cx.region.set_block(pos, self.chest.clone());
                cx.unsupported.record("block_entity", "minecraft:chest");
                cx.random.next_long();
                for [dx, dz] in HORIZONTAL {
                    let side = offset(pos, dx, 0, dz);
                    match cx.env.behaviours.can_survive(&self.torch, cx.region, side) {
                        Some(true) => {
                            cx.region.set_block(side, self.torch.clone());
                        }
                        Some(false) => {}
                        None => cx
                            .unsupported
                            .record("survival", &self.torch.name.to_string()),
                    }
                }
                return true;
            }
        }
        false
    }
}
