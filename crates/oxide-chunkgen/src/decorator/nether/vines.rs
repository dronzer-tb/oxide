//! `twisting_vines` and `weeping_vines`: columns of Nether vines.
//!
//! Twisting vines grow up from a warped floor: each of `spread_width²` tries picks a cell around
//! the origin, drops it to the first air above ground, and grows a column there. Weeping vines
//! hang from a ceiling: the origin becomes a nether wart block that 200 tries spread over the
//! roof, then 100 tries hang columns from netherrack or wart.
//!
//! Both columns end in the growing head block (`twisting_vines` / `weeping_vines`) with an `age`
//! of 17 to 25 drawn only for the head, under or over plant blocks. A column stops early, head
//! placed, when the next cell along it is not air.

use anyhow::{bail, Result};
use oxide_core::{BlockPos, BlockState, RandomSource, ResourceLocation};
use oxide_datapack::nether_feature::TwistingVinesConfig;
use oxide_datapack::BlockFacts;

use super::super::context::FeatureContext;
use super::{default_state, is_block_at, is_empty, mth_next_int, offset, DIRECTIONS};

/// The head's age range, `minAge`/`maxAge` in both vine features.
const MIN_AGE: i32 = 17;
const MAX_AGE: i32 = 25;

/// A head and its plant block, and the direction the column grows (`1` up, `-1` down).
#[derive(Debug)]
struct Column {
    head: BlockState,
    plant: BlockState,
    step: i32,
}

impl Column {
    fn compile(facts: &BlockFacts, head: &str, step: i32) -> Result<Self> {
        Ok(Self {
            head: default_state(facts, head)?,
            plant: default_state(facts, &format!("{head}_plant"))?,
            step,
        })
    }

    /// `placeWeepingVinesColumn` of either feature. Twisting vines count `1..=total`, weeping
    /// vines `0..=total`, one more cell; `first` is where the count starts.
    fn place(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        mut pos: BlockPos,
        first: i32,
        total: i32,
    ) {
        for height in first..=total {
            if is_empty(cx, pos) {
                if height == total || !is_empty(cx, offset(pos, 0, self.step, 0)) {
                    let age = mth_next_int(cx.random, MIN_AGE, MAX_AGE);
                    cx.region
                        .set_block(pos, self.head.clone().with_property("age", age.to_string()));
                    break;
                }
                cx.region.set_block(pos, self.plant.clone());
            }
            pos = offset(pos, 0, self.step, 0);
        }
    }
}

/// The column height both features roll: `Mth.nextInt(1, max)`, doubled one time in six, then
/// cut to one block one time in five.
fn roll_height(cx: &mut FeatureContext<'_, '_, '_>, max: i32) -> i32 {
    let mut height = mth_next_int(cx.random, 1, max);
    if cx.random.next_int_bounded(6) == 0 {
        height *= 2;
    }
    if cx.random.next_int_bounded(5) == 0 {
        height = 1;
    }
    height
}

#[derive(Debug)]
pub struct TwistingVinesFeature {
    spread_width: i32,
    spread_height: i32,
    max_height: i32,
    column: Column,
    /// The blocks a vine may stand on.
    grounds: [ResourceLocation; 3],
}

impl TwistingVinesFeature {
    pub fn compile(config: &TwistingVinesConfig, facts: &BlockFacts) -> Result<Self> {
        // `ExtraCodecs.POSITIVE_INT` on all three fields.
        if config.spread_width <= 0 || config.spread_height <= 0 || config.max_height <= 0 {
            bail!("twisting_vines spread_width, spread_height and max_height must be positive");
        }
        Ok(Self {
            spread_width: config.spread_width,
            spread_height: config.spread_height,
            max_height: config.max_height,
            column: Column::compile(facts, "twisting_vines", 1)?,
            grounds: ["netherrack", "warped_nylium", "warped_wart_block"]
                .map(ResourceLocation::minecraft),
        })
    }

