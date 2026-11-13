//! End decoration: chorus plants, the outer end islands, the obsidian spikes, gateways, the
//! obsidian and void-start platforms, and the spawn bonus chest.
//!
//! Each feature is a port of vanilla's algorithm with its random draws in vanilla's order, kept
//! in a module of its own under `end/`; this file is the seam to [`Feature`](super::Feature):
//! one enum, [`EndFeature`], compiled by [`EndFeature::compile`] and placed by
//! [`EndFeature::place`].
//!
//! The region carries block writes only, so what vanilla attaches to a block or spawns beside it
//! is tallied, not invented: `block_entity:minecraft:end_gateway` (the exit of a gateway that has
//! one), `block_entity:minecraft:chest` (the bonus chest's loot table) and
//! `entity:minecraft:end_crystal` (the crystal on every spike). The blocks themselves are
//! placed, as `ground/monster_room.rs` places its chest.
//!
//! Not here, because no reference code exists for 26.2's forms of them: `desert_well`, `kelp`,
//! `seagrass`, `sea_pickle`, `coral_tree`, `coral_claw` and `coral_mushroom`. The reference
//! implementation only knows their blocks' survival rules (`block_survival.rs`), not the
//! features. A port needs, per type, vanilla's block layout or growth loop and its random draw
//! order (`DesertWellFeature`'s sand and slab pattern; `KelpFeature`'s column loop;
//! `SeagrassFeature` and `SeaPickleFeature`'s scatter; the three `Coral*Feature` shape builders),
//! none of which is reconstructed from memory.

mod bonus_chest;
mod chorus_plant;
mod gateway;
mod island;
mod platform;
mod spike;

#[cfg(test)]
mod tests;

use anyhow::{anyhow, Result};
use oxide_core::{BlockPos, BlockState, ResourceLocation};
use oxide_datapack::feature::ConfiguredFeature;
use oxide_datapack::{BlockFact, BlockFacts};

use super::context::{block_in, FeatureContext};

use bonus_chest::BonusChestFeature;
use chorus_plant::ChorusPlantFeature;
use gateway::EndGatewayFeature;
use island::EndIslandFeature;
use platform::{EndPlatformFeature, VoidStartPlatformFeature};
use spike::EndSpikeFeature;

/// A compiled End feature.
#[derive(Debug)]
pub enum EndFeature {
    ChorusPlant(ChorusPlantFeature),
    EndIsland(EndIslandFeature),
    EndSpike(EndSpikeFeature),
    EndGateway(EndGatewayFeature),
    EndPlatform(EndPlatformFeature),
    VoidStartPlatform(VoidStartPlatformFeature),
    BonusChest(BonusChestFeature),
}

impl EndFeature {
    /// Compiles `ir` when it is one of the End feature types this module places; `None` for every
    /// other type.
    pub fn compile(ir: &ConfiguredFeature, facts: &BlockFacts) -> Result<Option<Self>> {
        Ok(Some(match ir {
            ConfiguredFeature::ChorusPlant(_) => Self::ChorusPlant(ChorusPlantFeature::compile(facts)?),
            ConfiguredFeature::EndIsland(_) => Self::EndIsland(EndIslandFeature::compile(facts)?),
            ConfiguredFeature::EndSpike(config) => Self::EndSpike(EndSpikeFeature::compile(config, facts)?),
            ConfiguredFeature::EndGateway(config) => Self::EndGateway(EndGatewayFeature::compile(config, facts)?),
            ConfiguredFeature::EndPlatform(_) => Self::EndPlatform(EndPlatformFeature::compile(facts)?),
            ConfiguredFeature::VoidStartPlatform(_) => {
                Self::VoidStartPlatform(VoidStartPlatformFeature::compile(facts)?)
            }
            ConfiguredFeature::BonusChest(_) => Self::BonusChest(BonusChestFeature::compile(facts)?),
            _ => return Ok(None),
        }))
    }

    /// Places the feature at `cx.origin`, returning whether vanilla's `place` returns `true`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        match self {
            Self::ChorusPlant(feature) => feature.place(cx),
            Self::EndIsland(feature) => feature.place(cx),
            Self::EndSpike(feature) => feature.place(cx),
            Self::EndGateway(feature) => feature.place(cx),
            Self::EndPlatform(feature) => feature.place(cx),
            Self::VoidStartPlatform(feature) => feature.place(cx),
            Self::BonusChest(feature) => feature.place(cx),
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

/// Whether `state` is the vanilla block `path`, whatever its properties.
fn is_block(state: &BlockState, path: &str) -> bool {
    state.name.namespace() == "minecraft" && state.name.path() == path
}

/// Whether the block at `pos` is air (`air`, `cave_air` or `void_air`); outside the window it is
/// `void_air`.
fn is_air(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
    cx.env.facts.is(block_in(cx.region, pos), BlockFact::Air)
}

fn offset(pos: BlockPos, dx: i32, dy: i32, dz: i32) -> BlockPos {
    BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz)
}

/// `state` with `name` set to `value`.
fn with_property(mut state: BlockState, name: &str, value: &str) -> BlockState {
    state.properties.insert(name.to_string(), value.to_string());
    state
}
