//! `geode`: an amethyst geode, a hollow ball of layered blocks with a crack in one side.
//!
//! A few random points inside the box around the origin each pull a field of `1 / distance`
//! (plus a little noise); a cell's summed field decides its layer: filling (air), inner layer,
//! middle layer or outer shell, and a second, narrower field around four fixed points cuts the
//! crack through the inner layers. Crystals then grow on some inner-layer cells.
//!
//! The geode is refused when more than `invalid_blocks_threshold` of its points sit in air or in
//! an invalid block (water, lava, ice, bedrock).
//!
//! Vanilla schedules a fluid tick at every fluid next to a crack cell it opens; those go to the
//! region's scheduled fluid ticks. A layer provider that cannot give a state is tallied and stops the geode where it is.
//!
//! PARITY-CHECK: the noise is sampled in doubles (`NormalNoise.getValue`); the reference
//! implementation targets 26.3 and narrows it to a float first. Check against a real 26.2 geode.
//!
//! PARITY-CHECK: vanilla accepts any `facing` property on a crystal and rotates it per
//! direction only when it has six values. The exported states carry no value sets, so only a
//! `facing` of `up` or `down` (necessarily the six-way property) is accepted; anything else makes
//! the feature unsupported.

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, RandomSource};
use oxide_datapack::cave_feature::GeodeConfig;
use oxide_datapack::placement::{BlockStateSpec, IntProvider};
use oxide_datapack::{BlockFact, BlockFacts};
use oxide_noise::{NormalNoise, WorldRandom};

use super::support::{int_bounds, is_block, offset, relative, state_has, waterlogged, Dir};
use crate::decorator::context::{block_in, fluid_id_at, FeatureContext};
use crate::decorator::predicate::IdSet;
use crate::decorator::provider::{next_bounded, sample_int};
use crate::decorator::state_provider::{state_from_spec, StateProvider, Unsupported};

#[derive(Debug)]
pub struct GeodeFeature {
    filling: StateProvider,
    inner: StateProvider,
    alternate_inner: StateProvider,
    middle: StateProvider,
    outer: StateProvider,
    inner_placements: Vec<BlockState>,
    cannot_replace: IdSet,
    invalid_blocks: IdSet,
    /// Filling, inner layer, middle layer and outer layer thickness constants.
    layers: [f64; 4],
    crack_chance: f64,
    base_crack_size: f64,
    crack_point_offset: i32,
    potential_chance: f64,
    alternate_chance: f64,
    require_alternate: bool,
    outer_wall_distance: IntProvider,
    outer_wall_max: i32,
    distribution_points: IntProvider,
    point_offset: IntProvider,
    min_gen_offset: i32,
    max_gen_offset: i32,
    noise_multiplier: f64,
    invalid_threshold: i32,
    air: BlockState,
}

/// `BlockPos.distSqr(BlockPos)`, in doubles.
fn dist_sqr(a: BlockPos, b: BlockPos) -> f64 {
    let d = |from: i32, to: i32| f64::from(from) - f64::from(to);
    d(a.x, b.x) * d(a.x, b.x) + d(a.y, b.y) * d(a.y, b.y) + d(a.z, b.z) * d(a.z, b.z)
}

