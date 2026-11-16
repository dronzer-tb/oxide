//! Cave decoration: dripstone and sulfur spikes, large dripstone, geodes and sculk patches.
//!
//! These are the features that fill cave air and carve cave walls rather than dress the surface.
//! Each is a port of vanilla's algorithm with its random draws in vanilla's order, kept in a module
//! of its own under `cave/`; this file is the seam to [`Feature`](super::Feature): one enum,
//! [`CaveFeature`], compiled by [`CaveFeature::compile`] and placed by [`CaveFeature::place`].
//!
//! A type that cannot be ported faithfully compiles to `None` and the caller records it as
//! unsupported. `minecraft:fossil` is in that state: it places structure templates (NBT files)
//! through a processor list, and neither the template reader (`oxide-structures`) nor the
//! processors are reachable from this crate without inverting the dependency between the two. It
//! needs a seam on [`Environment`](super::Environment) for loaded templates and a processor
//! engine, neither of which is reconstructed here.
//!
//! Vanilla's post-processing marks and scheduled ticks (a crack's fluid ticks, a sculk vein's
//! settling) go to the region next to the block writes.

mod cluster;
mod geode;
mod large;
mod sculk;
mod speleothem;
mod support;

#[cfg(test)]
mod tests;

use anyhow::Result;
use oxide_datapack::feature::ConfiguredFeature;
use oxide_datapack::BlockFacts;

use super::context::FeatureContext;

use cluster::SpeleothemClusterFeature;
use geode::GeodeFeature;
use large::LargeDripstoneFeature;
use sculk::SculkPatchFeature;
use speleothem::SpeleothemFeature;

/// A compiled cave feature.
#[derive(Debug)]
pub enum CaveFeature {
    Speleothem(SpeleothemFeature),
    SpeleothemCluster(SpeleothemClusterFeature),
    LargeDripstone(LargeDripstoneFeature),
    Geode(GeodeFeature),
    SculkPatch(SculkPatchFeature),
}

impl CaveFeature {
    /// Compiles `ir` when it is one of the cave types this module places and its config is one
    /// the port can follow exactly; `None` otherwise.
    pub fn compile(ir: &ConfiguredFeature, facts: &BlockFacts) -> Result<Option<Self>> {
        Ok(match ir {
            ConfiguredFeature::Speleothem(config) => {
                SpeleothemFeature::compile(config, facts)?.map(Self::Speleothem)
            }
            ConfiguredFeature::SpeleothemCluster(config) => {
                SpeleothemClusterFeature::compile(config, facts)?.map(Self::SpeleothemCluster)
            }
            ConfiguredFeature::LargeDripstone(config) => {
                LargeDripstoneFeature::compile(config, facts)?.map(Self::LargeDripstone)
            }
            ConfiguredFeature::Geode(config) => {
                GeodeFeature::compile(config, facts)?.map(Self::Geode)
            }
            ConfiguredFeature::SculkPatch(config) => {
                SculkPatchFeature::compile(config, facts)?.map(Self::SculkPatch)
            }
            _ => None,
        })
    }

    /// Places the feature at `cx.origin`, returning whether it placed.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        match self {
            Self::Speleothem(feature) => feature.place(cx),
            Self::SpeleothemCluster(feature) => feature.place(cx),
            Self::LargeDripstone(feature) => feature.place(cx),
            Self::Geode(feature) => feature.place(cx),
            Self::SculkPatch(feature) => feature.place(cx),
        }
    }
}
