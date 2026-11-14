//! `minecraft:tree` and `minecraft:fallen_tree` configuration, typed.
//!
//! Deserialization only. Every component type that appears in the 26.2 `configured_feature`
//! export has a variant here (trunk, foliage and root placers, feature sizes, tree decorators),
//! so a datapack naming one this crate has never heard of fails at load time instead of being
//! skipped at generation time. Types vanilla defines in later versions (poplar, shelf mushroom)
//! are deliberately absent: they are not 26.2 data.
//!
//! Shared fields (`base_height`, `radius`, ...) live on the struct and the per-type fields in a
//! flattened, `type`-tagged enum, which mirrors how the game's codecs are written.

use serde::{Deserialize, Serialize};

use crate::feature::BlockStateProvider;
use crate::placement::{BlockList, IntProvider};

/// `TreeConfiguration`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeConfig {
    pub trunk_provider: BlockStateProvider,
    pub trunk_placer: TrunkPlacer,
    pub foliage_provider: BlockStateProvider,
    pub foliage_placer: FoliagePlacer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_placer: Option<RootPlacer>,
    pub minimum_size: FeatureSize,
    pub decorators: Vec<TreeDecorator>,
    #[serde(default)]
    pub ignore_vines: bool,
    pub below_trunk_provider: BlockStateProvider,
}

