//! The neighbour-shape pass at the end of `StructureTemplate.placeInWorld`.
//!
//! After writing its blocks, vanilla treats them as one voxel shape and (1) for every face of
//! that shape (`updateShapeAtEdge`, faces in `DiscreteVoxelShape.forAllFaces` order) lets the
//! placed block react to the neighbour across the face and the neighbour react back, then (2)
//! re-derives every placed block from all six neighbours (`Block.updateFromNeighbourShapes`,
//! west, east, north, south, down, up). `updateShape` is per-block Java, so it is answered here
//! only where it is known:
//!
//! - blocks the exporter marks shape-inert (`k`: `updateShape` inherited, or overridden only to
//!   schedule ticks) keep their state;
//! - `potent_sulfur` re-derives its state from the water above and the block below
//!   (`PotentSulfurBlock.validBlockState`);
//! - an up-pointing speleothem (`sulfur_spike`, `pointed_dripstone`) re-derives its thickness
//!   on a vertical update (`SpeleothemBlock.updateShape`).
//!
//! Anything else is tallied as `shape_update:<block>` and left as it is.
//!
//! PARITY-CHECK: leaving an unknown block unchanged assumes its `updateShape` is a no-op here.
//! Every block of the 26.2 fossil and sulfur-spring templates has a rule above, so only world
//! blocks *next to* a placed template can hit this; the tally counts how often. Verify by
//! diffing those cells against the game.

use std::collections::HashSet;

use oxide_core::rotation::Direction;
use oxide_core::{BlockPos, BlockState, ResourceLocation};
use oxide_datapack::BlockFact;

use super::structure_template::step;
use super::structure_template::TemplateWorld;
use crate::decorator::context::block_in;

/// `BlockBehaviour.UPDATE_SHAPE_ORDER`.
const UPDATE_SHAPE_ORDER: [Direction; 6] = [
    Direction::West,
    Direction::East,
    Direction::North,
    Direction::South,
    Direction::Down,
    Direction::Up,
];

/// Runs both passes over the positions `placeInWorld` wrote, in the order it wrote them.
pub(super) fn update_shapes(world: &mut TemplateWorld<'_, '_>, placed: &[BlockPos]) {
    let cells: HashSet<BlockPos> = placed.iter().copied().collect();
    for (direction, pos) in faces(placed, &cells) {
        let neighbour_pos = step(pos, direction);
        let state = block_in(world.region, pos).clone();
        let neighbour = block_in(world.region, neighbour_pos).clone();
        let new_state = update_shape(world, &state, pos, direction);
        if new_state != state {
            world.region.set_block(pos, new_state.clone());
        }
        let new_neighbour = update_shape(world, &neighbour, neighbour_pos, direction.opposite());
        if new_neighbour != neighbour {
            world.region.set_block(neighbour_pos, new_neighbour);
        }
    }
    for &pos in placed {
        let state = block_in(world.region, pos).clone();
        let mut new_state = state.clone();
        for direction in UPDATE_SHAPE_ORDER {
            new_state = update_shape(world, &new_state, pos, direction);
        }
        if new_state != state {
            world.region.set_block(pos, new_state);
        }
    }
}

/// `DiscreteVoxelShape.forAllFaces` over the placed cells' bounding box: the Z faces (columns
/// along Z, X outer, Y inner), then the Y faces (Z outer, X inner), then the X faces (Y outer,
/// Z inner); along each column a face where it enters the shape (negative side) and where it
/// leaves (positive side).
fn faces(placed: &[BlockPos], cells: &HashSet<BlockPos>) -> Vec<(Direction, BlockPos)> {
    let Some(first) = placed.first() else {
        return Vec::new();
    };
    let (mut min, mut max) = (*first, *first);
    for pos in placed {
        min = BlockPos::new(min.x.min(pos.x), min.y.min(pos.y), min.z.min(pos.z));
        max = BlockPos::new(max.x.max(pos.x), max.y.max(pos.y), max.z.max(pos.z));
    }
    let size = [max.x - min.x + 1, max.y - min.y + 1, max.z - min.z + 1];
    let at = |x: i32, y: i32, z: i32| BlockPos::new(min.x + x, min.y + y, min.z + z);
    // (axis along the column, outer axis, inner axis, negative face, positive face)
    let passes = [
        (2usize, 0usize, 1usize, Direction::North, Direction::South),
        (1, 2, 0, Direction::Down, Direction::Up),
        (0, 1, 2, Direction::West, Direction::East),
    ];
    let mut out = Vec::new();
    for (c_axis, a_axis, b_axis, negative, positive) in passes {
        for a in 0..size[a_axis] {
            for b in 0..size[b_axis] {
                let mut last_full = false;
                for c in 0..=size[c_axis] {
                    let cell = |c: i32| {
                        let mut local = [0; 3];
                        local[a_axis] = a;
                        local[b_axis] = b;
                        local[c_axis] = c;
                        at(local[0], local[1], local[2])
                    };
                    let full = c != size[c_axis] && cells.contains(&cell(c));
                    if !last_full && full {
                        out.push((negative, cell(c)));
                    }
                    if last_full && !full {
                        out.push((positive, cell(c - 1)));
                    }
                    last_full = full;
                }
            }
        }
    }
    out
}

