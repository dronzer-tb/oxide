//! `minecraft:tree` and `minecraft:fallen_tree`: trunk, foliage and root placers, tree
//! decorators, the free-height check and the leaf-distance pass.
//!
//! A tree is built by drawing its height, foliage height and radius, checking the column is
//! free (clipping the tree to what fits), placing roots and trunk, hanging foliage off every
//! attachment point the trunk returned, running the decorators, and finally recomputing the
//! `distance` property of every leaf from the logs it can reach. Every one of those steps draws
//! from the shared random stream, so the order here is vanilla's, not a tidier one.
//!
//! The pieces live beside this file: [`trunk`], [`foliage`], [`root`], [`decorator`],
//! [`fallen`], and [`pos_set`] for the Java `HashSet` iteration order the draws depend on.
//!
//! # What is not reproduced
//!
//! - Vanilla's neighbour shape updates at the tree's outer faces (`updateShapeAtEdge`). The region
//!   stores block states only, and nothing in a freshly generated tree depends on them.
//! - Bee nest block-entity data (`BeehiveDecorator` stores two or three bees). The nest block and
//!   every random draw are placed; the occupants are not, since the region carries no block
//!   entities. The draws are still made so the stream stays aligned.
//! - A tree whose providers cannot answer ([`Unsupported`]) stops where it is: the blocks already
//!   written stay, and the gap is tallied under `provider:`. No 26.2 tree uses such a provider.
//!
//! A tree with a component this generator cannot complete (a decorator block the exported facts
//! do not know, a missing ground patch) places nothing, draws nothing, and is tallied under
//! `tree:<component>`.

mod decorator;
mod fallen;
mod foliage;
mod pos_set;
mod root;
mod trunk;

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, RandomSource, ResourceLocation, Xoroshiro128PlusPlus};
use oxide_datapack::tree::{FeatureSize, TreeConfig, TrunkKind};
use oxide_datapack::{BlockFact, BlockFacts};

pub use fallen::FallenTreeFeature;

use self::decorator::Decorator;
use self::pos_set::PosSet;
use self::root::Root;
use super::context::{block_in, Environment, FeatureContext};
use super::feature::Feature;
use super::predicate::IdSet;
use super::provider::{between_inclusive, next_bounded, sample_int};
use super::state_provider::{StateProvider, Unsupported};

pub(super) type Res<T> = Result<T, Unsupported>;

