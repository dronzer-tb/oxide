//! Tree decorators (`TreeDecorator`): vines, cocoa, bees, ground alteration and the rest, run
//! after the tree itself in config order.
//!
//! Decorators see the tree through [`Lists`]: the trunk, foliage and root positions in the
//! iteration order of Java's sets, stably sorted by Y. That order is what their random draws
//! follow, which is why [`super::pos_set`] exists.
//!
//! Every block a decorator places is resolved at compile time, against the exported facts. A
//! decorator whose block the facts do not know makes its whole tree unsupported (tallied as
//! `tree:<decorator>`) instead of failing the datapack load or placing something else.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use oxide_core::{BlockPos, BlockState, HeightmapType, ResourceLocation};
use oxide_datapack::tree::TreeDecorator;
use oxide_datapack::{BlockFact, BlockFacts};

use super::super::block_entity;
use super::super::feature::{place_configured_feature, Feature};
use super::super::state_provider::StateProvider;
use super::{off, rel, shuffle, with_property, Dir, FeatureLookup, Layer, Res, Run};

/// The configured feature the pale-moss decorator places its ground patch with.
const PALE_MOSS_PATCH: &str = "pale_moss_patch";

/// `TreeDecorator.Context` lists: set iteration order, stably sorted by Y.
pub(super) struct Lists {
    pub logs: Vec<BlockPos>,
    pub leaves: Vec<BlockPos>,
    pub roots: Vec<BlockPos>,
}

impl Lists {
    pub fn new(
        mut logs: Vec<BlockPos>,
        mut leaves: Vec<BlockPos>,
        mut roots: Vec<BlockPos>,
    ) -> Self {
        logs.sort_by_key(|p| p.y);
        leaves.sort_by_key(|p| p.y);
        roots.sort_by_key(|p| p.y);
        Self {
            logs,
            leaves,
            roots,
        }
    }

    /// `Context.logs()` combined with roots as `getLowestTrunkOrRootOfTree`.
    fn lowest(&self) -> Vec<BlockPos> {
        if self.roots.is_empty() {
            self.logs.clone()
        } else if !self.logs.is_empty() && self.roots[0].y == self.logs[0].y {
            let mut all = self.logs.clone();
            all.extend(&self.roots);
            all
        } else {
            self.roots.clone()
        }
    }
}

#[derive(Debug)]
pub(super) enum Decorator {
    TrunkVine {
        vine: BlockState,
    },
    LeaveVine {
        probability: f32,
        vine: BlockState,
    },
    Cocoa {
        probability: f32,
        cocoa: BlockState,
    },
    Beehive {
        probability: f32,
        nest: BlockState,
    },
    AlterGround(StateProvider),
    AttachedToLeaves {
        probability: f32,
        exclusion_xz: i32,
        exclusion_y: i32,
        provider: StateProvider,
        required_empty: i32,
        directions: Vec<Dir>,
    },
    AttachedToLogs {
        probability: f32,
        provider: StateProvider,
        directions: Vec<Dir>,
    },
    CreakingHeart {
        probability: f32,
        heart: BlockState,
    },
    PaleMoss {
        leaves: f32,
        trunk: f32,
        ground: f32,
        patch: Arc<Feature>,
        moss: BlockState,
    },
    PlaceOnGround {
        tries: i32,
        radius: i32,
        height: i32,
        provider: StateProvider,
    },
}

/// The default state of block `path`, with `properties` applied, if the facts know the result.
fn known_state(facts: &BlockFacts, path: &str, properties: &[(&str, &str)]) -> Option<BlockState> {
    let mut state = facts
        .default_state(&ResourceLocation::minecraft(path))?
        .clone();
    for (name, value) in properties {
        state = with_property(state, name, value);
    }
    facts.knows(&state).then_some(state)
}

