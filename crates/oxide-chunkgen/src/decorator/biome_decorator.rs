//! [`BiomeDecorator`]: vanilla's `ChunkGenerator.applyBiomeDecoration`, for one chunk.
//!
//! For the chunk at the centre of a [`DecorationRegion`] it:
//!
//! 1. derives the chunk's decoration seed from the world seed and the chunk's minimum block X/Z;
//! 2. collects the biomes present in the 3x3 chunks around it, keeping those the generator's
//!    biome source can produce;
//! 3. for each decoration step, takes the global indices of the features those biomes list at
//!    that step, sorted ascending and without duplicates;
//! 4. for each, reseeds with `setFeatureSeed(decorationSeed, index, step)` and places the
//!    feature with a biome check.
//!
//! The index is a feature's position in the per-step ordering `FeatureSorter` builds across every
//! possible biome, so which biomes the decorator is built with decides every feature's seed. Pass
//! the biome source's possible biomes in the order the source lists them.
//!
//! Vanilla runs structure placement at the start of each step; this does not. Structures draw
//! from the stream between features, but every feature reseeds, so skipping them changes nothing
//! a feature sees.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use oxide_core::{BiomeId, BlockPos, Xoroshiro128PlusPlus};
use oxide_datapack::Datapack;

use super::context::{BlockBehaviours, Environment, FeatureContext, UnsupportedTally};
use super::survival::SurvivalRules;
use super::feature::{place_placed, Compiler, Placed};
use super::ground::BiomeClimate;
use crate::decorate::Decorator;
use crate::decoration_seed::{set_decoration_seed, set_feature_seed};
use crate::feature_order::build_features_per_step;
use crate::region::DecorationRegion;

/// Decorates chunks with the features their biomes list. Owns everything it reads, so it can
/// outlive the `Datapack` it was built from and be shared between threads.
pub struct BiomeDecorator {
    env: Environment,
    /// Per step, the registered placed feature at each global index.
    steps: Vec<Vec<u32>>,
    /// Compiled placed features by content key.
    placed: Vec<Arc<Placed>>,
    /// Per possible biome, per step, the sorted global indices it lists.
    biome_steps: HashMap<BiomeId, Vec<Vec<u32>>>,
    unsupported: Mutex<UnsupportedTally>,
}

impl BiomeDecorator {
    /// Builds a decorator for one generator.
    ///
    /// `possible_biomes` are the biomes the generator's biome source can produce, in its order.
    /// `world_seed` and `sea_level` are the world's. The datapack must carry block facts: without
    /// them nothing about a block can be answered, so construction fails rather than decorate
    /// blindly.
    pub fn new(
        datapack: &Datapack,
        possible_biomes: &[BiomeId],
        world_seed: i64,
        sea_level: i32,
    ) -> Result<Self> {
        if datapack.block_facts.is_empty() {
            bail!("the datapack has no block facts (block_facts.tsv); decoration cannot run");
        }
        let mut compiler = Compiler::new(
            &datapack.block_facts,
            &datapack.placed_features,
            &datapack.configured_features,
        )
        .with_templates(datapack);

        // Placed features are the same feature when their content is, whatever registry ids they
        // sit under (vanilla's record equality), so each distinct content gets one key.
        let mut key_of: HashMap<String, u32> = HashMap::new();
        let mut placed: Vec<Arc<Placed>> = Vec::new();
        let mut lists: HashMap<BiomeId, Vec<Vec<u32>>> = HashMap::new();
        for (biome_id, biome) in datapack.biomes.iter() {
            let mut per_step = Vec::with_capacity(biome.features.len());
            for step in &biome.features {
                let mut keys = Vec::with_capacity(step.len());
                for feature_id in step {
                    let ir = datapack.placed_features.get(feature_id).ok_or_else(|| {
                        anyhow!("biome {biome_id} lists unknown placed feature {feature_id}")
                    })?;
                    let content = serde_json::to_string(ir)
                        .with_context(|| format!("placed feature {feature_id}"))?;
                    let key = match key_of.get(&content) {
                        Some(&key) => key,
                        None => {
                            let key = placed.len() as u32;
                            placed.push(compiler.placed_by_id(feature_id)?);
                            key_of.insert(content, key);
                            key
                        }
                    };
                    keys.push(key);
                }
                per_step.push(keys);
            }
            lists.insert(biome_id.clone(), per_step);
        }

        let mut possible: Vec<&BiomeId> = Vec::with_capacity(possible_biomes.len());
        for biome in possible_biomes {
            if !possible.contains(&biome) {
                possible.push(biome);
            }
        }
        let ordering_input = possible
            .iter()
            .map(|biome| {
                lists
                    .get(*biome)
                    .cloned()
                    .ok_or_else(|| anyhow!("possible biome {biome} is not in the datapack"))
            })
            .collect::<Result<Vec<_>>>()?;
        let steps = build_features_per_step(&ordering_input).map_err(|cycle| anyhow!("{cycle}"))?;

        let index_in_step: Vec<HashMap<u32, u32>> = steps
            .iter()
            .map(|features| {
                features.iter().enumerate().map(|(index, &key)| (key, index as u32)).collect()
            })
            .collect();
        let mut biome_steps = HashMap::new();
        for (biome, input) in possible.iter().zip(&ordering_input) {
            let sorted = input
                .iter()
                .enumerate()
                .map(|(step, keys)| {
                    let mut indices: Vec<u32> = keys
                        .iter()
                        .filter_map(|key| index_in_step.get(step).and_then(|map| map.get(key)).copied())
                        .collect();
                    indices.sort_unstable();
                    indices.dedup();
                    indices
                })
                .collect();
            biome_steps.insert((*biome).clone(), sorted);
        }

        let biome_features = lists
            .into_iter()
            .map(|(biome, per_step)| (biome, per_step.into_iter().flatten().collect::<HashSet<u32>>()))
            .collect();

        let env = Environment {
            block_tags: datapack.block_tags.clone(),
            fluid_tags: datapack.fluid_tags.clone(),
            biome_tags: datapack.biome_tags.clone(),
            facts: datapack.block_facts.clone(),
            seed: world_seed,
            sea_level,
            behaviours: Box::new(SurvivalRules::new(&datapack.block_tags)),
            placed: compiler.into_placed(),
            biome_climate: datapack
                .biomes
                .iter()
                .map(|(id, biome)| Ok((id.clone(), BiomeClimate::from_biome(biome).with_context(|| format!("biome {id}"))?)))
                .collect::<Result<_>>()?,
            biome_features,
        };
        Ok(Self { env, steps, placed, biome_steps, unsupported: Mutex::new(UnsupportedTally::default()) })
    }

