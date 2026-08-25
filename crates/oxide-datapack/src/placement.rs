//! `worldgen/placed_feature`: what a feature is attached to, and where it is allowed to land.
//!
//! Deserialization only, like the rest of this crate. A placed feature is a configured feature
//! plus an ordered chain of placement modifiers; evaluating that chain lives in
//! `oxide-chunkgen`, not here.
//!
//! Every variant below appears in the real 26.2 export -- the shapes are taken from it and from
//! the decompiled `net.minecraft.world.level.levelgen.placement` classes, not recalled. A type
//! Mojang defines but no vanilla placed feature uses is still modelled where the enum would
//! otherwise silently reject a datapack that does use it.

use serde::{Deserialize, Serialize};

use crate::carver::HeightProvider;
use crate::feature::{ConfiguredFeature, Holder};

/// One entry of `worldgen/placed_feature`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacedFeature {
    /// The `worldgen/configured_feature` this places, by id or written inline.
    ///
    /// Both really occur. Every file in `worldgen/placed_feature` uses an id, which is what made
    /// the inline form easy to miss -- but a `sequence` or `simple_random_selector` inside a
    /// configured feature embeds whole placed features, and those name their feature inline.
    pub feature: Holder<ConfiguredFeature>,
    #[serde(default)]
    pub placement: Vec<PlacementModifier>,
}

/// The heightmap a placement modifier measures against.
///
/// `_WG` variants are the worldgen-time pair, built during the noise stage before features run;
/// the others include what earlier features have already placed. Which one a modifier names
/// changes where it lands, so they are distinct rather than folded together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeightmapKind {
    #[serde(rename = "WORLD_SURFACE_WG")]
    WorldSurfaceWg,
    #[serde(rename = "WORLD_SURFACE")]
    WorldSurface,
    #[serde(rename = "OCEAN_FLOOR_WG")]
    OceanFloorWg,
    #[serde(rename = "OCEAN_FLOOR")]
    OceanFloor,
    #[serde(rename = "MOTION_BLOCKING")]
    MotionBlocking,
    #[serde(rename = "MOTION_BLOCKING_NO_LEAVES")]
    MotionBlockingNoLeaves,
}

/// Which way `environment_scan` walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PlacementModifier {
    /// Keeps a position only if the biome there still lists this placed feature. The check that
    /// stops a forest's trees spilling across the border into the desert it was offset into.
    #[serde(rename = "minecraft:biome")]
    Biome,

    /// Repeats the position `count` times. Everything downstream runs once per copy.
    #[serde(rename = "minecraft:count")]
    Count { count: IntProvider },

    /// Scatters within the 16x16 chunk footprint.
    #[serde(rename = "minecraft:in_square")]
    InSquare,

    #[serde(rename = "minecraft:height_range")]
    HeightRange { height: HeightProvider },

    #[serde(rename = "minecraft:heightmap")]
    Heightmap { heightmap: HeightmapKind },

    #[serde(rename = "minecraft:random_offset")]
    RandomOffset {
        xz_spread: IntProvider,
        y_spread: IntProvider,
    },

    /// Keeps one position in `chance`. `chance` of 64 means a 1-in-64 chunk gets it at all.
    #[serde(rename = "minecraft:rarity_filter")]
    RarityFilter { chance: i32 },

    #[serde(rename = "minecraft:block_predicate_filter")]
    BlockPredicateFilter { predicate: BlockPredicate },

    #[serde(rename = "minecraft:surface_water_depth_filter")]
    SurfaceWaterDepthFilter { max_water_depth: i32 },

    #[serde(rename = "minecraft:surface_relative_threshold_filter")]
    SurfaceRelativeThresholdFilter {
        heightmap: HeightmapKind,
        #[serde(default = "i32_min")]
        min_inclusive: i32,
        #[serde(default = "i32_max")]
        max_inclusive: i32,
    },

    /// Walks `direction_of_search` while `allowed_search_condition` holds, up to `max_steps`,
    /// and keeps the position only if it ends on `target_condition`.
    #[serde(rename = "minecraft:environment_scan")]
    EnvironmentScan {
        direction_of_search: SearchDirection,
        target_condition: BlockPredicate,
        #[serde(default = "always_true")]
        allowed_search_condition: BlockPredicate,
        max_steps: i32,
    },

    /// One count per solid-to-air layer in the column, not one per chunk -- what puts nether
    /// vegetation on every ledge rather than only the top one.
    #[serde(rename = "minecraft:count_on_every_layer")]
    CountOnEveryLayer { count: IntProvider },

    #[serde(rename = "minecraft:noise_threshold_count")]
    NoiseThresholdCount {
        noise_level: f64,
        below_noise: i32,
        above_noise: i32,
    },

    #[serde(rename = "minecraft:noise_based_count")]
    NoiseBasedCount {
        noise_to_count_ratio: i32,
        noise_factor: f64,
        #[serde(default)]
        noise_offset: f64,
    },

    /// Absolute world positions, ignoring the chunk being decorated except to filter to it.
    #[serde(rename = "minecraft:fixed_placement")]
    FixedPlacement { positions: Vec<[i32; 3]> },
}

