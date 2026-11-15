//! Placed features, configured features, and the dispatch that places them.
//!
//! [`Compiler`] turns the parsed `placed_feature` / `configured_feature` IR into [`Placed`] and
//! [`Feature`] values once, when a decorator is built: block states are completed against the
//! exported facts, ids are parsed, noise is seeded. Placing is then a walk over those values with
//! no string handling.
//!
//! # Adding a feature family
//!
//! 1. Give [`Feature`] a variant holding the compiled form of its config.
//! 2. Compile it in [`Compiler::feature`] -- the `Feature::Unsupported` arm at the bottom is where
//!    a type currently lands.
//! 3. Place it in [`place_configured_feature`], reading and writing only through the
//!    [`FeatureContext`] and drawing from `cx.random` in vanilla's order.
//!
//! Everything not listed in [`place_configured_feature`] is [`Feature::Unsupported`]: it places
//! nothing, returns `false`, and is counted in the [`UnsupportedTally`] under
//! `feature:<type id>`. Nothing is ever approximated.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use oxide_core::{BlockPos, BlockState, RandomSource, ResourceLocation};
use oxide_datapack::feature::Holder;
use oxide_datapack::feature::{ConfiguredFeature, HolderList};
use oxide_datapack::placement::PlacedFeature as PlacedFeatureIr;
use oxide_datapack::{BlockFact, BlockFacts, Datapack, Registry};

use super::cave::CaveFeature;
use super::context::FeatureContext;
use super::end::EndFeature;
use super::ground::GroundFeature;
use super::nether::NetherFeature;
use super::ore::OreFeature;
use super::placement::{run, Modifier};
use super::spring::SpringFeature;
use super::state_provider::{StateProvider, Unsupported};
use super::templated::{TemplateLibrary, TemplatedFeature};
use super::tree::{FallenTreeFeature, TreeFeature};
use super::vegetation::VegetationFeature;
use super::water::WaterFeature;

/// A placed feature: a modifier chain and the feature it leads to.
#[derive(Debug)]
pub struct Placed {
    pub modifiers: Vec<Modifier>,
    pub feature: Arc<Feature>,
}

/// A configured feature, compiled.
#[derive(Debug)]
pub enum Feature {
    Ore(OreFeature),
    ScatteredOre(OreFeature),
    SimpleBlock(StateProvider),
    Spring(SpringFeature),
    /// Rolls each entry's `chance` in order and places the first that passes, else `default`.
    RandomSelector {
        features: Vec<(f32, Arc<Placed>)>,
        default: Arc<Placed>,
    },
    SimpleRandomSelector(Vec<Arc<Placed>>),
    /// Places one entry picked by weight; a zero total weight places nothing.
    WeightedRandomSelector(Vec<(Arc<Placed>, i32)>),
    RandomBooleanSelector {
        when_true: Arc<Placed>,
        when_false: Arc<Placed>,
    },
    /// Places every entry in order, stopping at the first that fails.
    Sequence(Vec<Arc<Placed>>),
    /// Patches, root systems, multiface growth, vines, bamboo and huge mushrooms.
    Vegetation(VegetationFeature),
    /// Disks, piles, blobs, columns, lakes, snow and ice, spikes, icebergs, magma, monster rooms.
    Ground(GroundFeature),
    /// Dripstone and sulfur spikes, large dripstone, geodes and sculk patches.
    Cave(CaveFeature),
    /// Kelp, seagrass, sea pickles, coral reefs and the desert well.
    Water(WaterFeature),
    /// Fossils and template features: structure templates placed through processors.
    Templated(TemplatedFeature),
    /// Chorus plants, end islands and spikes, gateways, platforms and the bonus chest.
    End(EndFeature),
    /// Delta patches, netherrack blobs, huge fungi, basalt, glowstone, vines and nylium plants.
    Nether(NetherFeature),
    NoOp,
    Tree(TreeFeature),
    FallenTree(FallenTreeFeature),
    /// A feature type no family implements yet; the string is its `minecraft:` type id.
    Unsupported(String),
}

