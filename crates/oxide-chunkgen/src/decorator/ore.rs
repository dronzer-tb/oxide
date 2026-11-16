//! `ore` and `scattered_ore`: the two ore feature types.
//!
//! `ore` is the blob: `size` spheres strung along a line with a random tilt, replacing every
//! target block inside. `scattered_ore` drops up to `size` single blocks around the origin and
//! is what the tuff-style one-block veins use. Both share the replacement rules and the
//! air-exposure discard, ported from vanilla's `OreFeature`.
//!
//! The sphere maths is a straight port: draw order, float versus double, and the quirky bitset
//! index (it can collide, and vanilla's collisions are part of the result) are all kept.
//!
//! PARITY-CHECK: the ends of the blob use `f64::sin`/`cos` where vanilla uses `Math.sin`/`cos`
//! (the platform libm, not the `Mth` table). They agree to the last bit for almost every input
//! but are not guaranteed to; a mismatch would shift a blob end by a block now and then.

use std::f32::consts::PI;
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use oxide_core::{BlockPos, BlockState, HeightmapType, RandomSource, ResourceLocation};
use oxide_datapack::feature::{OreConfig, RuleTest as RuleTestIr};
use oxide_datapack::{BlockFact, BlockFacts, TagSet};

use super::context::{block_in, Environment, FeatureContext};
use super::provider::next_bounded;
use super::state_provider::state_from_spec;
use crate::region::{DecorationRegion, StateKey};

/// `RuleTest`: what a replacement target matches.
#[derive(Debug, Clone)]
pub enum RuleTest {
    AlwaysTrue,
    BlockMatch(ResourceLocation),
    BlockStateMatch(BlockState),
    TagMatch(ResourceLocation),
    RandomBlockMatch(ResourceLocation, f32),
    RandomBlockStateMatch(BlockState, f32),
}

impl RuleTest {
    pub(crate) fn compile(ir: &RuleTestIr, facts: &BlockFacts) -> Result<Self> {
        let block = |text: &str| {
            ResourceLocation::from_str(text).map_err(|_| anyhow!("invalid block id {text:?}"))
        };
        Ok(match ir {
            RuleTestIr::AlwaysTrue => Self::AlwaysTrue,
            RuleTestIr::BlockMatch { block: name } => Self::BlockMatch(block(name)?),
            RuleTestIr::BlockStateMatch { block_state } => {
                Self::BlockStateMatch(state_from_spec(block_state, facts)?)
            }
            RuleTestIr::TagMatch { tag } => Self::TagMatch(block(tag.trim_start_matches('#'))?),
            RuleTestIr::RandomBlockMatch {
                block: name,
                probability,
            } => Self::RandomBlockMatch(block(name)?, *probability),
            RuleTestIr::RandomBlockStateMatch {
                block_state,
                probability,
            } => Self::RandomBlockStateMatch(state_from_spec(block_state, facts)?, *probability),
        })
    }

    /// The random variants draw a float only when the block already matches, as vanilla's
    /// short-circuiting `&&` does.
    pub(crate) fn test(
        &self,
        state: &BlockState,
        random: &mut impl RandomSource,
        tags: &TagSet,
    ) -> bool {
        self.matches(state, tags) && self.roll(random)
    }

    /// The part of [`Self::test`] that depends only on the state: no random draw.
    fn matches(&self, state: &BlockState, tags: &TagSet) -> bool {
        match self {
            Self::AlwaysTrue => true,
            Self::BlockMatch(name) | Self::RandomBlockMatch(name, _) => state.name == *name,
            Self::BlockStateMatch(expected) | Self::RandomBlockStateMatch(expected, _) => {
                state == expected
            }
            Self::TagMatch(tag) => tags.contains(tag, &state.name),
        }
    }

    /// The rest of [`Self::test`] once [`Self::matches`] holds: the random variants' draw.
    fn roll(&self, random: &mut impl RandomSource) -> bool {
        match self {
            Self::RandomBlockMatch(_, probability) | Self::RandomBlockStateMatch(_, probability) => {
                random.next_float() < *probability
            }
            _ => true,
        }
    }
}

/// Per `place()`, which targets' rules match a state, by [`StateKey`]: an ore blob reads
/// thousands of cells but only a handful of distinct states (stone, deepslate, ...), so each
/// rule's state-only part is worked out once per state rather than hashed per cell.
/// Direct-mapped, 8 slots; bit `i` of the mask is target `i`'s answer.
#[derive(Default)]
struct MatchCache([Option<(StateKey, u64)>; 8]);

