//! Vegetation and cave-decoration features: vegetation patches, root systems, multiface growth
//! (glow lichen, sculk veins), vines, bamboo and the huge mushrooms.
//!
//! Each family is its own module; [`VegetationFeature`] is the one value [`super::Feature`]
//! carries for all of them and dispatches to the right one.
//!
//! Kelp, seagrass, sea pickles and the coral shapes are not here: they are the water family's
//! (`water.rs`), ported from the decompiled 26.2 classes.
//!
//! Two answers vanilla gets from per-block Java are routed through seams instead of guessed: see
//! [`support`] for survival and face-sturdiness.

mod bamboo;
mod geometry;
mod multiface;
mod mushroom;
mod patch;
mod root_system;
mod support;
mod vines;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use anyhow::Result;
use oxide_datapack::feature::{ConfiguredFeature, Holder};
use oxide_datapack::placement::PlacedFeature;
use oxide_datapack::BlockFacts;

use super::context::FeatureContext;
use super::feature::Placed;
use bamboo::BambooFeature;
use multiface::MultifaceFeature;
use mushroom::MushroomFeature;
use patch::PatchFeature;
use root_system::RootSystemFeature;
use vines::VinesFeature;

/// A compiled vegetation-family feature.
#[derive(Debug)]
pub enum VegetationFeature {
    Patch(PatchFeature),
    RootSystem(RootSystemFeature),
    Multiface(MultifaceFeature),
    Vines(VinesFeature),
    Bamboo(BambooFeature),
    HugeMushroom(MushroomFeature),
}

impl VegetationFeature {
    /// Compiles `ir` when it is one of this family's types and can be placed faithfully;
    /// `Ok(None)` means the caller should treat it as unsupported. `placed` compiles a nested
    /// placed feature.
    pub fn compile(
        ir: &ConfiguredFeature,
        facts: &BlockFacts,
        placed: &mut dyn FnMut(&Holder<PlacedFeature>) -> Result<Arc<Placed>>,
    ) -> Result<Option<Self>> {
        Ok(Some(match ir {
            ConfiguredFeature::VegetationPatch(config) => {
                Self::Patch(PatchFeature::compile(config, false, facts, placed)?)
            }
            ConfiguredFeature::WaterloggedVegetationPatch(config) => {
                Self::Patch(PatchFeature::compile(config, true, facts, placed)?)
            }
            ConfiguredFeature::RootSystem(config) => {
                Self::RootSystem(RootSystemFeature::compile(config, facts, placed)?)
            }
            ConfiguredFeature::MultifaceGrowth(config) => {
                match MultifaceFeature::compile(config, facts)? {
                    Some(feature) => Self::Multiface(feature),
                    None => return Ok(None),
                }
            }
            ConfiguredFeature::Vines(_) => Self::Vines(VinesFeature::compile(facts)?),
            ConfiguredFeature::Bamboo(config) => {
                Self::Bamboo(BambooFeature::compile(config, facts)?)
            }
            ConfiguredFeature::HugeRedMushroom(config) => {
                Self::HugeMushroom(MushroomFeature::compile(config, true, facts)?)
            }
            ConfiguredFeature::HugeBrownMushroom(config) => {
                Self::HugeMushroom(MushroomFeature::compile(config, false, facts)?)
            }
            _ => return Ok(None),
        }))
    }

    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        match self {
            Self::Patch(feature) => feature.place(cx),
            Self::RootSystem(feature) => feature.place(cx),
            Self::Multiface(feature) => feature.place(cx),
            Self::Vines(feature) => feature.place(cx),
            Self::Bamboo(feature) => feature.place(cx),
            Self::HugeMushroom(feature) => feature.place(cx),
        }
    }
}
