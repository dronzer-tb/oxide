//! The questions every vegetation feature asks of the world, answered from the exported facts.
//!
//! Two of them cannot be answered exactly from per-state facts alone, and neither is guessed:
//!
//! - *Survival* (`canSurvive`) is per-block Java; it goes through [`BlockBehaviours`], and an
//!   unanswered question is tallied as `survival:<block>` and read as "cannot place".
//! - *Is this face full?* (`isFaceSturdy`, `canAttachTo`) needs the voxel shape. The facts hold
//!   the up and down faces exactly (`SturdyTop`, `SturdyBottom`) and the collision shape's two
//!   extremes (`NoCollision`, `FullCollision`). A side face of a block that is neither empty nor
//!   a full cube (a stair, a wall) is the one case left open: it is tallied as
//!   `shape:<block>` and read as "not full".
//!
//! [`BlockBehaviours`]: crate::decorator::BlockBehaviours

use oxide_core::{BlockPos, BlockState, ResourceLocation};
use oxide_datapack::BlockFact;

use super::geometry::Dir;
use crate::decorator::context::{block_in, fact_at, sides_never_full, FeatureContext};
use crate::decorator::feature::{place_placed, Placed};
use crate::decorator::state_provider::{StateProvider, Unsupported};

/// Which shape a face is asked about.
///
/// `isFaceSturdy` reads the block's *support* shape; `MultifaceBlock.canAttachTo` also accepts a
/// full face of the *collision* shape. The two differ only for blocks that override
/// `getBlockSupportShape` (leaves), where the support shape is empty but the collision is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shape {
    Support,
    Collision,
}

/// Whether the face `face` of the block at `pos` is a full square.
///
/// PARITY-CHECK: `FullCollision && SturdyTop` stands in for "full support shape" on side faces,
/// and `FullCollision` for "full collision shape". A block whose support shape is full on the
/// sides while its top is not sturdy would be misread; none is known.
pub(super) fn face_full(
    cx: &mut FeatureContext<'_, '_, '_>,
    pos: BlockPos,
    face: Dir,
    shape: Shape,
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
            fact(cx, sturdy) || (shape == Shape::Collision && full_collision)
        }
        _ => {
            if fact(cx, BlockFact::Air) || fact(cx, BlockFact::NoCollision) {
                return false;
            }
            if full_collision {
                return shape == Shape::Collision || fact(cx, BlockFact::SturdyTop);
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
    let neighbour = super::geometry::relative(pos, direction, 1);
    face_full(cx, neighbour, direction.opposite(), Shape::Collision)
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

/// `BlockState.isAir`, with `void_air` outside the window as vanilla's level answers.
pub(super) fn is_air(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
    cx.env.facts.is(block_in(cx.region, pos), BlockFact::Air)
}

/// Places a nested placed feature with the caller's random stream, as `PlacedFeature.place` does.
pub(super) fn place_nested(
    cx: &mut FeatureContext<'_, '_, '_>,
    placed: &Placed,
    pos: BlockPos,
) -> bool {
    place_placed(placed, cx, pos, None)
}

/// `BlockStateProvider.getState` for a provider the features here assume always answers. A
/// provider the decorator cannot evaluate is tallied and the feature stops, rather than guessing
/// a state.
pub(super) fn sample_state(
    cx: &mut FeatureContext<'_, '_, '_>,
    provider: &StateProvider,
    pos: BlockPos,
) -> Option<BlockState> {
    match provider.state(cx, pos) {
        Ok(state) => state,
        Err(Unsupported(why)) => {
            cx.unsupported.record("provider", why);
            None
        }
    }
}

/// Whether the block at `pos` is in the block tag `tag`.
pub(super) fn in_tag(
    cx: &FeatureContext<'_, '_, '_>,
    pos: BlockPos,
    tag: &ResourceLocation,
) -> bool {
    cx.env
        .block_tags
        .contains(tag, &block_in(cx.region, pos).name)
}