/// `Direction`, in `Direction.values()` order.
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
    pub const ALL: [Dir; 6] = [Dir::Down, Dir::Up, Dir::North, Dir::South, Dir::West, Dir::East];
    /// `Direction.Plane.HORIZONTAL`.
    pub const HORIZONTAL: [Dir; 4] = [Dir::North, Dir::East, Dir::South, Dir::West];

    pub fn normal(self) -> [i32; 3] {
        match self {
            Dir::Down => [0, -1, 0],
            Dir::Up => [0, 1, 0],
            Dir::North => [0, 0, -1],
            Dir::South => [0, 0, 1],
            Dir::West => [-1, 0, 0],
            Dir::East => [1, 0, 0],
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

    pub fn clockwise(self) -> Dir {
        match self {
            Dir::North => Dir::East,
            Dir::East => Dir::South,
            Dir::South => Dir::West,
            Dir::West => Dir::North,
            other => other,
        }
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

    pub fn parse(name: &str) -> Option<Dir> {
        Dir::ALL.into_iter().find(|dir| dir.name() == name)
    }

    pub fn axis(self) -> Axis {
        match self {
            Dir::West | Dir::East => Axis::X,
            Dir::Down | Dir::Up => Axis::Y,
            Dir::North | Dir::South => Axis::Z,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    fn name(self) -> &'static str {
        match self {
            Axis::X => "x",
            Axis::Y => "y",
            Axis::Z => "z",
        }
    }
}

pub(super) fn off(p: BlockPos, dx: i32, dy: i32, dz: i32) -> BlockPos {
    BlockPos::new(p.x.wrapping_add(dx), p.y.wrapping_add(dy), p.z.wrapping_add(dz))
}

pub(super) fn rel(p: BlockPos, dir: Dir, distance: i32) -> BlockPos {
    let n = dir.normal();
    off(p, n[0].wrapping_mul(distance), n[1].wrapping_mul(distance), n[2].wrapping_mul(distance))
}

/// `Vec3i.distManhattan`, including its float accumulation.
pub(super) fn dist_manhattan(a: BlockPos, b: BlockPos) -> i32 {
    let d = |from: i32, to: i32| to.wrapping_sub(from).wrapping_abs() as f32;
    (d(a.x, b.x) + d(a.y, b.y) + d(a.z, b.z)) as i32
}

/// `Util.shuffle`: Fisher-Yates from the end with `nextInt(i)`.
pub(super) fn shuffle<T>(values: &mut [T], random: &mut Xoroshiro128PlusPlus) {
    for i in (2..=values.len()).rev() {
        let swap = next_bounded(random, i as i32) as usize;
        values.swap(i - 1, swap);
    }
}

pub(super) fn with_property(mut state: BlockState, name: &str, value: &str) -> BlockState {
    state.properties.insert(name.to_owned(), value.to_owned());
    state
}

/// Which of a tree's position sets a write belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Layer {
    Root,
    Trunk,
    Foliage,
    Decoration,
}

#[derive(Default)]
struct Sets {
    roots: PosSet,
    trunks: PosSet,
    foliage: PosSet,
    decorations: PosSet,
}

/// The block tags a tree reads, looked up once per tree instead of once per block.
struct Tags<'e> {
    leaves: Option<&'e HashSet<ResourceLocation>>,
    logs: Option<&'e HashSet<ResourceLocation>>,
    replaceable_by_trees: Option<&'e HashSet<ResourceLocation>>,
    prevents_leaf_decay: Option<&'e HashSet<ResourceLocation>>,
}

impl<'e> Tags<'e> {
    fn new(env: &'e Environment) -> Self {
        let tag = |path: &str| env.block_tags.get(&ResourceLocation::minecraft(path));
        Self {
            leaves: tag("leaves"),
            logs: tag("logs"),
            replaceable_by_trees: tag("replaceable_by_trees"),
            prevents_leaf_decay: tag("prevents_nearby_leaf_decay"),
        }
    }
}

fn in_tag(tag: Option<&HashSet<ResourceLocation>>, state: &BlockState) -> bool {
    tag.is_some_and(|members| members.contains(&state.name))
}

/// One tree being placed: the context, the position sets vanilla accumulates, and the tags.
pub(super) struct Run<'c, 'e, 'a, 'r> {
    pub cx: &'c mut FeatureContext<'e, 'a, 'r>,
    tags: Tags<'e>,
    sets: Sets,
    /// Fallen trees write decorations without recording them.
    track: bool,
}

impl<'c, 'e, 'a, 'r> Run<'c, 'e, 'a, 'r> {
    pub fn new(cx: &'c mut FeatureContext<'e, 'a, 'r>, track: bool) -> Self {
        let tags = Tags::new(cx.env);
        Self { cx, tags, sets: Sets::default(), track }
    }

    pub fn state(&self, p: BlockPos) -> &BlockState {
        block_in(self.cx.region, p)
    }

    pub fn fact(&self, p: BlockPos, fact: BlockFact) -> bool {
        self.cx.env.facts.is(self.state(p), fact)
    }

    pub fn is_air(&self, p: BlockPos) -> bool {
        self.fact(p, BlockFact::Air)
    }

    pub fn is_log(&self, p: BlockPos) -> bool {
        in_tag(self.tags.logs, self.state(p))
    }

    pub fn is_leaves(&self, p: BlockPos) -> bool {
        in_tag(self.tags.leaves, self.state(p))
    }

    /// `TreeFeature.validTreePos`: air, or replaceable by trees.
    pub fn valid_tree_pos(&self, p: BlockPos) -> bool {
        self.is_air(p) || in_tag(self.tags.replaceable_by_trees, self.state(p))
    }

    /// Writes `state`, recording `p` in the layer's set. Returns whether the region took it.
    pub fn set(&mut self, layer: Layer, p: BlockPos, state: BlockState) -> bool {
        if self.track {
            let sets = &mut self.sets;
            match layer {
                Layer::Root => sets.roots.insert(p),
                Layer::Trunk => sets.trunks.insert(p),
                Layer::Foliage => sets.foliage.insert(p),
                Layer::Decoration => sets.decorations.insert(p),
            };
        }
        self.cx.region.set_block(p, state)
    }

    pub fn sample(&mut self, provider: &StateProvider, p: BlockPos) -> Res<BlockState> {
        provider
            .state(self.cx, p)?
            .ok_or(Unsupported("state provider produced no state"))
    }

    pub fn sample_optional(&mut self, provider: &StateProvider, p: BlockPos) -> Res<Option<BlockState>> {
        provider.state(self.cx, p)
    }

    pub fn int(&mut self, provider: &oxide_datapack::placement::IntProvider) -> i32 {
        sample_int(provider, &mut *self.cx.random)
    }

    pub fn bounded(&mut self, bound: i32) -> i32 {
        next_bounded(&mut *self.cx.random, bound)
    }

    pub fn between(&mut self, min: i32, max: i32) -> i32 {
        between_inclusive(&mut *self.cx.random, min, max)
    }

    pub fn float(&mut self) -> f32 {
        self.cx.random.next_float()
    }

    pub fn boolean(&mut self) -> bool {
        self.cx.random.next_boolean()
    }

    pub fn horizontal(&mut self) -> Dir {
        Dir::HORIZONTAL[self.bounded(4) as usize]
    }

    /// `BlockState.trySetValue(RotatedPillarBlock.AXIS, axis)`.
    pub fn with_axis(&self, state: BlockState, axis: Axis) -> BlockState {
        if state.properties.contains_key("axis") {
            with_property(state, "axis", axis.name())
        } else {
            state
        }
    }

    /// Whether a foliage write has already recorded `p`.
    pub fn foliage_contains(&self, p: BlockPos) -> bool {
        self.sets.foliage.contains(p)
    }

    /// Whether the block at `p` is `minecraft:vine`.
    pub fn has_vine(&self, p: BlockPos) -> bool {
        self.state(p).name == ResourceLocation::minecraft("vine")
    }
}

/// `TreeFeature`, compiled.
#[derive(Debug)]
pub struct TreeFeature {
    config: TreeConfig,
    trunk: StateProvider,
    foliage: StateProvider,
    below_trunk: StateProvider,
    /// The upwards-branching trunk placer's `can_grow_through`.
    grow_through: Option<IdSet>,
    root: Option<Root>,
    decorators: Vec<Decorator>,
    /// A component this generator cannot complete; the tree then places nothing.
    unsupported: Option<String>,
}

/// Compiles a configured feature by registry id; how the pale-moss decorator finds its patch.
pub type FeatureLookup<'a> = dyn FnMut(&ResourceLocation) -> Result<Option<Arc<Feature>>> + 'a;

impl TreeFeature {
    pub fn compile(config: &TreeConfig, facts: &BlockFacts, lookup: &mut FeatureLookup<'_>) -> Result<Self> {
        let grow_through = match &config.trunk_placer.kind {
            TrunkKind::UpwardsBranching { can_grow_through, .. } => Some(IdSet::compile(can_grow_through)?),
            _ => None,
        };
        let (decorators, unsupported) = Decorator::compile_list(&config.decorators, facts, lookup)?;
        Ok(Self {
            config: config.clone(),
            trunk: StateProvider::compile(&config.trunk_provider, facts)?,
            foliage: StateProvider::compile(&config.foliage_provider, facts)?,
            below_trunk: StateProvider::compile(&config.below_trunk_provider, facts)?,
            grow_through,
            root: config.root_placer.as_ref().map(|root| Root::compile(root, facts)).transpose()?,
            decorators,
            unsupported,
        })
    }

    /// `TreeFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        if let Some(component) = &self.unsupported {
            cx.unsupported.record("tree", component);
            return false;
        }
        let origin = cx.origin;
        let mut run = Run::new(cx, true);
        match self.place_tree(&mut run, origin) {
            Ok(placed) => placed,
            Err(Unsupported(why)) => {
                run.cx.unsupported.record("provider", why);
                false
            }
        }
    }

    fn place_tree(&self, run: &mut Run<'_, '_, '_, '_>, origin: BlockPos) -> Res<bool> {
        if !self.do_place(run, origin)? || (run.sets.trunks.is_empty() && run.sets.foliage.is_empty()) {
            return Ok(false);
        }
        if !self.decorators.is_empty() {
            let lists = decorator::Lists::new(
                run.sets.trunks.to_vec(),
                run.sets.foliage.to_vec(),
                run.sets.roots.to_vec(),
            );
            for decorator in &self.decorators {
                decorator.place(run, &lists)?;
            }
        }
        let mut bounds: Option<(BlockPos, BlockPos)> = None;
        let sets = &run.sets;
        for set in [&sets.roots, &sets.trunks, &sets.foliage, &sets.decorations] {
            for p in set.to_vec() {
                bounds = Some(match bounds {
                    None => (p, p),
                    Some((lo, hi)) => (
                        BlockPos::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z)),
                        BlockPos::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z)),
                    ),
                });
            }
        }
        let Some((low, high)) = bounds else {
            return Ok(false);
        };
        update_leaves(run, low, high);
        Ok(true)
    }

    fn do_place(&self, run: &mut Run<'_, '_, '_, '_>, origin: BlockPos) -> Res<bool> {
        let trunk_placer = &self.config.trunk_placer;
        let a = run.bounded(trunk_placer.height_rand_a + 1);
        let b = run.bounded(trunk_placer.height_rand_b + 1);
        let tree_height = trunk_placer.base_height + a + b;
        let foliage_height = foliage::foliage_height(&self.config.foliage_placer, run, tree_height);
        let trunk_height = tree_height.wrapping_sub(foliage_height);
        let leaf_radius = foliage::foliage_radius(&self.config.foliage_placer, run, trunk_height);
        let trunk_origin = match &self.root {
            Some(root) => off(origin, 0, run.int(&root.trunk_offset_y), 0),
            None => origin,
        };
        let min_y = origin.y.min(trunk_origin.y);
        let max_y = origin.y.max(trunk_origin.y).wrapping_add(tree_height).wrapping_add(1);
        // PARITY-CHECK: vanilla compares against `getMaxY() + 1`, which is the exclusive top of
        // the world; `max_y()` here is that exclusive top already.
        if min_y < run.cx.region.min_y().wrapping_add(1) || max_y > run.cx.region.max_y() {
            return Ok(false);
        }
        let clipped = self.max_free_tree_height(run, tree_height, trunk_origin);
        if clipped < tree_height
            && self.config.minimum_size.min_clipped_height().is_none_or(|min| clipped < min)
        {
            return Ok(false);
        }
        if let Some(root) = &self.root {
            if !root.place_roots(run, origin, trunk_origin)? {
                return Ok(false);
            }
        }
        let attachments = trunk::place_trunk(self, run, clipped, trunk_origin)?;
        for attachment in attachments {
            foliage::create_foliage(self, run, attachment, foliage_height, leaf_radius)?;
        }
        Ok(true)
    }

    /// `FeatureSize.getSizeAtHeight`.
    fn size_at_height(&self, tree_height: i32, y: i32) -> i32 {
        match &self.config.minimum_size {
            FeatureSize::TwoLayers { limit, lower_size, upper_size, .. } => {
                if y < *limit { *lower_size } else { *upper_size }
            }
            FeatureSize::ThreeLayers { limit, upper_limit, lower_size, middle_size, upper_size, .. } => {
                if y < *limit {
                    *lower_size
                } else if y >= tree_height.wrapping_sub(*upper_limit) {
                    *upper_size
                } else {
                    *middle_size
                }
            }
        }
    }

    /// `TreeFeature.getMaxFreeTreeHeight`: how tall the tree can grow before something blocks
    /// its column, as the height to clip it to.
    fn max_free_tree_height(&self, run: &Run<'_, '_, '_, '_>, max_tree_height: i32, tree_pos: BlockPos) -> i32 {
        for y in 0..=max_tree_height.wrapping_add(1) {
            let radius = self.size_at_height(max_tree_height, y);
            for x in -radius..=radius {
                for z in -radius..=radius {
                    let p = off(tree_pos, x, y, z);
                    if !trunk::is_free(self, run, p) || (!self.config.ignore_vines && run.has_vine(p)) {
                        return y - 2;
                    }
                }
            }
        }
        max_tree_height
    }

    /// `FoliagePlacer.tryPlaceLeaf`.
    fn try_place_leaf(&self, run: &mut Run<'_, '_, '_, '_>, p: BlockPos) -> Res<bool> {
        let here = run.state(p);
        if here.properties.get("persistent").is_some_and(|value| value == "true") || !run.valid_tree_pos(p) {
            return Ok(false);
        }
        let mut state = run.sample(&self.foliage, p)?;
        if state.properties.contains_key("waterlogged") {
            let source = run.fact(p, BlockFact::Water) && run.fact(p, BlockFact::FluidSource);
            state = with_property(state, "waterlogged", if source { "true" } else { "false" });
        }
        run.set(Layer::Foliage, p, state);
        Ok(true)
    }
}

