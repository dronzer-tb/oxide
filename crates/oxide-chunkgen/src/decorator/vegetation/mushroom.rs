//! `huge_red_mushroom` and `huge_brown_mushroom`: a stem with a cap, written only into air and
//! mushroom-replaceable blocks after the whole volume has been checked clear.
//!
//! Red mushrooms cap with a dome of rings, brown mushrooms with a flat plate; both set the cap's
//! six face properties from where each block sits so the inside faces stay hidden.

use anyhow::{Context, Result};
use oxide_core::{RandomSource, BlockPos, BlockState, ResourceLocation};
use oxide_datapack::vegetation_feature::HugeMushroomConfig;
use oxide_datapack::BlockFacts;

use super::geometry::offset;
use super::support::{in_tag, is_air, sample_state};
use crate::decorator::context::FeatureContext;
use crate::decorator::predicate::Predicate;
use crate::decorator::state_provider::StateProvider;

#[derive(Debug)]
pub struct MushroomFeature {
    red: bool,
    cap: StateProvider,
    stem: StateProvider,
    foliage_radius: i32,
    can_place_on: Predicate,
    leaves: ResourceLocation,
    replaceable: ResourceLocation,
}

impl MushroomFeature {
    pub fn compile(config: &HugeMushroomConfig, red: bool, facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            red,
            cap: StateProvider::compile(&config.cap_provider, facts).context("cap_provider")?,
            stem: StateProvider::compile(&config.stem_provider, facts).context("stem_provider")?,
            foliage_radius: config.foliage_radius,
            can_place_on: Predicate::compile(&config.can_place_on, facts).context("can_place_on")?,
            leaves: ResourceLocation::minecraft("leaves"),
            replaceable: ResourceLocation::minecraft("replaceable_by_mushrooms"),
        })
    }

    /// `getTreeRadiusForHeight`, which the clearance check calls with a trunk and tree height of
    /// -1.
    fn radius_for_height(&self, tree_height: i32, leaf_radius: i32, yo: i32) -> i32 {
        if self.red {
            if (yo < tree_height && yo >= tree_height - 3) || yo == tree_height {
                leaf_radius
            } else {
                0
            }
        } else if yo <= 3 {
            0
        } else {
            leaf_radius
        }
    }

    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let mut height = cx.random.next_int_bounded(3) + 4;
        if cx.random.next_int_bounded(12) == 0 {
            height *= 2;
        }
        if origin.y < cx.region.min_y().wrapping_add(1)
            || origin.y.wrapping_add(height).wrapping_add(1) > cx.region.max_y()
        {
            return false;
        }
        if !self.can_place_on.test(cx, offset(origin, 0, -1, 0)) {
            return false;
        }
        if !self.volume_is_clear(cx, origin, height) {
            return false;
        }
        let r = self.foliage_radius;
        if self.red {
            for dy in height - 3..=height {
                let radius = if dy < height { r } else { r - 1 };
                let center = r - 2;
                for dx in -radius..=radius {
                    for dz in -radius..=radius {
                        let x_edge = dx == -radius || dx == radius;
                        let z_edge = dz == -radius || dz == radius;
                        if dy >= height || x_edge != z_edge {
                            let Some(state) = sample_state(cx, &self.cap, origin) else { return false };
                            let state = with_faces(
                                state,
                                &[
                                    ("west", dx < -center),
                                    ("east", dx > center),
                                    ("north", dz < -center),
                                    ("south", dz > center),
                                    ("up", dy >= height - 1),
                                ],
                            );
                            self.place_block(cx, offset(origin, dx, dy, dz), state);
                        }
                    }
                }
            }
        } else {
            for dx in -r..=r {
                for dz in -r..=r {
                    let (min_x, max_x, min_z, max_z) = (dx == -r, dx == r, dz == -r, dz == r);
                    let x_edge = min_x || max_x;
                    let z_edge = min_z || max_z;
                    if !x_edge || !z_edge {
                        let west = min_x || (z_edge && dx == 1 - r);
                        let east = max_x || (z_edge && dx == r - 1);
                        let north = min_z || (x_edge && dz == 1 - r);
                        let south = max_z || (x_edge && dz == r - 1);
                        let Some(state) = sample_state(cx, &self.cap, origin) else { return false };
                        let state = with_faces(
                            state,
                            &[("west", west), ("east", east), ("north", north), ("south", south)],
                        );
                        self.place_block(cx, offset(origin, dx, height, dz), state);
                    }
                }
            }
        }
        for dy in 0..height {
            let Some(state) = sample_state(cx, &self.stem, origin) else { return false };
            self.place_block(cx, offset(origin, 0, dy, 0), state);
        }
        true
    }

    /// `isValidPosition`'s volume test: every block the stem and cap could occupy is air or
    /// leaves.
    fn volume_is_clear(&self, cx: &FeatureContext<'_, '_, '_>, origin: BlockPos, height: i32) -> bool {
        for dy in 0..=height {
            let radius = self.radius_for_height(-1, self.foliage_radius, dy);
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let pos = offset(origin, dx, dy, dz);
                    if !is_air(cx, pos) && !in_tag(cx, pos, &self.leaves) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Writes `state` only over air or a block tagged `replaceable_by_mushrooms`.
    fn place_block(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos, state: BlockState) {
        if is_air(cx, pos) || in_tag(cx, pos, &self.replaceable) {
            cx.region.set_block(pos, state);
        }
    }
}

/// Sets each named face property, but only when the state has every one of them: a cap provider
/// naming some other block is left as the provider wrote it.
fn with_faces(mut state: BlockState, faces: &[(&str, bool)]) -> BlockState {
    if faces.iter().all(|(name, _)| state.properties.contains_key(*name)) {
        for (name, value) in faces {
            state.properties.insert((*name).to_string(), value.to_string());
        }
    }
    state
}
