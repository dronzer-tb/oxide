//! `worldgen/biome/*.json`. Faithful for temperature/downfall/precipitation/
//! effects/spawners/carvers/features; deep feature-config trees are held as
//! raw [`serde_json::Value`] (placed_feature/configured_feature registries
//! are out of this crate's scope per the task).

use oxide_core::ResourceLocation;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Biome {
    pub has_precipitation: bool,
    pub temperature: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature_modifier: Option<String>,
    pub downfall: f32,
    pub effects: BiomeEffects,
    #[serde(default, with = "crate::ident_serde::rl_vec")]
    pub carvers: Vec<ResourceLocation>,
    /// One list of `placed_feature` references per vanilla generation step.
    #[serde(default, with = "crate::ident_serde::rl_vec_vec")]
    pub features: Vec<Vec<ResourceLocation>>,
    /// Mob category name (e.g. `monster`, `creature`) → spawn entries.
    #[serde(default)]
    pub spawners: BTreeMap<String, Vec<SpawnerData>>,
    #[serde(default)]
    pub spawn_costs: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnerData {
    #[serde(rename = "type", with = "crate::ident_serde::rl")]
    pub entity_type: ResourceLocation,
    pub weight: i32,
    #[serde(rename = "minCount")]
    pub min_count: i32,
    #[serde(rename = "maxCount")]
    pub max_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiomeEffects {
    pub fog_color: i32,
    pub water_color: i32,
    pub water_fog_color: i32,
    pub sky_color: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foliage_color: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grass_color: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grass_color_modifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub particle: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambient_sound: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mood_sound: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additions_sound: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub music: Option<serde_json::Value>,
}
