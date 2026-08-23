//! `worldgen/multi_noise_biome_source_parameter_list/*.json` and the
//! `minecraft:multi_noise` biome source embedded in `dimension/*.json`.

use crate::climate::ClimateParam;
use oxide_core::{BiomeId, ResourceLocation};
use serde::{Deserialize, Serialize};

/// A `minecraft:multi_noise` biome source either points at a named preset
/// (a Java-hardcoded parameter list, e.g. `minecraft:overworld`) or embeds the
/// biome/parameter list directly. Presets are not in `data/` at all; the tables
/// vanilla builds in Java are ported in [`crate::presets`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MultiNoiseSource {
    Preset {
        #[serde(with = "crate::ident_serde::rl")]
        preset: ResourceLocation,
    },
    Explicit {
        biomes: Vec<MultiNoiseBiomeEntry>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiNoiseBiomeEntry {
    #[serde(with = "crate::ident_serde::rl")]
    pub biome: BiomeId,
    pub parameters: ClimateParameters,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClimateParameters {
    pub temperature: ClimateParam,
    pub humidity: ClimateParam,
    pub continentalness: ClimateParam,
    pub erosion: ClimateParam,
    pub depth: ClimateParam,
    pub weirdness: ClimateParam,
    #[serde(default)]
    pub offset: f32,
}

/// `worldgen/multi_noise_biome_source_parameter_list/*.json`.
// In practice vanilla ships these as `{"preset": "minecraft:overworld"}`, deferring to a
// Java-hardcoded table -- see [`crate::presets`], which ports it. The registry format also
// allows an explicit `biomes` list, so both are modeled.
pub type MultiNoiseBiomeSourceParameterList = MultiNoiseSource;
