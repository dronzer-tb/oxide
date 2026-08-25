//! oxide-datapack — see docs/ARCHITECTURE.md
//!
//! Loads a vanilla-exported worldgen datapack (JSON) into a typed,
//! reference-resolved IR consumed by `oxide-noise`, `oxide-biome`,
//! `oxide-chunkgen`, and `oxide-structures`. This crate deserializes and
//! validates; it does not evaluate density functions, surface rules, or
//! anything else — that belongs to the downstream crates.

pub mod biome;
pub mod carver;
pub mod climate;
pub mod density_function;
pub mod dimension;
pub mod error;
pub mod ident_serde;
pub mod loader;
pub mod multi_noise;
pub mod noise_param;
pub mod noise_settings;
pub mod placement;
pub mod presets;
pub mod registry;
pub mod resolve;
pub mod structure;
pub mod surface_rule;
pub mod version;

pub use biome::{Biome, BiomeEffects, SpawnerData};
pub use carver::{
    CanyonCarverConfig, CanyonShape, CarverConfig, CaveCarverConfig, ConfiguredCarver,
    FloatProvider, HeightProvider,
};
pub use density_function::{
    CubicSpline, DensityFunction, DensityFunctionObject, RarityValueMapper, SplinePoint,
    SplineValue,
};
pub use dimension::{BiomeSource, Dimension, DimensionType, DimensionTypeRef, Generator};
pub use error::{DatapackError, Result};
pub use loader::{load_datapack, Datapack};
pub use multi_noise::{
    ClimateParameters, MultiNoiseBiomeEntry, MultiNoiseBiomeSourceParameterList, MultiNoiseSource,
};
pub use noise_param::NormalNoiseParameters;
pub use noise_settings::{
    NoiseDimensionSettings, NoiseGeneratorSettings, NoiseRouter, SpawnTarget,
};
pub use presets::preset_entries;
pub use registry::Registry;
pub use structure::{
    BiomeFilter, ExclusionZone, FrequencyReductionMethod, ProcessorList, SpreadType, Structure,
    StructurePlacement, StructureSet, StructureSetEntry, TemplatePool,
};
pub use surface_rule::{SurfaceCondition, SurfaceRule, SurfaceType, VerticalAnchor};
pub use version::{DataVersion, PackMeta, PackMetaInner, VersionJson};

pub use oxide_core::{BiomeId, BlockState, ResourceLocation};
