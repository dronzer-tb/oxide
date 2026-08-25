//! `worldgen/configured_feature`: what a feature places, as opposed to where.
//!
//! Deserialization only, like the rest of this crate.
//!
//! The type tag is a closed set -- all 55 the vanilla export uses -- so a datapack naming a
//! feature this crate has never heard of fails loudly instead of being skipped at generation
//! time. Configs are typed per feature as the generator learns to place that feature; the rest
//! are held as raw [`serde_json::Value`], the same treatment `biome.rs` gives trees it does not
//! walk into. That keeps the registry complete from day one without pretending 55 feature
//! implementations exist.

use oxide_core::ResourceLocation;
use serde::{Deserialize, Serialize};

use crate::placement::{BlockList, BlockPredicate, BlockStateSpec, IntProvider, PlacedFeature};

/// A registry entry written either as a reference or inline.
///
/// Both forms occur: a top-level `placed_feature` names its configured feature by id, while a
/// `sequence` or `simple_random_selector` writes whole features inline, and those inline
/// features nest further. The vanilla export uses references 68 times, inline configured
/// features 30 times and inline placed features 5 times, so neither form is a special case.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Holder<T> {
    Reference(#[serde(with = "crate::ident_serde::rl")] ResourceLocation),
    Inline(Box<T>),
}

impl<T> Holder<T> {
    /// The id, when this is a reference. `None` for an inline definition, which has no id.
    pub fn reference(&self) -> Option<&ResourceLocation> {
        match self {
            Holder::Reference(id) => Some(id),
            Holder::Inline(_) => None,
        }
    }
}

/// One entry of `worldgen/configured_feature`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ConfiguredFeature {
    // ---- typed: features the generator can place ----
    /// Ore blobs. `scattered_ore` shares the configuration and differs only in how the blob is
    /// distributed, which lives in the generator.
    #[serde(rename = "minecraft:ore")]
    Ore(OreConfig),
    #[serde(rename = "minecraft:scattered_ore")]
    ScatteredOre(OreConfig),

    #[serde(rename = "minecraft:disk")]
    Disk(DiskConfig),

    #[serde(rename = "minecraft:spring_feature")]
    SpringFeature(SpringConfig),

    #[serde(rename = "minecraft:simple_block")]
    SimpleBlock(SimpleBlockConfig),

    #[serde(rename = "minecraft:random_selector")]
    RandomSelector(RandomSelectorConfig),

    #[serde(rename = "minecraft:random_boolean_selector")]
    RandomBooleanSelector(RandomBooleanSelectorConfig),

    #[serde(rename = "minecraft:simple_random_selector")]
    SimpleRandomSelector(SimpleRandomSelectorConfig),

    #[serde(rename = "minecraft:sequence")]
    Sequence(SequenceConfig),

    // ---- not yet placed by the generator: config held verbatim ----
    // The five coral/speleothem/template entries below never appear as a file in
    // worldgen/configured_feature -- they exist only inline, inside a sequence or a
    // simple_random_selector. Counting the files gives 55 feature types; the real closed set is
    // 60.
    #[serde(rename = "minecraft:coral_claw")]
    CoralClaw(serde_json::Value),
    #[serde(rename = "minecraft:coral_mushroom")]
    CoralMushroom(serde_json::Value),
    #[serde(rename = "minecraft:coral_tree")]
    CoralTree(serde_json::Value),
    #[serde(rename = "minecraft:speleothem")]
    Speleothem(serde_json::Value),
    #[serde(rename = "minecraft:template")]
    Template(serde_json::Value),
    #[serde(rename = "minecraft:bamboo")]
    Bamboo(serde_json::Value),
    #[serde(rename = "minecraft:basalt_columns")]
    BasaltColumns(serde_json::Value),
    #[serde(rename = "minecraft:basalt_pillar")]
    BasaltPillar(serde_json::Value),
    #[serde(rename = "minecraft:block_blob")]
    BlockBlob(serde_json::Value),
    #[serde(rename = "minecraft:block_column")]
    BlockColumn(serde_json::Value),
    #[serde(rename = "minecraft:block_pile")]
    BlockPile(serde_json::Value),
    #[serde(rename = "minecraft:blue_ice")]
    BlueIce(serde_json::Value),
    #[serde(rename = "minecraft:bonus_chest")]
    BonusChest(serde_json::Value),
    #[serde(rename = "minecraft:chorus_plant")]
    ChorusPlant(serde_json::Value),
    #[serde(rename = "minecraft:delta_feature")]
    DeltaFeature(serde_json::Value),
    #[serde(rename = "minecraft:desert_well")]
    DesertWell(serde_json::Value),
    #[serde(rename = "minecraft:end_gateway")]
    EndGateway(serde_json::Value),
    #[serde(rename = "minecraft:end_island")]
    EndIsland(serde_json::Value),
    #[serde(rename = "minecraft:end_platform")]
    EndPlatform(serde_json::Value),
    #[serde(rename = "minecraft:end_spike")]
    EndSpike(serde_json::Value),
    #[serde(rename = "minecraft:fallen_tree")]
    FallenTree(serde_json::Value),
    #[serde(rename = "minecraft:fossil")]
    Fossil(serde_json::Value),
    #[serde(rename = "minecraft:freeze_top_layer")]
    FreezeTopLayer(serde_json::Value),
    #[serde(rename = "minecraft:geode")]
    Geode(serde_json::Value),
    #[serde(rename = "minecraft:glowstone_blob")]
    GlowstoneBlob(serde_json::Value),
    #[serde(rename = "minecraft:huge_brown_mushroom")]
    HugeBrownMushroom(serde_json::Value),
    #[serde(rename = "minecraft:huge_fungus")]
    HugeFungus(serde_json::Value),
    #[serde(rename = "minecraft:huge_red_mushroom")]
    HugeRedMushroom(serde_json::Value),
    #[serde(rename = "minecraft:iceberg")]
    Iceberg(serde_json::Value),
    #[serde(rename = "minecraft:kelp")]
    Kelp(serde_json::Value),
    #[serde(rename = "minecraft:lake")]
    Lake(serde_json::Value),
    #[serde(rename = "minecraft:large_dripstone")]
    LargeDripstone(serde_json::Value),
    #[serde(rename = "minecraft:monster_room")]
    MonsterRoom(serde_json::Value),
    #[serde(rename = "minecraft:multiface_growth")]
    MultifaceGrowth(serde_json::Value),
    #[serde(rename = "minecraft:nether_forest_vegetation")]
    NetherForestVegetation(serde_json::Value),
    #[serde(rename = "minecraft:netherrack_replace_blobs")]
    NetherrackReplaceBlobs(serde_json::Value),
    #[serde(rename = "minecraft:root_system")]
    RootSystem(serde_json::Value),
    #[serde(rename = "minecraft:sculk_patch")]
    SculkPatch(serde_json::Value),
    #[serde(rename = "minecraft:sea_pickle")]
    SeaPickle(serde_json::Value),
    #[serde(rename = "minecraft:seagrass")]
    Seagrass(serde_json::Value),
    #[serde(rename = "minecraft:speleothem_cluster")]
    SpeleothemCluster(serde_json::Value),
    #[serde(rename = "minecraft:spike")]
    Spike(serde_json::Value),
    #[serde(rename = "minecraft:tree")]
    Tree(serde_json::Value),
    #[serde(rename = "minecraft:twisting_vines")]
    TwistingVines(serde_json::Value),
    #[serde(rename = "minecraft:underwater_magma")]
    UnderwaterMagma(serde_json::Value),
    #[serde(rename = "minecraft:vegetation_patch")]
    VegetationPatch(serde_json::Value),
    #[serde(rename = "minecraft:vines")]
    Vines(serde_json::Value),
    #[serde(rename = "minecraft:void_start_platform")]
    VoidStartPlatform(serde_json::Value),
    #[serde(rename = "minecraft:waterlogged_vegetation_patch")]
    WaterloggedVegetationPatch(serde_json::Value),
    #[serde(rename = "minecraft:weeping_vines")]
    WeepingVines(serde_json::Value),
    #[serde(rename = "minecraft:weighted_random_selector")]
    WeightedRandomSelector(serde_json::Value),
}

