//! Trunk placers (`TrunkPlacer` and its subclasses), in vanilla's draw order.
//!
//! Each placer returns the foliage attachment points the trunk produced; the foliage placer
//! hangs leaves off every one of them. The shared pieces are `place_log` (checks the position,
//! samples the trunk provider, orients `axis` logs) and the block under the trunk, which is
//! written through the same trunk set as the logs.

use oxide_core::BlockPos;
use oxide_datapack::placement::IntProvider;
use oxide_datapack::tree::{TrunkKind, UniformRange};

use super::foliage::Attachment;
use super::{dist_manhattan, off, rel, Axis, Dir, Layer, Res, Run, TreeFeature};

/// `TrunkPlacer.validTreePos`: air or replaceable by trees; the upwards-branching placer also
/// grows through its `can_grow_through` blocks.
fn valid_log_pos(tree: &TreeFeature, run: &Run<'_, '_, '_, '_>, p: BlockPos) -> bool {
    run.valid_tree_pos(p)
        || tree
            .grow_through
            .as_ref()
            .is_some_and(|set| set.contains_block(run.cx.env, &run.state(p).name))
}

/// `TrunkPlacer.isFree`: a position a tree may grow into (or already part of a log).
pub(super) fn is_free(tree: &TreeFeature, run: &Run<'_, '_, '_, '_>, p: BlockPos) -> bool {
    valid_log_pos(tree, run, p) || run.is_log(p)
}

fn place_log(
    tree: &TreeFeature,
    run: &mut Run<'_, '_, '_, '_>,
    p: BlockPos,
    axis: Option<Axis>,
) -> Res<bool> {
    if !valid_log_pos(tree, run, p) {
        return Ok(false);
    }
    let mut state = run.sample(&tree.trunk, p)?;
    if let Some(axis) = axis {
        state = run.with_axis(state, axis);
    }
    run.set(Layer::Trunk, p, state);
    Ok(true)
}

fn place_log_if_free(tree: &TreeFeature, run: &mut Run<'_, '_, '_, '_>, p: BlockPos) -> Res<()> {
    if is_free(tree, run, p) {
        place_log(tree, run, p, None)?;
    }
    Ok(())
}

/// `TrunkPlacer.placeBelowTrunkBlock`.
fn place_below_trunk(tree: &TreeFeature, run: &mut Run<'_, '_, '_, '_>, p: BlockPos) -> Res<()> {
    if let Some(state) = run.sample_optional(&tree.below_trunk, p)? {
        run.set(Layer::Trunk, p, state);
    }
    Ok(())
}

