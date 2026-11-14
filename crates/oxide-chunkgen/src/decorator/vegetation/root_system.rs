//! `root_system`: a nested tree planted on top of a column of rooted dirt, with hanging roots
//! scattered around the point where the column starts.
//!
//! The feature climbs from the origin until it finds a spot where the tree is allowed and has
//! room, places the tree there, and only then fills the column below it with root blocks. Hanging
//! roots go in last, on any air block whose ceiling can hold them.

use std::sync::Arc;

use anyhow::{Context, Result};
use oxide_core::{BlockPos, HeightmapType, RandomSource};
use oxide_datapack::feature::Holder;
use oxide_datapack::placement::PlacedFeature;
use oxide_datapack::vegetation_feature::RootSystemConfig;
use oxide_datapack::{BlockFact, BlockFacts};

use super::geometry::{offset, relative, Dir};
use super::support::{face_full, is_air, place_nested, sample_state, survives, Shape};
use crate::decorator::context::{block_in, FeatureContext};
use crate::decorator::feature::Placed;
use crate::decorator::predicate::{IdSet, Predicate};
use crate::decorator::state_provider::StateProvider;

#[derive(Debug)]
pub struct RootSystemFeature {
    tree: Arc<Placed>,
    required_vertical_space_for_tree: i32,
    root_radius: i32,
    root_replaceable: IdSet,
    root_state_provider: StateProvider,
    root_placement_attempts: i32,
    root_column_max_height: i32,
    hanging_root_radius: i32,
    hanging_roots_vertical_span: i32,
    hanging_root_state_provider: StateProvider,
    hanging_root_placement_attempts: i32,
    allowed_vertical_water_for_tree: i32,
    allowed_tree_position: Predicate,
    level_test_distance: i32,
    max_level_deviation: i32,
}

impl RootSystemFeature {
    pub fn compile(
        config: &RootSystemConfig,
        facts: &BlockFacts,
        placed: &mut dyn FnMut(&Holder<PlacedFeature>) -> Result<Arc<Placed>>,
    ) -> Result<Self> {
        Ok(Self {
            tree: placed(&config.feature).context("feature")?,
            required_vertical_space_for_tree: config.required_vertical_space_for_tree,
            root_radius: config.root_radius,
            root_replaceable: IdSet::compile(&config.root_replaceable)
                .context("root_replaceable")?,
            root_state_provider: StateProvider::compile(&config.root_state_provider, facts)
                .context("root_state_provider")?,
            root_placement_attempts: config.root_placement_attempts,
            root_column_max_height: config.root_column_max_height,
            hanging_root_radius: config.hanging_root_radius,
            hanging_roots_vertical_span: config.hanging_roots_vertical_span,
            hanging_root_state_provider: StateProvider::compile(
                &config.hanging_root_state_provider,
                facts,
            )
            .context("hanging_root_state_provider")?,
            hanging_root_placement_attempts: config.hanging_root_placement_attempts,
            allowed_vertical_water_for_tree: config.allowed_vertical_water_for_tree,
            allowed_tree_position: Predicate::compile(&config.allowed_tree_position, facts)
                .context("allowed_tree_position")?,
            level_test_distance: config.level_test_distance,
            max_level_deviation: config.max_level_deviation,
        })
    }

    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if !is_air(cx, origin) {
            return false;
        }
        if self.place_dirt_and_tree(cx, origin) {
            self.place_hanging_roots(cx, origin);
        }
        true
    }

    /// `RootSystemFeature.spaceForTree`: clear air (or a little water) above, and a level
    /// enough surface at the four corners of the test square.
    fn space_for_tree(&self, cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        let mut column = pos;
        for i in 1..=self.required_vertical_space_for_tree {
            column = offset(column, 0, 1, 0);
            let water = cx.region.is(column, BlockFact::Water);
            if !(is_air(cx, column) || (i < self.allowed_vertical_water_for_tree && water)) {
                return false;
            }
        }
        if self.level_test_distance > 0 {
            // `Direction.from2DDataValue`: south, west, north, east.
            for direction in [Dir::South, Dir::West, Dir::North, Dir::East] {
                let corner = relative(pos, direction, self.level_test_distance);
                let below = offset(corner, 0, -self.max_level_deviation, 0);
                let above = offset(corner, 0, self.max_level_deviation, 0);
                if is_air(cx, below) || !is_air(cx, above) {
                    return false;
                }
            }
        }
        true
    }

    fn place_dirt_and_tree(&self, cx: &mut FeatureContext<'_, '_, '_>, origin: BlockPos) -> bool {
        let mut working = origin;
        for y in 0..self.root_column_max_height {
            working = offset(working, 0, 1, 0);
            let Some(surface) = cx
                .region
                .height(HeightmapType::WorldSurface, working.x, working.z)
            else {
                return false;
            };
            if surface < working.y {
                return false;
            }
            if self.allowed_tree_position.test(cx, working) && self.space_for_tree(cx, working) {
                let below = offset(working, 0, -1, 0);
                if cx.region.is(below, BlockFact::Lava) || !cx.region.is(below, BlockFact::Solid) {
                    return false;
                }
                if place_nested(cx, &self.tree, working) {
                    let target = origin.y.wrapping_add(y);
                    let mut level = origin.y;
                    while level < target {
                        self.place_root_column(cx, BlockPos::new(origin.x, level, origin.z));
                        level += 1;
                    }
                    return true;
                }
            }
        }
        false
    }

    fn place_root_column(&self, cx: &mut FeatureContext<'_, '_, '_>, column: BlockPos) {
        let r = self.root_radius;
        for _ in 0..self.root_placement_attempts {
            let dx = cx.random.next_int_bounded(r) - cx.random.next_int_bounded(r);
            let dz = cx.random.next_int_bounded(r) - cx.random.next_int_bounded(r);
            let pos = offset(column, dx, 0, dz);
            if self
                .root_replaceable
                .contains_block(cx.env, &block_in(cx.region, pos).name)
            {
                let Some(state) = sample_state(cx, &self.root_state_provider, pos) else {
                    continue;
                };
                cx.region.set_block(pos, state);
            }
        }
    }

    fn place_hanging_roots(&self, cx: &mut FeatureContext<'_, '_, '_>, origin: BlockPos) {
        let (r, span) = (self.hanging_root_radius, self.hanging_roots_vertical_span);
        for _ in 0..self.hanging_root_placement_attempts {
            let dx = cx.random.next_int_bounded(r) - cx.random.next_int_bounded(r);
            let dy = cx.random.next_int_bounded(span) - cx.random.next_int_bounded(span);
            let dz = cx.random.next_int_bounded(r) - cx.random.next_int_bounded(r);
            let pos = offset(origin, dx, dy, dz);
            if !is_air(cx, pos) {
                continue;
            }
            let Some(state) = sample_state(cx, &self.hanging_root_state_provider, pos) else {
                continue;
            };
            if survives(cx, &state, pos)
                && face_full(cx, offset(pos, 0, 1, 0), Dir::Down, Shape::Support)
            {
                cx.region.set_block(pos, state);
            }
        }
    }
}
