//! Nether decoration: delta patches, netherrack blobs, huge fungi, basalt columns and pillars,
//! glowstone blobs, twisting and weeping vines, and forest vegetation.
//!
//! Each feature is a port of the 26.2 algorithm with its random draws in vanilla's order, in a
//! module of its own under `nether/`. This file is the seam to [`Feature`](super::Feature): one
//! enum, [`NetherFeature`], compiled by [`NetherFeature::compile`] and placed by
//! [`NetherFeature::place`].
//!
//! Not here, and tallied as `feature:minecraft:huge_fungus` rather than approximated: a huge
//! fungus with `planted: true`. Vanilla destroys the block it replaces with `destroyBlock`, which
//! leaves the fluid of a flooded cell behind. Only bonemeal plants a fungus that way, never world
//! generation, so the decorator does not model it.
//!
//! Post-processing marks and block ticks are not recorded, as for the ground features.

mod basalt_columns;
mod basalt_pillar;
mod delta;
mod forest_vegetation;
mod glowstone;
mod huge_fungus;
mod replace_blobs;
mod vines;

#[cfg(test)]
mod tests;

use anyhow::{anyhow, Result};
use oxide_core::{BlockPos, BlockState, RandomSource, ResourceLocation};
use oxide_datapack::feature::ConfiguredFeature;
use oxide_datapack::{BlockFact, BlockFacts};

use super::context::{block_in, FeatureContext};
use super::provider::between_inclusive;

use basalt_columns::BasaltColumnsFeature;
use basalt_pillar::BasaltPillarFeature;
use delta::DeltaFeature;
use forest_vegetation::ForestVegetationFeature;
use glowstone::GlowstoneFeature;
use huge_fungus::HugeFungusFeature;
use replace_blobs::ReplaceBlobsFeature;
use vines::{TwistingVinesFeature, WeepingVinesFeature};

/// A compiled Nether feature.
#[derive(Debug)]
pub enum NetherFeature {
    Delta(DeltaFeature),
    ReplaceBlobs(ReplaceBlobsFeature),
    HugeFungus(HugeFungusFeature),
    BasaltColumns(BasaltColumnsFeature),
    BasaltPillar(BasaltPillarFeature),
    Glowstone(GlowstoneFeature),
    TwistingVines(TwistingVinesFeature),
    WeepingVines(WeepingVinesFeature),
    ForestVegetation(ForestVegetationFeature),
}

impl NetherFeature {
    /// Compiles `ir` when it is a Nether type this module places; `None` for every other type
    /// and for the Nether configurations that stay unsupported.
    pub fn compile(ir: &ConfiguredFeature, facts: &BlockFacts) -> Result<Option<Self>> {
        Ok(Some(match ir {
            ConfiguredFeature::DeltaFeature(config) => {
                Self::Delta(DeltaFeature::compile(config, facts)?)
            }
            ConfiguredFeature::NetherrackReplaceBlobs(config) => {
                Self::ReplaceBlobs(ReplaceBlobsFeature::compile(config, facts)?)
            }
            ConfiguredFeature::HugeFungus(config) => {
                match HugeFungusFeature::compile(config, facts)? {
                    Some(feature) => Self::HugeFungus(feature),
                    None => return Ok(None),
                }
            }
            ConfiguredFeature::BasaltColumns(config) => {
                Self::BasaltColumns(BasaltColumnsFeature::compile(config, facts)?)
            }
            ConfiguredFeature::BasaltPillar(_) => {
                Self::BasaltPillar(BasaltPillarFeature::compile(facts)?)
            }
            ConfiguredFeature::GlowstoneBlob(_) => {
                Self::Glowstone(GlowstoneFeature::compile(facts)?)
            }
            ConfiguredFeature::TwistingVines(config) => {
                Self::TwistingVines(TwistingVinesFeature::compile(config, facts)?)
            }
            ConfiguredFeature::WeepingVines(_) => {
                Self::WeepingVines(WeepingVinesFeature::compile(facts)?)
            }
            ConfiguredFeature::NetherForestVegetation(config) => {
                Self::ForestVegetation(ForestVegetationFeature::compile(config, facts)?)
            }
            _ => return Ok(None),
        }))
    }

    /// Places the feature at `cx.origin`, returning whether it changed anything.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        match self {
            Self::Delta(feature) => feature.place(cx),
            Self::ReplaceBlobs(feature) => feature.place(cx),
            Self::HugeFungus(feature) => feature.place(cx),
            Self::BasaltColumns(feature) => feature.place(cx),
            Self::BasaltPillar(feature) => feature.place(cx),
            Self::Glowstone(feature) => feature.place(cx),
            Self::TwistingVines(feature) => feature.place(cx),
            Self::WeepingVines(feature) => feature.place(cx),
            Self::ForestVegetation(feature) => feature.place(cx),
        }
    }
}

/// The default state of the vanilla block `path`, from the exported facts.
fn default_state(facts: &BlockFacts, path: &str) -> Result<BlockState> {
    facts
        .default_state(&ResourceLocation::minecraft(path))
        .cloned()
        .ok_or_else(|| anyhow!("minecraft:{path} is not in the exported block facts"))
}

/// Whether the block at `pos` is the block `name`, whatever its properties.
fn is_block_at(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos, name: &ResourceLocation) -> bool {
    block_in(cx.region, pos).name == *name
}

/// Whether the block at `pos` has `fact`; outside the window it is `void_air`.
fn has(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos, fact: BlockFact) -> bool {
    crate::decorator::context::fact_in(cx.region, pos, fact)
}

/// `LevelReader.isEmptyBlock`: whether the block at `pos` is air.
fn is_empty(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
    has(cx, pos, BlockFact::Air)
}

fn offset(pos: BlockPos, dx: i32, dy: i32, dz: i32) -> BlockPos {
    BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz)
}

/// `Direction.values()` as offsets: down, up, north, south, west, east.
const DIRECTIONS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];

/// `Mth.nextInt`: `min` without a draw when the range is empty or a single value, otherwise
/// `nextInt(max - min + 1) + min` (a range too wide for an `int` draws nothing, where vanilla
/// throws).
fn mth_next_int(random: &mut impl RandomSource, min: i32, max: i32) -> i32 {
    if min >= max {
        min
    } else {
        between_inclusive(random, min, max)
    }
}
