//! `huge_fungus`: a crimson or warped fungus, a stem topped by a wart-block hat.
//!
//! Placement needs the fungus's valid base block (nylium) directly below. The stem is a one- or
//! three-wide column and the hat a stack of layers whose radius shrinks towards the top; the
//! wart-block hat of the crimson fungus also hangs weeping vines. Vanilla's `planted` variant
//! (bonemeal) destroys the blocks it replaces and is left unsupported, see the module doc of
//! [`super`].
//!
//! PARITY-CHECK: vanilla refuses a natural fungus whose top would reach `getGenDepth`, which this
//! port reads as the region's world height (`max_y - min_y`); in the Nether that is 128 for both
//! the dimension and its noise settings. Confirm against a real 26.2 run if a pack gives the
//! two different values.

use anyhow::{Context, Result};
use oxide_core::{BlockPos, BlockState, RandomSource, ResourceLocation};
use oxide_datapack::nether_feature::HugeFungusConfig;
use oxide_datapack::{BlockFact, BlockFacts};

use super::super::context::{block_in, FeatureContext};
use super::super::predicate::Predicate;
use super::super::provider::between_inclusive;
use super::super::state_provider::state_from_spec;
use super::{default_state, has, is_block_at, offset};

#[derive(Debug)]
pub struct HugeFungusFeature {
    valid_base: ResourceLocation,
    stem: BlockState,
    hat: BlockState,
    decor: BlockState,
    replaceable: Predicate,
    weeping_vines: BlockState,
    weeping_vines_plant: BlockState,
    /// Whether the hat is `nether_wart_block`, the only hat that grows vines.
    vines: bool,
}

impl HugeFungusFeature {
    /// `None` for a planted fungus, which stays unsupported.
    pub fn compile(config: &HugeFungusConfig, facts: &BlockFacts) -> Result<Option<Self>> {
        if config.planted {
            return Ok(None);
        }
        let hat = state_from_spec(&config.hat_state, facts).context("huge fungus hat")?;
        Ok(Some(Self {
            valid_base: state_from_spec(&config.valid_base_block, facts)
                .context("huge fungus base")?
                .name,
            stem: state_from_spec(&config.stem_state, facts).context("huge fungus stem")?,
            vines: hat.name == ResourceLocation::minecraft("nether_wart_block"),
            hat,
            decor: state_from_spec(&config.decor_state, facts).context("huge fungus decor")?,
            replaceable: Predicate::compile(&config.replaceable_blocks, facts)
                .context("huge fungus replaceable")?,
            weeping_vines: default_state(facts, "weeping_vines")?,
            weeping_vines_plant: default_state(facts, "weeping_vines_plant")?,
        }))
    }

