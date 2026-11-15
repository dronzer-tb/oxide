//! Ground and terrain decoration: disks, piles, blobs, columns, lakes, snow and ice, spikes and
//! icebergs, underwater magma and monster rooms.
//!
//! These are the features that reshape the surface rather than grow something on it. Each one is
//! a port of vanilla's algorithm with its random draws in vanilla's order, kept in a module of
//! its own under `ground/`; this file is the seam to [`Feature`](super::Feature): one enum,
//! [`GroundFeature`], compiled by [`GroundFeature::compile`] and placed by
//! [`GroundFeature::place`], so the shared dispatch gains one variant instead of eleven.
//!
//! A type that cannot be ported faithfully compiles to `None` and the caller records it as
//! unsupported. `minecraft:desert_well` is placed by the water family (`water/desert_well.rs`).
//!
//! Vanilla's post-processing marks (`markAboveForPostProcessing`) and scheduled block ticks are
//! not recorded: the region produces block writes only. They affect when a block updates after
//! the chunk loads, never which block is placed.

mod block_blob;
mod block_column;
mod block_pile;
mod blue_ice;
mod climate;
mod disk;
mod freeze;
mod iceberg;
mod lake;
mod monster_room;
mod spike;
mod underwater_magma;

#[cfg(test)]
mod tests;

use anyhow::{anyhow, Result};
use oxide_core::{BlockPos, BlockState, ResourceLocation};
use oxide_datapack::feature::ConfiguredFeature;
use oxide_datapack::{BlockFact, BlockFacts};

use super::context::{block_in, FeatureContext};
use super::state_provider::{StateProvider, Unsupported};

pub use climate::BiomeClimate;

use block_blob::BlockBlobFeature;
use block_column::BlockColumnFeature;
use block_pile::BlockPileFeature;
use blue_ice::BlueIceFeature;
use disk::DiskFeature;
use freeze::FreezeTopLayerFeature;
use iceberg::IcebergFeature;
use lake::LakeFeature;
use monster_room::MonsterRoomFeature;
use spike::SpikeFeature;
use underwater_magma::UnderwaterMagmaFeature;

/// A compiled ground feature.
#[derive(Debug)]
pub enum GroundFeature {
    Disk(DiskFeature),
    BlockPile(BlockPileFeature),
    BlockBlob(BlockBlobFeature),
    BlockColumn(BlockColumnFeature),
    Lake(LakeFeature),
    FreezeTopLayer(FreezeTopLayerFeature),
    Spike(SpikeFeature),
    Iceberg(IcebergFeature),
    BlueIce(BlueIceFeature),
    UnderwaterMagma(UnderwaterMagmaFeature),
    MonsterRoom(MonsterRoomFeature),
}

impl GroundFeature {
    /// Compiles `ir` when it is one of the ground feature types this module places; `None` for
    /// every other type, and for the ground types that stay unsupported.
    pub fn compile(ir: &ConfiguredFeature, facts: &BlockFacts) -> Result<Option<Self>> {
        Ok(Some(match ir {
            ConfiguredFeature::Disk(config) => Self::Disk(DiskFeature::compile(config, facts)?),
            ConfiguredFeature::BlockPile(config) => {
                Self::BlockPile(BlockPileFeature::compile(config, facts)?)
            }
            ConfiguredFeature::BlockBlob(config) => {
                Self::BlockBlob(BlockBlobFeature::compile(config, facts)?)
            }
            ConfiguredFeature::BlockColumn(config) => {
                Self::BlockColumn(BlockColumnFeature::compile(config, facts)?)
            }
            ConfiguredFeature::Lake(config) => Self::Lake(LakeFeature::compile(config, facts)?),
            ConfiguredFeature::FreezeTopLayer(_) => {
                Self::FreezeTopLayer(FreezeTopLayerFeature::compile(facts)?)
            }
            ConfiguredFeature::Spike(config) => Self::Spike(SpikeFeature::compile(config, facts)?),
            ConfiguredFeature::Iceberg(config) => {
                Self::Iceberg(IcebergFeature::compile(config, facts)?)
            }
            ConfiguredFeature::BlueIce(_) => Self::BlueIce(BlueIceFeature::compile(facts)?),
            ConfiguredFeature::UnderwaterMagma(config) => {
                Self::UnderwaterMagma(UnderwaterMagmaFeature::compile(config, facts)?)
            }
            ConfiguredFeature::MonsterRoom(_) => {
                Self::MonsterRoom(MonsterRoomFeature::compile(facts)?)
            }
            _ => return Ok(None),
        }))
    }