pub(super) fn place_trunk(
    tree: &TreeFeature,
    run: &mut Run<'_, '_, '_, '_>,
    tree_height: i32,
    origin: BlockPos,
) -> Res<Vec<Attachment>> {
    let below = off(origin, 0, -1, 0);
    match &tree.config.trunk_placer.kind {
        TrunkKind::Straight {} => {
            place_below_trunk(tree, run, below)?;
            for y in 0..tree_height {
                place_log(tree, run, off(origin, 0, y, 0), None)?;
            }
            Ok(vec![Attachment::new(
                off(origin, 0, tree_height, 0),
                0,
                false,
            )])
        }
        TrunkKind::Forking {} => forking(tree, run, tree_height, origin),
        TrunkKind::Giant {} => giant(tree, run, tree_height, origin),
        TrunkKind::MegaJungle {} => {
            let mut attachments = giant(tree, run, tree_height, origin)?;
            let mut branch_height = tree_height - 2 - run.bounded(4);
            while branch_height > tree_height / 2 {
                let angle = run.float() * (std::f64::consts::PI * 2.0) as f32;
                let (mut bx, mut bz) = (0, 0);
                for b in 0..5 {
                    bx = (1.5f32 + oxide_core::mth::cos(f64::from(angle)) * b as f32) as i32;
                    bz = (1.5f32 + oxide_core::mth::sin(f64::from(angle)) * b as f32) as i32;
                    place_log(
                        tree,
                        run,
                        off(origin, bx, branch_height - 3 + b / 2, bz),
                        None,
                    )?;
                }
                attachments.push(Attachment::new(
                    off(origin, bx, branch_height, bz),
                    -2,
                    false,
                ));
                branch_height -= 2 + run.bounded(4);
            }
            Ok(attachments)
        }
        TrunkKind::DarkOak {} => dark_oak(tree, run, tree_height, origin),
        TrunkKind::Fancy {} => fancy(tree, run, tree_height, origin),
        TrunkKind::Bending {
            min_height_for_leaves,
            bend_length,
        } => {
            let direction = run.horizontal();
            let log_height = tree_height - 1;
            let mut pos = origin;
            place_below_trunk(tree, run, below)?;
            let mut points = Vec::new();
            for i in 0..=log_height {
                if i + 1 >= log_height + run.bounded(2) {
                    pos = rel(pos, direction, 1);
                }
                if run.valid_tree_pos(pos) {
                    place_log(tree, run, pos, None)?;
                }
                if i >= *min_height_for_leaves {
                    points.push(Attachment::new(pos, 0, false));
                }
                pos = off(pos, 0, 1, 0);
            }
            let length = run.int(bend_length);
            for _ in 0..=length {
                if run.valid_tree_pos(pos) {
                    place_log(tree, run, pos, None)?;
                }
                points.push(Attachment::new(pos, 0, false));
                pos = rel(pos, direction, 1);
            }
            Ok(points)
        }
        TrunkKind::UpwardsBranching {
            extra_branch_steps,
            place_branch_per_log_probability,
            extra_branch_length,
            ..
        } => {
            let mut attachments = Vec::new();
            for height in 0..tree_height {
                let current = origin.y.wrapping_add(height);
                let log = BlockPos::new(origin.x, current, origin.z);
                if place_log(tree, run, log, None)?
                    && height < tree_height - 1
                    && run.float() < *place_branch_per_log_probability
                {
                    let direction = run.horizontal();
                    let length = run.int(extra_branch_length);
                    let branch_pos = 0.max(length - run.int(extra_branch_length) - 1);
                    let steps = run.int(extra_branch_steps);
                    upwards_branch(
                        tree,
                        run,
                        tree_height,
                        &mut attachments,
                        (log, current),
                        direction,
                        branch_pos,
                        steps,
                    )?;
                }
                if height == tree_height - 1 {
                    attachments.push(Attachment::new(
                        BlockPos::new(origin.x, current.wrapping_add(1), origin.z),
                        0,
                        false,
                    ));
                }
            }
            Ok(attachments)
        }
        TrunkKind::Cherry {
            branch_count,
            branch_horizontal_length,
            branch_start_offset_from_top,
            branch_end_offset_from_top,
        } => cherry(
            tree,
            run,
            tree_height,
            origin,
            (
                branch_count,
                branch_horizontal_length,
                branch_start_offset_from_top,
                branch_end_offset_from_top,
            ),
        ),
    }
}

/// `ForkingTrunkPlacer.placeTrunk`.
fn forking(
    tree: &TreeFeature,
    run: &mut Run<'_, '_, '_, '_>,
    tree_height: i32,
    origin: BlockPos,
) -> Res<Vec<Attachment>> {
    place_below_trunk(tree, run, off(origin, 0, -1, 0))?;
    let mut attachments = Vec::new();
    let lean = run.horizontal();
    let lean_height = tree_height - run.bounded(4) - 1;
    let mut lean_steps = 3 - run.bounded(3);
    let (mut tx, mut tz) = (origin.x, origin.z);
    let mut end_y = None;
    for yo in 0..tree_height {
        let yy = origin.y.wrapping_add(yo);
        if yo >= lean_height && lean_steps > 0 {
            let n = lean.normal();
            tx = tx.wrapping_add(n[0]);
            tz = tz.wrapping_add(n[2]);
            lean_steps -= 1;
        }
        if place_log(tree, run, BlockPos::new(tx, yy, tz), None)? {
            end_y = Some(yy.wrapping_add(1));
        }
    }
    if let Some(end_y) = end_y {
        attachments.push(Attachment::new(BlockPos::new(tx, end_y, tz), 1, false));
    }
    tx = origin.x;
    tz = origin.z;
    let branch = run.horizontal();
    if branch != lean {
        let branch_pos = lean_height - run.bounded(2) - 1;
        let mut steps = 1 + run.bounded(3);
        let mut end_y = None;
        let mut yo = branch_pos;
        while yo < tree_height && steps > 0 {
            if yo >= 1 {
                let yy = origin.y.wrapping_add(yo);
                let n = branch.normal();
                tx = tx.wrapping_add(n[0]);
                tz = tz.wrapping_add(n[2]);
                if place_log(tree, run, BlockPos::new(tx, yy, tz), None)? {
                    end_y = Some(yy.wrapping_add(1));
                }
            }
            yo += 1;
            steps -= 1;
        }
        if let Some(end_y) = end_y {
            attachments.push(Attachment::new(BlockPos::new(tx, end_y, tz), 0, false));
        }
    }
    Ok(attachments)
}