/// The span a tree can have before the distance pass declines to build its voxel grid. A real
/// tree is a few dozen blocks wide; this only guards against a malformed config.
const MAX_TREE_SPAN: i64 = 512;

/// Voxel occupancy over the tree's bounding box.
struct Shape {
    size: [i32; 3],
    bits: Vec<bool>,
}

impl Shape {
    fn new(size: [i32; 3]) -> Self {
        Self { size, bits: vec![false; size.iter().map(|&v| v as usize).product()] }
    }

    fn index(&self, p: [i32; 3]) -> usize {
        ((p[0] * self.size[1] + p[1]) * self.size[2] + p[2]) as usize
    }

    fn fill(&mut self, p: [i32; 3]) {
        let index = self.index(p);
        self.bits[index] = true;
    }

    fn full(&self, p: [i32; 3]) -> bool {
        p.iter().zip(self.size).all(|(v, size)| (0..size).contains(v)) && self.bits[self.index(p)]
    }
}

fn inside(low: BlockPos, high: BlockPos, p: BlockPos) -> bool {
    p.x >= low.x && p.x <= high.x && p.y >= low.y && p.y <= high.y && p.z >= low.z && p.z <= high.z
}

/// `LeavesBlock.getOptionalDistanceAt`: 0 for a block that holds leaves up, the `distance`
/// property for a leaf, nothing for anything else.
fn leaf_distance(run: &Run<'_, '_, '_, '_>, p: BlockPos) -> Option<i32> {
    let state = run.state(p);
    if in_tag(run.tags.prevents_leaf_decay, state) {
        return Some(0);
    }
    state.properties.get("distance")?.parse().ok()
}

