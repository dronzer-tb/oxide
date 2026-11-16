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
    pub(super) biome_features: HashMap<BiomeId, HashSet<u32>>,
}

impl Environment {
    /// Whether `biome` lists the placed feature with content key `key` at any step.
    pub(super) fn biome_lists(&self, biome: &BiomeId, key: u32) -> bool {
        self.biome_features
            .get(biome)
            .is_some_and(|features| features.contains(&key))
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

static VOID_AIR: LazyLock<BlockState> =
    LazyLock::new(|| BlockState::new(ResourceLocation::minecraft("void_air")));

/// The block at `pos`, with `void_air` outside the window: what vanilla's level answers above
/// and below the world.
pub fn block_in<'a>(region: &'a DecorationRegion<'_>, pos: BlockPos) -> &'a BlockState {
    region.block(pos).unwrap_or(&VOID_AIR)
}

/// `facts.is(block_in(region, pos), fact)`, read from the region's precomputed fact bits.
pub fn fact_in(region: &DecorationRegion<'_>, pos: BlockPos, fact: BlockFact) -> bool {
    region.bits_or_void(pos) & fact as u32 != 0
}

/// Whether `fact` holds for the block at `pos`, reading a property-less state the facts do not
/// list as its block's default state.
///
/// Terrain writes fluids as bare `minecraft:lava` (the aquifer and the noise fill never set
/// `level`), which the facts table, keyed by full state, does not list; the game's state is the
/// default one, `level=0`.
pub(crate) fn fact_at(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos, fact: BlockFact) -> bool {
    let Some(state) = cx.region.block(pos) else {
        return false;
    };
    let facts = &cx.env.facts;
    if state.properties.is_empty() && !facts.knows(state) {
        if let Some(default) = facts.default_state(&state.name) {
            return facts.is(default, fact);
        }
    }
    facts.is(state, fact)
}

/// The type of `level.getFluidState(pos)`, `None` for an empty fluid: `minecraft:water` or
/// `minecraft:lava` for a source (a liquid block or a waterlogged one), `flowing_` for the rest.
pub(crate) fn fluid_id_at(
    cx: &FeatureContext<'_, '_, '_>,
    pos: BlockPos,
) -> Option<ResourceLocation> {
    let kind = if fact_at(cx, pos, BlockFact::Water) {
        "water"
    } else if fact_at(cx, pos, BlockFact::Lava) {
        "lava"
    } else {
        return None;
    };
    Some(ResourceLocation::minecraft(
        if fact_at(cx, pos, BlockFact::FluidSource) {
            kind.to_string()
        } else {
            format!("flowing_{kind}")
        },
    ))
}

/// Blocks whose four side faces are not full squares in either the support or the collision
/// shape, in every state. The facts hold the up and down faces only; these are the side faces
/// decoration has been seen to ask about, read off the 26.2 shapes:
///
/// - `moss_carpet` (`CarpetBlock`): `column(16, 0, 1)`;
/// - `azalea`, `flowering_azalea` (`AzaleaBlock`): `column(16, 8, 16) | column(4, 0, 8)`, no
///   collision override;
/// - `bamboo` (`BambooStalkBlock`): collision `column(3, 0, 16)`;
/// - `cocoa` (`CocoaBlock`): at most `column(8, 3, 12)`, moved off-centre;
/// - `big_dripleaf` (`BigDripleafBlock`): collision is the leaf, `column(16, 11, 15)` at most.
///
/// None overrides `getBlockSupportShape`, so the support shape is the collision shape.
pub(crate) fn sides_never_full(state: &BlockState) -> bool {
    state.name.namespace() == "minecraft"
        && matches!(
            state.name.path(),
            "moss_carpet" | "azalea" | "flowering_azalea" | "bamboo" | "cocoa" | "big_dripleaf"
        )
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
