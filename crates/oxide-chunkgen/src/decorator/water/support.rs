//! Small helpers the water features share: directions, vanilla's shuffle, block and tag reads.

use anyhow::{anyhow, Result};
use oxide_core::{BlockPos, BlockState, RandomSource, ResourceLocation};
use oxide_datapack::{BlockFact, BlockFacts};

use crate::decorator::context::{block_in, FeatureContext};

/// The `Direction` values the water features use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Dir {
    Up,
    Down,
    North,
    South,
    West,
    East,
}

/// `Direction.Plane.HORIZONTAL`'s faces, in its declared order.
pub(super) const HORIZONTAL: [Dir; 4] = [Dir::North, Dir::East, Dir::South, Dir::West];

impl Dir {
    fn step(self) -> (i32, i32, i32) {
        match self {
            Dir::Up => (0, 1, 0),
            Dir::Down => (0, -1, 0),
            Dir::North => (0, 0, -1),
            Dir::South => (0, 0, 1),
            Dir::West => (-1, 0, 0),
            Dir::East => (1, 0, 0),
        }
    }

    pub(super) fn opposite(self) -> Dir {
        match self {
            Dir::Up => Dir::Down,
            Dir::Down => Dir::Up,
            Dir::North => Dir::South,
            Dir::South => Dir::North,
            Dir::West => Dir::East,
            Dir::East => Dir::West,
        }
    }

    /// `getClockWise` about the Y axis; vertical directions have none (vanilla throws), so
    /// they are returned unchanged and never passed.
    pub(super) fn clockwise(self) -> Dir {
        match self {
            Dir::North => Dir::East,
            Dir::South => Dir::West,
            Dir::West => Dir::North,
            Dir::East => Dir::South,
            vertical => vertical,
        }
    }

    /// `getCounterClockWise` about the Y axis; see [`Dir::clockwise`].
    pub(super) fn counter_clockwise(self) -> Dir {
        match self {
            Dir::North => Dir::West,
            Dir::South => Dir::East,
            Dir::West => Dir::South,
            Dir::East => Dir::North,
            vertical => vertical,
        }
    }

    /// The serialized name, as a `facing` property value.
    pub(super) fn name(self) -> &'static str {
        match self {
            Dir::Up => "up",
            Dir::Down => "down",
            Dir::North => "north",
            Dir::South => "south",
            Dir::West => "west",
            Dir::East => "east",
        }
    }
}

pub(super) fn relative(pos: BlockPos, dir: Dir, distance: i32) -> BlockPos {
    let (dx, dy, dz) = dir.step();
    BlockPos::new(
        pos.x + dx * distance,
        pos.y + dy * distance,
        pos.z + dz * distance,
    )
}

pub(super) fn offset(pos: BlockPos, dx: i32, dy: i32, dz: i32) -> BlockPos {
    BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz)
}

/// `Util.shuffle`: Fisher-Yates from the back, one `nextInt(i)` per slot.
pub(super) fn shuffle<T>(values: &mut [T], random: &mut impl RandomSource) {
    for i in (2..=values.len()).rev() {
        let swap_to = random.next_int_bounded(i as i32) as usize;
        values.swap(i - 1, swap_to);
    }
}

/// The default state of the vanilla block `path`, from the exported facts.
pub(super) fn default_state(facts: &BlockFacts, path: &str) -> Result<BlockState> {
    facts
        .default_state(&ResourceLocation::minecraft(path))
        .cloned()
        .ok_or_else(|| anyhow!("minecraft:{path} is not in the exported block facts"))
}

/// Whether the block at `pos` is the vanilla block `path`, whatever its properties.
pub(super) fn is_block_at(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos, path: &str) -> bool {
    let name = &block_in(cx.region, pos).name;
    name.namespace() == "minecraft" && name.path() == path
}

/// `is(Blocks.WATER)`: any water block state, source or flowing.
pub(super) fn is_water(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
    is_block_at(cx, pos, "water")
}

/// `isEmptyBlock`: whether the block at `pos` is air; outside the window it is `void_air`.
pub(super) fn is_air(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
    cx.env.facts.is(block_in(cx.region, pos), BlockFact::Air)
}

/// Whether the block at `pos` is in the block tag `minecraft:<tag>`.
pub(super) fn in_tag_at(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos, tag: &str) -> bool {
    cx.env.block_tags.contains(
        &ResourceLocation::minecraft(tag),
        &block_in(cx.region, pos).name,
    )
}

/// `BuiltInRegistries.BLOCK.getRandomElementOf(#minecraft:<tag>, random)`: one `nextInt(size)`
/// over the tag's ordered members, and no draw at all for an absent or empty tag.
// PARITY-CHECK: vanilla drops an optional (`required: false`) tag entry naming a block the
// registry lacks, shrinking the list; the tag loader keeps the id, so a datapack with such an
// entry draws over a longer list. Vanilla 26.2's coral tags have none; verify per datapack.
pub(super) fn random_member(
    cx: &mut FeatureContext<'_, '_, '_>,
    tag: &str,
) -> Option<ResourceLocation> {
    let members = cx
        .env
        .block_tags
        .ordered(&ResourceLocation::minecraft(tag))?;
    if members.is_empty() {
        return None;
    }
    let index = cx.random.next_int_bounded(members.len() as i32) as usize;
    Some(members[index].clone())
}

/// A tag member's default state; a block the facts do not list is tallied and yields `None`.
pub(super) fn member_default(
    cx: &mut FeatureContext<'_, '_, '_>,
    block: &ResourceLocation,
) -> Option<BlockState> {
    match cx.env.facts.default_state(block) {
        Some(state) => Some(state.clone()),
        None => {
            cx.unsupported.record("block_state", &block.to_string());
            None
        }
    }
}

/// `BlockState.canSurvive`, with an unanswered question tallied and read as "cannot place".
pub(super) fn survives(
    cx: &mut FeatureContext<'_, '_, '_>,
    state: &BlockState,
    pos: BlockPos,
) -> bool {
    match cx.env.behaviours.can_survive(state, cx.region, pos) {
        Some(answer) => answer,
        None => {
            cx.unsupported.record("survival", &state.name.to_string());
            false
        }
    }
}