/// Compiles registry entries into [`Placed`] / [`Feature`] values, memoising by id.
pub struct Compiler<'d> {
    facts: &'d BlockFacts,
    placed: &'d Registry<PlacedFeatureIr>,
    configured: &'d Registry<ConfiguredFeature>,
    placed_memo: HashMap<ResourceLocation, Arc<Placed>>,
    configured_memo: HashMap<ResourceLocation, Arc<Feature>>,
    /// Where `fossil` and `template` read their templates; without it they are unsupported.
    templates: Option<TemplateLibrary<'d>>,
    depth: u32,
}

/// Placed features nest through selectors; real data is a few levels deep, so a deeper chain
/// means a reference cycle.
const MAX_DEPTH: u32 = 32;

impl<'d> Compiler<'d> {
    pub fn new(
        facts: &'d BlockFacts,
        placed: &'d Registry<PlacedFeatureIr>,
        configured: &'d Registry<ConfiguredFeature>,
    ) -> Self {
        Self {
            facts,
            placed,
            configured,
            placed_memo: HashMap::new(),
            configured_memo: HashMap::new(),
            templates: None,
            depth: 0,
        }
    }

    /// Lets `fossil` and `template` features read structure templates and processor lists
    /// from `datapack`'s tree.
    pub fn with_templates(mut self, datapack: &'d Datapack) -> Self {
        self.templates = Some(TemplateLibrary::new(datapack));
        self
    }

    /// Every placed feature compiled so far, by registry id.
    pub fn into_placed(self) -> HashMap<ResourceLocation, Arc<Placed>> {
        self.placed_memo
    }

    /// The compiled `placed_feature` registered as `id`.
    pub fn placed_by_id(&mut self, id: &ResourceLocation) -> Result<Arc<Placed>> {
        if let Some(found) = self.placed_memo.get(id) {
            return Ok(found.clone());
        }
        let ir = self
            .placed
            .get(id)
            .ok_or_else(|| anyhow!("unknown placed feature {id}"))?;
        let compiled = Arc::new(
            self.compile_placed(ir)
                .with_context(|| format!("placed feature {id}"))?,
        );
        self.placed_memo.insert(id.clone(), compiled.clone());
        Ok(compiled)
    }

    /// The compiled `configured_feature` registered as `id`, or `None` when the registry has no
    /// such entry. For features that place another registered feature (the pale-moss patch).
    pub fn configured_by_id(&mut self, id: &ResourceLocation) -> Result<Option<Arc<Feature>>> {
        if let Some(found) = self.configured_memo.get(id) {
            return Ok(Some(found.clone()));
        }
        let Some(config) = self.configured.get(id) else {
            return Ok(None);
        };
        let compiled = Arc::new(
            self.feature(config)
                .with_context(|| format!("configured feature {id}"))?,
        );
        self.configured_memo.insert(id.clone(), compiled.clone());
        Ok(Some(compiled))
    }

    fn placed_holder(&mut self, holder: &Holder<PlacedFeatureIr>) -> Result<Arc<Placed>> {
        match holder {
            Holder::Reference(id) => self.placed_by_id(id),
            Holder::Inline(ir) => Ok(Arc::new(self.compile_placed(ir)?)),
        }
    }

    pub fn compile_placed(&mut self, ir: &PlacedFeatureIr) -> Result<Placed> {
        if self.depth >= MAX_DEPTH {
            bail!("placed features nest deeper than {MAX_DEPTH}: reference cycle");
        }
        self.depth += 1;
        let result = (|| {
            let modifiers = ir
                .placement
                .iter()
                .map(|modifier| Modifier::compile(modifier, self.facts))
                .collect::<Result<Vec<_>>>()?;
            let feature = match &ir.feature {
                Holder::Reference(id) => match self.configured_memo.get(id) {
                    Some(found) => found.clone(),
                    None => {
                        let config = self
                            .configured
                            .get(id)
                            .ok_or_else(|| anyhow!("unknown configured feature {id}"))?;
                        let compiled = Arc::new(
                            self.feature(config)
                                .with_context(|| format!("configured feature {id}"))?,
                        );
                        self.configured_memo.insert(id.clone(), compiled.clone());
                        compiled
                    }
                },
                Holder::Inline(config) => Arc::new(self.feature(config)?),
            };
            Ok(Placed { modifiers, feature })
        })();
        self.depth -= 1;
        result
    }

