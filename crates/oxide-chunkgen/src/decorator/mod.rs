//! Biome decoration: the driver, the placement chain, and the first feature families.
//!
//! This is the port of vanilla's decoration stage. [`BiomeDecorator`] is the entry point (an
//! implementation of `crate::decorate::Decorator`); everything under it exists so that what it
//! draws from the random stream, and in what order, is vanilla's:
//!
//! - `provider`: integer and height providers and vertical anchors;
//! - `predicate`: block predicates;
//! - `placement`: every placement modifier and the depth-first chain that runs them;
//! - `state_provider`: block state providers, including the noise-driven ones;
//! - `feature`: placed and configured features, and [`place_configured_feature`];
//! - `ore`, `spring` and the selectors in `feature`: the feature families implemented so far.
//!
//! Float providers are `oxide_datapack::FloatProvider`, which already samples as vanilla does.
//!
//! What is not here is listed, not hidden: a feature type with no family places nothing and is
//! counted in [`UnsupportedTally`], as is a block whose survival rule or two-block placement is
//! unknown. See [`BiomeDecorator::unsupported`].

mod biome_decorator;
mod cave;
mod context;
mod end;
mod feature;
mod ground;
mod nether;
mod ore;
mod placement;
mod predicate;
mod provider;
mod simplex;
mod spring;
mod state_provider;
mod survival;
mod templated;
mod tree;
mod vegetation;
mod water;

#[cfg(test)]
mod tests;

pub use biome_decorator::BiomeDecorator;
pub use context::{
    block_in, BlockBehaviours, Environment, FeatureContext, NoBlockBehaviours, UnsupportedTally,
};
pub use feature::{place_configured_feature, place_placed, Compiler, Feature, Placed};
pub use end::EndFeature;
pub use cave::CaveFeature;
pub use ground::{BiomeClimate, GroundFeature};
pub use nether::NetherFeature;
pub use ore::{OreFeature, RuleTest};
pub use placement::{run as run_placement, Modifier};
pub use predicate::{IdSet, Predicate};
pub use provider::{
    between_inclusive, next_bounded, resolve_anchor, sample_height, sample_int, WorldBounds,
};
pub use spring::SpringFeature;
pub use state_provider::{state_from_spec, StateProvider, Unsupported};
pub use survival::SurvivalRules;
pub use templated::{
    place_in_world, BlockInfo, BoundingBox, FossilFeature, PlaceSettings, Processor, Template,
    TemplateFeature, TemplateLibrary, TemplatedFeature,
};
pub use tree::{FallenTreeFeature, TreeFeature};
pub use vegetation::VegetationFeature;
pub use water::WaterFeature;
