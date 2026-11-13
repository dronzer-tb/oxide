//! `speleothem_cluster`: a field of stalactites and stalagmites over a rectangular patch of cave,
//! with the occasional water pool on the floor (dripstone clusters and sulfur spike clusters).
//!
//! Each column of the patch scans for its floor and ceiling, may flood the floor, thickens the
//! ceiling and floor with base blocks and grows a stalactite and a stalagmite whose heights fall
//! off with the distance from the patch centre. When they would overlap, the meeting point is
//! drawn and the two may merge into one pillar.

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, RandomSource};
use oxide_datapack::cave_feature::SpeleothemClusterConfig;
use oxide_datapack::placement::IntProvider;
use oxide_datapack::{BlockFact, BlockFacts, FloatProvider};

use super::speleothem::{Column, Speleothem};
use super::support::{has, int_bounds, is_block, offset, relative, state_has, Dir};
use crate::decorator::context::{block_in, FeatureContext};
use crate::decorator::provider::{next_bounded, sample_int};
use crate::decorator::state_provider::state_from_spec;

#[derive(Debug)]
pub struct SpeleothemClusterFeature {
    speleothem: Speleothem,
    water: BlockState,
    search_range: i32,
    height: IntProvider,
    radius: IntProvider,
    max_height_diff: i32,
    height_deviation: i32,
    layer_thickness: IntProvider,
    density: FloatProvider,
    wetness: FloatProvider,
    chance_at_max_distance: f32,
    max_distance_from_edge: i32,
    max_distance_from_center: i32,
}

/// `Mth.clampedLerp` in doubles.
fn clamped_lerp(factor: f64, min: f64, max: f64) -> f64 {
    if factor < 0.0 {
        min
    } else if factor > 1.0 {
        max
    } else {
        min + factor * (max - min)
    }
}

/// `Mth.clampedLerp` in floats.
fn clamped_lerp_f32(factor: f32, min: f32, max: f32) -> f32 {
    if factor < 0.0 {
        min
    } else if factor > 1.0 {
        max
    } else {
        min + factor * (max - min)
    }
}

impl SpeleothemClusterFeature {
    /// `None` when the pointed block lacks the properties formations set, or a provider has no
    /// possible value.
    pub fn compile(config: &SpeleothemClusterConfig, facts: &BlockFacts) -> Result<Option<Self>> {
        let Some(speleothem) = Speleothem::compile(
            &config.base_block,
            &config.pointed_block,
            &config.replaceable_blocks,
            facts,
        )?
        else {
            return Ok(None);
        };
        if int_bounds(&config.height).is_none()
            || int_bounds(&config.radius).is_none()
            || int_bounds(&config.speleothem_block_layer_thickness).is_none()
        {
            return Ok(None);
        }
        let water = state_from_spec(
            &oxide_datapack::placement::BlockStateSpec {
                name: "minecraft:water".into(),
                properties: Default::default(),
            },
            facts,
        )?;
        Ok(Some(Self {
            speleothem,
            water,
            search_range: config.floor_to_ceiling_search_range,
            height: config.height.clone(),
            radius: config.radius.clone(),
            max_height_diff: config.max_stalagmite_stalactite_height_diff,
            height_deviation: config.height_deviation,
            layer_thickness: config.speleothem_block_layer_thickness.clone(),
            density: config.density.clone(),
            wetness: config.wetness.clone(),
            chance_at_max_distance: config.chance_of_speleothem_at_max_distance_from_center,
            max_distance_from_edge: config.max_distance_from_edge_affecting_chance_of_speleothem,
            max_distance_from_center: config.max_distance_from_center_affecting_height_bias,
        }))
    }