    fn feature(&mut self, ir: &ConfiguredFeature) -> Result<Feature> {
        Ok(match ir {
            ConfiguredFeature::Ore(config) => {
                Feature::Ore(OreFeature::compile(config, self.facts)?)
            }
            ConfiguredFeature::ScatteredOre(config) => {
                Feature::ScatteredOre(OreFeature::compile(config, self.facts)?)
            }
            ConfiguredFeature::SimpleBlock(config) => {
                Feature::SimpleBlock(StateProvider::compile(&config.to_place, self.facts)?)
            }
            ConfiguredFeature::SpringFeature(config) => {
                match SpringFeature::compile(config, self.facts)? {
                    Some(spring) => Feature::Spring(spring),
                    None => {
                        Feature::Unsupported("minecraft:spring_feature (non-source fluid)".into())
                    }
                }
            }
            ConfiguredFeature::RandomSelector(config) => Feature::RandomSelector {
                features: config
                    .features
                    .iter()
                    .map(|entry| Ok((entry.chance, self.placed_holder(&entry.feature)?)))
                    .collect::<Result<_>>()?,
                default: self.placed_holder(&config.default)?,
            },
            ConfiguredFeature::SimpleRandomSelector(config) => match &config.features {
                HolderList::List(list) => Feature::SimpleRandomSelector(
                    list.iter()
                        .map(|holder| self.placed_holder(holder))
                        .collect::<Result<_>>()?,
                ),
                // A tag of placed features: the datapack has no placed-feature tags loaded.
                HolderList::Tag(_) => {
                    Feature::Unsupported("minecraft:simple_random_selector (tag)".into())
                }
            },
            ConfiguredFeature::RandomBooleanSelector(config) => Feature::RandomBooleanSelector {
                when_true: self.placed_holder(&config.feature_true)?,
                when_false: self.placed_holder(&config.feature_false)?,
            },
            ConfiguredFeature::Sequence(config) => Feature::Sequence(
                config
                    .features
                    .iter()
                    .map(|holder| self.placed_holder(holder))
                    .collect::<Result<_>>()?,
            ),
            ConfiguredFeature::NoOp(_) => Feature::NoOp,
            ConfiguredFeature::WeightedRandomSelector(config) => Feature::WeightedRandomSelector(
                config
                    .features
                    .iter()
                    .map(|entry| Ok((self.placed_holder(&entry.data)?, entry.weight)))
                    .collect::<Result<_>>()?,
            ),
            other @ (ConfiguredFeature::VegetationPatch(_)
            | ConfiguredFeature::WaterloggedVegetationPatch(_)
            | ConfiguredFeature::RootSystem(_)
            | ConfiguredFeature::MultifaceGrowth(_)
            | ConfiguredFeature::Vines(_)
            | ConfiguredFeature::Bamboo(_)
            | ConfiguredFeature::HugeRedMushroom(_)
            | ConfiguredFeature::HugeBrownMushroom(_)) => {
                match VegetationFeature::compile(other, self.facts, &mut |holder| {
                    self.placed_holder(holder)
                })? {
                    Some(feature) => Feature::Vegetation(feature),
                    None => Feature::Unsupported(type_id(other)),
                }
            }
            ConfiguredFeature::Tree(config) => {
                let facts = self.facts;
                Feature::Tree(TreeFeature::compile(config, facts, &mut |id| {
                    self.configured_by_id(id)
                })?)
            }
            ConfiguredFeature::FallenTree(config) => {
                let facts = self.facts;
                Feature::FallenTree(FallenTreeFeature::compile(config, facts, &mut |id| {
                    self.configured_by_id(id)
                })?)
            }
            other @ (ConfiguredFeature::DeltaFeature(_)
            | ConfiguredFeature::NetherrackReplaceBlobs(_)
            | ConfiguredFeature::HugeFungus(_)
            | ConfiguredFeature::BasaltColumns(_)
            | ConfiguredFeature::BasaltPillar(_)
            | ConfiguredFeature::GlowstoneBlob(_)
            | ConfiguredFeature::TwistingVines(_)
            | ConfiguredFeature::WeepingVines(_)
            | ConfiguredFeature::NetherForestVegetation(_)) => {
                match NetherFeature::compile(other, self.facts)? {
                    Some(feature) => Feature::Nether(feature),
                    None => Feature::Unsupported(type_id(other)),
                }
            }
            other @ (ConfiguredFeature::ChorusPlant(_)
            | ConfiguredFeature::EndIsland(_)
            | ConfiguredFeature::EndSpike(_)
            | ConfiguredFeature::EndGateway(_)
            | ConfiguredFeature::EndPlatform(_)
            | ConfiguredFeature::VoidStartPlatform(_)
            | ConfiguredFeature::BonusChest(_)) => match EndFeature::compile(other, self.facts)? {
                Some(feature) => Feature::End(feature),
                None => Feature::Unsupported(type_id(other)),
            },
            other @ (ConfiguredFeature::Speleothem(_)
            | ConfiguredFeature::SpeleothemCluster(_)
            | ConfiguredFeature::LargeDripstone(_)
            | ConfiguredFeature::Geode(_)
            | ConfiguredFeature::SculkPatch(_)) => match CaveFeature::compile(other, self.facts)? {
                Some(feature) => Feature::Cave(feature),
                None => Feature::Unsupported(type_id(other)),
            },
            other @ (ConfiguredFeature::Kelp(_)
            | ConfiguredFeature::Seagrass(_)
            | ConfiguredFeature::SeaPickle(_)
            | ConfiguredFeature::CoralTree(_)
            | ConfiguredFeature::CoralClaw(_)
            | ConfiguredFeature::CoralMushroom(_)
            | ConfiguredFeature::DesertWell(_)) => {
                match WaterFeature::compile(other, self.facts)? {
                    Some(feature) => Feature::Water(feature),
                    None => Feature::Unsupported(type_id(other)),
                }
            }
            other @ (ConfiguredFeature::Fossil(_) | ConfiguredFeature::Template(_)) => {
                match self.templates.as_mut() {
                    Some(library) => match TemplatedFeature::compile(other, self.facts, library)? {
                        Some(Ok(feature)) => Feature::Templated(feature),
                        Some(Err(detail)) => Feature::Unsupported(detail),
                        None => Feature::Unsupported(type_id(other)),
                    },
                    None => Feature::Unsupported(format!("{} (no datapack tree)", type_id(other))),
                }
            }
            other => match GroundFeature::compile(other, self.facts)? {
                Some(feature) => Feature::Ground(feature),
                None => Feature::Unsupported(type_id(other)),
            },
        })
    }
}