impl GeodeFeature {
    /// `None` when a provider has no possible value or a crystal's `facing` cannot be read as
    /// the six-way property.
    pub fn compile(config: &GeodeConfig, facts: &BlockFacts) -> Result<Option<Self>> {
        let Some((_, outer_wall_max)) = int_bounds(&config.outer_wall_distance) else {
            return Ok(None);
        };
        if int_bounds(&config.distribution_points).is_none()
            || int_bounds(&config.point_offset).is_none()
        {
            return Ok(None);
        }
        if config.blocks.inner_placements.is_empty() {
            return Ok(None);
        }
        let inner_placements = config
            .blocks
            .inner_placements
            .iter()
            .map(|spec| state_from_spec(spec, facts))
            .collect::<Result<Vec<_>>>()?;
        let six_way = |state: &BlockState| {
            state
                .properties
                .get("facing")
                .is_none_or(|value| value == "up" || value == "down")
        };
        if !inner_placements.iter().all(six_way) {
            return Ok(None);
        }
        let blocks = &config.blocks;
        let provider = |ir| StateProvider::compile(ir, facts);
        let air = state_from_spec(
            &BlockStateSpec {
                name: "minecraft:air".into(),
                properties: Default::default(),
            },
            facts,
        )?;
        Ok(Some(Self {
            filling: provider(&blocks.filling_provider)?,
            inner: provider(&blocks.inner_layer_provider)?,
            alternate_inner: provider(&blocks.alternate_inner_layer_provider)?,
            middle: provider(&blocks.middle_layer_provider)?,
            outer: provider(&blocks.outer_layer_provider)?,
            inner_placements,
            cannot_replace: IdSet::compile(&blocks.cannot_replace)?,
            invalid_blocks: IdSet::compile(&blocks.invalid_blocks)?,
            layers: [
                config.layers.filling,
                config.layers.inner_layer,
                config.layers.middle_layer,
                config.layers.outer_layer,
            ],
            crack_chance: config.crack.generate_crack_chance,
            base_crack_size: config.crack.base_crack_size,
            crack_point_offset: config.crack.crack_point_offset,
            potential_chance: config.use_potential_placements_chance,
            alternate_chance: config.use_alternate_layer0_chance,
            require_alternate: config.placements_require_layer0_alternate,
            outer_wall_distance: config.outer_wall_distance.clone(),
            outer_wall_max,
            distribution_points: config.distribution_points.clone(),
            point_offset: config.point_offset.clone(),
            min_gen_offset: config.min_gen_offset,
            max_gen_offset: config.max_gen_offset,
            noise_multiplier: config.noise_multiplier,
            invalid_threshold: config.invalid_blocks_threshold,
            air,
        }))
    }

    /// `Feature.safeSetBlock`: writes `state` unless the block there may not be replaced.
    fn safe_set(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos, state: BlockState) {
        if !self
            .cannot_replace
            .contains_block(cx.env, &block_in(cx.region, pos).name)
        {
            cx.region.set_block(pos, state);
        }
    }

    /// The provider's state at `pos`; a provider that cannot give one is tallied.
    fn sample(
        cx: &mut FeatureContext<'_, '_, '_>,
        provider: &StateProvider,
        pos: BlockPos,
    ) -> Option<BlockState> {
        match provider.state(cx, pos) {
            Ok(Some(state)) => Some(state),
            Ok(None) => {
                cx.unsupported.record("provider", "no state for position");
                None
            }
            Err(Unsupported(why)) => {
                cx.unsupported.record("provider", why);
                None
            }
        }
    }

