//! Configuration IR of the ground and terrain decoration features.
//!
//! One struct per configured-feature type whose JSON shape is not already typed in
//! `feature.rs`. The shapes are the 26.2 export's: `block_column` carries `layers` and a
//! `direction`, `lake` four predicates beside its two providers, and
//! `weighted_random_selector` lists `{ data, weight }` entries of placed features. Types whose
//! config is empty (`freeze_top_layer`, `blue_ice`, `monster_room`, `desert_well`) keep their
//! raw-value variants in `feature.rs`.
//!
//! Deserialization only; the generator compiles these in `oxide-chunkgen`.

use serde::{Deserialize, Serialize};

use crate::feature::{BlockStateProvider, Holder};
use crate::placement::{BlockPredicate, BlockStateSpec, IntProvider, PlacedFeature};

/// `block_pile`: a two-layer heap of blocks, such as a pumpkin patch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockPileConfig {
    pub state_provider: BlockStateProvider,
}

/// `block_blob`: a few overlapping blobs of one block, such as a forest rock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockBlobConfig {
    pub state: BlockStateSpec,
    pub can_place_on: BlockPredicate,
}

/// One run of a `block_column`: `height` blocks drawn from `provider`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnLayer {
    pub height: IntProvider,
    pub provider: BlockStateProvider,
}

/// `block_column`: runs of blocks laid along one direction, such as sugar cane or cave vines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockColumnConfig {
    pub layers: Vec<ColumnLayer>,
    pub direction: String,
    pub allowed_placement: BlockPredicate,
    pub prioritize_tip: bool,
}

/// `lake`: a pool of `fluid` inside a shell of `barrier`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LakeConfig {
    pub fluid: BlockStateProvider,
    pub barrier: BlockStateProvider,
    pub can_place_feature: BlockPredicate,
    pub can_replace_with_air_or_fluid: BlockPredicate,
    pub can_replace_with_barrier: BlockPredicate,
}

/// `spike`: an ice spike.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpikeConfig {
    pub state: BlockStateSpec,
    pub can_place_on: BlockPredicate,
    pub can_replace: BlockPredicate,
}

/// `iceberg`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergConfig {
    pub state: BlockStateSpec,
}

/// `underwater_magma`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnderwaterMagmaConfig {
    pub floor_search_range: i32,
    pub placement_radius_around_floor: i32,
    pub placement_probability_per_valid_position: f32,
}

/// One entry of a `weighted_random_selector`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightedPlacedEntry {
    pub data: Holder<PlacedFeature>,
    pub weight: i32,
}

/// `weighted_random_selector`: places one entry, picked by weight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightedRandomSelectorConfig {
    pub features: Vec<WeightedPlacedEntry>,
}