impl MatchCache {
    fn mask(
        &mut self,
        key: StateKey,
        state: &BlockState,
        targets: &[Target],
        tags: &TagSet,
    ) -> u64 {
        let slot = &mut self.0[key.slot() & 7];
        if let Some((cached, mask)) = *slot {
            if cached == key {
                return mask;
            }
        }
        let mask = targets
            .iter()
            .enumerate()
            .fold(0u64, |mask, (index, target)| {
                mask | (u64::from(target.rule.matches(state, tags)) << index)
            });
        *slot = Some((key, mask));
        mask
    }
}

#[derive(Debug, Clone)]
struct Target {
    rule: RuleTest,
    state: BlockState,
}

#[derive(Debug, Clone)]
pub struct OreFeature {
    size: i32,
    discard_chance_on_air_exposure: f32,
    targets: Vec<Target>,
}

impl OreFeature {
    pub fn compile(config: &OreConfig, facts: &BlockFacts) -> Result<Self> {
        let targets = config
            .targets
            .iter()
            .map(|target| {
                Ok(Target {
                    rule: RuleTest::compile(&target.target, facts)?,
                    state: state_from_spec(&target.state, facts)?,
                })
            })
            .collect::<Result<Vec<_>>>()
            .context("ore targets")?;
        Ok(Self {
            size: config.size,
            discard_chance_on_air_exposure: config.discard_chance_on_air_exposure,
            targets,
        })
    }

    /// `OreFeature.canPlaceOre`, with its draws: the rule first, then the discard roll.
    fn can_place(
        &self,
        target: &Target,
        state: &BlockState,
        region: &DecorationRegion<'_>,
        random: &mut impl RandomSource,
        env: &Environment,
        pos: BlockPos,
    ) -> bool {
        if !target.rule.test(state, random, &env.block_tags) {
            return false;
        }
        if skip_air_check(random, self.discard_chance_on_air_exposure) {
            return true;
        }
        !is_adjacent_to_air(region, pos)
    }

    /// `OreFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let direction = cx.random.next_float() * PI;
        let spread_xz = self.size as f32 / 8.0;
        let max_radius = ((self.size as f32 / 16.0 * 2.0 + 1.0) / 2.0).ceil() as i32;
        let spread = f64::from(spread_xz);
        let (sin, cos) = (f64::from(direction).sin(), f64::from(direction).cos());
        let x_ends = [
            f64::from(origin.x) + sin * spread,
            f64::from(origin.x) - sin * spread,
        ];
        let z_ends = [
            f64::from(origin.z) + cos * spread,
            f64::from(origin.z) - cos * spread,
        ];
        let y_ends = [
            f64::from(
                origin
                    .y
                    .wrapping_add(next_bounded(cx.random, 3))
                    .wrapping_sub(2),
            ),
            f64::from(
                origin
                    .y
                    .wrapping_add(next_bounded(cx.random, 3))
                    .wrapping_sub(2),
            ),
        ];
        let spread_ceil = spread.ceil() as i32;
        let x_start = origin.x.wrapping_sub(spread_ceil).wrapping_sub(max_radius);
        let y_start = origin.y.wrapping_sub(2).wrapping_sub(max_radius);
        let z_start = origin.z.wrapping_sub(spread_ceil).wrapping_sub(max_radius);
        let size_xz = 2 * (spread_ceil + max_radius);
        let size_y = 2 * (2 + max_radius);