/// `state.updateShape(level, pos, direction, ...)` where it is known; otherwise `state`
/// itself, tallied. The rules known read the level, which already holds the neighbour vanilla
/// passes in, so the neighbour is not an argument.
fn update_shape(
    world: &mut TemplateWorld<'_, '_>,
    state: &BlockState,
    pos: BlockPos,
    direction: Direction,
) -> BlockState {
    let facts = &world.facts;
    if facts.is(state, BlockFact::ShapeInert) {
        return state.clone();
    }
    match state.name.path() {
        "potent_sulfur" if state.name.namespace() == "minecraft" => {
            potent_sulfur(world, state, pos)
        }
        "sulfur_spike" | "pointed_dripstone"
            if state.name.namespace() == "minecraft"
                && state
                    .properties
                    .get("vertical_direction")
                    .is_some_and(|tip| tip == "up") =>
        {
            speleothem_up(world, state, pos, direction)
        }
        _ => {
            world.record_unsupported("shape_update", &state.name.to_string());
            state.clone()
        }
    }
}

/// `PotentSulfurBlock.validBlockState`.
fn potent_sulfur(world: &TemplateWorld<'_, '_>, state: &BlockState, pos: BlockPos) -> BlockState {
    let facts = &world.facts;
    let tags = &world.block_tags;
    let above = block_in(world.region, step(pos, Direction::Up));
    let water_source = facts.is(above, BlockFact::Water) && facts.is(above, BlockFact::FluidSource);
    let with = |value: &str| state.clone().with_property("potent_sulfur_state", value);
    if !water_source {
        return with("dry");
    }
    let below = block_in(world.region, step(pos, Direction::Down));
    // `isSourceIfFluid`: no fluid, or a source.
    let source_if_fluid = !(facts.is(below, BlockFact::Water) || facts.is(below, BlockFact::Lava))
        || facts.is(below, BlockFact::FluidSource);
    let tagged = |tag: &str| tags.contains(&ResourceLocation::minecraft(tag), &below.name);
    if tagged("causes_continuous_geyser_eruptions") && source_if_fluid {
        return with("continuous");
    }
    if tagged("causes_periodic_geyser_eruptions") && source_if_fluid {
        // The block entity's countdown reset is not state, so it is not modelled.
        return if state
            .properties
            .get("potent_sulfur_state")
            .is_some_and(|value| value == "erupting")
        {
            state.clone()
        } else {
            with("dormant")
        };
    }
    with("wet")
}

fn thickness(state: &BlockState) -> &str {
    state
        .properties
        .get("thickness")
        .map(String::as_str)
        .unwrap_or("")
}

/// `SpeleothemBlock.updateShape` for a tip pointing up. (Pointing down it first consults the
/// block's scheduled ticks, which the region does not keep, so those are left to the tally.)
fn speleothem_up(
    world: &TemplateWorld<'_, '_>,
    state: &BlockState,
    pos: BlockPos,
    direction: Direction,
) -> BlockState {
    if direction != Direction::Up && direction != Direction::Down {
        return state.clone();
    }
    let facts = &world.facts;
    let tags = &world.block_tags;
    let speleothem_pointing = |other: &BlockState, tip: &str| {
        tags.contains(&ResourceLocation::minecraft("speleothems"), &other.name)
            && other
                .properties
                .get("vertical_direction")
                .is_some_and(|value| value == tip)
    };
    let below = block_in(world.region, step(pos, Direction::Down));
    // `canSurvive` = `isValidSpeleothemPlacement(pos, UP)`: a sturdy top below, or the same
    // block pointing up. Failing it on the update from below only schedules a tick.
    let survives = facts.is(below, BlockFact::SturdyTop)
        || (speleothem_pointing(below, "up") && below.name == state.name);
    if direction == Direction::Down && !survives {
        return state.clone();
    }
    let in_front = block_in(world.region, step(pos, Direction::Up));
    let merge = thickness(state) == "tip_merge";
    let new_thickness = if speleothem_pointing(in_front, "down") && in_front.name == state.name {
        if !merge && thickness(in_front) != "tip_merge" {
            "tip"
        } else {
            "tip_merge"
        }
    } else if !speleothem_pointing(in_front, "up") {
        "tip"
    } else if thickness(in_front) != "tip" && thickness(in_front) != "tip_merge" {
        if !speleothem_pointing(below, "up") {
            "base"
        } else {
            "middle"
        }
    } else {
        "frustum"
    };
    state.clone().with_property("thickness", new_thickness)
}
