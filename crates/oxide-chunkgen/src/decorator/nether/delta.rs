//! `delta_feature`: a patch of lava ringed by magma on a Nether floor.
//!
//! The patch visits the cells of a diamond in order of Manhattan distance from the origin. A
//! cell is "clear" when it is none of the blocked blocks and has air above and a non-air block on
//! each other face. A clear cell gets the rim block (when a rim was rolled), and the cell shifted
//! by the rim size gets the contents when it is clear too.

use anyhow::{Context, Result};
use oxide_core::{BlockPos, BlockState, RandomSource, ResourceLocation};
use oxide_datapack::nether_feature::DeltaConfig;
use oxide_datapack::placement::IntProvider;
use oxide_datapack::{BlockFact, BlockFacts};

use super::super::context::{block_in, FeatureContext};
use super::super::provider::sample_int;
use super::super::state_provider::state_from_spec;
use super::{has, offset};

/// `DeltaFeature.CANNOT_REPLACE`; the contents block is added at compile time.
const BLOCKED: [&str; 7] =
    ["bedrock", "nether_bricks", "nether_brick_fence", "nether_brick_stairs", "nether_wart", "chest", "spawner"];

/// The six faces, up first; the delta's cell test needs air on exactly the up face.
const FACES: [([i32; 3], bool); 6] = [
    ([0, -1, 0], false),
    ([0, 1, 0], true),
    ([0, 0, -1], false),
    ([0, 0, 1], false),
    ([-1, 0, 0], false),
    ([1, 0, 0], false),
];

#[derive(Debug)]
pub struct DeltaFeature {
    contents: BlockState,
    rim: BlockState,
    size: IntProvider,
    rim_size: IntProvider,
    blocked: Vec<ResourceLocation>,
}

impl DeltaFeature {
    pub fn compile(config: &DeltaConfig, facts: &BlockFacts) -> Result<Self> {
        let contents = state_from_spec(&config.contents, facts).context("delta contents")?;
        let mut blocked: Vec<ResourceLocation> = BLOCKED.iter().map(|path| ResourceLocation::minecraft(*path)).collect();
        blocked.push(contents.name.clone());
        Ok(Self {
            contents,
            rim: state_from_spec(&config.rim, facts).context("delta rim")?,
            size: config.size.clone(),
            rim_size: config.rim_size.clone(),
            blocked,
        })
    }

    /// `DeltaFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let draw_rim = cx.random.next_double() < 0.9;
        let rim_x = if draw_rim { sample_int(&self.rim_size, cx.random) } else { 0 };
        let rim_z = if draw_rim { sample_int(&self.rim_size, cx.random) } else { 0 };
        let has_rim = draw_rim && rim_x != 0 && rim_z != 0;
        let size_x = sample_int(&self.size, cx.random);
        let size_z = sample_int(&self.size, cx.random);
        let mut placed = false;
        // `BlockPos.withinManhattan(origin, size_x, 0, size_z)`, stopping past the larger radius:
        // by distance, then x ascending, then +z before -z.
        for depth in 0..=size_x.max(size_z) {
            let max_x = size_x.min(depth);
            for x in -max_x..=max_x {
                let z = depth - x.abs();
                if z > size_z {
                    continue;
                }
                self.place_at(cx, offset(origin, x, 0, z), [rim_x, rim_z], has_rim, &mut placed);
                if z != 0 {
                    self.place_at(cx, offset(origin, x, 0, -z), [rim_x, rim_z], has_rim, &mut placed);
                }
            }
        }
        placed
    }

    fn place_at(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        pos: BlockPos,
        [rim_x, rim_z]: [i32; 2],
        has_rim: bool,
        placed: &mut bool,
    ) {
        if !self.is_clear(cx, pos) {
            return;
        }
        if has_rim {
            cx.region.set_block(pos, self.rim.clone());
            *placed = true;
        }
        let shifted = offset(pos, rim_x, 0, rim_z);
        if self.is_clear(cx, shifted) {
            cx.region.set_block(shifted, self.contents.clone());
            *placed = true;
        }
    }

    /// `DeltaFeature.isClear`.
    fn is_clear(&self, cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        let name = &block_in(cx.region, pos).name;
        if self.blocked.contains(name) {
            return false;
        }
        FACES.iter().all(|([dx, dy, dz], want_air)| {
            has(cx, offset(pos, *dx, *dy, *dz), BlockFact::Air) == *want_air
        })
    }
}