    /// `HugeFungusFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if !is_block_at(cx, offset(origin, 0, -1, 0), &self.valid_base) {
            return false;
        }
        let mut height = between_inclusive(cx.random, 4, 13);
        if cx.random.next_int_bounded(12) == 0 {
            height *= 2;
        }
        let depth = cx.region.max_y() - cx.region.min_y();
        if origin.y + height + 1 >= depth {
            return false;
        }
        let huge = cx.random.next_float() < 0.06;
        cx.region
            .set_block(origin, BlockState::new(ResourceLocation::minecraft("air")));
        self.place_stem(cx, origin, height, huge);
        self.place_hat(cx, origin, height, huge);
        true
    }

    /// `HugeFungusFeature.isReplaceable`: a replaceable block, or (for the stem) one the config
    /// lists.
    fn is_replaceable(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        pos: BlockPos,
        plants: bool,
    ) -> bool {
        if has(cx, pos, BlockFact::Replaceable) {
            return true;
        }
        plants && self.replaceable.test(cx, pos)
    }

    fn place_stem(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        origin: BlockPos,
        height: i32,
        huge: bool,
    ) {
        let radius = i32::from(huge);
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let corner = huge && dx.abs() == radius && dz.abs() == radius;
                for dy in 0..height {
                    let pos = offset(origin, dx, dy, dz);
                    if !self.is_replaceable(cx, pos, true) {
                        continue;
                    }
                    if corner {
                        if cx.random.next_float() < 0.1 {
                            cx.region.set_block(pos, self.stem.clone());
                        }
                    } else {
                        cx.region.set_block(pos, self.stem.clone());
                    }
                }
            }
        }
    }

    fn place_hat(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        origin: BlockPos,
        height: i32,
        huge: bool,
    ) {
        let hat_height = (cx.random.next_int_bounded(1 + height / 3) + 5).min(height);
        let start = height - hat_height;
        for dy in start..=height {
            let mut radius = if dy < height - cx.random.next_int_bounded(3) {
                2
            } else {
                1
            };
            if hat_height > 8 && dy < start + 4 {
                radius = 3;
            }
            if huge {
                radius += 1;
            }
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let edge_x = dx == -radius || dx == radius;
                    let edge_z = dz == -radius || dz == radius;
                    let inside = !edge_x && !edge_z && dy != height;
                    let corner = edge_x && edge_z;
                    let bottom = dy < start + 3;
                    let pos = offset(origin, dx, dy, dz);
                    if !self.is_replaceable(cx, pos, false) {
                        continue;
                    }
                    if bottom {
                        if !inside {
                            self.place_hat_drop(cx, pos);
                        }
                    } else if inside {
                        self.place_hat_block(cx, pos, 0.1, 0.2, if self.vines { 0.1 } else { 0.0 });
                    } else if corner {
                        self.place_hat_block(
                            cx,
                            pos,
                            0.01,
                            0.7,
                            if self.vines { 0.083 } else { 0.0 },
                        );
                    } else {
                        self.place_hat_block(
                            cx,
                            pos,
                            5.0e-4,
                            0.98,
                            if self.vines { 0.07 } else { 0.0 },
                        );
                    }
                }
            }
        }
    }

    /// `HugeFungusFeature.placeHatBlock`: decor, else hat (and maybe vines).
    fn place_hat_block(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        pos: BlockPos,
        decor: f32,
        hat: f32,
        vines: f32,
    ) {
        if cx.random.next_float() < decor {
            cx.region.set_block(pos, self.decor.clone());
        } else if cx.random.next_float() < hat {
            cx.region.set_block(pos, self.hat.clone());
            if cx.random.next_float() < vines {
                self.place_weeping_vines(cx, pos);
            }
        }
    }

    /// `HugeFungusFeature.placeHatDropBlock`: a hat block continues the one below it, else it
    /// starts with chance 0.15.
    fn place_hat_drop(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos) {
        if block_in(cx.region, offset(pos, 0, -1, 0)).name == self.hat.name {
            cx.region.set_block(pos, self.hat.clone());
        } else if f64::from(cx.random.next_float()) < 0.15 {
            cx.region.set_block(pos, self.hat.clone());
            if self.vines && cx.random.next_int_bounded(11) == 0 {
                self.place_weeping_vines(cx, pos);
            }
        }
    }

    /// the weeping vines column a wart-block hat grows.
    fn place_weeping_vines(&self, cx: &mut FeatureContext<'_, '_, '_>, hat: BlockPos) {
        let mut pos = offset(hat, 0, -1, 0);
        if !has(cx, pos, BlockFact::Air) {
            return;
        }
        let mut goal = between_inclusive(cx.random, 1, 5);
        if cx.random.next_int_bounded(7) == 0 {
            goal *= 2;
        }
        for height in 0..=goal {
            if has(cx, pos, BlockFact::Air) {
                if height == goal || !has(cx, offset(pos, 0, -1, 0), BlockFact::Air) {
                    let age = between_inclusive(cx.random, 23, 25);
                    let tip = self
                        .weeping_vines
                        .clone()
                        .with_property("age", age.to_string());
                    cx.region.set_block(pos, tip);
                    break;
                }
                cx.region.set_block(pos, self.weeping_vines_plant.clone());
            }
            pos = offset(pos, 0, -1, 0);
        }
    }
}
