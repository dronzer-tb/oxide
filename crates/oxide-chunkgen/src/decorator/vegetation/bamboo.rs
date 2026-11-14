//! `bamboo`: a stalk of 5 to 16 blocks with a three-block leafy crown, optionally over a podzol
//! disc.
//!
//! PARITY-CHECK: the feature reports success as soon as the origin is air, even when bamboo
//! cannot survive there and nothing is placed. That is the reference implementation's (26.3)
//! behaviour; confirm it matches 26.2, where the return value only matters to a `sequence` or
//! selector wrapping this feature.

use anyhow::{Context, Result};
use oxide_core::{BlockPos, BlockState, HeightmapType, RandomSource, ResourceLocation};
use oxide_datapack::vegetation_feature::BambooConfig;
use oxide_datapack::BlockFacts;

use super::geometry::offset;
use super::support::{in_tag, is_air};
use crate::decorator::context::FeatureContext;

#[derive(Debug)]
pub struct BambooFeature {
    probability: f32,
    /// `Blocks.BAMBOO.defaultBlockState()`, which is what the survival check asks about.
    sapling: BlockState,
    trunk: BlockState,
    /// The crown from the top down: large with `stage=1`, large, small.
    crown: [BlockState; 3],
    podzol: BlockState,
    podzol_replaceable: ResourceLocation,
}

impl BambooFeature {
    pub fn compile(config: &BambooConfig, facts: &BlockFacts) -> Result<Self> {
        let bamboo = ResourceLocation::minecraft("bamboo");
        let state = |leaves: &str, stage: &str| {
            let mut state = facts.complete(&BlockState::new(bamboo.clone()));
            for (key, value) in [("age", "1"), ("leaves", leaves), ("stage", stage)] {
                state.properties.insert(key.into(), value.into());
            }
            state
        };
        let podzol = facts
            .default_state(&ResourceLocation::minecraft("podzol"))
            .cloned()
            .context("podzol has no default state")?;
        Ok(Self {
            probability: config.probability,
            sapling: facts.complete(&BlockState::new(bamboo.clone())),
            trunk: state("none", "0"),
            crown: [
                state("large", "1"),
                state("large", "0"),
                state("small", "0"),
            ],
            podzol,
            podzol_replaceable: ResourceLocation::minecraft("beneath_bamboo_podzol_replaceable"),
        })
    }

    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if !is_air(cx, origin) {
            return false;
        }
        match cx
            .env
            .behaviours
            .can_survive(&self.sapling, cx.region, origin)
        {
            Some(true) => {}
            Some(false) => return true,
            None => {
                cx.unsupported
                    .record("survival", &self.sapling.name.to_string());
                return false;
            }
        }
        let height = cx.random.next_int_bounded(12) + 5;
        if cx.random.next_float() < self.probability {
            self.place_podzol(cx, origin);
        }
        let mut pos = origin;
        for _ in 0..height {
            if !is_air(cx, pos) {
                break;
            }
            cx.region.set_block(pos, self.trunk.clone());
            pos = offset(pos, 0, 1, 0);
        }
        if pos.y.wrapping_sub(origin.y) >= 3 {
            // The first crown block takes the stopping position, even when that is an obstacle.
            for state in &self.crown {
                cx.region.set_block(pos, state.clone());
                pos = offset(pos, 0, -1, 0);
            }
        }
        true
    }

    fn place_podzol(&self, cx: &mut FeatureContext<'_, '_, '_>, origin: BlockPos) {
        let radius = cx.random.next_int_bounded(4) + 1;
        for x in origin.x.wrapping_sub(radius)..=origin.x.wrapping_add(radius) {
            for z in origin.z.wrapping_sub(radius)..=origin.z.wrapping_add(radius) {
                let dx = x.wrapping_sub(origin.x);
                let dz = z.wrapping_sub(origin.z);
                if dx.wrapping_mul(dx).wrapping_add(dz.wrapping_mul(dz)) > radius * radius {
                    continue;
                }
                let Some(top) = cx.region.height(HeightmapType::WorldSurface, x, z) else {
                    continue;
                };
                let pos = BlockPos::new(x, top.wrapping_sub(1), z);
                if in_tag(cx, pos, &self.podzol_replaceable) {
                    cx.region.set_block(pos, self.podzol.clone());
                }
            }
        }
    }
}
