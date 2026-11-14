//! `basalt_columns`: a cluster of basalt columns on a floor or in a lava ocean.
//!
//! The cluster samples column sites at random on the origin's layer (many close ones nine times
//! in ten, a few spread ones otherwise). Each site with height left after its Manhattan distance
//! from the origin grows a square of columns: a cell in air or lava-sea sinks to the first
//! placeable floor, a cell in a solid block rises to the first air, and the column then stacks
//! basalt upward, passing through basalt already there and stopping at anything else.
//!
//! "Lava sea" is lava at or below the generator's sea level, `Environment::sea_level` (32 in the
//! Nether), not lava anywhere.

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, RandomSource, ResourceLocation};
use oxide_datapack::nether_feature::ColumnConfig;
use oxide_datapack::placement::IntProvider;
use oxide_datapack::BlockFacts;

use super::super::context::{block_in, FeatureContext};
use super::super::provider::{next_bounded, sample_int};
use super::super::tree::dist_manhattan;
use super::{default_state, is_empty, offset};

/// `BasaltColumnsFeature.CANNOT_PLACE_ON`.
const CANNOT_PLACE_ON: [&str; 10] = [
    "lava",
    "bedrock",
    "magma_block",
    "soul_sand",
    "nether_bricks",
    "nether_brick_fence",
    "nether_brick_stairs",
    "nether_wart",
    "chest",
    "spawner",
];

#[derive(Debug)]
pub struct BasaltColumnsFeature {
    reach: IntProvider,
    height: IntProvider,
    basalt: BlockState,
    lava: ResourceLocation,
    cannot_place_on: Vec<ResourceLocation>,
}

impl BasaltColumnsFeature {
    pub fn compile(config: &ColumnConfig, facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            reach: config.reach.clone(),
            height: config.height.clone(),
            basalt: default_state(facts, "basalt")?,
            lava: ResourceLocation::minecraft("lava"),
            cannot_place_on: CANNOT_PLACE_ON
                .iter()
                .map(|path| ResourceLocation::minecraft(*path))
                .collect(),
        })
    }

    /// `BasaltColumnsFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if !self.can_place_at(cx, origin) {
            return false;
        }
        let column_height = sample_int(&self.height, cx.random);
        let clustered = cx.random.next_float() < 0.9;
        let reach = column_height.min(if clustered { 5 } else { 8 });
        let count = if clustered { 50 } else { 15 };
        // `BlockPos.randomBetweenClosed` over the origin's layer: one x, y and z draw per site,
        // made as the iterator advances, so they interleave with the site's own `reach` draw.
        let width = 2 * reach + 1;
        let mut placed = false;
        for _ in 0..count {
            let x = origin.x - reach + next_bounded(cx.random, width);
            let y = origin.y + next_bounded(cx.random, 1);
            let z = origin.z - reach + next_bounded(cx.random, width);
            let pos = BlockPos::new(x, y, z);
            let blocks_to_place = column_height - dist_manhattan(pos, origin);
            if blocks_to_place >= 0 {
                let column_reach = sample_int(&self.reach, cx.random);
                placed |= self.place_column(cx, pos, blocks_to_place, column_reach);
            }
        }
        placed
    }

    /// `BasaltColumnsFeature.placeColumn`: the square of columns around one site.
    fn place_column(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        site: BlockPos,
        height: i32,
        reach: i32,
    ) -> bool {
        let mut placed_any = false;
        // `BlockPos.betweenClosed` on one layer: x fastest, then z.
        for dz in -reach..=reach {
            for dx in -reach..=reach {
                let pos = offset(site, dx, 0, dz);
                let step_limit = dist_manhattan(pos, site);
                let column = if self.is_air_or_lava_ocean(cx, pos) {
                    self.find_surface(cx, pos, step_limit)
                } else {
                    self.find_air(cx, pos, step_limit)
                };
                let Some(mut cursor) = column else {
                    continue;
                };
                let mut blocks_y = height - step_limit / 2;
                while blocks_y >= 0 {
                    if self.is_air_or_lava_ocean(cx, cursor) {
                        cx.region.set_block(cursor, self.basalt.clone());
                        cursor = offset(cursor, 0, 1, 0);
                        placed_any = true;
                    } else {
                        if block_in(cx.region, cursor).name != self.basalt.name {
                            break;
                        }
                        cursor = offset(cursor, 0, 1, 0);
                    }
                    blocks_y -= 1;
                }
            }
        }
        placed_any
    }

    /// `findSurface`: down to the first cell [`Self::can_place_at`] accepts, at most `limit` steps.
    fn find_surface(
        &self,
        cx: &FeatureContext<'_, '_, '_>,
        mut cursor: BlockPos,
        mut limit: i32,
    ) -> Option<BlockPos> {
        while cursor.y > cx.region.min_y() + 1 && limit > 0 {
            limit -= 1;
            if self.can_place_at(cx, cursor) {
                return Some(cursor);
            }
            cursor = offset(cursor, 0, -1, 0);
        }
        None
    }

    /// `canPlaceAt`: air or lava sea over a block that is neither air nor a blocked block.
    fn can_place_at(&self, cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        if !self.is_air_or_lava_ocean(cx, pos) {
            return false;
        }
        let below = offset(pos, 0, -1, 0);
        !is_empty(cx, below)
            && !self
                .cannot_place_on
                .contains(&block_in(cx.region, below).name)
    }

    /// `findAir`: up to the first air, at most `limit` steps, giving up at a blocked block.
    fn find_air(
        &self,
        cx: &FeatureContext<'_, '_, '_>,
        mut cursor: BlockPos,
        mut limit: i32,
    ) -> Option<BlockPos> {
        // `Level.getMaxY` is inclusive; the region's `max_y` is one past the top.
        while cursor.y < cx.region.max_y() && limit > 0 {
            limit -= 1;
            if self
                .cannot_place_on
                .contains(&block_in(cx.region, cursor).name)
            {
                return None;
            }
            if is_empty(cx, cursor) {
                return Some(cursor);
            }
            cursor = offset(cursor, 0, 1, 0);
        }
        None
    }

    /// `isAirOrLavaOcean`.
    fn is_air_or_lava_ocean(&self, cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        is_empty(cx, pos)
            || (block_in(cx.region, pos).name == self.lava && pos.y <= cx.env.sea_level)
    }
}