    /// Places the feature at `cx.origin`, returning whether it changed anything.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        match self {
            Self::Disk(feature) => feature.place(cx),
            Self::BlockPile(feature) => feature.place(cx),
            Self::BlockBlob(feature) => feature.place(cx),
            Self::BlockColumn(feature) => feature.place(cx),
            Self::Lake(feature) => feature.place(cx),
            Self::FreezeTopLayer(feature) => feature.place(cx),
            Self::Spike(feature) => feature.place(cx),
            Self::Iceberg(feature) => feature.place(cx),
            Self::BlueIce(feature) => feature.place(cx),
            Self::UnderwaterMagma(feature) => feature.place(cx),
            Self::MonsterRoom(feature) => feature.place(cx),
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

/// Whether the block at `pos` has `fact`; outside the window it is `void_air`.
fn has(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos, fact: BlockFact) -> bool {
    cx.env.facts.is(block_in(cx.region, pos), fact)
}

/// Whether the block at `pos` is air (`air`, `cave_air` or `void_air`).
fn is_air(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
    has(cx, pos, BlockFact::Air)
}

/// Whether the block at `pos` is the vanilla block `path`.
fn is_block_at(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos, path: &str) -> bool {
    is_block(block_in(cx.region, pos), path)
}

/// Whether `state`'s block is in the block tag `minecraft:<tag>`.
fn in_tag(cx: &FeatureContext<'_, '_, '_>, tag: &str, state: &BlockState) -> bool {
    cx.env
        .block_tags
        .contains(&ResourceLocation::minecraft(tag), &state.name)
}

/// `BlockStateProvider.getState` for a provider that must yield: a provider with no state, or
/// one the decorator cannot evaluate, is tallied and gives `None`.
fn provide(
    cx: &mut FeatureContext<'_, '_, '_>,
    provider: &StateProvider,
    pos: BlockPos,
) -> Option<BlockState> {
    match provider.state(cx, pos) {
        Ok(Some(state)) => Some(state),
        Ok(None) => {
            cx.unsupported.record("provider", "no state for position");
            None
        }
        Err(Unsupported(why)) => {
            cx.unsupported.record("provider", why);
            None
        }
    }
}

fn offset(pos: BlockPos, dx: i32, dy: i32, dz: i32) -> BlockPos {
    BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz)
}

/// The six directions as unit offsets, in `Direction.values()` order: down, up, north, south,
/// west, east.
const DIRECTIONS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];

/// `BlockPos.betweenClosed(a, b)`: every position of the box, x fastest, then y, then z.
fn between_closed(a: BlockPos, b: BlockPos) -> impl Iterator<Item = BlockPos> {
    let (min_x, min_y, min_z) = (a.x.min(b.x), a.y.min(b.y), a.z.min(b.z));
    let size_x = a.x.max(b.x) - min_x + 1;
    let size_y = a.y.max(b.y) - min_y + 1;
    let size_z = a.z.max(b.z) - min_z + 1;
    (0..size_x * size_y * size_z).map(move |index| {
        BlockPos::new(
            min_x + index % size_x,
            min_y + (index / size_x) % size_y,
            min_z + index / size_x / size_y,
        )
    })
}