/// The bare vine state, when the facts also know a vine with a face set.
fn vine(facts: &BlockFacts) -> Option<BlockState> {
    known_state(facts, "vine", &[])
        .filter(|_| known_state(facts, "vine", &[("east", "true")]).is_some())
}

fn directions(names: &[String]) -> Result<Vec<Dir>> {
    if names.is_empty() {
        return Err(anyhow!("tree decorator with no directions"));
    }
    names
        .iter()
        .map(|name| Dir::parse(name).ok_or_else(|| anyhow!("unknown direction {name:?}")))
        .collect()
}

impl Decorator {
    /// Compiles the list; the second value names the first decorator this generator cannot
    /// complete, if any.
    pub fn compile_list(
        irs: &[TreeDecorator],
        facts: &BlockFacts,
        lookup: &mut FeatureLookup<'_>,
    ) -> Result<(Vec<Self>, Option<String>)> {
        let mut decorators = Vec::with_capacity(irs.len());
        let mut unsupported = None;
        for ir in irs {
            match Self::compile(ir, facts, lookup)? {
                Ok(decorator) => decorators.push(decorator),
                Err(component) => {
                    unsupported.get_or_insert(component);
                }
            }
        }
        Ok((decorators, unsupported))
    }

    /// `Ok(Err(component))` when a block the decorator places is not in the exported facts.
    fn compile(
        ir: &TreeDecorator,
        facts: &BlockFacts,
        lookup: &mut FeatureLookup<'_>,
    ) -> Result<std::result::Result<Self, String>> {
        let missing = |name: &str| Ok(Err(format!("{name} (block not in the exported facts)")));
        Ok(Ok(match ir {
            TreeDecorator::TrunkVine {} => match vine(facts) {
                Some(vine) => Self::TrunkVine { vine },
                None => return missing("minecraft:trunk_vine"),
            },
            TreeDecorator::LeaveVine { probability } => match vine(facts) {
                Some(vine) => Self::LeaveVine {
                    probability: *probability,
                    vine,
                },
                None => return missing("minecraft:leave_vine"),
            },
            TreeDecorator::Cocoa { probability } => {
                match known_state(facts, "cocoa", &[("age", "2"), ("facing", "north")]) {
                    Some(cocoa) => Self::Cocoa {
                        probability: *probability,
                        cocoa,
                    },
                    None => return missing("minecraft:cocoa"),
                }
            }
            TreeDecorator::Beehive { probability } => {
                match known_state(facts, "bee_nest", &[("facing", "south")]) {
                    Some(nest) => Self::Beehive {
                        probability: *probability,
                        nest,
                    },
                    None => return missing("minecraft:beehive"),
                }
            }
            TreeDecorator::AlterGround { provider } => {
                Self::AlterGround(StateProvider::compile(provider, facts)?)
            }
            TreeDecorator::AttachedToLeaves {
                probability,
                exclusion_radius_xz,
                exclusion_radius_y,
                block_provider,
                required_empty_blocks,
                directions: names,
            } => Self::AttachedToLeaves {
                probability: *probability,
                exclusion_xz: *exclusion_radius_xz,
                exclusion_y: *exclusion_radius_y,
                provider: StateProvider::compile(block_provider, facts)?,
                required_empty: *required_empty_blocks,
                directions: directions(names)?,
            },
            TreeDecorator::AttachedToLogs {
                probability,
                block_provider,
                directions: names,
            } => Self::AttachedToLogs {
                probability: *probability,
                provider: StateProvider::compile(block_provider, facts)?,
                directions: directions(names)?,
            },
            TreeDecorator::CreakingHeart { probability } => {
                match known_state(
                    facts,
                    "creaking_heart",
                    &[("creaking_heart_state", "dormant"), ("natural", "true")],
                ) {
                    Some(heart) => Self::CreakingHeart {
                        probability: *probability,
                        heart,
                    },
                    None => return missing("minecraft:creaking_heart"),
                }
            }
            TreeDecorator::PaleMoss {
                leaves_probability,
                trunk_probability,
                ground_probability,
            } => {
                let moss = known_state(facts, "pale_hanging_moss", &[("tip", "true")]);
                let patch = lookup(&ResourceLocation::minecraft(PALE_MOSS_PATCH))?;
                match (moss, patch) {
                    (Some(moss), Some(patch)) => Self::PaleMoss {
                        leaves: *leaves_probability,
                        trunk: *trunk_probability,
                        ground: *ground_probability,
                        patch,
                        moss,
                    },
                    _ => return missing("minecraft:pale_moss (moss block or ground patch)"),
                }
            }
            TreeDecorator::PlaceOnGround {
                tries,
                radius,
                height,
                block_state_provider,
            } => Self::PlaceOnGround {
                tries: *tries,
                radius: *radius,
                height: *height,
                provider: StateProvider::compile(block_state_provider, facts)?,
            },
        }))
    }

