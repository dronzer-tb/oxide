//! `nether_forest_vegetation`: roots, fungi and sprouts scattered over nylium.
//!
//! On an origin over nylium, `spread_width²` tries each pick a cell in a box around it, roll the
//! state for that cell, and place it when the cell is air above the world's lowest layer and the
//! state can survive there. The state is rolled before the cell is tested, so every try draws
//! from the provider whether it places or not.

use anyhow::{bail, Result};
use oxide_core::{RandomSource, ResourceLocation};
use oxide_datapack::nether_feature::NetherForestVegetationConfig;
use oxide_datapack::BlockFacts;

use super::super::context::{block_in, FeatureContext};
use super::super::state_provider::{StateProvider, Unsupported};
use super::{is_empty, offset};

#[derive(Debug)]
pub struct ForestVegetationFeature {
    provider: StateProvider,
    spread_width: i32,
    spread_height: i32,
    nylium: ResourceLocation,
}

impl ForestVegetationFeature {
    pub fn compile(config: &NetherForestVegetationConfig, facts: &BlockFacts) -> Result<Self> {
        // `ExtraCodecs.POSITIVE_INT` on both spreads.
        if config.spread_width <= 0 || config.spread_height <= 0 {
            bail!("nether_forest_vegetation spread_width and spread_height must be positive");
        }
        Ok(Self {
            provider: StateProvider::compile(&config.state_provider, facts)?,
            spread_width: config.spread_width,
            spread_height: config.spread_height,
            nylium: ResourceLocation::minecraft("nylium"),
        })
    }

    /// `NetherForestVegetationFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let below = &block_in(cx.region, offset(origin, 0, -1, 0)).name;
        if !cx.env.block_tags.contains(&self.nylium, below) {
            return false;
        }
        let min_y = cx.region.min_y();
        // `Level.getMaxY` is inclusive; the region's `max_y` is one past the top.
        let max_y = cx.region.max_y() - 1;
        if origin.y < min_y + 1 || origin.y + 1 > max_y {
            return false;
        }
        let (width, height) = (self.spread_width, self.spread_height);
        let mut placed = 0;
        for _ in 0..width.wrapping_mul(width) {
            let dx = cx.random.next_int_bounded(width) - cx.random.next_int_bounded(width);
            let dy = cx.random.next_int_bounded(height) - cx.random.next_int_bounded(height);
            let dz = cx.random.next_int_bounded(width) - cx.random.next_int_bounded(width);
            let pos = offset(origin, dx, dy, dz);
            let state = match self.provider.state(cx, pos) {
                Ok(Some(state)) => state,
                // Vanilla's `getState` throws where a rule-based provider matches nothing; the
                // draws after it cannot be followed, so the feature stops here.
                Ok(None) => {
                    cx.unsupported.record("provider", "rule_based_state_provider_without_match");
                    return placed > 0;
                }
                Err(Unsupported(why)) => {
                    cx.unsupported.record("provider", why);
                    return placed > 0;
                }
            };
            if !is_empty(cx, pos) || pos.y <= min_y {
                continue;
            }
            match cx.env.behaviours.can_survive(&state, cx.region, pos) {
                Some(true) => {
                    cx.region.set_block(pos, state);
                    placed += 1;
                }
                Some(false) => {}
                None => cx.unsupported.record("survival", &state.name.to_string()),
            }
        }
        placed > 0
    }
}