/// `GiantTrunkPlacer.placeTrunk`: a 2x2 trunk.
fn giant(
    tree: &TreeFeature,
    run: &mut Run<'_, '_, '_, '_>,
    tree_height: i32,
    origin: BlockPos,
) -> Res<Vec<Attachment>> {
    let below = off(origin, 0, -1, 0);
    place_below_trunk(tree, run, below)?;
    place_below_trunk(tree, run, off(below, 1, 0, 0))?;
    place_below_trunk(tree, run, off(below, 0, 0, 1))?;
    place_below_trunk(tree, run, off(below, 1, 0, 1))?;
    for hh in 0..tree_height {
        place_log_if_free(tree, run, off(origin, 0, hh, 0))?;
        if hh < tree_height - 1 {
            place_log_if_free(tree, run, off(origin, 1, hh, 0))?;
            place_log_if_free(tree, run, off(origin, 1, hh, 1))?;
            place_log_if_free(tree, run, off(origin, 0, hh, 1))?;
        }
    }
    Ok(vec![Attachment::new(
        off(origin, 0, tree_height, 0),
        0,
        true,
    )])
}

/// `DarkOakTrunkPlacer.placeTrunk`.
fn dark_oak(
    tree: &TreeFeature,
    run: &mut Run<'_, '_, '_, '_>,
    tree_height: i32,
    origin: BlockPos,
) -> Res<Vec<Attachment>> {
    let mut attachments = Vec::new();
    let below = off(origin, 0, -1, 0);
    place_below_trunk(tree, run, below)?;
    place_below_trunk(tree, run, off(below, 1, 0, 0))?;
    place_below_trunk(tree, run, off(below, 0, 0, 1))?;
    place_below_trunk(tree, run, off(below, 1, 0, 1))?;
    let lean = run.horizontal();
    let lean_height = tree_height - run.bounded(4);
    let mut lean_steps = 2 - run.bounded(3);
    let (x, y, z) = (origin.x, origin.y, origin.z);
    let (mut tx, mut tz) = (x, z);
    let end_y = y.wrapping_add(tree_height).wrapping_sub(1);
    for dy in 0..tree_height {
        if dy >= lean_height && lean_steps > 0 {
            let n = lean.normal();
            tx = tx.wrapping_add(n[0]);
            tz = tz.wrapping_add(n[2]);
            lean_steps -= 1;
        }
        let p = BlockPos::new(tx, y.wrapping_add(dy), tz);
        if run.is_air(p) || run.is_leaves(p) {
            place_log(tree, run, p, None)?;
            place_log(tree, run, off(p, 1, 0, 0), None)?;
            place_log(tree, run, off(p, 0, 0, 1), None)?;
            place_log(tree, run, off(p, 1, 0, 1), None)?;
        }
    }
    attachments.push(Attachment::new(BlockPos::new(tx, end_y, tz), 0, true));
    for ox in -1..=2 {
        for oz in -1..=2 {
            if (!(0..=1).contains(&ox) || !(0..=1).contains(&oz)) && run.bounded(3) <= 0 {
                let length = run.bounded(3) + 2;
                for branch_y in 0..length {
                    place_log(
                        tree,
                        run,
                        BlockPos::new(
                            x.wrapping_add(ox),
                            end_y.wrapping_sub(branch_y).wrapping_sub(1),
                            z.wrapping_add(oz),
                        ),
                        None,
                    )?;
                }
                attachments.push(Attachment::new(
                    BlockPos::new(x.wrapping_add(ox), end_y, z.wrapping_add(oz)),
                    0,
                    false,
                ));
            }
        }
    }
    Ok(attachments)
}

/// `UpwardsBranchingTrunkPlacer.placeBranch`.
#[allow(clippy::too_many_arguments)]
fn upwards_branch(
    tree: &TreeFeature,
    run: &mut Run<'_, '_, '_, '_>,
    tree_height: i32,
    attachments: &mut Vec<Attachment>,
    (log, current_height): (BlockPos, i32),
    direction: Dir,
    branch_pos: i32,
    mut steps: i32,
) -> Res<()> {
    let mut height_along = current_height.wrapping_add(branch_pos);
    let (mut lx, mut lz) = (log.x, log.z);
    let mut index = branch_pos;
    let n = direction.normal();
    while index < tree_height && steps > 0 {
        if index >= 1 {
            let placement = current_height.wrapping_add(index);
            lx = lx.wrapping_add(n[0]);
            lz = lz.wrapping_add(n[2]);
            height_along = placement;
            let branch_log = BlockPos::new(lx, placement, lz);
            if place_log(tree, run, branch_log, None)? {
                height_along = height_along.wrapping_add(1);
            }
            attachments.push(Attachment::new(branch_log, 0, false));
        }
        index += 1;
        steps -= 1;
    }
    if height_along.wrapping_sub(current_height) > 1 {
        let foliage = BlockPos::new(lx, height_along, lz);
        attachments.push(Attachment::new(foliage, 0, false));
        attachments.push(Attachment::new(off(foliage, 0, -2, 0), 0, false));
    }
    Ok(())
}

