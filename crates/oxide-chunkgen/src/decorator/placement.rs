//! Placement modifiers and the chain that runs them.
//!
//! A placed feature is a list of modifiers followed by a feature. Each modifier turns one
//! position into zero or more, and what it draws from the random stream while doing so is part of
//! the world: vanilla runs the chain lazily, so the first position a modifier emits goes through
//! every later modifier and the feature before the modifier is asked about its second. [`run`]
//! reproduces exactly that depth-first order; each modifier computes all of its outputs for one
//! input position at once (vanilla's modifiers all draw eagerly when asked), then the rest of the
//! chain consumes them one at a time.

use std::cell::RefCell;

use anyhow::Result;
use oxide_core::{BlockPos, HeightmapType, RandomSource};
use oxide_datapack::placement::IntProvider;
use oxide_datapack::placement::{HeightmapKind, PlacementModifier, SearchDirection};
use oxide_datapack::HeightProvider;

use super::context::{block_in, FeatureContext};
use super::feature::{place_configured_feature, Feature};
use super::predicate::Predicate;
use super::provider::{next_bounded, sample_height, sample_int};
use super::simplex::info_noise;
use crate::region::StateKey;
use oxide_datapack::BlockFact;

#[derive(Debug, Clone)]
pub enum Modifier {
    Biome,
    Count(IntProvider),
    InSquare,
    HeightRange(HeightProvider),
    Heightmap(HeightmapType),
    RandomOffset {
        xz_spread: IntProvider,
        y_spread: IntProvider,
    },
    Rarity(i32),
    Predicate(Predicate),
    SurfaceWaterDepth(i32),
    SurfaceRelative {
        heightmap: HeightmapType,
        min: i32,
        max: i32,
    },
    EnvironmentScan {
        upwards: bool,
        target: Predicate,
        allowed: Predicate,
        max_steps: i32,
        /// Both predicates are [`Predicate::is_local`]: each step reads its position once.
        local: bool,
    },
    CountOnEveryLayer(IntProvider),
    NoiseThresholdCount {
        level: f64,
        below: i32,
        above: i32,
    },
    NoiseBasedCount {
        ratio: i32,
        factor: f64,
        offset: f64,
    },
    Fixed(Vec<BlockPos>),
}

fn heightmap_type(kind: HeightmapKind) -> HeightmapType {
    match kind {
        HeightmapKind::WorldSurfaceWg => HeightmapType::WorldSurfaceWg,
        HeightmapKind::WorldSurface => HeightmapType::WorldSurface,
        HeightmapKind::OceanFloorWg => HeightmapType::OceanFloorWg,
        HeightmapKind::OceanFloor => HeightmapType::OceanFloor,
        HeightmapKind::MotionBlocking => HeightmapType::MotionBlocking,
        HeightmapKind::MotionBlockingNoLeaves => HeightmapType::MotionBlockingNoLeaves,
    }
}

