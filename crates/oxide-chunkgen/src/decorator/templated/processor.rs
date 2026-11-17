//! Structure processors: `StructureProcessor.processBlock` for the processor types the 26.2
//! `fossil` processor lists use (`block_rot`, `protected_blocks`, `rule`) and those the
//! hard-coded structure pieces add (`block_ignore`, `block_age`, `blackstone_replace`,
//! `lava_submerged_block`, `capped`), and `StructureTemplate.processBlockInfos`, which runs a
//! list of them over a template.
//!
//! [`Processor::process`] reads nothing but the block already in the world at the target
//! position, the block tags and facts and a random stream, so jigsaw placement can run the same
//! values once it places through processors. The stream is `StructurePlaceSettings.getRandom
//! (pos)`: the settings' own random when they carry one (every decoration feature sets it),
//! otherwise a fresh `LegacyRandom` seeded by [`oxide_core::mth::get_seed`] of the position for
//! each call (every structure piece).
//!
//! Only `capped` evaluates the whole piece (`evaluatesEntirePieceState`): with it in the list
//! every block is processed, wherever it lands, so its choice is the same in every chunk the
//! piece crosses. Without it only blocks inside the settings' bounding box are processed, and
//! only they draw from the stream.

use anyhow::{Context, Result};
use oxide_core::mth::get_seed;
use oxide_core::rotation::transform;
use oxide_core::{
    BlockPos, BlockState, LegacyPositionalRandomFactory, LegacyRandom, PositionalRandomFactory,
    RandomSource, ResourceLocation,
};
use oxide_datapack::template_feature::{Processor as ProcessorIr, ProcessorRule};
use oxide_datapack::{BlockFact, BlockFacts, TagSet};

use super::structure_template::{offset, BlockInfo, BlockNbt, PlaceSettings};
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
    /// `BlockIgnoreProcessor`: drops these blocks.
    BlockIgnore(Vec<ResourceLocation>),
    /// `BlockAgeProcessor`.
    BlockAge { mossiness: f32 },
    /// `BlackstoneReplaceProcessor.INSTANCE`.
    BlackstoneReplace,
    /// `LavaSubmergedBlockProcessor.INSTANCE`.
    LavaSubmerged,
    /// `CappedProcessor` with a `ConstantInt` limit, the only kind 26.2 builds.
    Capped {
        delegate: Box<Processor>,
        limit: i32,
    },
}

/// `ProcessorRule` with the default position predicate (`always_true`), and a block-entity
/// modifier of `passthrough` or `append_loot`; any other is refused at compile time.
#[derive(Debug, Clone)]
pub struct Rule {
    input: RuleTest,
    location: RuleTest,
    output: BlockState,
    /// `AppendLoot`'s loot table.
    append_loot: Option<String>,
}

impl Rule {
    pub fn new(
        input: RuleTest,
        location: RuleTest,
        output: BlockState,
        append_loot: Option<String>,
    ) -> Self {
        Self {
            input,
            location,
            output,
            append_loot,
        }
    }
}

/// `settings.getRandom(pos)`: the stream when the settings carry one, else a fresh positional
/// random, handed to `f`.
fn with_settings_random<T>(
    stream: Option<&mut dyn RandomSource>,
    pos: BlockPos,
    f: impl FnOnce(&mut dyn RandomSource) -> T,
) -> T {
    match stream {
        Some(random) => f(random),
        None => f(&mut LegacyRandom::new(get_seed(pos.x, pos.y, pos.z))),
    }
}

/// `block.withPropertiesOf(state)`: `block`'s default state with every property `state` shares
/// with it copied over.
fn with_properties_of(block: &str, state: &BlockState, facts: &BlockFacts) -> BlockState {
    let name = ResourceLocation::minecraft(block);
    let mut out = facts
        .default_state(&name)
        .cloned()
        .unwrap_or_else(|| BlockState::new(name));
    for (key, value) in &state.properties {
        if let Some(slot) = out.properties.get_mut(key) {
            slot.clone_from(value);
        }
    }
    out
}

fn default_state(block: &str, facts: &BlockFacts) -> BlockState {
    let name = ResourceLocation::minecraft(block);
    facts
        .default_state(&name)
        .cloned()
        .unwrap_or_else(|| BlockState::new(name))
}