    /// Replaces the answers to per-block questions (see [`BlockBehaviours`]).
    pub fn with_behaviours(mut self, behaviours: Box<dyn BlockBehaviours>) -> Self {
        self.env.behaviours = behaviours;
        self
    }

    /// The block facts this decorator reads, for the caller to hand to `decorated_chunk`.
    pub fn facts(&self) -> &oxide_datapack::BlockFacts {
        &self.env.facts
    }

    /// Everything decoration was asked to do and did not, summed over every chunk decorated so
    /// far. Keys are `category:detail`; see [`UnsupportedTally`].
    pub fn unsupported(&self) -> UnsupportedTally {
        self.unsupported.lock().expect("unsupported tally poisoned").clone()
    }
}

impl Decorator for BiomeDecorator {
    fn decorate(&self, region: &mut DecorationRegion<'_>) {
        let center = region.center();
        let (min_x, min_z) = (center.min_block_x(), center.min_block_z());
        // The position every chain starts from: the chunk's corner at the bottom of the world.
        let origin = BlockPos::new(min_x, region.min_y(), min_z);

        let mut random = Xoroshiro128PlusPlus::new(0);
        let decoration_seed = set_decoration_seed(&mut random, self.env.seed, min_x, min_z);

        let present: Vec<BiomeId> = region
            .biomes_present()
            .into_iter()
            .filter(|biome| self.biome_steps.contains_key(*biome))
            .cloned()
            .collect();

        let mut tally = UnsupportedTally::default();
        for (step, features) in self.steps.iter().enumerate() {
            let mut indices: Vec<u32> = present
                .iter()
                .filter_map(|biome| self.biome_steps.get(biome).and_then(|steps| steps.get(step)))
                .flatten()
                .copied()
                .collect();
            indices.sort_unstable();
            indices.dedup();

            for index in indices {
                let key = features[index as usize];
                set_feature_seed(&mut random, decoration_seed, index as i32, step as i32);
                let mut cx = FeatureContext {
                    env: &self.env,
                    region: &mut *region,
                    random: &mut random,
                    origin,
                    unsupported: &mut tally,
                    top_feature: Some(key),
                };
                place_placed(&self.placed[key as usize], &mut cx, origin, Some(key));
            }
        }
        if !tally.is_empty() {
            self.unsupported.lock().expect("unsupported tally poisoned").merge(&tally);
        }
    }
}