impl Modifier {
    pub fn compile(ir: &PlacementModifier, facts: &oxide_datapack::BlockFacts) -> Result<Self> {
        Ok(match ir {
            PlacementModifier::Biome => Self::Biome,
            PlacementModifier::Count { count } => Self::Count(count.clone()),
            PlacementModifier::InSquare => Self::InSquare,
            PlacementModifier::HeightRange { height } => Self::HeightRange(height.clone()),
            PlacementModifier::Heightmap { heightmap } => {
                Self::Heightmap(heightmap_type(*heightmap))
            }
            PlacementModifier::RandomOffset {
                xz_spread,
                y_spread,
            } => Self::RandomOffset {
                xz_spread: xz_spread.clone(),
                y_spread: y_spread.clone(),
            },
            PlacementModifier::RarityFilter { chance } => Self::Rarity(*chance),
            PlacementModifier::BlockPredicateFilter { predicate } => {
                Self::Predicate(Predicate::compile(predicate, facts)?)
            }
            PlacementModifier::SurfaceWaterDepthFilter { max_water_depth } => {
                Self::SurfaceWaterDepth(*max_water_depth)
            }
            PlacementModifier::SurfaceRelativeThresholdFilter {
                heightmap,
                min_inclusive,
                max_inclusive,
            } => Self::SurfaceRelative {
                heightmap: heightmap_type(*heightmap),
                min: *min_inclusive,
                max: *max_inclusive,
            },
            PlacementModifier::EnvironmentScan {
                direction_of_search,
                target_condition,
                allowed_search_condition,
                max_steps,
            } => {
                let target = Predicate::compile(target_condition, facts)?;
                let allowed = Predicate::compile(allowed_search_condition, facts)?;
                Self::EnvironmentScan {
                    upwards: matches!(direction_of_search, SearchDirection::Up),
                    local: target.is_local() && allowed.is_local(),
                    target,
                    allowed,
                    max_steps: *max_steps,
                }
            }
            PlacementModifier::CountOnEveryLayer { count } => {
                Self::CountOnEveryLayer(count.clone())
            }
            PlacementModifier::NoiseThresholdCount {
                noise_level,
                below_noise,
                above_noise,
            } => Self::NoiseThresholdCount {
                level: *noise_level,
                below: *below_noise,
                above: *above_noise,
            },
            PlacementModifier::NoiseBasedCount {
                noise_to_count_ratio,
                noise_factor,
                noise_offset,
            } => Self::NoiseBasedCount {
                ratio: *noise_to_count_ratio,
                factor: *noise_factor,
                offset: *noise_offset,
            },
            PlacementModifier::FixedPlacement { positions } => Self::Fixed(
                positions
                    .iter()
                    .map(|p| BlockPos::new(p[0], p[1], p[2]))
                    .collect(),
            ),
        })
    }

    /// `PlacementModifier.getPositions`: appends the positions this modifier emits for `pos`.
    pub fn positions(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        pos: BlockPos,
        out: &mut Vec<BlockPos>,
    ) {
        match self {
            Self::Biome => {
                let Some(key) = cx.top_feature else {
                    // Vanilla throws here: a biome check inside a nested feature is a data error.
                    cx.unsupported
                        .record("placement", "biome modifier without a top feature");
                    return;
                };
                if let Some(biome) = cx.region.biome(pos) {
                    if cx.env.biome_lists(biome, key) {
                        out.push(pos);
                    }
                }
            }
            Self::Count(count) => {
                for _ in 0..sample_int(count, cx.random) {
                    out.push(pos);
                }
            }
            Self::InSquare => {
                let x = pos.x.wrapping_add(next_bounded(cx.random, 16));
                let z = pos.z.wrapping_add(next_bounded(cx.random, 16));
                out.push(BlockPos::new(x, pos.y, z));
            }
            Self::HeightRange(height) => {
                let bounds = cx.bounds();
                out.push(BlockPos::new(
                    pos.x,
                    sample_height(height, cx.random, bounds),
                    pos.z,
                ));
            }
            Self::Heightmap(kind) => {
                if let Some(y) = cx.region.height(*kind, pos.x, pos.z) {
                    if y > cx.region.min_y() {
                        out.push(BlockPos::new(pos.x, y, pos.z));
                    }
                }
            }
            // The draws are x, then y, then z, with the horizontal provider sampled twice.
            Self::RandomOffset {
                xz_spread,
                y_spread,
            } => {
                let x = pos.x.wrapping_add(sample_int(xz_spread, cx.random));
                let y = pos.y.wrapping_add(sample_int(y_spread, cx.random));
                let z = pos.z.wrapping_add(sample_int(xz_spread, cx.random));
                out.push(BlockPos::new(x, y, z));
            }
            Self::Rarity(chance) => {
                if cx.random.next_float() < 1.0 / *chance as f32 {
                    out.push(pos);
                }
            }
            Self::Predicate(predicate) => {
                if predicate.test(cx, pos) {
                    out.push(pos);
                }
            }
            Self::SurfaceWaterDepth(max) => {
                let floor = cx.region.height(HeightmapType::OceanFloor, pos.x, pos.z);
                let surface = cx.region.height(HeightmapType::WorldSurface, pos.x, pos.z);
                if let (Some(floor), Some(surface)) = (floor, surface) {
                    if surface.wrapping_sub(floor) <= *max {
                        out.push(pos);
                    }
                }
            }
            Self::SurfaceRelative {
                heightmap,
                min,
                max,
            } => {
                if let Some(height) = cx.region.height(*heightmap, pos.x, pos.z) {
                    let height = i64::from(height);
                    let y = i64::from(pos.y);
                    if height + i64::from(*min) <= y && y <= height + i64::from(*max) {
                        out.push(pos);
                    }
                }
            }
            Self::EnvironmentScan {
                upwards,
                target,
                allowed,
                max_steps,
                local,
            } => {
                let found = if *local {
                    scan_local(cx, pos, *upwards, target, allowed, *max_steps)
                } else {
                    scan(cx, pos, *upwards, target, allowed, *max_steps)
                };
                if let Some(found) = found {
                    out.push(found);
                }
            }
            Self::CountOnEveryLayer(count) => on_every_layer(cx, pos, count, out),
            Self::NoiseThresholdCount {
                level,
                below,
                above,
            } => {
                // PARITY-CHECK: `info_noise` is a port without a diffable reference.
                let noise = info_noise(f64::from(pos.x) / 200.0, f64::from(pos.z) / 200.0);
                let count = if noise < *level { *below } else { *above };
                for _ in 0..count {
                    out.push(pos);
                }
            }
            Self::NoiseBasedCount {
                ratio,
                factor,
                offset,
            } => {
                let noise = info_noise(f64::from(pos.x) / factor, f64::from(pos.z) / factor);
                let count = ((noise + offset) * f64::from(*ratio)).ceil() as i32;
                for _ in 0..count {
                    out.push(pos);
                }
            }
            Self::Fixed(positions) => {
                out.extend(
                    positions
                        .iter()
                        .copied()
                        .filter(|p| p.x >> 4 == pos.x >> 4 && p.z >> 4 == pos.z >> 4),
                );
            }
        }
    }
}

