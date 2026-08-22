//! `dimension/*.json` and `dimension_type/*.json`.

use oxide_core::{BiomeId, ResourceLocation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionType {
    // 26.2 moved the behaviour flags into an `attributes` map keyed by ids like
    // `minecraft:gameplay/respawn_anchor_works`, so none of them appear at the top
    // level any more. Defaulted rather than deleted so a pre-26.2 pack still loads
    // with the values it states; nothing in this workspace reads them today.
    #[serde(default)]
    pub ultrawarm: bool,
    #[serde(default)]
    pub natural: bool,
    #[serde(default)]
    pub piglin_safe: bool,
    #[serde(default)]
    pub respawn_anchor_works: bool,
    #[serde(default)]
    pub bed_works: bool,
    #[serde(default)]
    pub has_raids: bool,
    pub has_skylight: bool,
    pub has_ceiling: bool,
    pub coordinate_scale: f64,
    pub ambient_light: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_time: Option<i64>,
    pub logical_height: i32,
    /// Block tag reference, e.g. `#minecraft:infiniburn_overworld`.
    pub infiniburn: String,
    /// Gone in 26.2 -- the visual settings it named now live in `attributes`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ident_serde::rl_opt"
    )]
    pub effects: Option<ResourceLocation>,
    pub min_y: i32,
    pub height: i32,
    pub monster_spawn_light_level: MonsterSpawnLightLevel,
    pub monster_spawn_block_light_limit: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MonsterSpawnLightLevel {
    Constant(i32),
    Uniform {
        #[serde(rename = "type", with = "crate::ident_serde::rl")]
        kind: ResourceLocation,
        /// Pre-26.2 nesting. 26.2 writes the bounds beside `type` instead, so both
        /// shapes are optional here and exactly one of them is populated.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<UniformIntRange>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_inclusive: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_inclusive: Option<i32>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniformIntRange {
    pub min_inclusive: i32,
    pub max_inclusive: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dimension {
    /// Either an inline [`DimensionType`] or a reference to the registry.
    #[serde(rename = "type")]
    pub dimension_type: DimensionTypeRef,
    pub generator: Generator,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DimensionTypeRef {
    Reference(#[serde(with = "crate::ident_serde::rl")] ResourceLocation),
    Inline(Box<DimensionType>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Generator {
    #[serde(rename = "type", with = "crate::ident_serde::rl")]
    pub generator_type: ResourceLocation,
    /// Reference into the `worldgen/noise_settings` registry. Only present
    /// for `minecraft:noise` generators; other generator types (e.g.
    /// `minecraft:flat`, `minecraft:debug`) leave this `None`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ident_serde::rl_opt"
    )]
    pub settings: Option<ResourceLocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biome_source: Option<BiomeSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BiomeSource {
    #[serde(rename = "minecraft:fixed")]
    Fixed {
        #[serde(with = "crate::ident_serde::rl")]
        biome: BiomeId,
    },

    #[serde(rename = "minecraft:checkerboard")]
    Checkerboard {
        #[serde(with = "crate::ident_serde::rl_vec")]
        biomes: Vec<BiomeId>,
        #[serde(default)]
        scale: i32,
    },

    #[serde(rename = "minecraft:the_end")]
    TheEnd {},

    #[serde(rename = "minecraft:multi_noise")]
    MultiNoise(crate::multi_noise::MultiNoiseSource),

    /// Any biome source type this crate does not model in detail. Its raw
    /// biome references (if any) are not resolved.
    #[serde(other)]
    Unknown,
}
