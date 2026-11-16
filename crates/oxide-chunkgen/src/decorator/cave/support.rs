//! What the cave features ask of the world, answered from the exported facts and the tags.
//!
//! Directions, offsets and the integer-provider bounds the codecs check are small enough to live
//! here rather than reach into another family's private helpers.
//!
//! Face fullness (`isFaceSturdy`, `MultifaceBlock.canAttachTo`) needs a voxel shape. The facts
//! hold the up and down faces exactly (`SturdyTop`, `SturdyBottom`) and the collision shape's two
//! extremes (`NoCollision`, `FullCollision`). A side face of a block that is neither empty nor a
//! full cube is the one open case: it is tallied as `shape:<block>` and read as "not full".

use crate::decorator::context::fact_in;
use oxide_core::{BlockPos, BlockState, RandomSource};
use oxide_datapack::placement::{IntProvider, TaggedIntProvider};
use oxide_datapack::BlockFact;

use crate::decorator::context::{block_in, fact_at, sides_never_full, FeatureContext};

/// `net.minecraft.core.Direction`, in the order of `Direction.values()`.
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
    pub const ALL: [Dir; 6] = [
        Dir::Down,
        Dir::Up,
        Dir::North,
        Dir::South,
        Dir::West,
        Dir::East,
    ];

    pub fn step(self) -> (i32, i32, i32) {
        match self {
            Dir::Down => (0, -1, 0),
            Dir::Up => (0, 1, 0),
            Dir::North => (0, 0, -1),
            Dir::South => (0, 0, 1),
            Dir::West => (-1, 0, 0),
            Dir::East => (1, 0, 0),
        }
    }

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

    /// The block-state value that names this direction (`up`, `north`, ...).
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

    /// The axis as an index: 0 = x, 1 = y, 2 = z.
    pub fn axis(self) -> u8 {
        match self {
            Dir::West | Dir::East => 0,
            Dir::Down | Dir::Up => 1,
            Dir::North | Dir::South => 2,
        }
    }
}

/// `pos.offset(dx, dy, dz)`.
pub(super) fn offset(pos: BlockPos, dx: i32, dy: i32, dz: i32) -> BlockPos {
    BlockPos::new(
        pos.x.wrapping_add(dx),
        pos.y.wrapping_add(dy),
        pos.z.wrapping_add(dz),
    )
}

/// `pos.relative(dir, distance)`.
pub(super) fn relative(pos: BlockPos, dir: Dir, distance: i32) -> BlockPos {
    let (dx, dy, dz) = dir.step();
    offset(
        pos,
        dx.wrapping_mul(distance),
        dy.wrapping_mul(distance),
        dz.wrapping_mul(distance),
    )
}

/// `Vec3i.distManhattan`, including the float accumulation vanilla does it in.
pub(super) fn dist_manhattan(a: BlockPos, b: BlockPos) -> i32 {
    let d = |from: i32, to: i32| to.wrapping_sub(from).wrapping_abs() as f32;
    (d(a.x, b.x) + d(a.y, b.y) + d(a.z, b.z)) as i32
}

/// `Util.shuffle`: Fisher-Yates from the end, one `nextInt(i)` per step.
pub(super) fn shuffle<T>(values: &mut [T], random: &mut impl RandomSource) {
    for i in (2..=values.len()).rev() {
        let swap = random.next_int_bounded(i as i32) as usize;
        values.swap(i - 1, swap);
    }
}

/// Whether the block at `pos` has `fact`; outside the window it is `void_air`.
pub(super) fn has(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos, fact: BlockFact) -> bool {
    fact_in(cx.region, pos, fact)
}

/// Whether `state` has `fact`.
pub(super) fn state_has(
    cx: &FeatureContext<'_, '_, '_>,
    state: &BlockState,
    fact: BlockFact,
) -> bool {
    cx.env.facts.is(state, fact)
}

/// Whether `state` is the block `minecraft:<path>`, whatever its properties.
pub(super) fn is_block(state: &BlockState, path: &str) -> bool {
    state.name.namespace() == "minecraft" && state.name.path() == path
}

/// Whether the fluid in `state` is any fluid: water or lava, in a block or waterlogged.
pub(super) fn any_fluid(cx: &FeatureContext<'_, '_, '_>, state: &BlockState) -> bool {
    state_has(cx, state, BlockFact::Water) || state_has(cx, state, BlockFact::Lava)
}