/// `EnvironmentScanPlacement`: walk while `allowed` holds, stopping at the first `target`.
fn scan(
    cx: &mut FeatureContext<'_, '_, '_>,
    start: BlockPos,
    upwards: bool,
    target: &Predicate,
    allowed: &Predicate,
    max_steps: i32,
) -> Option<BlockPos> {
    let mut pos = start;
    if !allowed.test(cx, pos) {
        return None;
    }
    for _ in 0..max_steps {
        if target.test(cx, pos) {
            return Some(pos);
        }
        pos.y = pos.y.wrapping_add(if upwards { 1 } else { -1 });
        if cx.outside_build_height(pos.y) {
            return None;
        }
        if !allowed.test(cx, pos) {
            break;
        }
    }
    target.test(cx, pos).then_some(pos)
}

/// Answers already worked out during one scan: `(key, allowed, target)` per [`StateKey`], in a
/// few direct-mapped slots. The predicates are local, so their answers depend on the state alone
/// -- except `inside_world_bounds`, which a scan never needs at a position outside the world.
type ScanCache = [Option<(StateKey, bool, bool)>; 4];

/// `allowed` and `target` at `pos`, both local, from one read of the position.
fn test_both(
    cx: &mut FeatureContext<'_, '_, '_>,
    pos: BlockPos,
    allowed: &Predicate,
    target: &Predicate,
    cache: &mut ScanCache,
) -> (bool, bool) {
    let (min_y, max_y) = (cx.region.min_y(), cx.region.max_y());
    match cx.region.probe(pos) {
        Some(probe) => {
            let slot = &mut cache[probe.key.slot() & 3];
            if let Some((key, ok, hit)) = *slot {
                if key == probe.key {
                    return (ok, hit);
                }
            }
            let ok = allowed.test_probe(cx.env, &probe, pos, min_y, max_y);
            let hit = target.test_probe(cx.env, &probe, pos, min_y, max_y);
            *slot = Some((probe.key, ok, hit));
            (ok, hit)
        }
        None => (allowed.test(cx, pos), target.test(cx, pos)),
    }
}