    /// `SpeleothemClusterFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if !self.speleothem.blocks.empty_or_water(cx, block_in(cx.region, origin)) {
            return false;
        }
        let height = sample_int(&self.height, cx.random);
        let wetness = self.wetness.sample(cx.random);
        let density = self.density.sample(cx.random);
        let x_radius = sample_int(&self.radius, cx.random);
        let z_radius = sample_int(&self.radius, cx.random);
        for dx in -x_radius..=x_radius {
            for dz in -z_radius..=z_radius {
                let distance = (x_radius - dx.abs()).min(z_radius - dz.abs());
                let chance = f64::from(clamped_lerp_f32(
                    (distance as f32 - 0.0) / (self.max_distance_from_edge as f32 - 0.0),
                    self.chance_at_max_distance,
                    1.0,
                ));
                let pos = offset(origin, dx, 0, dz);
                self.column(cx, pos, dx, dz, wetness, chance, height, density);
            }
        }
        true
    }

    /// `placeColumn`.
    #[allow(clippy::too_many_arguments)]
    fn column(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        pos: BlockPos,
        dx: i32,
        dz: i32,
        chance_of_water: f32,
        chance: f64,
        cluster_height: i32,
        density: f32,
    ) {
        let blocks = &self.speleothem.blocks;
        let Some(base_column) = blocks.scan(cx, pos, self.search_range, &|state| !blocks.empty_or_water(cx, state))
        else {
            return;
        };
        let ceiling = base_column.ceiling;
        let base_floor = base_column.floor;
        if ceiling.is_none() && base_floor.is_none() {
            return;
        }
        let want_pool = cx.random.next_float() < chance_of_water;
        let column = match base_floor {
            Some(floor_y) if want_pool && self.can_place_pool(cx, BlockPos::new(pos.x, floor_y, pos.z)) => {
                cx.region.set_block(BlockPos::new(pos.x, floor_y, pos.z), self.water.clone());
                Column { floor: Some(floor_y - 1), ceiling }
            }
            _ => base_column,
        };
        let floor = column.floor;
        let want_stalactite = cx.random.next_double() < chance;
        let stalactite_height = match ceiling {
            Some(c)
                if want_stalactite && !is_block(block_in(cx.region, BlockPos::new(pos.x, c, pos.z)), "lava") =>
            {
                let thickness = sample_int(&self.layer_thickness, cx.random);
                self.replace_with_base(cx, BlockPos::new(pos.x, c, pos.z), thickness, 1);
                let max = match floor {
                    Some(f) => cluster_height.min(c - f),
                    None => cluster_height,
                };
                self.speleothem_height(cx, dx, dz, density, max)
            }
            _ => 0,
        };
        let want_stalagmite = cx.random.next_double() < chance;
        let stalagmite_height = match floor {
            Some(f)
                if want_stalagmite && !is_block(block_in(cx.region, BlockPos::new(pos.x, f, pos.z)), "lava") =>
            {
                let thickness = sample_int(&self.layer_thickness, cx.random);
                self.replace_with_base(cx, BlockPos::new(pos.x, f, pos.z), thickness, -1);
                if ceiling.is_some() {
                    let diff = self.max_height_diff;
                    0.max(stalactite_height + next_bounded(cx.random, diff * 2 + 1) - diff)
                } else {
                    self.speleothem_height(cx, dx, dz, density, cluster_height)
                }
            }
            _ => 0,
        };
        let (actual_stalactite, actual_stalagmite) = match (ceiling, floor) {
            (Some(c), Some(f)) if c - stalactite_height <= f + stalagmite_height => {
                let lowest_bottom = (c - stalactite_height).max(f + 1);
                let highest_top = (f + stalagmite_height).min(c - 1);
                let bottom = next_bounded(cx.random, highest_top + 1 - lowest_bottom + 1) + lowest_bottom;
                (c - bottom, bottom - 1 - f)
            }
            _ => (stalactite_height, stalagmite_height),
        };
        let merge = cx.random.next_boolean()
            && actual_stalactite > 0
            && actual_stalagmite > 0
            && column.height().is_some_and(|h| actual_stalactite + actual_stalagmite == h);
        if let Some(c) = ceiling {
            self.speleothem.grow(cx, BlockPos::new(pos.x, c - 1, pos.z), false, actual_stalactite, merge);
        }
        if let Some(f) = floor {
            self.speleothem.grow(cx, BlockPos::new(pos.x, f + 1, pos.z), true, actual_stalagmite, merge);
        }
    }

    /// `getSpeleothemHeight`.
    fn speleothem_height(&self, cx: &mut FeatureContext<'_, '_, '_>, dx: i32, dz: i32, density: f32, max_height: i32) -> i32 {
        if cx.random.next_float() > density {
            return 0;
        }
        let distance = dx.abs() + dz.abs();
        let mean = clamped_lerp(
            (f64::from(distance) - 0.0) / (f64::from(self.max_distance_from_center) - 0.0),
            f64::from(max_height) / 2.0,
            0.0,
        ) as f32;
        let value = mean + cx.random.next_gaussian() as f32 * self.height_deviation as f32;
        let clamped = if value < 0.0 { 0.0 } else { value.min(max_height as f32) };
        clamped as i32
    }

    /// `canPlacePool`: a floor cell that is not water, base or pointed, with no water above it and
    /// base stone or water on every other side.
    fn can_place_pool(&self, cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        let state = block_in(cx.region, pos);
        if is_block(state, "water")
            || state.name == self.speleothem.blocks.base.name
            || state.name == self.speleothem.pointed.block().name
        {
            return false;
        }
        if has(cx, offset(pos, 0, 1, 0), BlockFact::Water) {
            return false;
        }
        for direction in [Dir::North, Dir::East, Dir::South, Dir::West] {
            if !self.adjacent_ok(cx, relative(pos, direction, 1)) {
                return false;
            }
        }
        self.adjacent_ok(cx, offset(pos, 0, -1, 0))
    }

    /// Base stone of the overworld, or water, holds a pool in.
    fn adjacent_ok(&self, cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        let state = block_in(cx.region, pos);
        let stone = cx.env.block_tags.contains(&oxide_core::ResourceLocation::minecraft("base_stone_overworld"), &state.name);
        stone || state_has(cx, state, BlockFact::Water)
    }

    /// `replaceBlocksWithBase`: up to `count` base blocks from `first` along y, stopping at the
    /// first cell that is not replaceable.
    fn replace_with_base(&self, cx: &mut FeatureContext<'_, '_, '_>, first: BlockPos, count: i32, step: i32) {
        let mut at = first;
        for _ in 0..count {
            if !self.speleothem.blocks.place_base_if_possible(cx, at) {
                return;
            }
            at = offset(at, 0, step, 0);
        }
    }
}