/// `BlackstoneReplaceProcessor.replacements`.
fn blackstone_replacement(path: &str) -> Option<&'static str> {
    Some(match path {
        "cobblestone" | "mossy_cobblestone" => "blackstone",
        "stone" => "polished_blackstone",
        "stone_bricks" | "mossy_stone_bricks" => "polished_blackstone_bricks",
        "cobblestone_stairs" | "mossy_cobblestone_stairs" => "blackstone_stairs",
        "stone_stairs" => "polished_blackstone_stairs",
        "stone_brick_stairs" | "mossy_stone_brick_stairs" => "polished_blackstone_brick_stairs",
        "cobblestone_slab" | "mossy_cobblestone_slab" => "blackstone_slab",
        "smooth_stone_slab" | "stone_slab" => "polished_blackstone_slab",
        "stone_brick_slab" | "mossy_stone_brick_slab" => "polished_blackstone_brick_slab",
        "stone_brick_wall" | "mossy_stone_brick_wall" => "polished_blackstone_brick_wall",
        "cobblestone_wall" | "mossy_cobblestone_wall" => "blackstone_wall",
        "chiseled_stone_bricks" => "chiseled_polished_blackstone",
        "cracked_stone_bricks" => "cracked_polished_blackstone_bricks",
        "iron_bars" => "iron_chain",
        _ => return None,
    })
}

/// `BlockAgeProcessor.processBlock` past its random: the replacement, if any.
fn age_block(
    state: &BlockState,
    mossiness: f32,
    tags: &TagSet,
    facts: &BlockFacts,
    random: &mut dyn RandomSource,
) -> Option<BlockState> {
    // `getRandomFacingStairs`: `Plane.HORIZONTAL.getRandomDirection` then `Util.getRandom(
    // Half.values())`, drawn while the replacement arrays are built, before the choice.
    let random_stairs = |block: &str, random: &mut dyn RandomSource| {
        let facing = ["north", "east", "south", "west"][random.next_int_bounded(4) as usize];
        let half = ["top", "bottom"][random.next_int_bounded(2) as usize];
        default_state(block, facts)
            .with_property("facing", facing)
            .with_property("half", half)
    };
    let pick = |random: &mut dyn RandomSource, plain: [BlockState; 2], mossy: [BlockState; 2]| {
        let [a, b] = if random.next_float() < mossiness {
            mossy
        } else {
            plain
        };
        if random.next_int_bounded(2) == 0 {
            a
        } else {
            b
        }
    };
    let path = state.name.path();
    let minecraft = state.name.namespace() == "minecraft";
    let tagged = |tag: &str| tags.contains(&ResourceLocation::minecraft(tag), &state.name);
    if minecraft && matches!(path, "stone_bricks" | "stone" | "chiseled_stone_bricks") {
        if random.next_float() >= 0.5 {
            return None;
        }
        let plain = [
            default_state("cracked_stone_bricks", facts),
            random_stairs("stone_brick_stairs", random),
        ];
        let mossy = [
            default_state("mossy_stone_bricks", facts),
            random_stairs("mossy_stone_brick_stairs", random),
        ];
        Some(pick(random, plain, mossy))
    } else if tagged("stairs") {
        if random.next_float() >= 0.5 {
            return None;
        }
        let plain = [
            default_state("stone_slab", facts),
            default_state("stone_brick_slab", facts),
        ];
        let mossy = [
            with_properties_of("mossy_stone_brick_stairs", state, facts),
            default_state("mossy_stone_brick_slab", facts),
        ];
        Some(pick(random, plain, mossy))
    } else if tagged("slabs") {
        (random.next_float() < mossiness)
            .then(|| with_properties_of("mossy_stone_brick_slab", state, facts))
    } else if tagged("walls") {
        (random.next_float() < mossiness)
            .then(|| with_properties_of("mossy_stone_brick_wall", state, facts))
    } else if minecraft && path == "obsidian" {
        (random.next_float() < 0.15).then(|| default_state("crying_obsidian", facts))
    } else {
        None
    }
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
    /// processor drops it. `world` is the block already at `info.pos`; `stream` is the
    /// settings' random when they carry one (see the module docs).
    pub fn process(
        &self,
        info: BlockInfo,
        world: &BlockState,
        tags: &TagSet,
        facts: &BlockFacts,
        stream: Option<&mut dyn RandomSource>,
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
                if eligible
                    && with_settings_random(stream, info.pos, |random| random.next_float())
                        > *integrity
                {
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
                        // `AppendLoot.apply`: the seed is the rule random's next long.
                        let nbt = match &rule.append_loot {
                            Some(table) => Some(BlockNbt::Loot {
                                table: table.clone(),
                                seed: rule_random.next_long(),
                            }),
                            None => info.nbt,
                        };
                        return Some(BlockInfo {
                            pos: info.pos,
                            state: rule.output.clone(),
                            nbt,
                        });
                    }
                }
                Some(info)
            }
            Self::BlockIgnore(blocks) => (!blocks.contains(&info.state.name)).then_some(info),
            Self::BlockAge { mossiness } => {
                match with_settings_random(stream, info.pos, |random| {
                    age_block(&info.state, *mossiness, tags, facts, random)
                }) {
                    Some(state) => Some(BlockInfo { state, ..info }),
                    None => Some(info),
                }
            }
            Self::BlackstoneReplace => {
                let minecraft = info.state.name.namespace() == "minecraft";
                let Some(block) =
                    blackstone_replacement(info.state.name.path()).filter(|_| minecraft)
                else {
                    return Some(info);
                };
                let mut state = default_state(block, facts);
                for key in ["facing", "half", "type"] {
                    if let Some(value) = info.state.properties.get(key) {
                        if state.properties.contains_key(key) {
                            state = state.with_property(key, value);
                        }
                    }
                }
                Some(BlockInfo { state, ..info })
            }
            Self::LavaSubmerged => {
                // PARITY-CHECK: `Block.isShapeFullBlock(state.getShape(..))` is read as a full
                // collision cube; the two differ only for blocks whose outline and collision
                // shapes disagree, none of which a ruined-portal template holds. Verify against
                // `BlockFactsExporter` if a template using this processor gains one.
                let full = facts.is(&info.state, BlockFact::FullCollision);
                if world.name == ResourceLocation::minecraft("lava") && !full {
                    Some(BlockInfo {
                        state: default_state("lava", facts),
                        ..info
                    })
                } else {
                    Some(info)
                }
            }
            Self::Capped { .. } => Some(info),
        }
    }

    /// `evaluatesEntirePieceState`.
    fn evaluates_entire_piece(&self) -> bool {
        matches!(self, Self::Capped { .. })
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
            append_loot: None,
        }))
    }
}

