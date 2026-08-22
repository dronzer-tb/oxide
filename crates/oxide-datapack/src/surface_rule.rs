//! Surface rule tree embedded in `noise_settings.surface_rule`. Recursive,
//! same treatment as density functions: typed IR, no evaluation.
//!
// PARITY-CHECK: field names/shape recalled from vanilla's `SurfaceRules`
// codecs, unverified against a real Minecraft 26.2 data export; confirm
// before trusting this on real noise_settings files (see docs/REFERENCE_DATA.md).

use oxide_core::{BlockState, ResourceLocation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SurfaceRule {
    #[serde(rename = "minecraft:sequence")]
    Sequence { sequence: Vec<SurfaceRule> },

    #[serde(rename = "minecraft:condition")]
    Condition {
        if_true: SurfaceCondition,
        then_run: Box<SurfaceRule>,
    },

    #[serde(rename = "minecraft:block")]
    Block {
        #[serde(with = "crate::ident_serde::block_state")]
        result_state: BlockState,
    },

    #[serde(rename = "minecraft:bandlands")]
    Badlands {},
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SurfaceCondition {
    #[serde(rename = "minecraft:biome")]
    Biome {
        #[serde(with = "crate::ident_serde::rl_vec")]
        biome_is: Vec<ResourceLocation>,
    },

    #[serde(rename = "minecraft:noise_threshold")]
    NoiseThreshold {
        #[serde(with = "crate::ident_serde::rl")]
        noise: ResourceLocation,
        min_threshold: f64,
        max_threshold: f64,
        /// 26.2 addition; absent in older exports, where the sample was always 2D.
        #[serde(default)]
        is_3d: bool,
    },

    #[serde(rename = "minecraft:vertical_gradient")]
    VerticalGradient {
        random_name: String,
        true_at_and_below: VerticalAnchor,
        false_at_and_above: VerticalAnchor,
    },

    #[serde(rename = "minecraft:y_above")]
    YAbove {
        anchor: VerticalAnchor,
        #[serde(default)]
        surface_depth_multiplier: i32,
        #[serde(default)]
        add_stone_depth: bool,
    },

    #[serde(rename = "minecraft:water")]
    Water {
        #[serde(default)]
        offset: i32,
        #[serde(default)]
        surface_depth_multiplier: i32,
        #[serde(default)]
        add_stone_depth: bool,
    },

    #[serde(rename = "minecraft:temperature")]
    Temperature {},

    #[serde(rename = "minecraft:steep")]
    Steep {},

    #[serde(rename = "minecraft:hole")]
    Hole {},

    #[serde(rename = "minecraft:above_preliminary_surface")]
    AbovePreliminarySurface {},

    #[serde(rename = "minecraft:stone_depth")]
    StoneDepth {
        #[serde(default)]
        offset: i32,
        add_surface_depth: bool,
        secondary_depth_range: i32,
        surface_type: SurfaceType,
    },

    #[serde(rename = "minecraft:not")]
    Not { invert: Box<SurfaceCondition> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SurfaceType {
    #[serde(rename = "floor")]
    Floor,
    #[serde(rename = "ceiling")]
    Ceiling,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VerticalAnchor {
    Absolute { absolute: i32 },
    AboveBottom { above_bottom: i32 },
    BelowTop { below_top: i32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 26.2 writes a single biome as a bare string where earlier exports always
    /// wrote a one-element array. Both mean the same condition.
    #[test]
    fn biome_is_accepts_a_bare_id_or_a_list() {
        let scalar: SurfaceCondition = serde_json::from_slice(
            br#"{"type":"minecraft:biome","biome_is":"minecraft:badlands"}"#,
        )
        .expect("26.2 scalar biome_is");
        let list: SurfaceCondition = serde_json::from_slice(
            br#"{"type":"minecraft:biome","biome_is":["minecraft:badlands"]}"#,
        )
        .expect("pre-26.2 list biome_is");
        let ids = |condition: &SurfaceCondition| match condition {
            SurfaceCondition::Biome { biome_is } => biome_is.clone(),
            other => panic!("expected a biome condition, got {other:?}"),
        };
        assert_eq!(ids(&scalar), ids(&list));
        assert_eq!(ids(&scalar).len(), 1);
    }
}
