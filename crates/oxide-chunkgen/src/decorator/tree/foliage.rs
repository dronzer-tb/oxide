//! Foliage placers (`FoliagePlacer` and its subclasses), in vanilla's draw order.
//!
//! A foliage placer lays rows of leaves around each attachment point the trunk returned. What
//! differs between placers is the row radius per height and `shouldSkipLocation`, which for
//! several of them draws from the random stream (blob and bush corners, cherry holes), so the
//! skip test is evaluated for every candidate position in vanilla's scan order, even ones that
//! would not have placed a leaf anyway.

use oxide_core::BlockPos;
use oxide_datapack::tree::{FoliageKind, FoliagePlacer};

use super::{dist_manhattan, off, rel, Dir, Res, Run, TreeFeature};

/// `FoliagePlacer.FoliageAttachment`.
#[derive(Debug, Clone, Copy)]
pub(super) struct Attachment {
    pub pos: BlockPos,
    pub radius_offset: i32,
    pub double_trunk: bool,
}

impl Attachment {
    pub fn new(pos: BlockPos, radius_offset: i32, double_trunk: bool) -> Self {
        Self {
            pos,
            radius_offset,
            double_trunk,
        }
    }
}

/// `FoliagePlacer.foliageHeight`.
pub(super) fn foliage_height(
    placer: &FoliagePlacer,
    run: &mut Run<'_, '_, '_, '_>,
    tree_height: i32,
) -> i32 {
    match &placer.kind {
        FoliageKind::Blob { height }
        | FoliageKind::Fancy { height }
        | FoliageKind::Bush { height }
        | FoliageKind::Jungle { height } => *height,
        FoliageKind::Acacia {} => 0,
        FoliageKind::DarkOak {} => 4,
        FoliageKind::Spruce { trunk_height } => 4.max(tree_height - run.int(trunk_height)),
        FoliageKind::Pine { height } => run.int(height),
        FoliageKind::MegaPine { crown_height } => run.int(crown_height),
        FoliageKind::RandomSpread { foliage_height, .. } => run.int(foliage_height),
        FoliageKind::Cherry { height, .. } => run.int(height),
    }
}

/// `FoliagePlacer.foliageRadius`.
pub(super) fn foliage_radius(
    placer: &FoliagePlacer,
    run: &mut Run<'_, '_, '_, '_>,
    trunk_height: i32,
) -> i32 {
    let radius = run.int(&placer.radius);
    if matches!(placer.kind, FoliageKind::Pine { .. }) {
        return radius + run.bounded(trunk_height.wrapping_add(1).max(1));
    }
    radius
}

/// `shouldSkipLocation` on offsets already folded to their absolute values.
fn skip(
    kind: &FoliageKind,
    run: &mut Run<'_, '_, '_, '_>,
    dx: i32,
    y: i32,
    dz: i32,
    r: i32,
    double: bool,
) -> bool {
    match kind {
        FoliageKind::Blob { .. } => dx == r && dz == r && (run.bounded(2) == 0 || y == 0),
        FoliageKind::Bush { .. } => dx == r && dz == r && run.bounded(2) == 0,
        FoliageKind::Fancy { .. } => {
            let a = dx as f32 + 0.5;
            let b = dz as f32 + 0.5;
            a * a + b * b > r.wrapping_mul(r) as f32
        }
        FoliageKind::Acacia {} => {
            if y == 0 {
                (dx > 1 || dz > 1) && dx != 0 && dz != 0
            } else {
                dx == r && dz == r && r > 0
            }
        }
        FoliageKind::Spruce { .. } | FoliageKind::Pine { .. } => dx == r && dz == r && r > 0,
        FoliageKind::MegaPine { .. } | FoliageKind::Jungle { .. } => {
            dx.wrapping_add(dz) >= 7
                || dx.wrapping_mul(dx).wrapping_add(dz.wrapping_mul(dz)) > r.wrapping_mul(r)
        }
        FoliageKind::DarkOak {} => {
            if y == -1 && !double {
                dx == r && dz == r
            } else if y == 1 {
                dx.wrapping_add(dz) > r.wrapping_mul(2).wrapping_sub(2)
            } else {
                false
            }
        }
        FoliageKind::RandomSpread { .. } => false,
        FoliageKind::Cherry {
            wide_bottom_layer_hole_chance,
            corner_hole_chance,
            ..
        } => {
            if y == -1 && (dx == r || dz == r) && run.float() < *wide_bottom_layer_hole_chance {
                return true;
            }
            let corner = dx == r && dz == r;
            if r > 2 {
                corner
                    || (dx.wrapping_add(dz) > r.wrapping_mul(2).wrapping_sub(2)
                        && run.float() < *corner_hole_chance)
            } else {
                corner && run.float() < *corner_hole_chance
            }
        }
    }
}

