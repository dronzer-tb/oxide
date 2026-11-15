//! `large_dripstone`: a stalactite and a stalagmite, each a tapering cone of dripstone, on one
//! vertical line across a cave at least four blocks tall.
//!
//! The pair share a radius drawn from the cave's height. Each cone is first backed into the rock
//! until a full circle of its footprint sits embedded in solid blocks, halving its radius until it
//! does; when both are wide and blunt enough a shared wind then shears every column sideways by
//! its distance from the origin. The base block is always `dripstone_block`; the config names only
//! what may be replaced.
//!
//! PARITY-CHECK: the cone profile uses `f64::powf` and `f64::ln`, not Java's `StrictMath`; they
//! agree to the last bit for almost every input but are not guaranteed to. `Math.cos`/`Math.sin`
//! of the wind direction go through `Mth`'s table, as the reference does.

use anyhow::Result;
use oxide_core::{mth, BlockPos, BlockState, HeightmapType, RandomSource};
use oxide_datapack::cave_feature::LargeDripstoneConfig;
use oxide_datapack::{BlockFacts, FloatProvider};

use super::speleothem::Blocks;
use super::support::{int_bounds, is_block, offset};
use crate::decorator::context::{block_in, FeatureContext};
use crate::decorator::provider::next_bounded;
use crate::decorator::state_provider::state_from_spec;

#[derive(Debug)]
pub struct LargeDripstoneFeature {
    blocks: Blocks,
    search_range: i32,
    /// Smallest and largest column radius the provider can give.
    column_radius_bounds: (i32, i32),
    height_scale: FloatProvider,
    max_radius_ratio: f32,
    stalactite_bluntness: FloatProvider,
    stalagmite_bluntness: FloatProvider,
    wind_speed: FloatProvider,
    min_radius_for_wind: i32,
    min_bluntness_for_wind: f32,
}

/// One cone, rooted in the ceiling (`up == false`) or the floor.
struct Cone {
    root: BlockPos,
    up: bool,
    radius: i32,
    bluntness: f64,
    scale: f64,
}

/// The shear applied to every column of a pair of cones.
#[derive(Clone, Copy)]
struct Wind {
    origin_y: i32,
    speed: Option<(f64, f64)>,
    max_offset: i32,
}

impl Wind {
    const CALM: Wind = Wind {
        origin_y: 0,
        speed: None,
        max_offset: 0,
    };

    fn offset(&self, pos: BlockPos) -> BlockPos {
        let Some((speed_x, speed_z)) = self.speed else {
            return pos;
        };
        let dy = f64::from(self.origin_y.wrapping_sub(pos.y));
        let clamp = |value: f64| {
            let value = value.floor() as i32;
            if value < -self.max_offset {
                -self.max_offset
            } else {
                value.min(self.max_offset)
            }
        };
        offset(pos, clamp(speed_x * dy), 0, clamp(speed_z * dy))
    }
}

/// `LargeDripstoneFeature.getHeightAtRadius`'s profile.
fn speleothem_height(distance: f64, radius: f64, scale: f64, bluntness: f64) -> f64 {
    let distance = if distance < bluntness {
        bluntness
    } else {
        distance
    };
    let r = distance / radius * 0.384;
    let part1 = 0.75 * r.powf(1.3333333333333333);
    let part2 = r.powf(0.6666666666666666);
    let part3 = 0.3333333333333333 * r.ln();
    let height = (scale * (part1 - part2 - part3)).max(0.0);
    height / 0.384 * radius
}

impl Cone {
    fn height_at(&self, radius: f32) -> i32 {
        speleothem_height(
            f64::from(radius),
            f64::from(self.radius),
            self.scale,
            self.bluntness,
        ) as i32
    }
}

impl LargeDripstoneFeature {
    /// `None` when the column radius provider has no possible value.
    pub fn compile(config: &LargeDripstoneConfig, facts: &BlockFacts) -> Result<Option<Self>> {
        let Some(column_radius_bounds) = int_bounds(&config.column_radius) else {
            return Ok(None);
        };
        let dripstone = state_from_spec(
            &oxide_datapack::placement::BlockStateSpec {
                name: "minecraft:dripstone_block".into(),
                properties: Default::default(),
            },
            facts,
        )?;
        Ok(Some(Self {
            blocks: Blocks::compile(dripstone, &config.replaceable_blocks)?,
            search_range: config.floor_to_ceiling_search_range,
            column_radius_bounds,
            height_scale: config.height_scale.clone(),
            max_radius_ratio: config.max_column_radius_to_cave_height_ratio,
            stalactite_bluntness: config.stalactite_bluntness.clone(),
            stalagmite_bluntness: config.stalagmite_bluntness.clone(),
            wind_speed: config.wind_speed.clone(),
            min_radius_for_wind: config.min_radius_for_wind,
            min_bluntness_for_wind: config.min_bluntness_for_wind,
        }))
    }

