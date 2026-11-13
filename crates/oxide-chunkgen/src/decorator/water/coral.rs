//! `minecraft:coral_tree`, `coral_claw` and `coral_mushroom`: warm-ocean coral reefs.
//!
//! Port of `CoralFeature` and its three shapes. The base draws one block from
//! `#minecraft:coral_blocks` per feature and hands it to the shape, which lays it out through
//! `placeCoralBlock`. That call only writes into water (or an existing coral) with water above,
//! and then decorates the new block: a `nextFloat() < 0.25` coral from `#minecraft:corals` on top,
//! else a `nextFloat() < 0.05` sea pickle (`nextInt(4) + 1` pickles), then for each horizontal
//! side in `Plane.HORIZONTAL` order a `nextFloat() < 0.2` wall fan from `#minecraft:wall_corals`
//! facing away from the block, if that side is water.
//!
//! Vanilla never asks these blocks whether they survive, so no survival rule is involved. Tag
//! draws index the tag's ordered members (see [`random_member`]).

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, RandomSource};
use oxide_datapack::BlockFacts;

use super::support::{
    default_state, in_tag_at, is_water, member_default, offset, random_member, relative, shuffle, Dir,
    HORIZONTAL,
};
use crate::decorator::context::FeatureContext;

/// Which `CoralFeature` subclass builds the shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CoralShape {
    Tree,
    Claw,
    Mushroom,
}

#[derive(Debug)]
pub struct CoralFeature {
    shape: CoralShape,
    pickle: BlockState,
}

impl CoralFeature {
    pub(super) fn compile(shape: CoralShape, facts: &BlockFacts) -> Result<Self> {
        Ok(Self { shape, pickle: default_state(facts, "sea_pickle")? })
    }

    pub(super) fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let Some(block) = random_member(cx, "coral_blocks") else {
            return false;
        };
        let Some(state) = member_default(cx, &block) else {
            return false;
        };
        let origin = cx.origin;
        match self.shape {
            CoralShape::Tree => self.tree(cx, origin, &state),
            CoralShape::Claw => self.claw(cx, origin, &state),
            CoralShape::Mushroom => self.mushroom(cx, origin, &state),
        }
    }

    /// `placeCoralBlock`.
    fn place_block(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos, state: &BlockState) -> bool {
        let above = relative(pos, Dir::Up, 1);
        if !((is_water(cx, pos) || in_tag_at(cx, pos, "corals")) && is_water(cx, above)) {
            return false;
        }
        cx.region.set_block(pos, state.clone());
        if cx.random.next_float() < 0.25 {
            if let Some(coral) = random_member(cx, "corals") {
                if let Some(top) = member_default(cx, &coral) {
                    cx.region.set_block(above, top);
                }
            }
        } else if cx.random.next_float() < 0.05 {
            let pickles = cx.random.next_int_bounded(4) + 1;
            cx.region.set_block(above, self.pickle.clone().with_property("pickles", pickles.to_string()));
        }
        for direction in HORIZONTAL {
            if cx.random.next_float() < 0.2 {
                let side = relative(pos, direction, 1);
                if is_water(cx, side) {
                    if let Some(fan) = random_member(cx, "wall_corals") {
                        if let Some(mut fan_state) = member_default(cx, &fan) {
                            // `hasProperty(FACING)`: the exported default state lists every
                            // property its block has.
                            if fan_state.properties.contains_key("facing") {
                                fan_state = fan_state.with_property("facing", direction.name());
                            }
                            cx.region.set_block(side, fan_state);
                        }
                    }
                }
            }
        }
        true
    }

    /// `CoralTreeFeature.placeFeature`.
    fn tree(&self, cx: &mut FeatureContext<'_, '_, '_>, origin: BlockPos, state: &BlockState) -> bool {
        let mut pos = origin;
        let trunk_height = cx.random.next_int_bounded(3) + 1;
        for _ in 0..trunk_height {
            if !self.place_block(cx, pos, state) {
                return true;
            }
            pos = relative(pos, Dir::Up, 1);
        }
        let trunk_top = pos;
        let branches = (cx.random.next_int_bounded(3) + 2) as usize;
        let mut directions = HORIZONTAL;
        shuffle(&mut directions, &mut *cx.random);
        for &branch in &directions[..branches] {
            pos = relative(trunk_top, branch, 1);
            let branch_height = cx.random.next_int_bounded(5) + 2;
            let mut segment = 0;
            let mut j = 0;
            while j < branch_height && self.place_block(cx, pos, state) {
                segment += 1;
                pos = relative(pos, Dir::Up, 1);
                if j == 0 || segment >= 2 && cx.random.next_float() < 0.25 {
                    pos = relative(pos, branch, 1);
                    segment = 0;
                }
                j += 1;
            }
        }
        true
    }

    /// `CoralClawFeature.placeFeature`.
    fn claw(&self, cx: &mut FeatureContext<'_, '_, '_>, origin: BlockPos, state: &BlockState) -> bool {
        if !self.place_block(cx, origin, state) {
            return false;
        }
        let claw = HORIZONTAL[cx.random.next_int_bounded(HORIZONTAL.len() as i32) as usize];
        let branches = (cx.random.next_int_bounded(2) + 2) as usize;
        let mut directions = [claw, claw.clockwise(), claw.counter_clockwise()];
        shuffle(&mut directions, &mut *cx.random);
        for &branch in &directions[..branches] {
            let mut pos = origin;
            let sideways = cx.random.next_int_bounded(2) + 1;
            pos = relative(pos, branch, 1);
            let (segment, inward) = if branch == claw {
                (claw, cx.random.next_int_bounded(3) + 2)
            } else {
                pos = relative(pos, Dir::Up, 1);
                let choices = [branch, Dir::Up];
                let segment = choices[cx.random.next_int_bounded(choices.len() as i32) as usize];
                (segment, cx.random.next_int_bounded(3) + 3)
            };
            let mut i = 0;
            while i < sideways && self.place_block(cx, pos, state) {
                pos = relative(pos, segment, 1);
                i += 1;
            }
            pos = relative(pos, segment.opposite(), 1);
            pos = relative(pos, Dir::Up, 1);
            for _ in 0..inward {
                pos = relative(pos, claw, 1);
                if !self.place_block(cx, pos, state) {
                    break;
                }
                if cx.random.next_float() < 0.25 {
                    pos = relative(pos, Dir::Up, 1);
                }
            }
        }
        true
    }

    /// `CoralMushroomFeature.placeFeature`: the shell of a box sunk into the floor, missing its
    /// twelve edges, each shell cell skipped with chance 0.1.
    fn mushroom(&self, cx: &mut FeatureContext<'_, '_, '_>, origin: BlockPos, state: &BlockState) -> bool {
        let height = cx.random.next_int_bounded(3) + 3;
        let width = cx.random.next_int_bounded(3) + 3;
        let length = cx.random.next_int_bounded(3) + 3;
        let sink = cx.random.next_int_bounded(3) + 1;
        for x in 0..=width {
            for y in 0..=height {
                for z in 0..=length {
                    let pos = offset(origin, x, y - sink, z);
                    let x_inner = x != 0 && x != width;
                    let y_inner = y != 0 && y != height;
                    let z_inner = z != 0 && z != length;
                    let on_shell = !x_inner || !y_inner || !z_inner;
                    if (x_inner || y_inner) && (z_inner || y_inner) && (x_inner || z_inner) && on_shell
                        && cx.random.next_float() >= 0.1
                    {
                        self.place_block(cx, pos, state);
                    }
                }
            }
        }
        true
    }
}