/// `shouldSkipLocationSigned`: folds a double-trunk row's offsets toward the nearer trunk column
/// before the placer's own test. The dark oak placer additionally skips the far corners of its
/// widened middle row outright.
fn skip_signed(
    kind: &FoliageKind,
    run: &mut Run<'_, '_, '_, '_>,
    dx: i32,
    y: i32,
    dz: i32,
    r: i32,
    double: bool,
) -> bool {
    if matches!(kind, FoliageKind::DarkOak {}) {
        let defer = y != 0
            || !double
            || (dx != r.wrapping_neg() && dx < r)
            || (dz != r.wrapping_neg() && dz < r);
        if !defer {
            return true;
        }
    }
    let (mx, mz) = if double {
        (
            dx.wrapping_abs().min(dx.wrapping_sub(1).wrapping_abs()),
            dz.wrapping_abs().min(dz.wrapping_sub(1).wrapping_abs()),
        )
    } else {
        (dx.wrapping_abs(), dz.wrapping_abs())
    };
    skip(kind, run, mx, y, mz, r, double)
}

/// `FoliagePlacer.placeLeavesRow`.
fn leaves_row(
    tree: &TreeFeature,
    run: &mut Run<'_, '_, '_, '_>,
    kind: &FoliageKind,
    origin: BlockPos,
    r: i32,
    y: i32,
    double: bool,
) -> Res<()> {
    let offset = i32::from(double);
    let mut dx = r.wrapping_neg();
    while dx <= r.wrapping_add(offset) {
        let mut dz = r.wrapping_neg();
        while dz <= r.wrapping_add(offset) {
            if !skip_signed(kind, run, dx, y, dz, r, double) {
                tree.try_place_leaf(run, off(origin, dx, y, dz))?;
            }
            dz += 1;
        }
        dx += 1;
    }
    Ok(())
}

/// `CherryFoliagePlacer.placeLeavesRowWithHangingLeavesBelow`.
#[allow(clippy::too_many_arguments)]
fn leaves_row_with_hanging(
    tree: &TreeFeature,
    run: &mut Run<'_, '_, '_, '_>,
    kind: &FoliageKind,
    origin: BlockPos,
    r: i32,
    y: i32,
    double: bool,
    (chance, extension): (f32, f32),
) -> Res<()> {
    leaves_row(tree, run, kind, origin, r, y, double)?;
    let offset = i32::from(double);
    let log = off(origin, 0, -1, 0);
    for along in Dir::HORIZONTAL {
        let to_edge = along.clockwise();
        let positive = matches!(to_edge, Dir::East | Dir::South);
        let to_edge_offset = if positive { r + offset } else { r };
        let mut pos = rel(
            rel(off(origin, 0, y - 1, 0), to_edge, to_edge_offset),
            along,
            r.wrapping_neg(),
        );
        let mut along_offset = r.wrapping_neg();
        while along_offset < r + offset {
            let above = run.foliage_contains(off(pos, 0, 1, 0));
            if above && try_extension(tree, run, chance, log, pos)? {
                try_extension(tree, run, extension, log, off(pos, 0, -1, 0))?;
            }
            along_offset += 1;
            pos = rel(pos, along, 1);
        }
    }
    Ok(())
}

/// `CherryFoliagePlacer.tryPlaceExtension`.
fn try_extension(
    tree: &TreeFeature,
    run: &mut Run<'_, '_, '_, '_>,
    chance: f32,
    log: BlockPos,
    pos: BlockPos,
) -> Res<bool> {
    if dist_manhattan(pos, log) >= 7 {
        return Ok(false);
    }
    if run.float() > chance {
        return Ok(false);
    }
    tree.try_place_leaf(run, pos)
}

