//! Nether decoration: delta patches, netherrack blobs and huge fungi.
//!
//! Each feature is a port of the reference implementation's algorithm with its random draws in
//! vanilla's order, in a module of its own under `nether/`. This file is the seam to
//! [`Feature`](super::Feature): one enum, [`NetherFeature`], compiled by
//! [`NetherFeature::compile`] and placed by [`NetherFeature::place`].
//!
//! Not here, and tallied as `feature:<type>` rather than approximated:
//!
//! - `basalt_columns`, `basalt_pillar`, `glowstone_blob`, `twisting_vines`, `weeping_vines` and
//!   `nether_forest_vegetation`: the reference targets 26.3, which replaced these 26.2 types with
//!   compositions of other features (`stepped_column_cluster`, patches). It holds no code for the
//!   26.2 algorithms, so their draw order cannot be ported.
//! - `huge_fungus` with `planted: true`: vanilla destroys the block it replaces with
//!   `destroyBlock`, which leaves the fluid of a flooded cell behind. Only bonemeal plants a
//!   fungus that way, never world generation, so the decorator does not model it.
//!
//! Post-processing marks and block ticks are not recorded, as for the ground features.

mod delta;
mod huge_fungus;
mod replace_blobs;

#[cfg(test)]
mod tests;

use anyhow::{anyhow, Result};
use oxide_core::{BlockPos, BlockState, ResourceLocation};
use oxide_datapack::feature::ConfiguredFeature;
use oxide_datapack::{BlockFact, BlockFacts};

use super::context::{block_in, FeatureContext};

use delta::DeltaFeature;
use huge_fungus::HugeFungusFeature;
use replace_blobs::ReplaceBlobsFeature;

/// A compiled Nether feature.
#[derive(Debug)]
pub enum NetherFeature {
    Delta(DeltaFeature),
    ReplaceBlobs(ReplaceBlobsFeature),
    HugeFungus(HugeFungusFeature),
}

impl NetherFeature {
    /// Compiles `ir` when it is a Nether type this module places; `None` for every other type
    /// and for the Nether types that stay unsupported.
    pub fn compile(ir: &ConfiguredFeature, facts: &BlockFacts) -> Result<Option<Self>> {
        Ok(Some(match ir {
            ConfiguredFeature::DeltaFeature(config) => Self::Delta(DeltaFeature::compile(config, facts)?),
            ConfiguredFeature::NetherrackReplaceBlobs(config) => {
                Self::ReplaceBlobs(ReplaceBlobsFeature::compile(config, facts)?)
            }
            ConfiguredFeature::HugeFungus(config) => match HugeFungusFeature::compile(config, facts)? {
                Some(feature) => Self::HugeFungus(feature),
                None => return Ok(None),
            },
            _ => return Ok(None),
        }))
    }

    /// Places the feature at `cx.origin`, returning whether it changed anything.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        match self {
            Self::Delta(feature) => feature.place(cx),
            Self::ReplaceBlobs(feature) => feature.place(cx),
            Self::HugeFungus(feature) => feature.place(cx),
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
    cx.env.facts.is(block_in(cx.region, pos), fact)
}

fn offset(pos: BlockPos, dx: i32, dy: i32, dz: i32) -> BlockPos {
    BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz)
}
