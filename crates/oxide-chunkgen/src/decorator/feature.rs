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
use oxide_core::{BlockPos, RandomSource, ResourceLocation};
use oxide_datapack::feature::{ConfiguredFeature, HolderList};
use oxide_datapack::feature::Holder;
use oxide_datapack::placement::PlacedFeature as PlacedFeatureIr;
use oxide_datapack::{BlockFacts, Registry};

use super::context::FeatureContext;
use super::ore::OreFeature;
use super::placement::{run, Modifier};
use super::spring::SpringFeature;
use super::state_provider::{StateProvider, Unsupported};

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
    RandomSelector { features: Vec<(f32, Arc<Placed>)>, default: Arc<Placed> },
    SimpleRandomSelector(Vec<Arc<Placed>>),
    RandomBooleanSelector { when_true: Arc<Placed>, when_false: Arc<Placed> },
    /// Places every entry in order, stopping at the first that fails.
    Sequence(Vec<Arc<Placed>>),
    NoOp,
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
            depth: 0,
        }
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
        let ir = self.placed.get(id).ok_or_else(|| anyhow!("unknown placed feature {id}"))?;
        let compiled = Arc::new(self.compile_placed(ir).with_context(|| format!("placed feature {id}"))?);
        self.placed_memo.insert(id.clone(), compiled.clone());
        Ok(compiled)
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
                            self.feature(config).with_context(|| format!("configured feature {id}"))?,
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
            ConfiguredFeature::Ore(config) => Feature::Ore(OreFeature::compile(config, self.facts)?),
            ConfiguredFeature::ScatteredOre(config) => {
                Feature::ScatteredOre(OreFeature::compile(config, self.facts)?)
            }
            ConfiguredFeature::SimpleBlock(config) => {
                Feature::SimpleBlock(StateProvider::compile(&config.to_place, self.facts)?)
            }
            ConfiguredFeature::SpringFeature(config) => match SpringFeature::compile(config, self.facts)? {
                Some(spring) => Feature::Spring(spring),
                None => Feature::Unsupported("minecraft:spring_feature (non-source fluid)".into()),
            },
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
                    list.iter().map(|holder| self.placed_holder(holder)).collect::<Result<_>>()?,
                ),
                // A tag of placed features: the datapack has no placed-feature tags loaded.
                HolderList::Tag(_) => Feature::Unsupported("minecraft:simple_random_selector (tag)".into()),
            },
            ConfiguredFeature::RandomBooleanSelector(config) => Feature::RandomBooleanSelector {
                when_true: self.placed_holder(&config.feature_true)?,
                when_false: self.placed_holder(&config.feature_false)?,
            },
            ConfiguredFeature::Sequence(config) => Feature::Sequence(
                config.features.iter().map(|holder| self.placed_holder(holder)).collect::<Result<_>>()?,
            ),
            ConfiguredFeature::NoOp(_) => Feature::NoOp,
            other => Feature::Unsupported(type_id(other)),
        })
    }
}

/// The `minecraft:` type id of a configured feature, read back from its serialised tag.
fn type_id(ir: &ConfiguredFeature) -> String {
    serde_json::to_value(ir)
        .ok()
        .and_then(|value| value.get("type").and_then(|tag| tag.as_str().map(str::to_owned)))
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
        Feature::RandomBooleanSelector { when_true, when_false } => {
            let chosen = if cx.random.next_boolean() { when_true } else { when_false };
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
        Feature::NoOp => true,
        Feature::Unsupported(type_id) => {
            cx.unsupported.record("feature", type_id);
            false
        }
    }
}

/// `SimpleBlockFeature.place`: the provider's state, if it can survive here, written at the
/// origin.
///
/// Two things stop it short of vanilla, and both are tallied rather than guessed. Survival is
/// per-block Java (`BlockBehaviours`); with no implementation every state is unknown. And a
/// two-block plant (a state with `half=lower`) needs both halves written with the waterlogging
/// copied from the level, which is not ported. Vanilla's scheduled tick is not recorded.
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
    if state.properties.get("half").is_some_and(|half| half == "lower") {
        cx.unsupported.record("feature", "minecraft:simple_block (two-block plant)");
        return false;
    }
    cx.region.set_block(pos, state)
}