type CherryProviders<'a> = (
    &'a IntProvider,
    &'a IntProvider,
    &'a UniformRange,
    &'a IntProvider,
);

/// `CherryTrunkPlacer.placeTrunk`.
fn cherry(
    tree: &TreeFeature,
    run: &mut Run<'_, '_, '_, '_>,
    tree_height: i32,
    origin: BlockPos,
    (branch_count, horizontal_length, start_offset, end_offset): CherryProviders<'_>,
) -> Res<Vec<Attachment>> {
    place_below_trunk(tree, run, off(origin, 0, -1, 0))?;
    let first = 0
        .max(tree_height - 1 + run.between(start_offset.min_inclusive, start_offset.max_inclusive));
    let mut second = 0.max(
        tree_height - 1 + run.between(start_offset.min_inclusive, start_offset.max_inclusive - 1),
    );
    if second >= first {
        second += 1;
    }
    let count = run.int(branch_count);
    let middle = count == 3;
    let both = count >= 2;
    let trunk_height = if middle {
        tree_height
    } else if both {
        first.max(second) + 1
    } else {
        first + 1
    };
    for y in 0..trunk_height {
        place_log(tree, run, off(origin, 0, y, 0), None)?;
    }
    let mut attachments = Vec::new();
    if middle {
        attachments.push(Attachment::new(off(origin, 0, trunk_height, 0), 0, false));
    }
    let direction = run.horizontal();
    let sideways = direction.axis();
    let branch = |run: &mut Run<'_, '_, '_, '_>,
                  dir: Dir,
                  offset: i32,
                  middle_continues: bool|
     -> Res<Attachment> {
        let mut log = off(origin, 0, offset, 0);
        let end_y = tree_height - 1 + run.int(end_offset);
        let extend = middle_continues || end_y < offset;
        let distance = run.int(horizontal_length) + i32::from(extend);
        let end = off(rel(origin, dir, distance), 0, end_y, 0);
        for _ in 0..(if extend { 2 } else { 1 }) {
            log = rel(log, dir, 1);
            place_log(tree, run, log, Some(sideways))?;
        }
        let vertical = if end.y > log.y { Dir::Up } else { Dir::Down };
        loop {
            let distance = dist_manhattan(log, end);
            if distance == 0 {
                return Ok(Attachment::new(off(end, 0, 1, 0), 0, false));
            }
            let chance = end.y.wrapping_sub(log.y).wrapping_abs() as f32 / distance as f32;
            let grow_vertically = run.float() < chance;
            log = rel(log, if grow_vertically { vertical } else { dir }, 1);
            place_log(
                tree,
                run,
                log,
                if grow_vertically {
                    None
                } else {
                    Some(sideways)
                },
            )?;
        }
    };
    attachments.push(branch(run, direction, first, first < trunk_height - 1)?);
    if both {
        attachments.push(branch(
            run,
            direction.opposite(),
            second,
            second < trunk_height - 1,
        )?);
    }
    Ok(attachments)
}

