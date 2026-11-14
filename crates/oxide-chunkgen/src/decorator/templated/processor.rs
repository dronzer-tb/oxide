//! Structure processors: `StructureProcessor.processBlock` for the processor types the 26.2
//! `fossil` processor lists use (`block_rot`, `protected_blocks`, `rule`), and
//! `StructureTemplate.processBlockInfos`, which runs a list of them over a template.
//!
//! [`Processor::process`] reads nothing but the block already in the world at the target
//! position, the block tags and a random stream, so jigsaw placement can run the same values
//! once it places through processors. The stream is `StructurePlaceSettings.getRandom(pos)`:
//! the settings' own random when they carry one (every decoration feature sets it), otherwise a
//! `LegacyRandom` seeded by [`oxide_core::mth::get_seed`] of the position, which is the
//! caller's to build.
//!
//! None of the three types evaluates the whole piece (`evaluatesEntirePieceState`) or
//! finalizes (`finalizeProcessing`), so processing is block by block and only blocks inside the
//! settings' bounding box are processed — and only they draw from the stream.

use anyhow::{Context, Result};
use oxide_core::mth::get_seed;
use oxide_core::rotation::{transform, Mirror};
use oxide_core::{BlockPos, BlockState, LegacyRandom, RandomSource};
use oxide_datapack::template_feature::{Processor as ProcessorIr, ProcessorRule};
use oxide_datapack::{BlockFacts, TagSet};

use super::structure_template::{offset, BlockInfo, PlaceSettings};
use crate::decorator::context::{block_in, FeatureContext};
use crate::decorator::ore::RuleTest;
use crate::decorator::predicate::IdSet;
use crate::decorator::state_provider::state_from_spec;

/// A compiled processor.
#[derive(Debug, Clone)]
pub enum Processor {
    /// `BlockRotProcessor`.
    BlockRot {
        rottable: Option<IdSet>,
        integrity: f32,
    },
    /// `ProtectedBlockProcessor`.
    ProtectedBlocks(IdSet),
    /// `RuleProcessor`.
    Rule(Vec<Rule>),
}

/// `ProcessorRule` with the default position predicate (`always_true`) and block-entity
/// modifier (`passthrough`); any other is refused at compile time.
#[derive(Debug, Clone)]
pub struct Rule {
    input: RuleTest,
    location: RuleTest,
    output: BlockState,
}

impl Processor {
    /// Compiles one processor. `Ok(Err(type))` names a processor type, or a part of a rule,
    /// that is not implemented.
    pub fn compile(value: &serde_json::Value, facts: &BlockFacts) -> Result<Result<Self, String>> {
        let kind = value
            .get("processor_type")
            .and_then(|kind| kind.as_str())
            .unwrap_or("unknown");
        if !ProcessorIr::TYPES.contains(&kind) {
            return Ok(Err(kind.to_string()));
        }
        let ir: ProcessorIr =
            serde_json::from_value(value.clone()).with_context(|| format!("processor {kind}"))?;
        Ok(Ok(match ir {
            ProcessorIr::BlockRot {
                integrity,
                rottable_blocks,
            } => Self::BlockRot {
                rottable: rottable_blocks.as_ref().map(IdSet::compile).transpose()?,
                integrity,
            },
            ProcessorIr::ProtectedBlocks { value } => {
                Self::ProtectedBlocks(IdSet::compile(&value)?)
            }
            ProcessorIr::Rule { rules } => {
                let mut compiled = Vec::with_capacity(rules.len());
                for rule in &rules {
                    match Rule::compile(rule, facts)? {
                        Ok(rule) => compiled.push(rule),
                        Err(part) => return Ok(Err(format!("minecraft:rule ({part})"))),
                    }
                }
                Self::Rule(compiled)
            }
        }))
    }

    /// `processBlock`: `info` (at its world position) after this processor, or `None` when the
    /// processor drops it. `world` is the block already at `info.pos`; `random` is the
    /// settings' stream (see the module docs).
    pub fn process(
        &self,
        info: BlockInfo,
        world: &BlockState,
        tags: &TagSet,
        random: &mut impl RandomSource,
    ) -> Option<BlockInfo> {
        match self {
            Self::BlockRot {
                rottable,
                integrity,
            } => {
                let eligible = rottable
                    .as_ref()
                    .is_none_or(|set| set.contains_in(tags, &info.state.name));
                // `!(nextFloat() <= integrity)`: the draw happens only for an eligible block.
                if eligible && random.next_float() > *integrity {
                    None
                } else {
                    Some(info)
                }
            }
            Self::ProtectedBlocks(set) => (!set.contains_in(tags, &world.name)).then_some(info),
            Self::Rule(rules) => {
                // `RandomSource.create(Mth.getSeed(pos))`, shared by every rule of this block.
                let mut rule_random =
                    LegacyRandom::new(get_seed(info.pos.x, info.pos.y, info.pos.z));
                for rule in rules {
                    if rule.input.test(&info.state, &mut rule_random, tags)
                        && rule.location.test(world, &mut rule_random, tags)
                    {
                        return Some(BlockInfo {
                            state: rule.output.clone(),
                            ..info
                        });
                    }
                }
                Some(info)
            }
        }
    }
}

impl Rule {
    fn compile(ir: &ProcessorRule, facts: &BlockFacts) -> Result<Result<Self, String>> {
        let kind_of = |value: &serde_json::Value, tag: &str| {
            value
                .get(tag)
                .and_then(|kind| kind.as_str())
                .map(str::to_owned)
                .unwrap_or_default()
        };
        if let Some(predicate) = &ir.position_predicate {
            let kind = kind_of(predicate, "predicate_type");
            if kind != "minecraft:always_true" {
                return Ok(Err(format!("position_predicate {kind}")));
            }
        }
        if let Some(modifier) = &ir.block_entity_modifier {
            let kind = kind_of(modifier, "type");
            if kind != "minecraft:passthrough" {
                return Ok(Err(format!("block_entity_modifier {kind}")));
            }
        }
        Ok(Ok(Self {
            input: RuleTest::compile(&ir.input_predicate, facts)?,
            location: RuleTest::compile(&ir.location_predicate, facts)?,
            output: state_from_spec(&ir.output_state, facts)?,
        }))
    }
}

/// `StructureTemplate.processBlockInfos` with the feature's random as the settings' stream:
/// each block moved to its world position, then run through every processor in order until
/// one drops it. Blocks outside the bounding box are skipped before any processor sees them.
pub(super) fn process_block_infos(
    cx: &mut FeatureContext<'_, '_, '_>,
    position: BlockPos,
    settings: &PlaceSettings<'_>,
    blocks: &[BlockInfo],
) -> Vec<BlockInfo> {
    let origin = BlockPos::new(0, 0, 0);
    let tags = &cx.env.block_tags;
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        let moved = transform(block.pos, Mirror::None, settings.rotation, origin);
        let pos = offset(moved, position.x, position.y, position.z);
        if settings
            .bounding_box
            .is_some_and(|bounds| !bounds.is_inside(pos))
        {
            continue;
        }
        let mut info = Some(BlockInfo {
            pos,
            ..block.clone()
        });
        for processor in settings.processors {
            let Some(current) = info else { break };
            let world = block_in(cx.region, current.pos);
            info = processor.process(current, world, tags, cx.random);
        }
        out.extend(info);
    }
    out
}