    /// `LargeDripstoneFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let blocks = &self.blocks;
        if !blocks.empty_or_water(cx, block_in(cx.region, origin)) {
            return false;
        }
        let Some(column) = blocks.scan(cx, origin, self.search_range, &|state| {
            blocks.is_base(cx, state) || is_block(state, "lava")
        }) else {
            return false;
        };
        let (Some(floor), Some(ceiling)) = (column.floor, column.ceiling) else {
            return false;
        };
        let height = ceiling - floor - 1;
        if height < 4 {
            return false;
        }
        let by_height = (height as f32 * self.max_radius_ratio) as i32;
        let (min_radius, max_radius_bound) = self.column_radius_bounds;
        let max_radius = if by_height < min_radius {
            min_radius
        } else {
            by_height.min(max_radius_bound)
        };
        let radius = next_bounded(cx.random, max_radius - min_radius + 1) + min_radius;
        let mut stalactite = Cone {
            root: BlockPos::new(origin.x, ceiling - 1, origin.z),
            up: false,
            radius,
            bluntness: f64::from(self.stalactite_bluntness.sample(cx.random)),
            scale: f64::from(self.height_scale.sample(cx.random)),
        };
        let mut stalagmite = Cone {
            root: BlockPos::new(origin.x, floor + 1, origin.z),
            up: true,
            radius,
            bluntness: f64::from(self.stalagmite_bluntness.sample(cx.random)),
            scale: f64::from(self.height_scale.sample(cx.random)),
        };
        let suitable = |cone: &Cone| {
            cone.radius >= self.min_radius_for_wind
                && cone.bluntness >= f64::from(self.min_bluntness_for_wind)
        };
        let wind = if suitable(&stalactite) && suitable(&stalagmite) {
            let speed = self.wind_speed.sample(cx.random);
            let direction = cx.random.next_float() * (std::f32::consts::PI - 0.0) + 0.0;
            Wind {
                origin_y: origin.y,
                speed: Some((
                    f64::from(mth::cos(f64::from(direction)) * speed),
                    f64::from(mth::sin(f64::from(direction)) * speed),
                )),
                max_offset: 16 - radius,
            }
        } else {
            Wind::CALM
        };
        let top = self.move_back(cx, &mut stalactite, wind);
        let bottom = self.move_back(cx, &mut stalagmite, wind);
        if top {
            self.place_blocks(cx, &stalactite, wind);
        }
        if bottom {
            self.place_blocks(cx, &stalagmite, wind);
        }
        true
    }

    /// `LargeDripstone.moveBackUntilBaseIsInsideStoneAndShrinkRadiusIfNecessary`.
    fn move_back(&self, cx: &FeatureContext<'_, '_, '_>, cone: &mut Cone, wind: Wind) -> bool {
        while cone.radius > 1 {
            let mut root = cone.root;
            let tries = 10.min(cone.height_at(0.0));
            for _ in 0..tries {
                if is_block(block_in(cx.region, root), "lava") {
                    return false;
                }
                if self.circle_embedded(cx, wind.offset(root), cone.radius) {
                    cone.root = root;
                    return true;
                }
                root = offset(root, 0, if cone.up { -1 } else { 1 }, 0);
            }
            cone.radius /= 2;
        }
        false
    }

    /// `isCircleMostlyEmbeddedInStone`: no cell of the circle is air, water or lava.
    fn circle_embedded(
        &self,
        cx: &FeatureContext<'_, '_, '_>,
        center: BlockPos,
        radius: i32,
    ) -> bool {
        if self
            .blocks
            .empty_water_or_lava(cx, block_in(cx.region, center))
        {
            return false;
        }
        let increment = 6.0f32 / radius as f32;
        let mut angle = 0.0f32;
        while angle < (std::f64::consts::PI * 2.0) as f32 {
            let dx = (mth::cos(f64::from(angle)) * radius as f32) as i32;
            let dz = (mth::sin(f64::from(angle)) * radius as f32) as i32;
            if self
                .blocks
                .empty_water_or_lava(cx, block_in(cx.region, offset(center, dx, 0, dz)))
            {
                return false;
            }
            angle += increment;
        }
        true
    }

    /// `LargeDripstone.placeBlocks`.
    fn place_blocks(&self, cx: &mut FeatureContext<'_, '_, '_>, cone: &Cone, wind: Wind) {
        let base: &BlockState = &self.blocks.base;
        let base_stone = oxide_core::ResourceLocation::minecraft("base_stone_overworld");
        for dx in -cone.radius..=cone.radius {
            for dz in -cone.radius..=cone.radius {
                let current = ((dx * dx + dz * dz) as f32).sqrt();
                if current > cone.radius as f32 {
                    continue;
                }
                let mut height = cone.height_at(current);
                if height <= 0 {
                    continue;
                }
                if f64::from(cx.random.next_float()) < 0.2 {
                    let factor = cx.random.next_float() * (1.0f32 - 0.8f32) + 0.8f32;
                    height = (height as f32 * factor) as i32;
                }
                let mut pos = offset(cone.root, dx, 0, dz);
                let mut out_of_stone = false;
                let max_y = if cone.up {
                    match cx
                        .region
                        .height(HeightmapType::WorldSurfaceWg, pos.x, pos.z)
                    {
                        Some(top) => top,
                        None => continue,
                    }
                } else {
                    i32::MAX
                };
                let mut i = 0;
                while i < height && pos.y < max_y {
                    let adjusted = wind.offset(pos);
                    let state = block_in(cx.region, adjusted);
                    if self.blocks.empty_water_or_lava(cx, state) {
                        out_of_stone = true;
                        cx.region.set_block_ref(adjusted, &base);
                    } else if out_of_stone && cx.env.block_tags.contains(&base_stone, &state.name) {
                        break;
                    }
                    pos = offset(pos, 0, if cone.up { 1 } else { -1 }, 0);
                    i += 1;
                }
            }
        }
    }
}
