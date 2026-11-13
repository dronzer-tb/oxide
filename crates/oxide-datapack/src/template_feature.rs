//! Configuration IR of the two features that place structure templates, `fossil` and
//! `template`, and of the structure processors a `fossil` runs its templates through.
//!
//! `fossil` is vanilla's `FossilFeatureConfiguration`: parallel lists of base and overlay
//! templates (the same index is used in both), a processor list for each, and how many of the
//! base's eight bounding-box corners may sit in air or fluid. `template` is
//! `TemplateFeatureConfiguration`: a weighted list of template ids, each with the rotations it
//! may be placed under.
//!
//! Processor lists stay raw JSON values here, one per processor, like the
//! `worldgen/processor_list` registry they are usually referenced from; [`Processor`] is the
//! typed form of the processor types the generator implements, parsed one at a time so that an
//! unknown type is reported by name instead of failing the whole datapack.
//!
//! Deserialization only; the generator compiles these in `oxide-chunkgen`.

use oxide_core::ResourceLocation;
use serde::{Deserialize, Serialize};

use crate::feature::{Holder, RuleTest};
use crate::placement::{BlockList, BlockStateSpec};

/// `fossil`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FossilConfig {
    #[serde(with = "crate::ident_serde::rl_vec")]
    pub fossil_structures: Vec<ResourceLocation>,
    #[serde(with = "crate::ident_serde::rl_vec")]
    pub overlay_structures: Vec<ResourceLocation>,
    pub fossil_processors: Holder<ProcessorList>,
    pub overlay_processors: Holder<ProcessorList>,
    pub max_empty_corners_allowed: i32,
}

/// `template`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateConfig {
    pub templates: Vec<WeightedTemplate>,
}

/// One `WeightedList` entry of [`TemplateConfig::templates`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightedTemplate {
    pub data: TemplateEntry,
    pub weight: i32,
}

/// `TemplateFeatureConfiguration.TemplateEntry`. An absent `rotations` means all four, in
/// `Rotation.values()` order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateEntry {
    #[serde(with = "crate::ident_serde::rl")]
    pub id: ResourceLocation,
    #[serde(default)]
    pub rotations: Option<Vec<String>>,
}

/// A processor list: `{"processors": [...]}` or the bare list, as
/// `StructureProcessorType.DIRECT_CODEC` accepts both.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProcessorList {
    Object { processors: Vec<serde_json::Value> },
    List(Vec<serde_json::Value>),
}

impl ProcessorList {
    pub fn processors(&self) -> &[serde_json::Value] {
        match self {
            Self::Object { processors } | Self::List(processors) => processors,
        }
    }
}

/// The processor types the generator implements: the ones the 26.2 `fossil` processor lists
/// use.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "processor_type")]
pub enum Processor {
    /// Drops each block (of `rottable_blocks`, or any block) unless a float draw is at most
    /// `integrity`.
    #[serde(rename = "minecraft:block_rot")]
    BlockRot {
        integrity: f32,
        #[serde(default)]
        rottable_blocks: Option<BlockList>,
    },
    /// Drops each block whose target position already holds one of `value`.
    #[serde(rename = "minecraft:protected_blocks")]
    ProtectedBlocks { value: BlockList },
    /// Replaces each block with the output of the first rule that matches.
    #[serde(rename = "minecraft:rule")]
    Rule { rules: Vec<ProcessorRule> },
}

impl Processor {
    /// The `processor_type` ids [`Processor`] parses. A value naming any other type is a
    /// processor the generator does not implement.
    pub const TYPES: [&'static str; 3] =
        ["minecraft:block_rot", "minecraft:protected_blocks", "minecraft:rule"];
}

/// `ProcessorRule`. `position_predicate` and `block_entity_modifier` stay raw: absent they are
/// `always_true` and `passthrough`, and anything else is reported unsupported by the generator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessorRule {
    pub input_predicate: RuleTest,
    pub location_predicate: RuleTest,
    #[serde(default)]
    pub position_predicate: Option<serde_json::Value>,
    pub output_state: BlockStateSpec,
    #[serde(default)]
    pub block_entity_modifier: Option<serde_json::Value>,
}