/// Whether `state`'s fluid is a water source.
pub(super) fn water_source(cx: &FeatureContext<'_, '_, '_>, state: &BlockState) -> bool {
    state_has(cx, state, BlockFact::Water) && state_has(cx, state, BlockFact::FluidSource)
}

/// `state.with(waterlogged, value)`.
pub(super) fn waterlogged(state: BlockState, value: bool) -> BlockState {
    state.with_property("waterlogged", value.to_string())
}

/// Whether the face `face` of the block at `pos` is a full square. `collision` asks about the
/// collision shape (as `MultifaceBlock.canAttachTo` also does); otherwise only the support shape
/// counts (as `isFaceSturdy` does). The two differ only for blocks such as leaves.
///
/// PARITY-CHECK: `FullCollision && SturdyTop` stands in for "full support shape" on side faces and
/// `FullCollision` for "full collision shape". A block whose support shape is full on the sides
/// while its top is not sturdy would be misread; none is known.
pub(super) fn face_full(
    cx: &mut FeatureContext<'_, '_, '_>,
    pos: BlockPos,
    face: Dir,
    collision: bool,
) -> bool {
    let fact = |cx: &FeatureContext<'_, '_, '_>, fact: BlockFact| fact_at(cx, pos, fact);
    let full_collision = fact(cx, BlockFact::FullCollision);
    match face {
        Dir::Up | Dir::Down => {
            let sturdy = if face == Dir::Up {
                BlockFact::SturdyTop
            } else {
                BlockFact::SturdyBottom
            };
            fact(cx, sturdy) || (collision && full_collision)
        }
        _ => {
            if fact(cx, BlockFact::Air) || fact(cx, BlockFact::NoCollision) {
                return false;
            }
            if full_collision {
                return collision || fact(cx, BlockFact::SturdyTop);
            }
            if sides_never_full(block_in(cx.region, pos)) {
                return false;
            }
            let name = block_in(cx.region, pos).name.to_string();
            cx.unsupported.record("shape", &name);
            false
        }
    }
}

/// `MultifaceBlock.canAttachTo` for a block at `pos` looking at `direction`: whether the
/// neighbour that way presents a full face back.
pub(super) fn can_attach_to(
    cx: &mut FeatureContext<'_, '_, '_>,
    pos: BlockPos,
    direction: Dir,
) -> bool {
    face_full(cx, relative(pos, direction, 1), direction.opposite(), true)
}

/// The smallest and largest value `provider` can produce, as the codecs' range checks and
/// `IntProvider.getMaxValue` see them. `None` for a provider with no entries.
pub(super) fn int_bounds(provider: &IntProvider) -> Option<(i32, i32)> {
    let tagged = match provider {
        IntProvider::Constant(value) => return Some((*value, *value)),
        IntProvider::Tagged(tagged) => &**tagged,
    };
    match tagged {
        TaggedIntProvider::Constant { value } => Some((*value, *value)),
        TaggedIntProvider::Uniform {
            min_inclusive,
            max_inclusive,
        }
        | TaggedIntProvider::BiasedToBottom {
            min_inclusive,
            max_inclusive,
        }
        | TaggedIntProvider::ClampedNormal {
            min_inclusive,
            max_inclusive,
            ..
        } => Some((*min_inclusive, *max_inclusive)),
        TaggedIntProvider::Clamped {
            source,
            min_inclusive,
            max_inclusive,
        } => {
            // `ClampedInt.getMinValue` is `max(min, source.min)`, `getMaxValue` `min(max, source.max)`.
            let (low, high) = int_bounds(source)?;
            Some(((*min_inclusive).max(low), (*max_inclusive).min(high)))
        }
        TaggedIntProvider::Trapezoid { min, max, .. } => Some((*min, *max)),
        TaggedIntProvider::WeightedList { distribution } => {
            let mut bounds: Option<(i32, i32)> = None;
            for entry in distribution {
                let (low, high) = int_bounds(&entry.data)?;
                bounds =
                    Some(bounds.map_or((low, high), |(min, max)| (min.min(low), max.max(high))));
            }
            bounds
        }
    }
}
