//! Block predicates: the position tests placement filters and state-provider rules ask.
//!
//! Compiled once from the parsed IR so the hot path compares resource locations instead of
//! parsing strings. Every predicate type in the 26.2 placed- and configured-feature data is
//! covered. Two cannot be answered from the exported facts and say so through the tally instead
//! of guessing: `would_survive` (per-block Java) and `has_sturdy_face` on a horizontal face
//! (the export only records the top and bottom faces).

use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use oxide_core::{BlockPos, BlockState, ResourceLocation};
use oxide_datapack::placement::{BlockList, BlockPredicate};
use oxide_datapack::{BlockFact, BlockFacts};

use super::context::{block_in, Environment, FeatureContext};
use super::state_provider::state_from_spec;

/// A set of blocks or fluids written as one id, a list of ids, or a `#tag`.
#[derive(Debug, Clone)]
pub enum IdSet {
    Ids(Vec<ResourceLocation>),
    Tag(ResourceLocation),
}

impl IdSet {
    pub fn compile(list: &BlockList) -> Result<Self> {
        let parse = |text: &str| {
            ResourceLocation::from_str(text).map_err(|_| anyhow!("invalid resource location {text:?}"))
        };
        match list {
            BlockList::One(text) => match text.strip_prefix('#') {
                Some(tag) => Ok(Self::Tag(parse(tag)?)),
                None => Ok(Self::Ids(vec![parse(text)?])),
            },
            BlockList::Many(texts) => texts
                .iter()
                .map(|text| parse(text))
                .collect::<Result<Vec<_>>>()
                .map(Self::Ids),
        }
    }

    /// Whether the block named `name` is in the set; a tag is looked up in the block tags.
    pub fn contains_block(&self, env: &Environment, name: &ResourceLocation) -> bool {
        match self {
            Self::Ids(ids) => ids.contains(name),
            Self::Tag(tag) => env.block_tags.contains(tag, name),
        }
    }
}

/// The offset every positional predicate carries, defaulting to none.
fn offset_of(offset: &Option<[i32; 3]>) -> [i32; 3] {
    offset.unwrap_or([0, 0, 0])
}

fn moved(pos: BlockPos, offset: [i32; 3]) -> BlockPos {
    BlockPos::new(
        pos.x.wrapping_add(offset[0]),
        pos.y.wrapping_add(offset[1]),
        pos.z.wrapping_add(offset[2]),
    )
}

/// The fluid id a block state's fluid has, as the path under `minecraft:`.
///
/// The facts record whether a state's fluid is water or lava and whether it is a source; those
/// are the only four fluids that exist plus "none".
fn fluid_path(facts: &BlockFacts, state: &BlockState) -> &'static str {
    let source = facts.is(state, BlockFact::FluidSource);
    if facts.is(state, BlockFact::Water) {
        if source { "water" } else { "flowing_water" }
    } else if facts.is(state, BlockFact::Lava) {
        if source { "lava" } else { "flowing_lava" }
    } else {
        "empty"
    }
}

#[derive(Debug, Clone)]
pub enum Direction {
    Up,
    Down,
    Horizontal,
}

#[derive(Debug, Clone)]
pub enum Predicate {
    True,
    Not(Box<Predicate>),
    AllOf(Vec<Predicate>),
    AnyOf(Vec<Predicate>),
    MatchingBlocks { blocks: IdSet, offset: [i32; 3] },
    MatchingBlockTag { tag: ResourceLocation, offset: [i32; 3] },
    MatchingFluids { fluids: IdSet, offset: [i32; 3] },
    WouldSurvive { state: BlockState, offset: [i32; 3] },
    Replaceable { offset: [i32; 3] },
    Solid { offset: [i32; 3] },
    InsideWorldBounds { offset: [i32; 3] },
    HasSturdyFace { direction: Direction, offset: [i32; 3] },
}