    pub fn place(&self, run: &mut Run<'_, '_, '_, '_>, lists: &Lists) -> Res<()> {
        match self {
            Self::TrunkVine { vine } => {
                for &pos in &lists.logs {
                    for dir in [Dir::West, Dir::East, Dir::North, Dir::South] {
                        if run.bounded(3) > 0 {
                            let p = rel(pos, dir, 1);
                            if run.is_air(p) {
                                place_vine(run, vine, p, dir.opposite());
                            }
                        }
                    }
                }
            }
            Self::LeaveVine { probability, vine } => {
                for &pos in &lists.leaves {
                    for dir in [Dir::West, Dir::East, Dir::North, Dir::South] {
                        if run.float() < *probability {
                            let p = rel(pos, dir, 1);
                            if run.is_air(p) {
                                place_vine(run, vine, p, dir.opposite());
                                let mut below = off(p, 0, -1, 0);
                                let mut remaining = 4;
                                while run.is_air(below) && remaining > 0 {
                                    place_vine(run, vine, below, dir.opposite());
                                    below = off(below, 0, -1, 0);
                                    remaining -= 1;
                                }
                            }
                        }
                    }
                }
            }
            Self::Cocoa { probability, cocoa } => {
                if run.float() >= *probability {
                    return Ok(());
                }
                let Some(first) = lists.logs.first() else {
                    return Ok(());
                };
                let tree_y = first.y;
                for &pos in lists.logs.iter().filter(|p| p.y.wrapping_sub(tree_y) <= 2) {
                    for dir in Dir::HORIZONTAL {
                        if run.float() <= 0.25 {
                            let p = rel(pos, dir.opposite(), 1);
                            if run.is_air(p) {
                                let age = run.bounded(3);
                                let state = with_property(cocoa.clone(), "age", &age.to_string());
                                let state = with_property(state, "facing", dir.name());
                                run.set(Layer::Decoration, p, state);
                            }
                        }
                    }
                }
            }
            Self::Beehive { probability, nest } => beehive(run, lists, *probability, nest),
            Self::AlterGround(provider) => {
                let positions = lists.lowest();
                let Some(first) = positions.first() else {
                    return Ok(());
                };
                let min_y = first.y;
                for &pos in positions.iter().filter(|p| p.y == min_y) {
                    for (dx, dz) in [(-1, -1), (2, -1), (-1, 2), (2, 2)] {
                        place_circle(run, provider, off(pos, dx, 0, dz))?;
                    }
                    for _ in 0..5 {
                        let placement = run.bounded(64);
                        let (xx, zz) = (placement % 8, placement / 8);
                        if xx == 0 || xx == 7 || zz == 0 || zz == 7 {
                            place_circle(run, provider, off(pos, -3 + xx, 0, -3 + zz))?;
                        }
                    }
                }
            }
            Self::AttachedToLeaves {
                probability,
                exclusion_xz,
                exclusion_y,
                provider,
                required_empty,
                directions,
            } => {
                let mut blacklist = HashSet::new();
                let mut leaves = lists.leaves.clone();
                shuffle(&mut leaves, &mut *run.cx.random);
                for leaf in leaves {
                    let dir = directions[run.bounded(directions.len() as i32) as usize];
                    let p = rel(leaf, dir, 1);
                    if blacklist.contains(&p) || run.float() >= *probability {
                        continue;
                    }
                    if !(1..=*required_empty).all(|i| run.is_air(rel(leaf, dir, i))) {
                        continue;
                    }
                    for dx in -exclusion_xz..=*exclusion_xz {
                        for dy in -exclusion_y..=*exclusion_y {
                            for dz in -exclusion_xz..=*exclusion_xz {
                                blacklist.insert(off(p, dx, dy, dz));
                            }
                        }
                    }
                    let state = run.sample(provider, p)?;
                    run.set(Layer::Decoration, p, state);
                }
            }
            Self::AttachedToLogs {
                probability,
                provider,
                directions,
            } => {
                let mut logs = lists.logs.clone();
                shuffle(&mut logs, &mut *run.cx.random);
                for log in logs {
                    let dir = directions[run.bounded(directions.len() as i32) as usize];
                    let p = rel(log, dir, 1);
                    if run.float() <= *probability && run.is_air(p) {
                        let state = run.sample(provider, p)?;
                        run.set(Layer::Decoration, p, state);
                    }
                }
            }
            Self::CreakingHeart { probability, heart } => {
                if lists.logs.is_empty() || run.float() >= *probability {
                    return Ok(());
                }
                let mut candidates = lists.logs.clone();
                shuffle(&mut candidates, &mut *run.cx.random);
                for pos in candidates {
                    if Dir::ALL.iter().all(|&dir| run.is_log(rel(pos, dir, 1))) {
                        run.set(Layer::Decoration, pos, heart.clone());
                        break;
                    }
                }
            }
            Self::PaleMoss {
                leaves,
                trunk,
                ground,
                patch,
                moss,
            } => {
                let mut logs = lists.logs.clone();
                shuffle(&mut logs, &mut *run.cx.random);
                let Some(origin) = logs
                    .iter()
                    .copied()
                    .reduce(|best, p| if p.y < best.y { p } else { best })
                else {
                    return Ok(());
                };
                if run.float() < *ground {
                    let saved = std::mem::replace(&mut run.cx.origin, off(origin, 0, 1, 0));
                    place_configured_feature(patch, run.cx);
                    run.cx.origin = saved;
                }
                for (positions, chance) in [(&lists.logs, *trunk), (&lists.leaves, *leaves)] {
                    for &pos in positions {
                        if run.float() < chance {
                            let down = off(pos, 0, -1, 0);
                            if run.is_air(down) {
                                moss_hanger(run, moss, down);
                            }
                        }
                    }
                }
            }
            Self::PlaceOnGround {
                tries,
                radius,
                height,
                provider,
            } => {
                place_on_ground(run, lists, (*tries, *radius, *height), provider)?;
            }
        }
        Ok(())
    }
}