        // The scan below only asks whether some column's floor reaches `y_start`, and draws no
        // random numbers, so when no column of the window can reach it the answer is `false`
        // without reading any. Upper ores mostly start far above the ground.
        if y_start > cx.region.height_bound(HeightmapType::OceanFloorWg) {
            return false;
        }
        for x in x_start..=x_start + size_xz {
            for z in z_start..=z_start + size_xz {
                let Some(floor) = cx.region.height(HeightmapType::OceanFloorWg, x, z) else {
                    continue;
                };
                if y_start <= floor {
                    return self.place_blob(
                        cx,
                        [x_ends, y_ends, z_ends],
                        [x_start, y_start, z_start],
                        [size_xz, size_y],
                    );
                }
            }
        }
        false
    }

    /// `OreFeature.doPlace`.
    fn place_blob(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        [x_ends, y_ends, z_ends]: [[f64; 2]; 3],
        [x_start, y_start, z_start]: [i32; 3],
        [size_xz, size_y]: [i32; 2],
    ) -> bool {
        let count = self.size.max(0) as usize;
        // Per sphere: centre x, y, z and radius, the radius going negative once it is swallowed.
        let mut spheres = vec![[0f64; 4]; count];
        for (index, sphere) in spheres.iter_mut().enumerate() {
            let along = index as f32 / self.size as f32;
            let t = f64::from(along);
            let x = x_ends[0] + t * (x_ends[1] - x_ends[0]);
            let y = y_ends[0] + t * (y_ends[1] - y_ends[0]);
            let z = z_ends[0] + t * (z_ends[1] - z_ends[0]);
            let scale = cx.random.next_double() * f64::from(self.size) / 16.0;
            let bulge = oxide_core::mth::sin(f64::from(PI * along));
            let radius = (f64::from(bulge + 1.0) * scale + 1.0) / 2.0;
            *sphere = [x, y, z, radius];
        }
        for first in 0..count.saturating_sub(1) {
            if spheres[first][3] <= 0.0 {
                continue;
            }
            for second in first + 1..count {
                if spheres[second][3] <= 0.0 {
                    continue;
                }
                let dx = spheres[first][0] - spheres[second][0];
                let dy = spheres[first][1] - spheres[second][1];
                let dz = spheres[first][2] - spheres[second][2];
                let dr = spheres[first][3] - spheres[second][3];
                if dr * dr > dx * dx + dy * dy + dz * dz {
                    if dr > 0.0 {
                        spheres[second][3] = -1.0;
                    } else {
                        spheres[first][3] = -1.0;
                    }
                }
            }
        }

        let mut tested = BitSet::default();
        let mut placed = 0;
        // Masks hold one bit per target; more than 64 targets falls back to the plain path.
        let ids: Vec<u32> = self
            .targets
            .iter()
            .map(|target| cx.region.intern(&target.state))
            .collect();
        let mut matches = MatchCache::default();
        for [sx, sy, sz, radius] in spheres {
            if radius < 0.0 {
                continue;
            }
            let x_lo = ((sx - radius).floor() as i32).max(x_start);
            let y_lo = ((sy - radius).floor() as i32).max(y_start);
            let z_lo = ((sz - radius).floor() as i32).max(z_start);
            let x_hi = ((sx + radius).floor() as i32).max(x_lo);
            let y_hi = ((sy + radius).floor() as i32).max(y_lo);
            let z_hi = ((sz + radius).floor() as i32).max(z_lo);
            for x in x_lo..=x_hi {
                let dx = (f64::from(x) + 0.5 - sx) / radius;
                if dx * dx >= 1.0 {
                    continue;
                }
                for y in y_lo..=y_hi {
                    let dy = (f64::from(y) + 0.5 - sy) / radius;
                    // A row outside the build height skips every cell, as the per-cell test
                    // vanilla makes would; nothing in the row draws or marks `tested` first.
                    if dx * dx + dy * dy >= 1.0 || cx.outside_build_height(y) {
                        continue;
                    }
                    for z in z_lo..=z_hi {
                        let dz = (f64::from(z) + 0.5 - sz) / radius;
                        if dx * dx + dy * dy + dz * dz >= 1.0 {
                            continue;
                        }
                        // Vanilla's index, collisions included: it strides by the box size,
                        // not by the box size plus one, so distinct cells can share a bit.
                        let index = i64::from(x.wrapping_sub(x_start))
                            + i64::from(y.wrapping_sub(y_start)) * i64::from(size_xz)
                            + i64::from(z.wrapping_sub(z_start))
                                * i64::from(size_xz)
                                * i64::from(size_y);
                        if !tested.insert(index) {
                            continue;
                        }
                        let pos = BlockPos::new(x, y, z);
                        let done = if self.targets.len() <= 64 {
                            self.place_at_cached(cx, pos, &ids, &mut matches)
                        } else {
                            self.place_at(cx, pos)
                        };
                        if done {
                            placed += 1;
                        }
                    }
                }
            }
        }
        placed > 0
    }

    /// Tries each target in order at `pos`; the first that matches and survives the air check
    /// wins.
    fn place_at(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        if !cx.ensure_can_write(pos) {
            return false;
        }
        let mut chosen = None;
        {
            let region: &DecorationRegion<'_> = &*cx.region;
            let Some(state) = region.block(pos) else {
                return false;
            };
            for (index, target) in self.targets.iter().enumerate() {
                if self.can_place(target, state, region, &mut *cx.random, cx.env, pos) {
                    chosen = Some(index);
                    break;
                }
            }
        }
        match chosen {
            Some(index) => cx.region.set_block_ref(pos, &self.targets[index].state),
            None => false,
        }
    }

    /// [`Self::place_at`] with the targets' states pre-interned (`ids`, one per target) and the
    /// rules' state-only answers cached per state. Same draws in the same order: a rule's roll
    /// happens exactly when its state part matches, then the discard roll, as in `can_place`.
    fn place_at_cached(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        pos: BlockPos,
        ids: &[u32],
        matches: &mut MatchCache,
    ) -> bool {
        // `ensure_can_write` and the read's own window test in one: `cell` refuses exactly the
        // positions outside the window (which `ensure_can_write` refuses) or the world's height
        // (which `block_keyed` refuses), both before any draw, so the answer is the same `false`.
        let Some(cell) = cx.region.cell(pos) else {
            return false;
        };
        let mut chosen = None;
        {
            let region: &DecorationRegion<'_> = &*cx.region;
            let Some((key, state)) = region.block_keyed_at(cell) else {
                return false;
            };
            let mask = matches.mask(key, state, &self.targets, &cx.env.block_tags);
            for (index, target) in self.targets.iter().enumerate() {
                if mask >> index & 1 == 0 || !target.rule.roll(&mut *cx.random) {
                    continue;
                }
                if skip_air_check(&mut *cx.random, self.discard_chance_on_air_exposure)
                    || !is_adjacent_to_air(region, pos)
                {
                    chosen = Some(index);
                    break;
                }
            }
        }
        match chosen {
            Some(index) => {
                cx.region.set_block_id_at(pos, cell, ids[index]);
                true
            }
            None => false,
        }
    }

    /// `ScatteredOreFeature.place`: up to `size` single blocks, each within a spread that grows
    /// with the attempt number to a cap of 7.
    ///
    /// The return value is always true, as in the reference; decoration ignores it.
    pub fn place_scattered(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let attempts = next_bounded(cx.random, self.size.wrapping_add(1));
        for attempt in 0..attempts {
            let spread = attempt.min(7) as f32;
            let mut offset = || {
                let delta = (cx.random.next_float() - cx.random.next_float()) * spread;
                // `Math.round(float)`: floor(x + 0.5) exactly, widened so the add cannot round.
                (f64::from(delta) + 0.5).floor() as i32
            };
            let x = origin.x.wrapping_add(offset());
            let y = origin.y.wrapping_add(offset());
            let z = origin.z.wrapping_add(offset());
            let pos = BlockPos::new(x, y, z);
            let mut chosen = None;
            {
                let region: &DecorationRegion<'_> = &*cx.region;
                let state = block_in(region, pos);
                for (index, target) in self.targets.iter().enumerate() {
                    if self.can_place(target, state, region, &mut *cx.random, cx.env, pos) {
                        chosen = Some(index);
                        break;
                    }
                }
            }
            if let Some(index) = chosen {
                if cx.ensure_can_write(pos) {
                    cx.region.set_block_ref(pos, &self.targets[index].state);
                }
            }
        }
        true
    }
}