impl Predicate {
    pub fn compile(ir: &BlockPredicate, facts: &BlockFacts) -> Result<Self> {
        Ok(match ir {
            BlockPredicate::AlwaysTrue => Self::True,
            BlockPredicate::Not { predicate } => {
                Self::Not(Box::new(Self::compile(predicate, facts)?))
            }
            BlockPredicate::AllOf { predicates } => Self::AllOf(compile_all(predicates, facts)?),
            BlockPredicate::AnyOf { predicates } => Self::AnyOf(compile_all(predicates, facts)?),
            BlockPredicate::MatchingBlocks { blocks, offset } => Self::MatchingBlocks {
                blocks: IdSet::compile(blocks)?,
                offset: offset_of(offset),
            },
            BlockPredicate::MatchingBlockTag { tag, offset } => Self::MatchingBlockTag {
                tag: ResourceLocation::from_str(tag.trim_start_matches('#'))
                    .map_err(|_| anyhow!("invalid tag {tag:?}"))?,
                offset: offset_of(offset),
            },
            BlockPredicate::MatchingFluids { fluids, offset } => Self::MatchingFluids {
                fluids: IdSet::compile(fluids)?,
                offset: offset_of(offset),
            },
            BlockPredicate::WouldSurvive { state, offset } => Self::WouldSurvive {
                state: state_from_spec(state, facts).context("would_survive state")?,
                offset: offset_of(offset),
            },
            BlockPredicate::Replaceable { offset } => Self::Replaceable { offset: offset_of(offset) },
            BlockPredicate::Solid { offset } => Self::Solid { offset: offset_of(offset) },
            BlockPredicate::InsideWorldBounds { offset } => {
                Self::InsideWorldBounds { offset: offset_of(offset) }
            }
            BlockPredicate::HasSturdyFace { offset, direction } => Self::HasSturdyFace {
                direction: match direction.as_str() {
                    "up" => Direction::Up,
                    "down" => Direction::Down,
                    "north" | "south" | "east" | "west" => Direction::Horizontal,
                    other => return Err(anyhow!("unknown direction {other:?}")),
                },
                offset: offset_of(offset),
            },
        })
    }

    /// Evaluates the predicate at `pos`. `all_of` and `any_of` short-circuit in list order.
    pub fn test(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        match self {
            Self::True => true,
            Self::Not(inner) => !inner.test(cx, pos),
            Self::AllOf(all) => all.iter().all(|predicate| predicate.test(cx, pos)),
            Self::AnyOf(any) => any.iter().any(|predicate| predicate.test(cx, pos)),
            Self::MatchingBlocks { blocks, offset } => {
                let state = block_in(cx.region, moved(pos, *offset));
                blocks.contains_block(cx.env, &state.name)
            }
            Self::MatchingBlockTag { tag, offset } => {
                let state = block_in(cx.region, moved(pos, *offset));
                cx.env.block_tags.contains(tag, &state.name)
            }
            Self::MatchingFluids { fluids, offset } => {
                let state = block_in(cx.region, moved(pos, *offset));
                let path = fluid_path(&cx.env.facts, state);
                match fluids {
                    IdSet::Ids(ids) => ids
                        .iter()
                        .any(|id| id.namespace() == "minecraft" && id.path() == path),
                    IdSet::Tag(tag) => {
                        cx.env.fluid_tags.contains(tag, &ResourceLocation::minecraft(path))
                    }
                }
            }
            Self::WouldSurvive { state, offset } => {
                match cx.env.behaviours.can_survive(state, cx.region, moved(pos, *offset)) {
                    Some(survives) => survives,
                    None => {
                        cx.unsupported.record("survival", &state.name.to_string());
                        false
                    }
                }
            }
            Self::Replaceable { offset } => {
                cx.region.is(moved(pos, *offset), BlockFact::Replaceable)
            }
            Self::Solid { offset } => cx.region.is(moved(pos, *offset), BlockFact::Solid),
            Self::InsideWorldBounds { offset } => {
                !cx.outside_build_height(moved(pos, *offset).y)
            }
            Self::HasSturdyFace { direction, offset } => {
                let at = moved(pos, *offset);
                match direction {
                    Direction::Up => cx.region.is(at, BlockFact::SturdyTop),
                    Direction::Down => cx.region.is(at, BlockFact::SturdyBottom),
                    Direction::Horizontal => {
                        cx.unsupported.record("predicate", "has_sturdy_face horizontal");
                        false
                    }
                }
            }
        }
    }
}

fn compile_all(predicates: &[BlockPredicate], facts: &BlockFacts) -> Result<Vec<Predicate>> {
    predicates
        .iter()
        .map(|predicate| Predicate::compile(predicate, facts))
        .collect()
}