/// The `minecraft:` type id of a configured feature, read back from its serialised tag.
fn type_id(ir: &ConfiguredFeature) -> String {
    serde_json::to_value(ir)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(|tag| tag.as_str().map(str::to_owned))
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Places `placed` at `origin`: runs its modifier chain and the feature at every position that
/// comes out. `top_feature` is the registry key to biome-check against, or `None` for a feature
/// nested inside another. Returns whether any placement succeeded.
pub fn place_placed(
    placed: &Placed,
    cx: &mut FeatureContext<'_, '_, '_>,
    origin: BlockPos,
    top_feature: Option<u32>,
) -> bool {
    let saved = std::mem::replace(&mut cx.top_feature, top_feature);
    let result = run(&placed.modifiers, cx, origin, &placed.feature);
    cx.top_feature = saved;
    result
}

/// `ConfiguredFeature.place`: places `feature` at `cx.origin`.
///
/// Like vanilla's `Feature.place`, it first refuses an origin outside the chunks the region may
/// write, before any random draw.
pub fn place_configured_feature(feature: &Feature, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
    if !cx.ensure_can_write(cx.origin) {
        return false;
    }
    let origin = cx.origin;
    match feature {
        Feature::Ore(ore) => ore.place(cx),
        Feature::ScatteredOre(ore) => ore.place_scattered(cx),
        Feature::SimpleBlock(provider) => place_simple_block(provider, cx),
        Feature::Spring(spring) => spring.place(cx),
        Feature::RandomSelector { features, default } => {
            for (chance, placed) in features {
                if cx.random.next_float() < *chance {
                    return place_placed(placed, cx, origin, None);
                }
            }
            place_placed(default, cx, origin, None)
        }
        Feature::SimpleRandomSelector(features) => {
            if features.is_empty() {
                return false;
            }
            let index = cx.random.next_int_bounded(features.len() as i32) as usize;
            place_placed(&features[index], cx, origin, None)
        }
        Feature::WeightedRandomSelector(features) => {
            let total = features
                .iter()
                .fold(0i32, |sum, (_, weight)| sum.wrapping_add(*weight));
            if total <= 0 {
                return false;
            }
            // One bounded draw, then the first entry whose cumulative weight passes it.
            let mut remaining = cx.random.next_int_bounded(total);
            for (placed, weight) in features {
                remaining -= weight;
                if remaining < 0 {
                    return place_placed(placed, cx, origin, None);
                }
            }
            false
        }
        Feature::Ground(feature) => feature.place(cx),
        Feature::Cave(feature) => feature.place(cx),
        Feature::Water(feature) => feature.place(cx),
        Feature::Templated(feature) => feature.place(cx),
        Feature::End(feature) => feature.place(cx),
        Feature::Nether(feature) => feature.place(cx),
        Feature::RandomBooleanSelector {
            when_true,
            when_false,
        } => {
            let chosen = if cx.random.next_boolean() {
                when_true
            } else {
                when_false
            };
            place_placed(chosen, cx, origin, None)
        }
        Feature::Sequence(features) => {
            for placed in features {
                if !place_placed(placed, cx, origin, None) {
                    return false;
                }
            }
            true
        }
        Feature::Vegetation(feature) => feature.place(cx),
        Feature::Tree(tree) => tree.place(cx),
        Feature::FallenTree(tree) => tree.place(cx),
        Feature::NoOp => true,
        Feature::Unsupported(type_id) => {
            cx.unsupported.record("feature", type_id);
            false
        }
    }
}

/// `SimpleBlockFeature.place`: the provider's state, if it can survive here, written at the
/// origin. A two-block plant (a state with `half=lower`) needs the cell above it empty and has
/// both halves written, each taking its waterlogging from the water already there
/// (`DoublePlantBlock.placeAt`).
///
/// Survival is per-block Java (`BlockBehaviours`); a block whose rule is unknown is tallied and
/// not placed rather than guessed. Vanilla's scheduled tick is not recorded.
fn place_simple_block(provider: &StateProvider, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
    let pos = cx.origin;
    let state = match provider.state(cx, pos) {
        Ok(Some(state)) => state,
        Ok(None) => return false,
        Err(Unsupported(why)) => {
            cx.unsupported.record("provider", why);
            return false;
        }
    };
    match cx.env.behaviours.can_survive(&state, cx.region, pos) {
        Some(true) => {}
        Some(false) => return false,
        None => {
            cx.unsupported.record("survival", &state.name.to_string());
            return false;
        }
    }
    if state
        .properties
        .get("half")
        .is_some_and(|half| half == "lower")
    {
        return place_two_block_plant(cx, state, pos);
    }
    cx.region.set_block(pos, state)
}

/// `DoublePlantBlock.placeAt`, behind `SimpleBlockFeature`'s check that the cell above is empty.
fn place_two_block_plant(
    cx: &mut FeatureContext<'_, '_, '_>,
    state: BlockState,
    pos: BlockPos,
) -> bool {
    let above = BlockPos::new(pos.x, pos.y + 1, pos.z);
    if !crate::decorator::context::fact_in(cx.region, above, BlockFact::Air) {
        return false;
    }
    for (at, half) in [(pos, "lower"), (above, "upper")] {
        let mut half_state = state.clone().with_property("half", half);
        if half_state.properties.contains_key("waterlogged") {
            let in_water = crate::decorator::context::fact_in(cx.region, at, BlockFact::Water);
            half_state = half_state.with_property("waterlogged", in_water.to_string());
        }
        cx.region.set_block(at, half_state);
    }
    true
}