    /// `GeodeFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let num_points = sample_int(&self.distribution_points, cx.random);
        // `NormalNoise.create(new WorldgenRandom(new LegacyRandomSource(seed)), -4, 1.0)`.
        let noise = NormalNoise::create(&mut WorldRandom::new(cx.env.seed, true), -4, vec![1.0]);
        let crack_adjust = f64::from(num_points) / f64::from(self.outer_wall_max);
        let [filling, inner, middle, outer] = self.layers;
        let inner_air = 1.0 / filling.sqrt();
        let innermost = 1.0 / (inner + crack_adjust).sqrt();
        let inner_crust = 1.0 / (middle + crack_adjust).sqrt();
        let outer_crust = 1.0 / (outer + crack_adjust).sqrt();
        let crack_size = 1.0
            / (self.base_crack_size
                + cx.random.next_double() / 2.0
                + if num_points > 3 { crack_adjust } else { 0.0 })
            .sqrt();
        let crack = f64::from(cx.random.next_float()) < self.crack_chance;
        let mut invalid = 0;
        let mut points = Vec::new();
        for _ in 0..num_points {
            let x = sample_int(&self.outer_wall_distance, cx.random);
            let y = sample_int(&self.outer_wall_distance, cx.random);
            let z = sample_int(&self.outer_wall_distance, cx.random);
            let pos = offset(origin, x, y, z);
            let state = block_in(cx.region, pos);
            if state_has(cx, state, BlockFact::Air)
                || self.invalid_blocks.contains_block(cx.env, &state.name)
            {
                invalid += 1;
                if invalid > self.invalid_threshold {
                    return false;
                }
            }
            points.push((pos, sample_int(&self.point_offset, cx.random)));
        }
        let mut crack_points = Vec::new();
        if crack {
            let (cx_off, cz_off) = {
                let end = num_points * 2 + 1;
                match cx.random.next_int_bounded(4) {
                    0 => (end, 0),
                    1 => (0, end),
                    2 => (end, end),
                    _ => (0, 0),
                }
            };
            for y in [7, 5, 1] {
                crack_points.push(offset(origin, cx_off, y, cz_off));
            }
        }
        let mut potential = Vec::new();
        let (low, high) = (self.min_gen_offset, self.max_gen_offset);
        // `BlockPos.betweenClosed` walks x fastest, then y, then z.
        for dz in low..=high {
            for dy in low..=high {
                for dx in low..=high {
                    let pos = offset(origin, dx, dy, dz);
                    let noise_offset =
                        noise.get_value(f64::from(pos.x), f64::from(pos.y), f64::from(pos.z))
                            * self.noise_multiplier;
                    let mut shell = 0.0;
                    for (point, point_offset) in &points {
                        shell += 1.0 / (dist_sqr(pos, *point) + f64::from(*point_offset)).sqrt()
                            + noise_offset;
                    }
                    if shell < outer_crust {
                        continue;
                    }
                    if shell >= inner_air {
                        let Some(state) = Self::sample(cx, &self.filling, pos) else {
                            return false;
                        };
                        self.safe_set(cx, pos, state);
                        continue;
                    }
                    let mut crack_sum = 0.0;
                    for point in &crack_points {
                        crack_sum += 1.0
                            / (dist_sqr(pos, *point) + f64::from(self.crack_point_offset)).sqrt()
                            + noise_offset;
                    }
                    if crack && crack_sum >= crack_size {
                        self.safe_set(cx, pos, self.air.clone());
                        for direction in Dir::ALL {
                            let adjacent = relative(pos, direction, 1);
                            if let Some(fluid) = fluid_id_at(cx, adjacent) {
                                cx.region.schedule_fluid_tick(adjacent, fluid);
                            }
                        }
                    } else if shell >= innermost {
                        let alternate = f64::from(cx.random.next_float()) < self.alternate_chance;
                        let provider = if alternate {
                            &self.alternate_inner
                        } else {
                            &self.inner
                        };
                        let Some(state) = Self::sample(cx, provider, pos) else {
                            return false;
                        };
                        self.safe_set(cx, pos, state);
                        if (!self.require_alternate || alternate)
                            && f64::from(cx.random.next_float()) < self.potential_chance
                        {
                            potential.push(pos);
                        }
                    } else if shell >= inner_crust {
                        let Some(state) = Self::sample(cx, &self.middle, pos) else {
                            return false;
                        };
                        self.safe_set(cx, pos, state);
                    } else {
                        let Some(state) = Self::sample(cx, &self.outer, pos) else {
                            return false;
                        };
                        self.safe_set(cx, pos, state);
                    }
                }
            }
        }
        for crystal in potential {
            let index = next_bounded(cx.random, self.inner_placements.len() as i32) as usize;
            let mut state = self.inner_placements[index].clone();
            for direction in Dir::ALL {
                if state.properties.contains_key("facing") {
                    state = state.with_property("facing", direction.name());
                }
                let target = relative(crystal, direction, 1);
                let existing = block_in(cx.region, target);
                if state.properties.contains_key("waterlogged") {
                    state = waterlogged(state, state_has(cx, existing, BlockFact::FluidSource));
                }
                if self.cluster_can_grow_at(cx, existing) {
                    self.safe_set(cx, target, state);
                    break;
                }
            }
        }
        true
    }

    /// `BuddingAmethystBlock.canClusterGrowAtState`: air, or a full block of water.
    fn cluster_can_grow_at(&self, cx: &FeatureContext<'_, '_, '_>, state: &BlockState) -> bool {
        if state_has(cx, state, BlockFact::Air) {
            return true;
        }
        // A water block's fluid is full at level 0 (source) and at the falling levels 8 and up.
        is_block(state, "water")
            && state
                .properties
                .get("level")
                .and_then(|level| level.parse::<i32>().ok())
                .is_some_and(|level| level == 0 || level >= 8)
    }
}
