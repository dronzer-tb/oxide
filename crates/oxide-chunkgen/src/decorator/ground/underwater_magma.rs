//! `underwater_magma`: magma blocks on the sea floor under a water column.
//!
//! The feature scans down through water from the origin to the floor, then visits every cell of
//! the cube of `placement_radius_around_floor` around the floor block. Each cell rolls the
//! per-position probability, and a cell that wins becomes magma when it is neither water nor air
//! and its lower and four side neighbours present a full face towards it.
//!
//! PARITY-CHECK: "present a full face" is vanilla's occlusion shape test
//! (`Block.isFaceFull(state.getOcclusionShape(), direction)`). The facts carry no per-face
//! occlusion, only `SolidRender` (a full cube that occludes), which agrees for every full cube
//! and disagrees only for partial-shape blocks that still have one full face (a slab's bottom, a
//! stair's back). The terrain this runs on, a sea floor before vegetation, has none; confirm
//! against a real 26.2 run over a coral or shipwreck area.

use anyhow::{ensure, Result};
use oxide_core::{BlockPos, BlockState, RandomSource};
use oxide_datapack::ground_feature::UnderwaterMagmaConfig;
use oxide_datapack::{BlockFact, BlockFacts};

use super::super::context::FeatureContext;
use super::{between_closed, default_state, is_air, is_block_at, offset};

#[derive(Debug)]
pub struct UnderwaterMagmaFeature {
    floor_search_range: i32,
    radius: i32,
    probability: f32,
    magma: BlockState,
}

impl UnderwaterMagmaFeature {
    pub fn compile(config: &UnderwaterMagmaConfig, facts: &BlockFacts) -> Result<Self> {
        ensure!(
            (0.0..=1.0).contains(&config.placement_probability_per_valid_position),
            "underwater_magma probability {} is outside 0..=1",
            config.placement_probability_per_valid_position
        );
        ensure!(
            (0..=512).contains(&config.floor_search_range),
            "underwater_magma floor_search_range {} is outside 0..=512",
            config.floor_search_range
        );
        ensure!(
            (0..=64).contains(&config.placement_radius_around_floor),
            "underwater_magma placement_radius_around_floor {} is outside 0..=64",
            config.placement_radius_around_floor
        );
        Ok(Self {
            floor_search_range: config.floor_search_range,
            radius: config.placement_radius_around_floor,
            probability: config.placement_probability_per_valid_position,
            magma: default_state(facts, "magma_block")?,
        })
    }

    /// `UnderwaterMagmaFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if !is_block_at(cx, origin, "water") {
            return false;
        }
        let Some(floor) = self.floor(cx, origin) else {
            return false;
        };
        let center = BlockPos::new(origin.x, floor, origin.z);
        let from = offset(center, -self.radius, -self.radius, -self.radius);
        let to = offset(center, self.radius, self.radius, self.radius);
        let mut placed = false;
        for pos in between_closed(from, to) {
            if cx.random.next_float() < self.probability && self.valid(cx, pos) {
                cx.region.set_block_ref(pos, &self.magma);
                placed = true;
            }
        }
        placed
    }

    /// The Y of the first block that is not water below `pos`, searching at most
    /// `floor_search_range` blocks; `None` when the water runs deeper.
    fn floor(&self, cx: &FeatureContext<'_, '_, '_>, from: BlockPos) -> Option<i32> {
        let mut pos = from;
        let mut distance = 1;
        while distance < self.floor_search_range {
            if !is_block_at(cx, pos, "water") {
                break;
            }
            pos = offset(pos, 0, -1, 0);
            distance += 1;
        }
        (!is_block_at(cx, pos, "water")).then_some(pos.y)
    }

    /// `UnderwaterMagmaFeature.isValidPlacement`.
    fn valid(&self, cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        if is_block_at(cx, pos, "water") || is_air(cx, pos) {
            return false;
        }
        if !occludes(cx, offset(pos, 0, -1, 0)) {
            return false;
        }
        [[0, -1], [1, 0], [0, 1], [-1, 0]]
            .iter()
            .all(|[dx, dz]| occludes(cx, offset(pos, *dx, 0, *dz)))
    }
}

/// Whether the block at `pos` presents a full face to a neighbour (see the module doc).
fn occludes(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
    cx.region.is(pos, BlockFact::SolidRender)
}