/// `StructureTemplate.processBlockInfos`: each block moved to its world position, then run
/// through every processor in order until one drops it, then each processor's
/// `finalizeProcessing`. Unless a processor evaluates the whole piece, blocks outside the
/// bounding box are skipped before any processor sees them.
pub(super) fn process_block_infos(
    cx: &mut FeatureContext<'_, '_, '_>,
    position: BlockPos,
    settings: &PlaceSettings<'_>,
    blocks: &[BlockInfo],
) -> Vec<BlockInfo> {
    let whole_piece = settings
        .processors
        .iter()
        .any(Processor::evaluates_entire_piece);
    let env = cx.env;
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        let moved = transform(
            block.pos,
            settings.mirror,
            settings.rotation,
            settings.pivot,
        );
        let pos = offset(moved, position.x, position.y, position.z);
        if !whole_piece
            && settings
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
            let stream =
                (!settings.positional_random).then_some(&mut *cx.random as &mut dyn RandomSource);
            info = processor.process(current, world, &env.block_tags, &env.facts, stream);
        }
        out.extend(info);
    }
    for processor in settings.processors {
        if let Processor::Capped { delegate, limit } = processor {
            finalize_capped(cx, position, delegate, *limit, &mut out);
        }
    }
    out
}

/// `CappedProcessor.finalizeProcessing`: up to `limit` blocks, in an order shuffled by a random
/// forked from the world seed at the piece's position, go through `delegate`; only a change
/// counts towards the limit.
fn finalize_capped(
    cx: &mut FeatureContext<'_, '_, '_>,
    position: BlockPos,
    delegate: &Processor,
    limit: i32,
    processed: &mut [BlockInfo],
) {
    if limit == 0 || processed.is_empty() {
        return;
    }
    // `RandomSource.createThreadLocalInstance(seed).forkPositional().at(position)`.
    let factory = LegacyPositionalRandomFactory::new(LegacyRandom::new(cx.env.seed).next_long());
    let mut random = factory.at(position.x, position.y, position.z);
    let max_to_replace = limit.min(processed.len() as i32);
    if max_to_replace < 1 {
        return;
    }
    // `Util.toShuffledList(IntStream.range(0, size), random)`.
    let mut indices: Vec<usize> = (0..processed.len()).collect();
    for i in (2..=indices.len()).rev() {
        let swap_to = random.next_int_bounded(i as i32) as usize;
        indices.swap(i - 1, swap_to);
    }
    let env = cx.env;
    let mut replaced = 0;
    for index in indices {
        if replaced >= max_to_replace {
            break;
        }
        let current = processed[index].clone();
        let world = block_in(cx.region, current.pos);
        let Some(altered) = delegate.process(current, world, &env.block_tags, &env.facts, None)
        else {
            continue;
        };
        if altered != processed[index] {
            replaced += 1;
            processed[index] = altered;
        }
    }
}