// ---------------------------------------------------------------------------
// Typed configurations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OreConfig {
    pub size: i32,
    /// Probability of dropping a block that ended up next to air, which is what keeps ores from
    /// studding the walls of caves. Required, not defaulted -- vanilla's codec has no `orElse`
    /// here, and defaulting it would accept a datapack the server rejects.
    pub discard_chance_on_air_exposure: f32,
    pub targets: Vec<OreTarget>,
}

/// One "replace this, with that" pair. An ore lists several so the same vein places deepslate
/// variants below the transition without needing a second feature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OreTarget {
    pub target: RuleTest,
    pub state: BlockStateSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskConfig {
    pub state_provider: BlockStateProvider,
    pub target: BlockPredicate,
    pub radius: IntProvider,
    pub half_height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpringConfig {
    pub state: FluidStateSpec,
    #[serde(default = "default_true")]
    pub requires_block_below: bool,
    #[serde(default = "default_rock_count")]
    pub rock_count: i32,
    #[serde(default = "default_hole_count")]
    pub hole_count: i32,
    pub valid_blocks: BlockList,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleBlockConfig {
    pub to_place: BlockStateProvider,
}

/// Picks the first entry whose roll succeeds, else `default`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RandomSelectorConfig {
    pub features: Vec<WeightedPlacedFeature>,
    pub default: Holder<PlacedFeature>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightedPlacedFeature {
    pub feature: Holder<PlacedFeature>,
    pub chance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RandomBooleanSelectorConfig {
    pub feature_true: Holder<PlacedFeature>,
    pub feature_false: Holder<PlacedFeature>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleRandomSelectorConfig {
    pub features: HolderList<PlacedFeature>,
}

/// Places every entry in order, unconditionally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequenceConfig {
    pub features: Vec<Holder<PlacedFeature>>,
}

/// A `HolderSet`: either a tag name or an explicit list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HolderList<T> {
    Tag(String),
    List(Vec<Holder<T>>),
}

// ---------------------------------------------------------------------------
// Shared sub-registries
// ---------------------------------------------------------------------------

/// `net.minecraft.world.level.levelgen.structure.templatesystem.RuleTest`.
///
/// Keyed on `predicate_type`, not `type` -- the one sub-registry in worldgen that does not use
/// the usual tag, which is why it does not show up in a search for feature types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "predicate_type")]
pub enum RuleTest {
    #[serde(rename = "minecraft:always_true")]
    AlwaysTrue,
    #[serde(rename = "minecraft:block_match")]
    BlockMatch { block: String },
    #[serde(rename = "minecraft:blockstate_match")]
    BlockStateMatch { block_state: BlockStateSpec },
    /// A block tag, which this crate does not resolve -- see `CarverConfig::replaceable`.
    #[serde(rename = "minecraft:tag_match")]
    TagMatch { tag: String },
    #[serde(rename = "minecraft:random_block_match")]
    RandomBlockMatch { block: String, probability: f32 },
    #[serde(rename = "minecraft:random_blockstate_match")]
    RandomBlockStateMatch {
        block_state: BlockStateSpec,
        probability: f32,
    },
}

/// `net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BlockStateProvider {
    #[serde(rename = "minecraft:simple_state_provider")]
    Simple { state: BlockStateSpec },

    #[serde(rename = "minecraft:rotated_block_provider")]
    RotatedBlock { state: BlockStateSpec },

    #[serde(rename = "minecraft:weighted_state_provider")]
    Weighted { entries: Vec<WeightedState> },

    /// Rolls a property of the state produced by `source` -- how a sapling gets a random stage.
    #[serde(rename = "minecraft:randomized_int_state_provider")]
    RandomizedInt {
        source: Box<BlockStateProvider>,
        property: String,
        values: IntProvider,
    },

    /// First rule whose `if_true` predicate holds wins, else `fallback`.
    #[serde(rename = "minecraft:rule_based_state_provider")]
    RuleBased {
        fallback: Box<BlockStateProvider>,
        rules: Vec<StateProviderRule>,
    },

    #[serde(rename = "minecraft:noise_provider")]
    Noise {
        seed: i64,
        noise: NoiseProviderParameters,
        scale: f32,
        states: Vec<BlockStateSpec>,
    },

    #[serde(rename = "minecraft:dual_noise_provider")]
    DualNoise {
        seed: i64,
        noise: NoiseProviderParameters,
        scale: f32,
        states: Vec<BlockStateSpec>,
        variety: [i32; 2],
        slow_noise: NoiseProviderParameters,
        slow_scale: f32,
    },

    #[serde(rename = "minecraft:noise_threshold_provider")]
    NoiseThreshold {
        seed: i64,
        noise: NoiseProviderParameters,
        scale: f32,
        threshold: f32,
        high_chance: f32,
        default_state: BlockStateSpec,
        low_states: Vec<BlockStateSpec>,
        high_states: Vec<BlockStateSpec>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightedState {
    pub data: BlockStateSpec,
    pub weight: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateProviderRule {
    pub if_true: BlockPredicate,
    pub then: Box<BlockStateProvider>,
}

/// The inline noise parameters a state provider carries, distinct from a `worldgen/noise` entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseProviderParameters {
    #[serde(rename = "firstOctave")]
    pub first_octave: i32,
    pub amplitudes: Vec<f64>,
}

/// A fluid state, which spring configs write with the same `Name`/`Properties` shape as blocks.
pub type FluidStateSpec = BlockStateSpec;

fn default_true() -> bool {
    true
}

fn default_rock_count() -> i32 {
    4
}

fn default_hole_count() -> i32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Parses every `configured_feature` in the extracted vanilla export.
    ///
    /// Same reasoning as the placed-feature test: the type tag is a closed set, so this is what
    /// proves no feature type is missing from the enum. It also exercises the nested path --
    /// inline placed features inside `sequence` and `simple_random_selector` -- which the
    /// top-level `placed_feature` files never reach. Skipped when the export is absent; it is
    /// gitignored and must not be redistributed (see docs/REFERENCE_DATA.md).
    #[test]
    fn parses_every_vanilla_configured_feature() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../reference/data/minecraft/worldgen/configured_feature");
        if !dir.is_dir() {
            eprintln!("skipping: no extracted vanilla export at {}", dir.display());
            return;
        }

        let mut parsed = 0usize;
        let mut failures = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("read configured_feature dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read configured feature");
            match serde_json::from_str::<ConfiguredFeature>(&text) {
                Ok(_) => parsed += 1,
                Err(e) => failures.push(format!(
                    "{}: {e}",
                    path.file_name().unwrap().to_string_lossy()
                )),
            }
        }

        assert!(
            failures.is_empty(),
            "{} of {} configured features failed to parse:\n{}",
            failures.len(),
            parsed + failures.len(),
            failures.join("\n")
        );
        assert!(
            parsed > 0,
            "found no configured features to parse in {}",
            dir.display()
        );
        eprintln!("parsed {parsed} configured features");
    }
}