/// `TreeFeature.updateLeaves`: a breadth-first pass, bucketed by distance, that sets each leaf's
/// `distance` to its steps from the nearest log.
fn update_leaves(run: &mut Run<'_, '_, '_, '_>, low: BlockPos, high: BlockPos) {
    let span = [
        i64::from(high.x) - i64::from(low.x) + 1,
        i64::from(high.y) - i64::from(low.y) + 1,
        i64::from(high.z) - i64::from(low.z) + 1,
    ];
    if span.iter().any(|&s| s > MAX_TREE_SPAN) {
        return;
    }
    let mut shape = Shape::new(span.map(|v| v as i32));
    let local = |p: BlockPos| [p.x.wrapping_sub(low.x), p.y.wrapping_sub(low.y), p.z.wrapping_sub(low.z)];
    let mut fixed = run.sets.decorations.to_vec();
    fixed.extend(run.sets.roots.to_vec());
    for p in fixed {
        if inside(low, high, p) {
            shape.fill(local(p));
        }
    }
    let mut to_check: [PosSet; 7] = Default::default();
    for p in run.sets.trunks.to_vec() {
        to_check[0].insert(p);
    }
    let mut smallest = 0usize;
    loop {
        while smallest >= 7 || !to_check[smallest].is_empty() {
            if smallest >= 7 {
                return;
            }
            let Some(pos) = to_check[smallest].pop_first() else {
                break;
            };
            if !inside(low, high, pos) {
                continue;
            }
            if smallest != 0 {
                let updated = with_property(run.state(pos).clone(), "distance", &smallest.to_string());
                run.cx.region.set_block(pos, updated);
            }
            shape.fill(local(pos));
            for dir in Dir::ALL {
                let neighbor = rel(pos, dir, 1);
                if !inside(low, high, neighbor) || shape.full(local(neighbor)) {
                    continue;
                }
                if let Some(distance) = leaf_distance(run, neighbor) {
                    let next = distance.min(smallest as i32 + 1);
                    if next < 7 {
                        to_check[next as usize].insert(neighbor);
                        smallest = smallest.min(next as usize);
                    }
                }
            }
        }
        smallest += 1;
    }
}

#[cfg(test)]
mod tests;