/// `FallenTreeConfiguration`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallenTreeConfig {
    pub trunk_provider: BlockStateProvider,
    pub log_length: IntProvider,
    pub stump_decorators: Vec<TreeDecorator>,
    pub log_decorators: Vec<TreeDecorator>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrunkPlacer {
    pub base_height: i32,
    pub height_rand_a: i32,
    pub height_rand_b: i32,
    #[serde(flatten)]
    pub kind: TrunkKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TrunkKind {
    #[serde(rename = "minecraft:straight_trunk_placer")]
    Straight {},
    #[serde(rename = "minecraft:forking_trunk_placer")]
    Forking {},
    #[serde(rename = "minecraft:giant_trunk_placer")]
    Giant {},
    #[serde(rename = "minecraft:mega_jungle_trunk_placer")]
    MegaJungle {},
    #[serde(rename = "minecraft:dark_oak_trunk_placer")]
    DarkOak {},
    #[serde(rename = "minecraft:fancy_trunk_placer")]
    Fancy {},
    #[serde(rename = "minecraft:bending_trunk_placer")]
    Bending {
        #[serde(default = "one")]
        min_height_for_leaves: i32,
        bend_length: IntProvider,
    },
    #[serde(rename = "minecraft:upwards_branching_trunk_placer")]
    UpwardsBranching {
        extra_branch_steps: IntProvider,
        place_branch_per_log_probability: f32,
        extra_branch_length: IntProvider,
        can_grow_through: BlockList,
    },
    #[serde(rename = "minecraft:cherry_trunk_placer")]
    Cherry {
        branch_count: IntProvider,
        branch_horizontal_length: IntProvider,
        branch_start_offset_from_top: UniformRange,
        branch_end_offset_from_top: IntProvider,
    },
}

/// A `uniform` int provider written without the optional `type` tag, as the cherry trunk's
/// branch start offset is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniformRange {
    pub min_inclusive: i32,
    pub max_inclusive: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FoliagePlacer {
    pub radius: IntProvider,
    pub offset: IntProvider,
    #[serde(flatten)]
    pub kind: FoliageKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FoliageKind {
    #[serde(rename = "minecraft:blob_foliage_placer")]
    Blob { height: i32 },
    #[serde(rename = "minecraft:fancy_foliage_placer")]
    Fancy { height: i32 },
    #[serde(rename = "minecraft:bush_foliage_placer")]
    Bush { height: i32 },
    #[serde(rename = "minecraft:jungle_foliage_placer")]
    Jungle { height: i32 },
    #[serde(rename = "minecraft:acacia_foliage_placer")]
    Acacia {},
    #[serde(rename = "minecraft:dark_oak_foliage_placer")]
    DarkOak {},
    #[serde(rename = "minecraft:spruce_foliage_placer")]
    Spruce { trunk_height: IntProvider },
    #[serde(rename = "minecraft:pine_foliage_placer")]
    Pine { height: IntProvider },
    #[serde(rename = "minecraft:mega_pine_foliage_placer")]
    MegaPine { crown_height: IntProvider },
    #[serde(rename = "minecraft:random_spread_foliage_placer")]
    RandomSpread {
        foliage_height: IntProvider,
        leaf_placement_attempts: i32,
    },
    #[serde(rename = "minecraft:cherry_foliage_placer")]
    Cherry {
        height: IntProvider,
        wide_bottom_layer_hole_chance: f32,
        corner_hole_chance: f32,
        hanging_leaves_chance: f32,
        hanging_leaves_extension_chance: f32,
    },
}

/// `FeatureSize`: how wide a tree's free column must be at each height.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FeatureSize {
    #[serde(rename = "minecraft:two_layers_feature_size")]
    TwoLayers {
        #[serde(default = "one")]
        limit: i32,
        #[serde(default)]
        lower_size: i32,
        #[serde(default = "one")]
        upper_size: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_clipped_height: Option<i32>,
    },
    #[serde(rename = "minecraft:three_layers_feature_size")]
    ThreeLayers {
        #[serde(default = "one")]
        limit: i32,
        #[serde(default = "one")]
        upper_limit: i32,
        #[serde(default)]
        lower_size: i32,
        #[serde(default = "one")]
        middle_size: i32,
        #[serde(default = "one")]
        upper_size: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_clipped_height: Option<i32>,
    },
}

impl FeatureSize {
    /// `FeatureSize.minClippedHeight`.
    pub fn min_clipped_height(&self) -> Option<i32> {
        match self {
            Self::TwoLayers {
                min_clipped_height, ..
            }
            | Self::ThreeLayers {
                min_clipped_height, ..
            } => *min_clipped_height,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RootPlacer {
    #[serde(rename = "minecraft:mangrove_root_placer")]
    Mangrove {
        trunk_offset_y: IntProvider,
        root_provider: BlockStateProvider,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        above_root_placement: Option<AboveRootPlacement>,
        mangrove_root_placement: MangroveRootPlacement,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AboveRootPlacement {
    pub above_root_provider: BlockStateProvider,
    pub above_root_placement_chance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MangroveRootPlacement {
    pub can_grow_through: BlockList,
    pub muddy_roots_in: BlockList,
    pub muddy_roots_provider: BlockStateProvider,
    pub max_root_width: i32,
    pub max_root_length: i32,
    pub random_skew_chance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TreeDecorator {
    #[serde(rename = "minecraft:trunk_vine")]
    TrunkVine {},
    #[serde(rename = "minecraft:leave_vine")]
    LeaveVine { probability: f32 },
    #[serde(rename = "minecraft:cocoa")]
    Cocoa { probability: f32 },
    #[serde(rename = "minecraft:beehive")]
    Beehive { probability: f32 },
    #[serde(rename = "minecraft:alter_ground")]
    AlterGround { provider: BlockStateProvider },
    #[serde(rename = "minecraft:attached_to_leaves")]
    AttachedToLeaves {
        probability: f32,
        exclusion_radius_xz: i32,
        exclusion_radius_y: i32,
        block_provider: BlockStateProvider,
        required_empty_blocks: i32,
        directions: Vec<String>,
    },
    #[serde(rename = "minecraft:attached_to_logs")]
    AttachedToLogs {
        probability: f32,
        block_provider: BlockStateProvider,
        directions: Vec<String>,
    },
    #[serde(rename = "minecraft:creaking_heart")]
    CreakingHeart { probability: f32 },
    #[serde(rename = "minecraft:pale_moss")]
    PaleMoss {
        leaves_probability: f32,
        trunk_probability: f32,
        ground_probability: f32,
    },
    #[serde(rename = "minecraft:place_on_ground")]
    PlaceOnGround {
        #[serde(default = "default_tries")]
        tries: i32,
        #[serde(default = "default_radius")]
        radius: i32,
        #[serde(default = "one")]
        height: i32,
        block_state_provider: BlockStateProvider,
    },
}

fn one() -> i32 {
    1
}

fn default_tries() -> i32 {
    128
}

fn default_radius() -> i32 {
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flattened_placers_and_defaults() {
        let tree: TreeConfig = serde_json::from_str(
            r#"{
              "trunk_provider": {"type": "minecraft:simple_state_provider", "state": {"Name": "minecraft:oak_log"}},
              "below_trunk_provider": {"type": "minecraft:simple_state_provider", "state": {"Name": "minecraft:dirt"}},
              "foliage_provider": {"type": "minecraft:simple_state_provider", "state": {"Name": "minecraft:oak_leaves"}},
              "decorators": [{"type": "minecraft:trunk_vine"}],
              "foliage_placer": {"type": "minecraft:blob_foliage_placer", "height": 3, "offset": 0, "radius": 2},
              "minimum_size": {"type": "minecraft:two_layers_feature_size"},
              "trunk_placer": {"type": "minecraft:straight_trunk_placer", "base_height": 4, "height_rand_a": 2, "height_rand_b": 0}
            }"#,
        )
        .expect("tree config");
        assert_eq!(tree.trunk_placer.base_height, 4);
        assert!(matches!(tree.trunk_placer.kind, TrunkKind::Straight {}));
        assert!(matches!(
            tree.foliage_placer.kind,
            FoliageKind::Blob { height: 3 }
        ));
        assert!(matches!(
            tree.minimum_size,
            FeatureSize::TwoLayers {
                limit: 1,
                lower_size: 0,
                upper_size: 1,
                ..
            }
        ));
        assert!(!tree.ignore_vines);
    }
}