/// `OreFeature.shouldSkipAirCheck`: whether to place without looking for air. At 0 the check is
/// always skipped, at 1 never, and in between it is skipped with probability `1 - chance`.
fn skip_air_check(random: &mut impl RandomSource, chance: f32) -> bool {
    if chance <= 0.0 {
        true
    } else if chance >= 1.0 {
        false
    } else {
        random.next_float() >= chance
    }
}

/// `OreFeature.isAdjacentToAir`: any of the six neighbours is air. Outside the world is air.
fn is_adjacent_to_air(region: &DecorationRegion<'_>, pos: BlockPos) -> bool {
    const NEIGHBOURS: [[i32; 3]; 6] = [
        [0, -1, 0],
        [0, 1, 0],
        [0, 0, -1],
        [0, 0, 1],
        [-1, 0, 0],
        [1, 0, 0],
    ];
    NEIGHBOURS.iter().any(|d| {
        let neighbour = BlockPos::new(
            pos.x.wrapping_add(d[0]),
            pos.y.wrapping_add(d[1]),
            pos.z.wrapping_add(d[2]),
        );
        crate::decorator::context::fact_in(region, neighbour, BlockFact::Air)
    })
}

/// A growable bit set keyed by a signed index, like `java.util.BitSet`.
#[derive(Default)]
struct BitSet(Vec<u64>);

impl BitSet {
    /// Sets the bit and returns whether it was clear. A negative index is never set.
    fn insert(&mut self, index: i64) -> bool {
        let Ok(index) = usize::try_from(index) else {
            return false;
        };
        let (word, bit) = (index / 64, 1u64 << (index % 64));
        if self.0.len() <= word {
            self.0.resize(word + 1, 0);
        }
        let fresh = self.0[word] & bit == 0;
        self.0[word] |= bit;
        fresh
    }
}
