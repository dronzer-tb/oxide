//! `FossilFeature`: a buried skeleton and its ore overlay, sunk below the lowest ocean-floor
//! height under its footprint, refused when too many of its box's corners sit in air or fluid.

use oxide_core::rotation::Rotation;
use oxide_core::{BlockPos, HeightmapType, RandomSource, ResourceLocation};
use oxide_datapack::BlockFact;

use super::processor::Processor;
use super::structure_template::{offset, place_in_world, BoundingBox, PlaceSettings};
use super::TemplateRef;
use crate::decorator::context::{block_in, FeatureContext};

#[derive(Debug)]
pub struct FossilFeature {
    /// `(base, overlay)` pairs: vanilla indexes both lists with one draw.
    pub(super) pairs: Vec<(TemplateRef, TemplateRef)>,
    pub(super) fossil_processors: Vec<Processor>,
    pub(super) overlay_processors: Vec<Processor>,
    pub(super) max_empty_corners: i32,
}

impl FossilFeature {
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let rotation = Rotation::get_random(cx.random);
        let index = cx.random.next_int_bounded(self.pairs.len() as i32) as usize;
        let (base_ref, overlay_ref) = &self.pairs[index];
        let base = base_ref.resolve(cx);
        let overlay = overlay_ref.resolve(cx);

        let (min_y, max_y) = (cx.region.min_y(), cx.region.max_y() - 1);
        let (chunk_x, chunk_z) = (origin.x >> 4 << 4, origin.z >> 4 << 4);
        let bounds = BoundingBox {
            min: BlockPos::new(chunk_x - 16, min_y, chunk_z - 16),
            max: BlockPos::new(chunk_x + 15 + 16, max_y, chunk_z + 15 + 16),
        };
        let size = base.size(rotation);
        let low_corner = offset(origin, -size[0] / 2, 0, -size[2] / 2);
        let mut lowest_surface = origin.y;
        for x in 0..size[0] {
            for z in 0..size[2] {
                let Some(height) = cx.region.height(
                    HeightmapType::OceanFloorWg,
                    low_corner.x + x,
                    low_corner.z + z,
                ) else {
                    // Out of the window: vanilla's region would throw. Cannot happen for an
                    // origin in the centre chunk and a footprint under 32 blocks.
                    cx.unsupported
                        .record("fossil", "footprint outside the region");
                    return false;
                };
                lowest_surface = lowest_surface.min(height);
            }
        }
        let target_y = (lowest_surface - 15 - cx.random.next_int_bounded(10)).max(min_y + 10);
        let target = base.zero_position(
            BlockPos::new(low_corner.x, target_y, low_corner.z),
            rotation,
        );
        if count_empty_corners(cx, base.bounding_box(target, rotation)) > self.max_empty_corners {
            return false;
        }

        let mut settings = PlaceSettings::new(rotation, Some(bounds), &self.fossil_processors);
        place_in_world(base, &base_ref.id, cx, target, &settings);
        settings.processors = &self.overlay_processors;
        place_in_world(overlay, &overlay_ref.id, cx, target, &settings);
        true
    }
}

/// `countEmptyCorners`: air, or the `lava` / `water` blocks themselves.
fn count_empty_corners(cx: &FeatureContext<'_, '_, '_>, bounds: BoundingBox) -> i32 {
    let lava = ResourceLocation::minecraft("lava");
    let water = ResourceLocation::minecraft("water");
    bounds
        .corners()
        .into_iter()
        .filter(|&pos| {
            let state = block_in(cx.region, pos);
            cx.env.facts.is(state, BlockFact::Air) || state.name == lava || state.name == water
        })
        .count() as i32
}