/// `FoliagePlacer.createFoliage` for every placer.
pub(super) fn create_foliage(
    tree: &TreeFeature,
    run: &mut Run<'_, '_, '_, '_>,
    attachment: Attachment,
    foliage_height: i32,
    leaf_radius: i32,
) -> Res<()> {
    let placer = &tree.config.foliage_placer;
    let kind = &placer.kind;
    let offset = run.int(&placer.offset);
    let double = attachment.double_trunk;
    let pos = attachment.pos;
    let radius_offset = attachment.radius_offset;
    // `foliageHeight + attachment.foliageHeight()`; no placer here gives an attachment one.
    let with_offset = foliage_height;
    match kind {
        FoliageKind::Blob { .. } => {
            let mut yo = offset;
            while yo >= offset - with_offset {
                let r = 0.max(leaf_radius + radius_offset - 1 - yo / 2);
                leaves_row(tree, run, kind, pos, r, yo, double)?;
                yo -= 1;
            }
        }
        FoliageKind::Fancy { .. } => {
            let mut yo = offset;
            while yo >= offset - foliage_height {
                let r = leaf_radius + i32::from(yo != offset && yo != offset - foliage_height);
                leaves_row(tree, run, kind, pos, r, yo, double)?;
                yo -= 1;
            }
        }
        FoliageKind::Bush { .. } => {
            let mut yo = offset;
            while yo >= offset - with_offset {
                let r = leaf_radius + radius_offset - 1 - yo;
                leaves_row(tree, run, kind, pos, r, yo, double)?;
                yo -= 1;
            }
        }
        FoliageKind::Acacia {} => {
            let p = off(pos, 0, offset, 0);
            leaves_row(
                tree,
                run,
                kind,
                p,
                leaf_radius + radius_offset,
                -1 - with_offset,
                double,
            )?;
            leaves_row(tree, run, kind, p, leaf_radius - 1, -with_offset, double)?;
            leaves_row(
                tree,
                run,
                kind,
                p,
                leaf_radius + radius_offset - 1,
                0,
                double,
            )?;
        }
        FoliageKind::Spruce { .. } => {
            let mut r = run.bounded(2);
            let mut max_r = 1;
            let mut min_r = 0;
            let mut yo = offset;
            while yo >= -with_offset {
                leaves_row(tree, run, kind, pos, r, yo, double)?;
                if r >= max_r {
                    r = min_r;
                    min_r = 1;
                    max_r = (max_r + 1).min(leaf_radius + radius_offset);
                } else {
                    r += 1;
                }
                yo -= 1;
            }
        }
        FoliageKind::Pine { .. } => {
            let mut r = 0;
            let mut yo = offset;
            while yo >= offset - with_offset {
                leaves_row(tree, run, kind, pos, r, yo, double)?;
                if r >= 1 && yo == offset - with_offset + 1 {
                    r -= 1;
                } else if r < leaf_radius + radius_offset {
                    r += 1;
                }
                yo -= 1;
            }
        }
        FoliageKind::MegaPine { .. } => {
            let mut previous = 0;
            let mut yy = pos.y - with_offset + offset;
            while yy <= pos.y + offset {
                let yo = pos.y - yy;
                let smooth = leaf_radius
                    .wrapping_add(radius_offset)
                    .wrapping_add((yo as f32 / with_offset as f32 * 3.5f32).floor() as i32);
                let jagged = if yo > 0 && smooth == previous && (yy & 1) == 0 {
                    smooth.wrapping_add(1)
                } else {
                    smooth
                };
                leaves_row(
                    tree,
                    run,
                    kind,
                    BlockPos::new(pos.x, yy, pos.z),
                    jagged,
                    0,
                    double,
                )?;
                previous = smooth;
                yy += 1;
            }
        }
        FoliageKind::Jungle { .. } => {
            let leaf_height = if double {
                foliage_height
            } else {
                1 + run.bounded(2)
            };
            let mut yo = offset;
            while yo >= offset - leaf_height {
                let r = leaf_radius + radius_offset + 1 - yo;
                leaves_row(tree, run, kind, pos, r, yo, double)?;
                yo -= 1;
            }
        }
        FoliageKind::DarkOak {} => {
            let p = off(pos, 0, offset, 0);
            if double {
                leaves_row(tree, run, kind, p, leaf_radius + 2, -1, double)?;
                leaves_row(tree, run, kind, p, leaf_radius + 3, 0, double)?;
                leaves_row(tree, run, kind, p, leaf_radius + 2, 1, double)?;
                if run.boolean() {
                    leaves_row(tree, run, kind, p, leaf_radius, 2, double)?;
                }
            } else {
                leaves_row(tree, run, kind, p, leaf_radius + 2, -1, double)?;
                leaves_row(tree, run, kind, p, leaf_radius + 1, 0, double)?;
            }
        }
        FoliageKind::RandomSpread {
            leaf_placement_attempts,
            ..
        } => {
            for _ in 0..*leaf_placement_attempts {
                let dx = run.bounded(leaf_radius) - run.bounded(leaf_radius);
                let dy = run.bounded(foliage_height) - run.bounded(foliage_height);
                let dz = run.bounded(leaf_radius) - run.bounded(leaf_radius);
                tree.try_place_leaf(run, off(pos, dx, dy, dz))?;
            }
        }
        FoliageKind::Cherry {
            hanging_leaves_chance,
            hanging_leaves_extension_chance,
            ..
        } => {
            let p = off(pos, 0, offset, 0);
            let r = leaf_radius + radius_offset - 1;
            let hanging = (*hanging_leaves_chance, *hanging_leaves_extension_chance);
            leaves_row(tree, run, kind, p, r - 2, with_offset - 3, double)?;
            leaves_row(tree, run, kind, p, r - 1, with_offset - 4, double)?;
            let mut y = with_offset - 5;
            while y >= 0 {
                leaves_row(tree, run, kind, p, r, y, double)?;
                y -= 1;
            }
            leaves_row_with_hanging(tree, run, kind, p, r, -1, double, hanging)?;
            leaves_row_with_hanging(tree, run, kind, p, r - 1, -2, double, hanging)?;
        }
    }
    Ok(())
}
