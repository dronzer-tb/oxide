//! `chorus_plant`: a branching chorus tree grown from one block, capped with flowers.
//!
//! It starts on a block of `#minecraft:supports_chorus_plant` with air above, writes the root,
//! then grows recursively: each stem is one to four blocks tall (one more at the root), and at
//! its top up to four branches try to start one block sideways, never reaching eight or more
//! blocks from the root horizontally. A stem that grew no branch ends in a chorus flower of age
//! 5. Every plant block takes its six connection properties from its neighbours at the moment it
//! is written, as `ChorusPlantBlock.getStateWithConnections` does.

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, RandomSource};
use oxide_datapack::BlockFacts;

use super::super::context::{block_in, FeatureContext};
use super::{default_state, is_air, is_block, offset, with_property};

/// `Direction.Plane.HORIZONTAL`: north, east, south, west.
const HORIZONTAL: [[i32; 3]; 4] = [[0, 0, -1], [1, 0, 0], [0, 0, 1], [-1, 0, 0]];
/// The connection properties in the order of the neighbour offsets below and of the mask bits.
const FACES: [&str; 6] = ["down", "up", "north", "east", "south", "west"];
const NEIGHBOURS: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [1, 0, 0], [0, 0, 1], [-1, 0, 0]];
/// `ChorusFlowerBlock.generatePlant`'s maximum depth.
const MAX_DEPTH: u8 = 4;
/// `ChorusFlowerBlock.generatePlant`'s maximum horizontal distance from the root.
const MAX_HORIZONTAL: i32 = 8;

#[derive(Debug)]
pub struct ChorusPlantFeature {
    /// The plant block for each combination of connections, indexed by the mask.
    plant: Vec<BlockState>,
    flower: BlockState,
}

impl ChorusPlantFeature {
    pub fn compile(facts: &BlockFacts) -> Result<Self> {
        let base = default_state(facts, "chorus_plant")?;
        let plant = (0..64usize)
            .map(|mask| {
                FACES.iter().enumerate().fold(base.clone(), |state, (bit, face)| {
                    with_property(state, face, if mask & (1 << bit) != 0 { "true" } else { "false" })
                })
            })
            .collect();
        let flower = with_property(default_state(facts, "chorus_flower")?, "age", "5");
        Ok(Self { plant, flower })
    }

    /// `ChorusPlantFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if !is_air(cx, origin) || !supports(cx, offset(origin, 0, -1, 0)) {
            return false;
        }
        self.connect(cx, origin);
        self.grow(cx, origin, origin, 0);
        true
    }

    /// `ChorusPlantBlock.getStateWithConnections` written at `pos`.
    fn connect(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos) {
        let mut mask = 0usize;
        for (bit, d) in NEIGHBOURS.iter().enumerate() {
            let state = block_in(cx.region, offset(pos, d[0], d[1], d[2]));
            let connects = is_block(state, "chorus_plant")
                || is_block(state, "chorus_flower")
                || (bit == 0 && cx.env.block_tags.contains(&supports_tag(), &state.name));
            if connects {
                mask |= 1 << bit;
            }
        }
        cx.region.set_block(pos, self.plant[mask].clone());
    }

    /// Whether the four horizontal neighbours of `pos` are air, ignoring side `except`.
    fn neighbours_empty(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos, except: Option<usize>) -> bool {
        HORIZONTAL
            .iter()
            .enumerate()
            .all(|(side, d)| except == Some(side) || is_air(cx, offset(pos, d[0], d[1], d[2])))
    }

    /// `ChorusFlowerBlock.growTreeRecursive`.
    fn grow(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos, root: BlockPos, depth: u8) {
        let height = cx.random.next_int_bounded(4) + 1 + i32::from(depth == 0);
        for step in 1..=height {
            let above = offset(pos, 0, step, 0);
            if !Self::neighbours_empty(cx, above, None) {
                return;
            }
            self.connect(cx, above);
            self.connect(cx, offset(above, 0, -1, 0));
        }
        let top = offset(pos, 0, height, 0);
        let mut branched = false;
        if depth < MAX_DEPTH {
            let attempts = cx.random.next_int_bounded(4) + i32::from(depth == 0);
            for _ in 0..attempts {
                let direction = cx.random.next_int_bounded(4) as usize;
                let d = HORIZONTAL[direction];
                let branch = offset(top, d[0], d[1], d[2]);
                if (branch.x - root.x).abs() >= MAX_HORIZONTAL || (branch.z - root.z).abs() >= MAX_HORIZONTAL {
                    continue;
                }
                let opposite = (direction + 2) % 4;
                if is_air(cx, branch)
                    && is_air(cx, offset(branch, 0, -1, 0))
                    && Self::neighbours_empty(cx, branch, Some(opposite))
                {
                    branched = true;
                    self.connect(cx, branch);
                    let o = HORIZONTAL[opposite];
                    self.connect(cx, offset(branch, o[0], o[1], o[2]));
                    self.grow(cx, branch, root, depth + 1);
                }
            }
        }
        if !branched {
            cx.region.set_block(top, self.flower.clone());
        }
    }
}

fn supports_tag() -> oxide_core::ResourceLocation {
    oxide_core::ResourceLocation::minecraft("supports_chorus_plant")
}

fn supports(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
    cx.env.block_tags.contains(&supports_tag(), &block_in(cx.region, pos).name)
}
