//! Configurations of the vegetation and underwater feature families: patches, root systems,
//! multiface growth, bamboo and the huge mushrooms.
//!
//! They live apart from `feature.rs` so that file stays the registry of feature types and the
//! shared sub-registries, while each family's field list sits next to the others of its kind.
//! Defaults are only the ones vanilla's codecs declare (`optionalFieldOf`); every other field is
//! required, so a datapack the server would reject fails here too.

use serde::{Deserialize, Serialize};

use crate::feature::{BlockStateProvider, Holder};
use crate::placement::{BlockList, BlockPredicate, IntProvider, PlacedFeature};

/// Which face of the cave a vegetation patch grows on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaveSurface {
    Floor,
    Ceiling,
}

/// `vegetation_patch` and `waterlogged_vegetation_patch`, which share one codec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VegetationPatchConfig {
    pub surface: CaveSurface,
    pub depth: IntProvider,
    pub vertical_range: i32,
    pub extra_bottom_block_chance: f32,
    pub extra_edge_column_chance: f32,
    pub vegetation_chance: f32,
    pub xz_radius: IntProvider,
    pub replaceable: BlockList,
    pub ground_state: BlockStateProvider,
    pub vegetation_feature: Holder<PlacedFeature>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootSystemConfig {
    pub feature: Holder<PlacedFeature>,
    pub required_vertical_space_for_tree: i32,
    pub root_radius: i32,
    pub root_replaceable: BlockList,
    pub root_state_provider: BlockStateProvider,
    pub root_placement_attempts: i32,
    pub root_column_max_height: i32,
    pub hanging_root_radius: i32,
    pub hanging_roots_vertical_span: i32,
    pub hanging_root_state_provider: BlockStateProvider,
    pub hanging_root_placement_attempts: i32,
    pub allowed_vertical_water_for_tree: i32,
    pub allowed_tree_position: BlockPredicate,
    pub level_test_distance: i32,
    pub max_level_deviation: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultifaceGrowthConfig {
    /// The block id; vanilla's codec only accepts a spreadable multiface block.
    pub block: String,
    #[serde(default = "default_search_range")]
    pub search_range: i32,
    #[serde(default)]
    pub can_place_on_floor: bool,
    #[serde(default)]
    pub can_place_on_ceiling: bool,
    #[serde(default)]
    pub can_place_on_wall: bool,
    #[serde(default = "default_chance_of_spreading")]
    pub chance_of_spreading: f32,
    pub can_be_placed_on: BlockList,
}

fn default_search_range() -> i32 {
    10
}

fn default_chance_of_spreading() -> f32 {
    0.5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BambooConfig {
    pub probability: f32,
}

/// `huge_red_mushroom` and `huge_brown_mushroom`, which share one codec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HugeMushroomConfig {
    pub cap_provider: BlockStateProvider,
    pub stem_provider: BlockStateProvider,
    #[serde(default = "default_foliage_radius")]
    pub foliage_radius: i32,
    pub can_place_on: BlockPredicate,
}

fn default_foliage_radius() -> i32 {
    2
}
