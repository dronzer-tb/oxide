//! Underwater decoration (kelp, seagrass, sea pickles, the three coral shapes) and the desert
//! well.
//!
//! Each feature is a port of vanilla 26.2's algorithm with its random draws in vanilla's order,
//! in a module of its own under `water/`. This file is the seam to [`Feature`](super::Feature):
//! one enum, [`WaterFeature`], compiled by [`WaterFeature::compile`] and placed by
//! [`WaterFeature::place`]. The desert well sits here because, like the rest, it is a fixed
//! block layout around a fluid, with two random draws.
//!
//! The corals pick their blocks from block tags at placement time (`Registry.getRandomElementOf`
//! draws an index into the tag's ordered member list), so they read
//! [`TagSet::ordered`](oxide_datapack::TagSet::ordered) rather than a set.
//!
//! The desert well's suspicious sand gets its archaeology loot table
//! (`minecraft:archaeology/desert_well`) as block-entity SNBT next to the block write.

mod coral;
mod desert_well;
mod kelp;
mod pickle;
mod seagrass;
mod support;

#[cfg(test)]
mod tests;

use anyhow::Result;
use oxide_datapack::feature::ConfiguredFeature;
use oxide_datapack::BlockFacts;

use super::context::FeatureContext;

use coral::{CoralFeature, CoralShape};
use desert_well::DesertWellFeature;
use kelp::KelpFeature;
use pickle::SeaPickleFeature;
use seagrass::SeagrassFeature;

/// A compiled underwater feature or desert well.
#[derive(Debug)]
pub enum WaterFeature {
    Kelp(KelpFeature),
    Seagrass(SeagrassFeature),
    SeaPickle(SeaPickleFeature),
    Coral(CoralFeature),
    DesertWell(DesertWellFeature),
}

impl WaterFeature {
    /// Compiles `ir` when it is one of the types this module places; `None` for every other type.
    pub fn compile(ir: &ConfiguredFeature, facts: &BlockFacts) -> Result<Option<Self>> {
        Ok(Some(match ir {
            ConfiguredFeature::Kelp(_) => Self::Kelp(KelpFeature::compile(facts)?),
            ConfiguredFeature::Seagrass(config) => {
                Self::Seagrass(SeagrassFeature::compile(config, facts)?)
            }
            ConfiguredFeature::SeaPickle(config) => {
                Self::SeaPickle(SeaPickleFeature::compile(config, facts)?)
            }
            ConfiguredFeature::CoralTree(_) => {
                Self::Coral(CoralFeature::compile(CoralShape::Tree, facts)?)
            }
            ConfiguredFeature::CoralClaw(_) => {
                Self::Coral(CoralFeature::compile(CoralShape::Claw, facts)?)
            }
            ConfiguredFeature::CoralMushroom(_) => {
                Self::Coral(CoralFeature::compile(CoralShape::Mushroom, facts)?)
            }
            ConfiguredFeature::DesertWell(_) => {
                Self::DesertWell(DesertWellFeature::compile(facts)?)
            }
            _ => return Ok(None),
        }))
    }

    /// Places the feature at `cx.origin`, returning whether vanilla would report it placed.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        match self {
            Self::Kelp(feature) => feature.place(cx),
            Self::Seagrass(feature) => feature.place(cx),
            Self::SeaPickle(feature) => feature.place(cx),
            Self::Coral(feature) => feature.place(cx),
            Self::DesertWell(feature) => feature.place(cx),
        }
    }
}
