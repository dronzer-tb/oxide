//! Configuration IR of the Nether decoration features.
//!
//! One struct per configured-feature type that `oxide-chunkgen` places natively. The shapes are
//! the 26.2 export's: `delta_feature` carries two block states and two int providers,
//! `netherrack_replace_blobs` a target, a state and a radius, and `huge_fungus` four states, a
//! `matching_blocks` predicate and an optional `planted` flag (absent means `false`).
//!
//! Deserialization only; the generator compiles these in `oxide-chunkgen`.

use serde::{Deserialize, Serialize};

use crate::placement::{BlockPredicate, BlockStateSpec, IntProvider};

/// `delta_feature`: a patch of `contents` (lava) ringed by `rim` (magma) on a Nether floor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaConfig {
    pub contents: BlockStateSpec,
    pub rim: BlockStateSpec,
    pub size: IntProvider,
    pub rim_size: IntProvider,
}

/// `netherrack_replace_blobs`: replaces every `target` block in a diamond with `state`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaceBlobsConfig {
    pub target: BlockStateSpec,
    pub state: BlockStateSpec,
    pub radius: IntProvider,
}

/// `huge_fungus`: a crimson or warped fungus tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HugeFungusConfig {
    pub valid_base_block: BlockStateSpec,
    pub stem_state: BlockStateSpec,
    pub hat_state: BlockStateSpec,
    pub decor_state: BlockStateSpec,
    pub replaceable_blocks: BlockPredicate,
    #[serde(default)]
    pub planted: bool,
}
