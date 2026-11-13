//! `TemplateFeature`: one template picked by weight, turned by one of its allowed rotations,
//! centred on the origin and placed with no bounding box and no processors.

use oxide_core::rotation::{Direction, Rotation};
use oxide_core::RandomSource;

use super::structure_template::{offset, place_in_world, PlaceSettings};
use super::TemplateRef;
use crate::decorator::context::FeatureContext;

#[derive(Debug)]
pub struct TemplateFeature {
    /// `(template, rotations, weight)` in list order; rotations and the total weight are
    /// non-empty and positive (checked at compile time, where vanilla would throw).
    pub(super) entries: Vec<(TemplateRef, Vec<Rotation>, i32)>,
    pub(super) total_weight: i32,
}

impl TemplateFeature {
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        // `WeightedList.getRandomOrThrow`: one bounded draw, the entry whose cumulative weight
        // passes it.
        let mut remaining = cx.random.next_int_bounded(self.total_weight);
        let Some((template_ref, rotations, _)) = self.entries.iter().find(|(_, _, weight)| {
            remaining -= weight;
            remaining < 0
        }) else {
            return false;
        };
        // `Util.getRandom(rotations, random)`.
        let rotation = rotations[cx.random.next_int_bounded(rotations.len() as i32) as usize];
        let template = template_ref.resolve(cx);

        // `getRotatedOffset`: half the size along each horizontal axis, toward that axis'
        // negative direction as turned by the rotation.
        let size = template.raw_size();
        let mut pos = cx.origin;
        for (negative, half) in [(Direction::West, size[0] / 2), (Direction::North, size[2] / 2)] {
            let turned = rotation.rotate(negative);
            pos = offset(pos, turned.step_x() * half, turned.step_y() * half, turned.step_z() * half);
        }
        let settings = PlaceSettings { rotation, bounding_box: None, processors: &[] };
        place_in_world(template, &template_ref.id, cx, pos, &settings)
    }
}
