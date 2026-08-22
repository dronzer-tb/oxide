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
    // All optional since 26.2: colours are `"#rrggbb"` strings now, and keys the
    // export considers defaulted are simply absent. See `ident_serde::color_opt`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ident_serde::color_opt"
    )]
    pub fog_color: Option<i32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ident_serde::color_opt"
    )]
    pub water_color: Option<i32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ident_serde::color_opt"
    )]
    pub water_fog_color: Option<i32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ident_serde::color_opt"
    )]
    pub sky_color: Option<i32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ident_serde::color_opt"
    )]
    pub foliage_color: Option<i32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ident_serde::color_opt"
    )]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 26.2 writes colours as `"#rrggbb"` and omits keys it treats as defaulted;
    /// pre-26.2 exports wrote packed ints and always wrote all four.
    #[test]
    fn effects_accept_hex_strings_and_absent_colors() {
        let json = br##"{"foliage_color":"#9e814d","water_color":"#3f76e4"}"##;
        let effects: BiomeEffects = serde_json::from_slice(json).expect("26.2 effects parse");
        assert_eq!(effects.foliage_color, Some(0x9e814d));
        assert_eq!(effects.water_color, Some(0x3f76e4));
        assert_eq!(effects.fog_color, None);
        assert_eq!(effects.sky_color, None);
    }

    #[test]
    fn effects_still_accept_packed_ints() {
        let json = br#"{"fog_color":12638463,"water_color":4159204,
                        "water_fog_color":329011,"sky_color":7907327}"#;
        let effects: BiomeEffects = serde_json::from_slice(json).expect("pre-26.2 effects parse");
        assert_eq!(effects.fog_color, Some(12638463));
        assert_eq!(effects.sky_color, Some(7907327));
    }
}
