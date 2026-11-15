//! What a survival rule can ask about the world around the block it judges.
//!
//! Vanilla's `canSurvive` reads neighbouring states and their shapes. The exported facts carry
//! the shape questions the rules need only for the top and bottom face (`SturdyTop`,
//! `SturdyBottom`) and for whole-block collision (`FullCollision`, `NoCollision`), so every
//! shape answer here is three-valued: `Some(true)`, `Some(false)`, or `None` when the facts do
//! not settle it (a slab's side, a fence post). A rule that needs such an answer passes the
//! `None` up and the decorator tallies it, instead of guessing a shape.

use oxide_core::{BlockPos, BlockState};
use oxide_datapack::BlockFact;

use super::{SurvivalRules, Tag};
use crate::decorator::context::block_in;
use crate::region::DecorationRegion;

/// A block face or step direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Dir {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Dir {
    pub const HORIZONTAL: [Dir; 4] = [Dir::North, Dir::East, Dir::South, Dir::West];
    pub const ALL: [Dir; 6] = [
        Dir::Down,
        Dir::Up,
        Dir::North,
        Dir::East,
        Dir::South,
        Dir::West,
    ];

    pub fn opposite(self) -> Dir {
        match self {
            Dir::Down => Dir::Up,
            Dir::Up => Dir::Down,
            Dir::North => Dir::South,
            Dir::South => Dir::North,
            Dir::West => Dir::East,
            Dir::East => Dir::West,
        }
    }

    pub fn offset(self, pos: BlockPos) -> BlockPos {
        let (dx, dy, dz) = match self {
            Dir::Down => (0, -1, 0),
            Dir::Up => (0, 1, 0),
            Dir::North => (0, 0, -1),
            Dir::South => (0, 0, 1),
            Dir::West => (-1, 0, 0),
            Dir::East => (1, 0, 0),
        };
        BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz)
    }

    /// The direction a state property such as `facing=north` names.
    pub fn from_name(name: &str) -> Option<Dir> {
        Some(match name {
            "down" => Dir::Down,
            "up" => Dir::Up,
            "north" => Dir::North,
            "south" => Dir::South,
            "west" => Dir::West,
            "east" => Dir::East,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Dir::Down => "down",
            Dir::Up => "up",
            Dir::North => "north",
            Dir::South => "south",
            Dir::West => "west",
            Dir::East => "east",
        }
    }
}

/// `a || b` over answers that may be unknown: a known `true` wins, two known `false`s lose.
pub(super) fn or(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

/// `a && b` over answers that may be unknown: a known `false` wins, two known `true`s win.
pub(super) fn and(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => None,
    }
}

/// The block being judged, where, and the world it is judged in.
pub(super) struct Probe<'a, 'r> {
    pub rules: &'a SurvivalRules,
    pub region: &'a DecorationRegion<'r>,
    pub state: &'a BlockState,
    pub pos: BlockPos,
}

impl<'a> Probe<'a, '_> {
    /// The block at `pos` (`void_air` outside the window, as the game's level answers).
    pub fn block(&self, pos: BlockPos) -> &'a BlockState {
        block_in(self.region, pos)
    }

    pub fn at(&self, dir: Dir) -> &'a BlockState {
        self.block(dir.offset(self.pos))
    }

    /// The block already standing where `state` would go.
    pub fn here(&self) -> &'a BlockState {
        self.block(self.pos)
    }

    pub fn has(&self, state: &BlockState, fact: BlockFact) -> bool {
        self.region.facts().is(state, fact)
    }

    pub fn tag(&self, tag: Tag, state: &BlockState) -> bool {
        self.rules.in_tag(tag, state)
    }

    pub fn is_air(&self, state: &BlockState) -> bool {
        self.has(state, BlockFact::Air)
    }

    pub fn same_block(&self, state: &BlockState) -> bool {
        state.name == self.state.name
    }

    pub fn prop<'s>(state: &'s BlockState, key: &str) -> Option<&'s str> {
        state.properties.get(key).map(String::as_str)
    }

    pub fn prop_is(state: &BlockState, key: &str, value: &str) -> bool {
        Self::prop(state, key) == Some(value)
    }

    /// Whether `state` has a fluid in it: water or lava, source or flowing.
    pub fn has_fluid(&self, state: &BlockState) -> bool {
        self.has(state, BlockFact::Water) || self.has(state, BlockFact::Lava)
    }

    /// `FluidState.getAmount() == 8` for water: a source, or the falling column (`level=8`).
    pub fn is_full_water(&self, state: &BlockState) -> bool {
        self.has(state, BlockFact::Water)
            && (self.has(state, BlockFact::FluidSource) || Self::prop_is(state, "level", "8"))
    }

    /// `state.isFaceSturdy(level, pos, face)`: its support shape fills `face`. Exact for the
    /// top and bottom faces; a side face is known only when the block is sturdy both ways (a
    /// full cube) or has no shape at all.
    pub fn sturdy(&self, state: &BlockState, face: Dir) -> Option<bool> {
        let top = self.has(state, BlockFact::SturdyTop);
        let bottom = self.has(state, BlockFact::SturdyBottom);
        match face {
            Dir::Up => Some(top),
            Dir::Down => Some(bottom),
            _ if top && bottom => Some(true),
            _ if self.is_shapeless(state) => Some(false),
            _ => None,
        }
    }

    /// `Block.canSupportCenter`: a full face supports the centre, a shapeless block does not,
    /// and `unstable_bottom_center` blocks (fence gates) never support a hanging block.
    pub fn center(&self, state: &BlockState, face: Dir) -> Option<bool> {
        if face == Dir::Down && self.tag(Tag::UnstableBottomCenter, state) {
            return Some(false);
        }
        if self.sturdy(state, face) == Some(true) {
            Some(true)
        } else if self.is_shapeless(state) {
            Some(false)
        } else {
            None
        }
    }

    /// Whether the collision shape of `state` fills a face: a full cube does, a shapeless
    /// block does not, anything in between is unknown.
    pub fn collision_fills(&self, state: &BlockState) -> Option<bool> {
        if self.has(state, BlockFact::FullCollision) {
            Some(true)
        } else if self.is_shapeless(state) {
            Some(false)
        } else {
            None
        }
    }

    /// `MultifaceBlock.canAttachTo` / `VineBlock.canSupportAtFace`: `state` offers a full
    /// `face`, through its support shape or its collision shape.
    pub fn attachable(&self, state: &BlockState, face: Dir) -> Option<bool> {
        or(self.sturdy(state, face), self.collision_fills(state))
    }

    fn is_shapeless(&self, state: &BlockState) -> bool {
        self.has(state, BlockFact::Air) || self.has(state, BlockFact::NoCollision)
    }
}