    /// `TwistingVinesFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if self.is_invalid(cx, origin) {
            return false;
        }
        let (width, height) = (self.spread_width, self.spread_height);
        for _ in 0..width.wrapping_mul(width) {
            let dx = mth_next_int(cx.random, -width, width);
            let dy = mth_next_int(cx.random, -height, height);
            let dz = mth_next_int(cx.random, -width, width);
            let Some(pos) = first_air_above_ground(cx, offset(origin, dx, dy, dz)) else {
                continue;
            };
            if self.is_invalid(cx, pos) {
                continue;
            }
            let vine_height = roll_height(cx, self.max_height);
            self.column.place(cx, pos, 1, vine_height);
        }
        true
    }

    /// `isInvalidPlacementLocation`: not air, or not on one of the grounds.
    fn is_invalid(&self, cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        if !is_empty(cx, pos) {
            return true;
        }
        let below = offset(pos, 0, -1, 0);
        !self.grounds.iter().any(|name| is_block_at(cx, below, name))
    }
}

/// `findFirstAirBlockAboveGround`: steps down from one below `pos` through air to the first
/// non-air block and answers the cell above it; `None` when the walk leaves the build height.
fn first_air_above_ground(cx: &FeatureContext<'_, '_, '_>, mut pos: BlockPos) -> Option<BlockPos> {
    loop {
        pos = offset(pos, 0, -1, 0);
        if cx.outside_build_height(pos.y) {
            return None;
        }
        if !is_empty(cx, pos) {
            return Some(offset(pos, 0, 1, 0));
        }
    }
}

#[derive(Debug)]
pub struct WeepingVinesFeature {
    wart: BlockState,
    netherrack: ResourceLocation,
    column: Column,
}

impl WeepingVinesFeature {
    pub fn compile(facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            wart: default_state(facts, "nether_wart_block")?,
            netherrack: ResourceLocation::minecraft("netherrack"),
            column: Column::compile(facts, "weeping_vines", -1)?,
        })
    }

    /// `WeepingVinesFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if !is_empty(cx, origin) || !self.is_roof(cx, offset(origin, 0, 1, 0)) {
            return false;
        }
        self.place_roof_wart(cx, origin);
        self.place_roof_vines(cx, origin);
        true
    }

    /// Netherrack or nether wart block: what wart spreads over and vines hang from.
    fn is_roof(&self, cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        is_block_at(cx, pos, &self.netherrack) || is_block_at(cx, pos, &self.wart.name)
    }

    /// `placeRoofNetherWart`.
    fn place_roof_wart(&self, cx: &mut FeatureContext<'_, '_, '_>, origin: BlockPos) {
        cx.region.set_block(origin, self.wart.clone());
        for _ in 0..200 {
            let dx = cx.random.next_int_bounded(6) - cx.random.next_int_bounded(6);
            let dy = cx.random.next_int_bounded(2) - cx.random.next_int_bounded(5);
            let dz = cx.random.next_int_bounded(6) - cx.random.next_int_bounded(6);
            let pos = offset(origin, dx, dy, dz);
            if !is_empty(cx, pos) {
                continue;
            }
            let mut neighbours = 0;
            for [nx, ny, nz] in DIRECTIONS {
                if self.is_roof(cx, offset(pos, nx, ny, nz)) {
                    neighbours += 1;
                }
                if neighbours > 1 {
                    break;
                }
            }
            if neighbours == 1 {
                cx.region.set_block(pos, self.wart.clone());
            }
        }
    }

    /// `placeRoofWeepingVines`.
    fn place_roof_vines(&self, cx: &mut FeatureContext<'_, '_, '_>, origin: BlockPos) {
        for _ in 0..100 {
            let dx = cx.random.next_int_bounded(8) - cx.random.next_int_bounded(8);
            let dy = cx.random.next_int_bounded(2) - cx.random.next_int_bounded(7);
            let dz = cx.random.next_int_bounded(8) - cx.random.next_int_bounded(8);
            let pos = offset(origin, dx, dy, dz);
            if is_empty(cx, pos) && self.is_roof(cx, offset(pos, 0, 1, 0)) {
                let vine_height = roll_height(cx, 8);
                self.column.place(cx, pos, 0, vine_height);
            }
        }
    }
}