/// `FancyTrunkPlacer.placeTrunk`: a trunk with limbs to clusters of foliage.
fn fancy(
    tree: &TreeFeature,
    run: &mut Run<'_, '_, '_, '_>,
    tree_height: i32,
    origin: BlockPos,
) -> Res<Vec<Attachment>> {
    let height = tree_height + 2;
    let trunk_height = (f64::from(height) * 0.618).floor() as i32;
    place_below_trunk(tree, run, off(origin, 0, -1, 0))?;
    let ratio = 1.0 * f64::from(height) / 13.0;
    let clusters_per_y = 1.min((1.382 + ratio * ratio).floor() as i32);
    let trunk_top = origin.y.wrapping_add(trunk_height);
    let mut relative_y = height - 5;
    // (attachment position, y the limb starts from)
    let mut coords = vec![(off(origin, 0, relative_y, 0), trunk_top)];
    while relative_y >= 0 {
        let shape = tree_shape(height, relative_y);
        if shape >= 0.0 {
            for _ in 0..clusters_per_y {
                let radius = 1.0 * f64::from(shape) * (f64::from(run.float()) + 0.328);
                let angle = f64::from(run.float() * 2.0f32) * std::f64::consts::PI;
                // PARITY-CHECK: Java's `Math.sin`/`Math.cos` against Rust's, which can differ by
                // an ulp; it only matters when the product lands exactly on an integer boundary.
                let x = radius * angle.sin() + 0.5;
                let z = radius * angle.cos() + 0.5;
                let check_start = off(origin, x.floor() as i32, relative_y - 1, z.floor() as i32);
                let check_end = off(check_start, 0, 5, 0);
                if make_limb(tree, run, check_start, check_end, false)? {
                    let dx = origin.x.wrapping_sub(check_start.x);
                    let dz = origin.z.wrapping_sub(check_start.z);
                    let branch_height = f64::from(check_start.y)
                        - f64::from(dx.wrapping_mul(dx).wrapping_add(dz.wrapping_mul(dz))).sqrt()
                            * 0.381;
                    let branch_top = if branch_height > f64::from(trunk_top) {
                        trunk_top
                    } else {
                        branch_height as i32
                    };
                    let base = BlockPos::new(origin.x, branch_top, origin.z);
                    if make_limb(tree, run, base, check_start, false)? {
                        coords.push((check_start, base.y));
                    }
                }
            }
        }
        relative_y -= 1;
    }
    make_limb(tree, run, origin, off(origin, 0, trunk_height, 0), true)?;
    for (end, branch_base) in &coords {
        let base = BlockPos::new(origin.x, *branch_base, origin.z);
        if base != *end && trim_branches(height, branch_base.wrapping_sub(origin.y)) {
            make_limb(tree, run, base, *end, true)?;
        }
    }
    Ok(coords
        .into_iter()
        .filter(|(_, base)| trim_branches(height, base.wrapping_sub(origin.y)))
        .map(|(p, _)| Attachment::new(p, 0, false))
        .collect())
}

/// `FancyTrunkPlacer.makeLimb`: with `place` false, whether a straight line is free.
fn make_limb(
    tree: &TreeFeature,
    run: &mut Run<'_, '_, '_, '_>,
    start: BlockPos,
    end: BlockPos,
    place: bool,
) -> Res<bool> {
    if !place && start == end {
        return Ok(true);
    }
    let delta = [
        end.x.wrapping_sub(start.x),
        end.y.wrapping_sub(start.y),
        end.z.wrapping_sub(start.z),
    ];
    let steps = delta[0]
        .wrapping_abs()
        .max(delta[1].wrapping_abs().max(delta[2].wrapping_abs()));
    let step = delta.map(|d| d as f32 / steps as f32);
    for i in 0..=steps {
        let at = |axis: usize, from: i32| {
            from.wrapping_add((0.5f32 + i as f32 * step[axis]).floor() as i32)
        };
        let p = BlockPos::new(at(0, start.x), at(1, start.y), at(2, start.z));
        if place {
            place_log(tree, run, p, Some(log_axis(start, p)))?;
        } else if !is_free(tree, run, p) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `FancyTrunkPlacer.getLogAxis`.
fn log_axis(start: BlockPos, p: BlockPos) -> Axis {
    let x_diff = p.x.wrapping_sub(start.x).wrapping_abs();
    let z_diff = p.z.wrapping_sub(start.z).wrapping_abs();
    let max = x_diff.max(z_diff);
    if max > 0 {
        if x_diff == max {
            Axis::X
        } else {
            Axis::Z
        }
    } else {
        Axis::Y
    }
}

fn trim_branches(height: i32, local_y: i32) -> bool {
    f64::from(local_y) >= f64::from(height) * 0.2
}

/// `FancyTrunkPlacer.treeShape`: the foliage cluster radius at height `y`, or -1 below the
/// crown.
fn tree_shape(height: i32, y: i32) -> f32 {
    if (y as f32) < height as f32 * 0.3f32 {
        return -1.0;
    }
    let radius = height as f32 / 2.0;
    let adjacent = radius - y as f32;
    let mut distance = (radius * radius - adjacent * adjacent).sqrt();
    if adjacent == 0.0 {
        distance = radius;
    } else if adjacent.abs() >= radius {
        return 0.0;
    }
    distance * 0.5
}