/// A vine on `p` attached to the face pointing at `face`.
fn place_vine(run: &mut Run<'_, '_, '_, '_>, vine: &BlockState, p: BlockPos, face: Dir) {
    run.set(
        Layer::Decoration,
        p,
        with_property(vine.clone(), face.name(), "true"),
    );
}

/// `AlterGroundDecorator.placeCircle`.
fn place_circle(
    run: &mut Run<'_, '_, '_, '_>,
    provider: &StateProvider,
    center: BlockPos,
) -> Res<()> {
    for xx in -2i32..=2 {
        for zz in -2i32..=2 {
            if xx.abs() != 2 || zz.abs() != 2 {
                let column = off(center, xx, 0, zz);
                for dy in (-3..=2).rev() {
                    let cursor = off(column, 0, dy, 0);
                    if let Some(state) = run.sample_optional(provider, cursor)? {
                        run.set(Layer::Decoration, cursor, state);
                        break;
                    }
                    if !run.is_air(cursor) && dy < 0 {
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}

/// `PaleMossDecorator.addMossHanger`: a column of hanging moss ending in a tip.
fn moss_hanger(run: &mut Run<'_, '_, '_, '_>, tip: &BlockState, mut p: BlockPos) {
    let middle = with_property(tip.clone(), "tip", "false");
    while run.is_air(off(p, 0, -1, 0)) && run.float() >= 0.5 {
        run.set(Layer::Decoration, p, middle.clone());
        p = off(p, 0, -1, 0);
    }
    run.set(Layer::Decoration, p, tip.clone());
}

/// `BeehiveDecorator.place`, with the nest's bees as block-entity SNBT.
fn beehive(run: &mut Run<'_, '_, '_, '_>, lists: &Lists, probability: f32, nest: &BlockState) {
    let (Some(first_log), Some(last_log)) = (lists.logs.first(), lists.logs.last()) else {
        return;
    };
    if run.float() >= probability {
        return;
    }
    let hive_y = match lists.leaves.first() {
        Some(leaf) => (leaf.y - 1).max(first_log.y + 1),
        None => (first_log.y + 1 + run.bounded(3)).min(last_log.y),
    };
    let mut placements: Vec<BlockPos> = lists
        .logs
        .iter()
        .filter(|p| p.y == hive_y)
        .flat_map(|p| [Dir::East, Dir::South, Dir::West].map(|dir| rel(*p, dir, 1)))
        .collect();
    if placements.is_empty() {
        return;
    }
    shuffle(&mut placements, &mut *run.cx.random);
    for p in placements {
        if run.is_air(p) && run.is_air(rel(p, Dir::South, 1)) {
            if run.set(Layer::Decoration, p, nest.clone()) {
                let count = 2 + run.bounded(2);
                let ticks: Vec<i32> = (0..count).map(|_| run.bounded(599)).collect();
                run.cx
                    .region
                    .put_block_entity(p, block_entity::beehive(p, &ticks));
            }
            break;
        }
    }
}

/// `PlaceOnGroundDecorator.place`.
fn place_on_ground(
    run: &mut Run<'_, '_, '_, '_>,
    lists: &Lists,
    (tries, radius, height): (i32, i32, i32),
    provider: &StateProvider,
) -> Res<()> {
    let positions = lists.lowest();
    let Some(&origin) = positions.first() else {
        return Ok(());
    };
    let min_y = origin.y;
    let (mut min_x, mut max_x, mut min_z, mut max_z) = (origin.x, origin.x, origin.z, origin.z);
    for p in positions.iter().filter(|p| p.y == min_y) {
        min_x = min_x.min(p.x);
        max_x = max_x.max(p.x);
        min_z = min_z.min(p.z);
        max_z = max_z.max(p.z);
    }
    let (low_x, high_x) = (min_x.wrapping_sub(radius), max_x.wrapping_add(radius));
    let (low_y, high_y) = (min_y.wrapping_sub(height), min_y.wrapping_add(height));
    let (low_z, high_z) = (min_z.wrapping_sub(radius), max_z.wrapping_add(radius));
    for _ in 0..tries {
        let p = BlockPos::new(
            run.between(low_x, high_x),
            run.between(low_y, high_y),
            run.between(low_z, high_z),
        );
        let above = off(p, 0, 1, 0);
        if !(run.is_air(above) || run.has_vine(above)) {
            continue;
        }
        if !run.fact(p, BlockFact::SolidRender) {
            continue;
        }
        let Some(surface) = run
            .cx
            .region
            .height(HeightmapType::MotionBlockingNoLeaves, p.x, p.z)
        else {
            continue;
        };
        if surface <= above.y {
            let state = run.sample(provider, above)?;
            run.set(Layer::Decoration, above, state);
        }
    }
    Ok(())
}