/// [`scan`] for local predicates. They are pure in the position's state and never tally, so
/// testing both at each position, from one read, gives the same answers in the same places.
fn scan_local(
    cx: &mut FeatureContext<'_, '_, '_>,
    start: BlockPos,
    upwards: bool,
    target: &Predicate,
    allowed: &Predicate,
    max_steps: i32,
) -> Option<BlockPos> {
    let mut cache: ScanCache = [None; 4];
    let mut pos = start;
    let (ok, mut hit) = test_both(cx, pos, allowed, target, &mut cache);
    if !ok {
        return None;
    }
    for _ in 0..max_steps {
        if hit {
            return Some(pos);
        }
        pos.y = pos.y.wrapping_add(if upwards { 1 } else { -1 });
        if cx.outside_build_height(pos.y) {
            return None;
        }
        let (ok, next_hit) = test_both(cx, pos, allowed, target, &mut cache);
        hit = next_hit;
        if !ok {
            break;
        }
    }
    hit.then_some(pos)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layer {
    Empty,
    Bedrock,
    Ground,
}

fn layer_of(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> Layer {
    let state = block_in(cx.region, pos);
    let named = |path: &str| state.name.namespace() == "minecraft" && state.name.path() == path;
    if cx.env.facts.is(state, BlockFact::Air) || named("water") || named("lava") {
        Layer::Empty
    } else if named("bedrock") {
        Layer::Bedrock
    } else {
        Layer::Ground
    }
}

/// `CountOnEveryLayerPlacement.findOnGroundYPosition`: the `layer`th ground-under-air Y going
/// down from `pos`, counting from the top.
fn ground_y(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos, layer: i32) -> Option<i32> {
    let mut above = layer_of(cx, pos);
    let mut found = 0i32;
    let mut y = pos.y;
    while y >= cx.region.min_y().wrapping_add(1) {
        let below = layer_of(cx, BlockPos::new(pos.x, y.wrapping_sub(1), pos.z));
        if below == Layer::Ground && above == Layer::Empty {
            if found == layer {
                return Some(y);
            }
            found = found.wrapping_add(1);
        }
        above = below;
        y = y.wrapping_sub(1);
    }
    None
}

/// `CountOnEveryLayerPlacement.getPositions`. The count provider is sampled on every loop
/// condition check, including the final failing one, so the stream advances once more than the
/// attempts that run.
fn on_every_layer(
    cx: &mut FeatureContext<'_, '_, '_>,
    pos: BlockPos,
    count: &IntProvider,
    out: &mut Vec<BlockPos>,
) {
    let mut layer = 0i32;
    loop {
        let mut found = false;
        let mut attempt = 0i32;
        while attempt < sample_int(count, cx.random) {
            let x = pos.x.wrapping_add(next_bounded(cx.random, 16));
            let z = pos.z.wrapping_add(next_bounded(cx.random, 16));
            let Some(top) = cx.region.height(HeightmapType::MotionBlocking, x, z) else {
                attempt = attempt.wrapping_add(1);
                continue;
            };
            if let Some(y) = ground_y(cx, BlockPos::new(x, top, z), layer) {
                out.push(BlockPos::new(x, y, z));
                found = true;
            }
            attempt = attempt.wrapping_add(1);
        }
        layer = layer.wrapping_add(1);
        if !found {
            return;
        }
    }
}

/// The constant an int provider always answers without drawing, if it is one.
fn constant_int(provider: &IntProvider) -> Option<i32> {
    match provider {
        IntProvider::Constant(value) => Some(*value),
        IntProvider::Tagged(tagged) => match &**tagged {
            oxide_datapack::placement::TaggedIntProvider::Constant { value } => Some(*value),
            _ => None,
        },
    }
}

impl Modifier {
    /// Whether the modifier emits at most one position per input, in the input's column, with no
    /// random draw and no effect besides its answer; if so, `Some(span)` where `span` bounds the
    /// emitted y relative to the input's (`None` when it is not bounded).
    fn quiet_span(&self) -> Option<Option<(i32, i32)>> {
        match self {
            Self::EnvironmentScan {
                target,
                allowed,
                upwards,
                max_steps,
                ..
            } => (target.is_quiet() && allowed.is_quiet()).then(|| {
                let steps = (*max_steps).max(0);
                Some(if *upwards { (0, steps) } else { (-steps, 0) })
            }),
            Self::RandomOffset {
                xz_spread,
                y_spread,
            } => match (constant_int(xz_spread), constant_int(y_spread)) {
                (Some(0), Some(dy)) => Some(Some((dy, dy))),
                _ => None,
            },
            Self::Predicate(predicate) => predicate.is_quiet().then_some(Some((0, 0))),
            Self::Heightmap(_) | Self::SurfaceWaterDepth(_) | Self::SurfaceRelative { .. } => {
                Some(None)
            }
            _ => None,
        }
    }
}

/// Where a chain may drop a position early because the biome filter later on would drop it:
/// the index of the first modifier of a run of [`Modifier::keeps_column_quietly`] modifiers
/// that ends in a `biome` modifier. Such a run neither draws nor leaves its column, so a
/// position entering it whose column holds no biome that lists the feature cannot pass the
/// filter, and skipping the run for it changes nothing but the time spent.
pub fn biome_gate(modifiers: &[Modifier]) -> Option<(usize, Option<(i32, i32)>)> {
    let biome = modifiers.iter().position(|m| matches!(m, Modifier::Biome))?;
    let mut start = biome;
    let mut span = Some((0i32, 0i32));
    while start > 0 {
        let Some(step) = modifiers[start - 1].quiet_span() else {
            break;
        };
        span = match (span, step) {
            (Some((lo, hi)), Some((a, b))) => Some((lo.saturating_add(a), hi.saturating_add(b))),
            _ => None,
        };
        start -= 1;
    }
    (start < biome).then_some((start, span))
}

/// A [`biome_gate`] resolved for one run: positions entering the chain with `remaining`
/// modifiers left are dropped when they are in the center chunk and their 4x4 column's bit in
/// `columns` (see `DecorationRegion::center_biome_columns`) is clear.
pub struct Gate {
    pub remaining: usize,
    /// Every level's column bits ORed: the answer when `span` is unbounded.
    pub columns: u16,
    /// Column bits per quart level from the bottom of the world (`center_biome_levels`).
    pub levels: Vec<u16>,
    /// Bounds on the y the biome filter will see, relative to the entering position.
    pub span: Option<(i32, i32)>,
    pub min_y: i32,
}

impl Gate {
    /// Whether a center-chunk position entering the gated run can still pass the filter: some
    /// quart it can reach (clamped into the world, as `DecorationRegion::biome` clamps) has a
    /// biome that lists the feature in its 4x4 column.
    fn may_pass(&self, pos: BlockPos) -> bool {
        let bit = 1u16 << (((pos.z >> 2) & 3) * 4 + ((pos.x >> 2) & 3));
        let Some((lo, hi)) = self.span else {
            return self.columns & bit != 0;
        };
        if self.columns & bit == 0 {
            return false;
        }
        let top = self.levels.len() as i32 - 1;
        let quart = |y: i32| ((y >> 2) - (self.min_y >> 2)).clamp(0, top) as usize;
        let (a, b) = (quart(pos.y.saturating_add(lo)), quart(pos.y.saturating_add(hi)));
        self.levels[a..=b].iter().any(|level| level & bit != 0)
    }
}

/// Runs `modifiers` from `pos`, then `feature` at every position that comes out, depth first.
/// Returns whether any placement reported success.
pub fn run(
    modifiers: &[Modifier],
    cx: &mut FeatureContext<'_, '_, '_>,
    pos: BlockPos,
    feature: &Feature,
    gate: Option<&Gate>,
) -> bool {
    if let Some(gate) = gate {
        if modifiers.len() == gate.remaining {
            let center = cx.region.center();
            if pos.x >> 4 == center.x && pos.z >> 4 == center.z && !gate.may_pass(pos) {
                return false;
            }
        }
    }
    let Some((modifier, rest)) = modifiers.split_first() else {
        cx.origin = pos;
        return place_configured_feature(feature, cx);
    };
    let mut emitted = SPARE_POSITIONS
        .with(|spare| spare.borrow_mut().pop())
        .unwrap_or_default();
    modifier.positions(cx, pos, &mut emitted);
    let mut placed = false;
    for &next in &emitted {
        placed |= run(rest, cx, next, feature, gate);
    }
    emitted.clear();
    SPARE_POSITIONS.with(|spare| spare.borrow_mut().push(emitted));
    placed
}

thread_local! {
    /// Emptied position lists for [`run`] to fill, one per chain level in use: every level of
    /// every chain used to allocate (and grow, and free) a fresh list.
    static SPARE_POSITIONS: RefCell<Vec<Vec<BlockPos>>> = const { RefCell::new(Vec::new()) };
}