/// `net.minecraft.world.level.levelgen.blockpredicates.BlockPredicate`.
///
/// Modelled here rather than in the feature layer because placement filters are the only thing
/// in this crate that reference it -- features carry their own copies inside their configs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BlockPredicate {
    #[serde(rename = "minecraft:matching_blocks")]
    MatchingBlocks {
        blocks: BlockList,
        #[serde(default)]
        offset: Option<[i32; 3]>,
    },

    /// A block tag (`#minecraft:...`), which this crate does not resolve -- see the note on
    /// `CarverConfig::replaceable`. Held so the datapack round-trips and the evaluator can
    /// decide what to do with an unresolved tag.
    #[serde(rename = "minecraft:matching_block_tag")]
    MatchingBlockTag {
        tag: String,
        #[serde(default)]
        offset: Option<[i32; 3]>,
    },

    #[serde(rename = "minecraft:matching_fluids")]
    MatchingFluids {
        fluids: BlockList,
        #[serde(default)]
        offset: Option<[i32; 3]>,
    },

    /// Whether the block state could stay where it is put -- a sapling needs dirt under it.
    #[serde(rename = "minecraft:would_survive")]
    WouldSurvive {
        state: BlockStateSpec,
        #[serde(default)]
        offset: Option<[i32; 3]>,
    },

    #[serde(rename = "minecraft:replaceable")]
    Replaceable {
        #[serde(default)]
        offset: Option<[i32; 3]>,
    },

    #[serde(rename = "minecraft:solid")]
    Solid {
        #[serde(default)]
        offset: Option<[i32; 3]>,
    },

    #[serde(rename = "minecraft:inside_world_bounds")]
    InsideWorldBounds {
        #[serde(default)]
        offset: Option<[i32; 3]>,
    },

    #[serde(rename = "minecraft:has_sturdy_face")]
    HasSturdyFace {
        #[serde(default)]
        offset: Option<[i32; 3]>,
        direction: String,
    },

    #[serde(rename = "minecraft:all_of")]
    AllOf { predicates: Vec<BlockPredicate> },

    #[serde(rename = "minecraft:any_of")]
    AnyOf { predicates: Vec<BlockPredicate> },

    #[serde(rename = "minecraft:not")]
    Not { predicate: Box<BlockPredicate> },

    #[serde(rename = "minecraft:true")]
    AlwaysTrue,
}

/// A single block id or a list of them -- the export writes both shapes for the same field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BlockList {
    One(String),
    Many(Vec<String>),
}

/// A block state as the worldgen registries write it: `Name` plus optional string `Properties`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockStateSpec {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Properties", default)]
    pub properties: std::collections::BTreeMap<String, String>,
}

/// `net.minecraft.util.valueproviders.IntProvider`. A bare integer is the constant shorthand.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IntProvider {
    Constant(i32),
    Tagged(Box<TaggedIntProvider>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TaggedIntProvider {
    #[serde(rename = "minecraft:constant")]
    Constant { value: i32 },
    #[serde(rename = "minecraft:uniform")]
    Uniform {
        min_inclusive: i32,
        max_inclusive: i32,
    },
    #[serde(rename = "minecraft:biased_to_bottom")]
    BiasedToBottom {
        min_inclusive: i32,
        max_inclusive: i32,
    },
    #[serde(rename = "minecraft:clamped")]
    Clamped {
        source: IntProvider,
        min_inclusive: i32,
        max_inclusive: i32,
    },
    /// Deliberately float-valued: vanilla rounds a normal sample, it does not sample integers.
    #[serde(rename = "minecraft:clamped_normal")]
    ClampedNormal {
        mean: f32,
        deviation: f32,
        min_inclusive: i32,
        max_inclusive: i32,
    },
    /// `min`/`max`, not the `min_inclusive`/`max_inclusive` every other int provider uses --
    /// `TrapezoidInt`'s codec really does differ from `UniformInt`'s, and the height-provider
    /// trapezoid differs again. Verified against the decompiled codecs, not assumed.
    #[serde(rename = "minecraft:trapezoid")]
    Trapezoid { min: i32, max: i32, plateau: i32 },
    #[serde(rename = "minecraft:weighted_list")]
    WeightedList { distribution: Vec<WeightedInt> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightedInt {
    pub data: IntProvider,
    pub weight: i32,
}

fn always_true() -> BlockPredicate {
    BlockPredicate::AlwaysTrue
}

fn i32_min() -> i32 {
    i32::MIN
}

fn i32_max() -> i32 {
    i32::MAX
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Parses every `placed_feature` in the extracted vanilla export.
    ///
    /// The point is coverage, not a spot check: a modifier or predicate shape this enum does not
    /// model is a feature that silently never places, and the only way to know the set is closed
    /// is to run it over all of them. Skipped when the export is absent -- it is gitignored and
    /// must not be redistributed (see docs/REFERENCE_DATA.md).
    #[test]
    fn parses_every_vanilla_placed_feature() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../reference/data/minecraft/worldgen/placed_feature");
        if !dir.is_dir() {
            eprintln!("skipping: no extracted vanilla export at {}", dir.display());
            return;
        }

        let mut parsed = 0usize;
        let mut failures = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("read placed_feature dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read placed feature");
            match serde_json::from_str::<PlacedFeature>(&text) {
                Ok(_) => parsed += 1,
                Err(e) => failures.push(format!(
                    "{}: {e}",
                    path.file_name().unwrap().to_string_lossy()
                )),
            }
        }

        assert!(
            failures.is_empty(),
            "{} of {} placed features failed to parse:\n{}",
            failures.len(),
            parsed + failures.len(),
            failures.join("\n")
        );
        assert!(
            parsed > 0,
            "found no placed features to parse in {}",
            dir.display()
        );
        eprintln!("parsed {parsed} placed features");
    }
}
