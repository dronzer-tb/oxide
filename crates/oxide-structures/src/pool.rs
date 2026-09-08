//! Parsed `worldgen/template_pool` definitions for Jigsaw assembly.

use oxide_core::ResourceLocation;
use serde::{Deserialize, Serialize};

fn default_empty_rl() -> ResourceLocation {
    ResourceLocation::minecraft("empty")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplatePool {
    #[serde(default, with = "oxide_datapack::ident_serde::rl_opt")]
    pub fallback: Option<ResourceLocation>,
    pub elements: Vec<PoolElementEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolElementEntry {
    pub weight: i32,
    pub element: PoolElement,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "element_type")]
pub enum PoolElement {
    #[serde(rename = "minecraft:single_pool_element")]
    Single {
        #[serde(with = "oxide_datapack::ident_serde::rl")]
        location: ResourceLocation,
        #[serde(default)]
        projection: String,
        #[serde(default = "default_empty_rl", with = "oxide_datapack::ident_serde::rl")]
        processors: ResourceLocation,
    },
    #[serde(rename = "minecraft:legacy_single_pool_element")]
    LegacySingle {
        #[serde(with = "oxide_datapack::ident_serde::rl")]
        location: ResourceLocation,
        #[serde(default)]
        projection: String,
        #[serde(default = "default_empty_rl", with = "oxide_datapack::ident_serde::rl")]
        processors: ResourceLocation,
    },
    #[serde(rename = "minecraft:empty_pool_element")]
    Empty,
    #[serde(rename = "minecraft:list_pool_element")]
    List {
        elements: Vec<PoolElement>,
        #[serde(default)]
        projection: String,
    },
    #[serde(rename = "minecraft:feature_pool_element")]
    Feature {
        #[serde(with = "oxide_datapack::ident_serde::rl")]
        feature: ResourceLocation,
        #[serde(default)]
        projection: String,
    },
}
