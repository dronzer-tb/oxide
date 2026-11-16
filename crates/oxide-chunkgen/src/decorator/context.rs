//! What a feature can see and touch while it places.
//!
//! [`FeatureContext`] is the one argument every feature family is written against: the region it
//! reads and writes, the random stream the placement chain already advanced, the position it was
//! asked to place at, and read access to the world-level data (tags, block facts, seed, sea level,
//! the compiled placed-feature registry) through [`Environment`].
//!
//! Two things vanilla answers from per-block Java code cannot be derived from the exported data:
//! whether a block state can survive where it is placed (`BlockState.canSurvive`), and which
//! blocks are two-block plants. [`BlockBehaviours`] is the seam for the first. Until something
//! implements it, every question about it is answered "unknown" and the caller records the gap in
//! the [`UnsupportedTally`] and treats the placement as failed, rather than guessing.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use oxide_core::{BiomeId, BlockPos, BlockState, ResourceLocation, WorldgenRandom};
use oxide_datapack::{BlockFact, BlockFacts, TagSet};

use super::feature::Placed;
use super::ground::BiomeClimate;
use super::provider::WorldBounds;
use crate::region::DecorationRegion;

/// Answers the per-block questions the exported facts cannot.
pub trait BlockBehaviours: Send + Sync {
    /// `BlockState.canSurvive(level, pos)`: whether `state` could stay at `pos`, which for a
    /// sapling means "has dirt under it". `None` means this implementation does not know that
    /// block's rule.
    fn can_survive(
        &self,
        state: &BlockState,
        region: &DecorationRegion<'_>,
        pos: BlockPos,
    ) -> Option<bool>;
}

/// The default [`BlockBehaviours`]: it knows nothing, so every survival question is unanswered.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoBlockBehaviours;

impl BlockBehaviours for NoBlockBehaviours {
    fn can_survive(&self, _: &BlockState, _: &DecorationRegion<'_>, _: BlockPos) -> Option<bool> {
        None
    }
}

/// Counts of things decoration was asked to do and did not, keyed by what they were.
///
/// Keys are `category:detail`, for example `feature:minecraft:tree` or `survival:minecraft:poppy`.
/// A count is a number of times the gap was hit, not a number of distinct features.
#[derive(Debug, Clone, Default)]
pub struct UnsupportedTally(BTreeMap<String, u64>);

impl UnsupportedTally {
    pub fn record(&mut self, category: &str, detail: &str) {
        *self.0.entry(format!("{category}:{detail}")).or_insert(0) += 1;
    }

    pub fn merge(&mut self, other: &UnsupportedTally) {
        for (key, count) in &other.0 {
            *self.0.entry(key.clone()).or_insert(0) += count;
        }
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, u64)> {
        self.0.iter().map(|(key, count)| (key.as_str(), *count))
    }

    pub fn get(&self, key: &str) -> u64 {
        self.0.get(key).copied().unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// World-level data a feature may read. Owned, so a decorator holding it can outlive the
/// `Datapack` it was built from and be shared between threads.
pub struct Environment {
    pub block_tags: TagSet,
    pub fluid_tags: TagSet,
    pub biome_tags: TagSet,
    pub facts: BlockFacts,
    pub seed: i64,
    pub sea_level: i32,
    pub behaviours: Box<dyn BlockBehaviours>,
    /// Every placed feature a biome lists, compiled, by registry id. Feature families that
    /// reference another placed feature by id (rather than inline) look it up here.
    pub placed: HashMap<ResourceLocation, Arc<Placed>>,
    /// Each biome's temperature and precipitation settings, for snow and ice.
    pub biome_climate: HashMap<BiomeId, BiomeClimate>,
    /// The placed features each biome lists, by the content key that identifies them: what the
    /// `minecraft:biome` placement modifier checks against.
    pub(super) biome_features: BiomeFeatures,
}

impl Environment {
    /// Whether `biome` lists the placed feature with content key `key` at any step.
    pub(super) fn biome_lists(&self, biome: &BiomeId, key: u32) -> bool {
        self.biome_features.lists(biome, key)
    }
}

/// Per biome, the content keys of the placed features it lists, as a bit set: the `biome`
/// placement modifier and the biome gate ask this for every position and every feature, so it
/// is one fast hash of the biome and one bit test rather than two SipHash lookups.
#[derive(Default)]
pub(crate) struct BiomeFeatures(ahash::AHashMap<BiomeId, Box<[u64]>>);

impl BiomeFeatures {
    pub(crate) fn new(lists: impl IntoIterator<Item = (BiomeId, HashSet<u32>)>) -> Self {
        Self(
            lists
                .into_iter()
                .map(|(biome, keys)| {
                    let words = keys.iter().max().map_or(0, |&max| max as usize / 64 + 1);
                    let mut bits = vec![0u64; words].into_boxed_slice();
                    for key in keys {
                        bits[key as usize / 64] |= 1 << (key % 64);
                    }
                    (biome, bits)
                })
                .collect(),
        )
    }

    fn lists(&self, biome: &BiomeId, key: u32) -> bool {
        self.0.get(biome).is_some_and(|bits| {
            bits.get(key as usize / 64)
                .is_some_and(|word| word >> (key % 64) & 1 != 0)
        })
    }
}

/// Everything a placing feature works with. Features read and write only through this.
///
/// `origin` is the position the feature was asked to place at: the output of the last placement
/// modifier. The random stream is the one the placement chain used, so draws a feature makes
/// continue the same sequence vanilla's do; do not reseed it.
pub struct FeatureContext<'e, 'a, 'r> {
    pub env: &'e Environment,
    pub region: &'a mut DecorationRegion<'r>,
    pub random: &'a mut WorldgenRandom,
    pub origin: BlockPos,
    pub unsupported: &'a mut UnsupportedTally,
    /// The registered placed feature whose chain is running, for the `biome` modifier. `None`
    /// inside a feature nested in another, which vanilla does not biome-check.
    pub(super) top_feature: Option<u32>,
}

pub(crate) static VOID_AIR: LazyLock<BlockState> =
    LazyLock::new(|| BlockState::new(ResourceLocation::minecraft("void_air")));

/// The block at `pos`, with `void_air` outside the window: what vanilla's level answers above
/// and below the world.
pub fn block_in<'a>(region: &'a DecorationRegion<'_>, pos: BlockPos) -> &'a BlockState {
    region.block(pos).unwrap_or(&VOID_AIR)
}

/// Whether `fact` holds for [`block_in`]`(region, pos)`, through the region's per-state fact
/// cache; outside the window it asks about `void_air`, as `block_in` would answer.
pub fn fact_in(region: &DecorationRegion<'_>, pos: BlockPos, fact: BlockFact) -> bool {
    region
        .fact_at(pos, fact)
        .unwrap_or_else(|| region.facts().is(&VOID_AIR, fact))
}

impl FeatureContext<'_, '_, '_> {
    /// `WorldGenLevel.ensureCanWrite`: whether `pos` is in a chunk the region may write.
    pub fn ensure_can_write(&self, pos: BlockPos) -> bool {
        let center = self.region.center();
        ((pos.x >> 4) - center.x).abs() <= 1 && ((pos.z >> 4) - center.z).abs() <= 1
    }

    pub fn bounds(&self) -> WorldBounds {
        WorldBounds {
            min_y: self.region.min_y(),
            height: self.region.max_y() - self.region.min_y(),
        }
    }

    /// `isOutsideBuildHeight`.
    pub fn outside_build_height(&self, y: i32) -> bool {
        y < self.region.min_y() || y >= self.region.max_y()
    }
}
