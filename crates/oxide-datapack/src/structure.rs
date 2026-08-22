//! `worldgen/structure/*.json`, `worldgen/structure_set/*.json` (parsed
//! fully); `worldgen/template_pool/*.json` and `worldgen/processor_list/*.json`
//! are held as raw [`serde_json::Value`] per the task scope.

use oxide_core::ResourceLocation;
use serde::{Deserialize, Serialize};

/// `biomes` fields accept either an explicit id list or a `#tag` reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BiomeFilter {
    Tag(String),
    List(#[serde(with = "crate::ident_serde::rl_vec")] Vec<ResourceLocation>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Structure {
    #[serde(rename = "type", with = "crate::ident_serde::rl")]
    pub structure_type: ResourceLocation,
    pub biomes: BiomeFilter,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(default)]
    pub spawn_overrides: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terrain_adaptation: Option<String>,
    /// Structure-type-specific fields (e.g. jigsaw's `start_pool`, mineshaft's
    /// `mineshaft_type`) not modeled individually — piece layout is
    /// `oxide-structures`' concern, not this crate's.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructureSetEntry {
    #[serde(with = "crate::ident_serde::rl")]
    pub structure: ResourceLocation,
    #[serde(default = "one")]
    pub weight: i32,
}

fn one() -> i32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructureSet {
    pub structures: Vec<StructureSetEntry>,
    pub placement: StructurePlacement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrequencyReductionMethod {
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "legacy_type_1")]
    LegacyType1,
    #[serde(rename = "legacy_type_2")]
    LegacyType2,
    /// 26.2 addition -- `mineshafts` uses it. Parsed only; nothing in this
    /// workspace branches on the method yet.
    #[serde(rename = "legacy_type_3")]
    LegacyType3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpreadType {
    #[serde(rename = "linear")]
    Linear,
    #[serde(rename = "triangular")]
    Triangular,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExclusionZone {
    #[serde(with = "crate::ident_serde::rl")]
    pub other_set: ResourceLocation,
    pub chunk_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StructurePlacement {
    #[serde(rename = "minecraft:random_spread")]
    RandomSpread {
        spacing: i32,
        separation: i32,
        salt: i32,
        #[serde(default)]
        frequency_reduction_method: Option<FrequencyReductionMethod>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frequency: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locate_offset: Option<[i32; 3]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exclusion_zone: Option<ExclusionZone>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spread_type: Option<SpreadType>,
    },

    #[serde(rename = "minecraft:concentric_rings")]
    ConcentricRings {
        distance: i32,
        spread: i32,
        count: i32,
        preferred_biomes: BiomeFilter,
        #[serde(default)]
        frequency_reduction_method: Option<FrequencyReductionMethod>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frequency: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locate_offset: Option<[i32; 3]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exclusion_zone: Option<ExclusionZone>,
    },
}

/// `worldgen/template_pool/*.json` — left as raw JSON (jigsaw pool internals
/// are out of scope here).
pub type TemplatePool = serde_json::Value;

/// `worldgen/processor_list/*.json` — left as raw JSON.
pub type ProcessorList = serde_json::Value;
