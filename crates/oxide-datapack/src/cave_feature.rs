//! Configuration IR of the cave decoration features: speleothems (dripstone and sulfur spikes),
//! large dripstone, geodes and sculk patches.
//!
//! The shapes are the 26.2 export's. Fields with a default are the ones the real
//! `configured_feature` files leave out (`large_dripstone` has no search range, `amethyst_geode`
//! has an empty `layers`); the defaults are the codecs' own, as the reference implementation
//! reads them. `fossil` stays a raw value in `feature.rs`: it names structure templates and
//! processor lists, which this crate does not load.
//!
//! Deserialization only; the generator compiles these in `oxide-chunkgen`.

use serde::{Deserialize, Serialize};

use crate::carver::FloatProvider;
use crate::feature::BlockStateProvider;
use crate::placement::{BlockList, BlockStateSpec, IntProvider, TaggedIntProvider};

/// `speleothem`: one pointed formation grown from a ceiling or a floor, with a base patch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeleothemConfig {
    pub base_block: BlockStateSpec,
    pub pointed_block: BlockStateSpec,
    pub replaceable_blocks: BlockList,
    #[serde(default = "chance_of_taller_generation")]
    pub chance_of_taller_generation: f32,
    #[serde(default = "chance_of_directional_spread")]
    pub chance_of_directional_spread: f32,
    #[serde(default = "chance_of_spread_radius")]
    pub chance_of_spread_radius2: f32,
    #[serde(default = "chance_of_spread_radius")]
    pub chance_of_spread_radius3: f32,
}

fn chance_of_taller_generation() -> f32 {
    0.2
}

fn chance_of_directional_spread() -> f32 {
    0.7
}

fn chance_of_spread_radius() -> f32 {
    0.5
}

/// `speleothem_cluster`: a pool-shaped field of stalactites and stalagmites.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeleothemClusterConfig {
    pub base_block: BlockStateSpec,
    pub pointed_block: BlockStateSpec,
    pub replaceable_blocks: BlockList,
    pub floor_to_ceiling_search_range: i32,
    pub height: IntProvider,
    pub radius: IntProvider,
    pub max_stalagmite_stalactite_height_diff: i32,
    pub height_deviation: i32,
    pub speleothem_block_layer_thickness: IntProvider,
    pub density: FloatProvider,
    pub wetness: FloatProvider,
    pub chance_of_speleothem_at_max_distance_from_center: f32,
    pub max_distance_from_edge_affecting_chance_of_speleothem: i32,
    pub max_distance_from_center_affecting_height_bias: i32,
}

/// `large_dripstone`: a stalactite and stalagmite pair spanning a cave. The base block is always
/// `dripstone_block`; the config only names what it may replace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LargeDripstoneConfig {
    #[serde(default = "default_search_range")]
    pub floor_to_ceiling_search_range: i32,
    pub column_radius: IntProvider,
    pub height_scale: FloatProvider,
    pub max_column_radius_to_cave_height_ratio: f32,
    pub stalactite_bluntness: FloatProvider,
    pub stalagmite_bluntness: FloatProvider,
    pub wind_speed: FloatProvider,
    pub min_radius_for_wind: i32,
    pub min_bluntness_for_wind: f32,
    pub replaceable_blocks: BlockList,
}

fn default_search_range() -> i32 {
    30
}

/// The five layer providers and block sets of a geode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeodeBlocks {
    pub filling_provider: BlockStateProvider,
    pub inner_layer_provider: BlockStateProvider,
    pub alternate_inner_layer_provider: BlockStateProvider,
    pub middle_layer_provider: BlockStateProvider,
    pub outer_layer_provider: BlockStateProvider,
    pub inner_placements: Vec<BlockStateSpec>,
    pub cannot_replace: BlockList,
    pub invalid_blocks: BlockList,
}

/// Layer thicknesses, as the inverse-square-root falloff constants vanilla names them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeodeLayers {
    #[serde(default = "layer_filling")]
    pub filling: f64,
    #[serde(default = "layer_inner")]
    pub inner_layer: f64,
    #[serde(default = "layer_middle")]
    pub middle_layer: f64,
    #[serde(default = "layer_outer")]
    pub outer_layer: f64,
}

fn layer_filling() -> f64 {
    1.7
}

fn layer_inner() -> f64 {
    2.2
}

fn layer_middle() -> f64 {
    3.2
}

fn layer_outer() -> f64 {
    4.2
}

/// The crack that opens one side of a geode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeodeCrack {
    #[serde(default = "one")]
    pub generate_crack_chance: f64,
    #[serde(default = "two_f64")]
    pub base_crack_size: f64,
    #[serde(default = "two_i32")]
    pub crack_point_offset: i32,
}

fn one() -> f64 {
    1.0
}

fn two_f64() -> f64 {
    2.0
}

fn two_i32() -> i32 {
    2
}

/// `geode`: amethyst geodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeodeConfig {
    pub blocks: GeodeBlocks,
    pub layers: GeodeLayers,
    pub crack: GeodeCrack,
    #[serde(default = "potential_chance")]
    pub use_potential_placements_chance: f64,
    #[serde(default)]
    pub use_alternate_layer0_chance: f64,
    #[serde(default = "yes")]
    pub placements_require_layer0_alternate: bool,
    #[serde(default = "outer_wall_distance")]
    pub outer_wall_distance: IntProvider,
    #[serde(default = "distribution_points")]
    pub distribution_points: IntProvider,
    #[serde(default = "point_offset")]
    pub point_offset: IntProvider,
    #[serde(default = "min_gen_offset")]
    pub min_gen_offset: i32,
    #[serde(default = "max_gen_offset")]
    pub max_gen_offset: i32,
    #[serde(default = "noise_multiplier")]
    pub noise_multiplier: f64,
    pub invalid_blocks_threshold: i32,
}

fn potential_chance() -> f64 {
    0.35
}

fn yes() -> bool {
    true
}

fn uniform(min_inclusive: i32, max_inclusive: i32) -> IntProvider {
    IntProvider::Tagged(Box::new(TaggedIntProvider::Uniform { min_inclusive, max_inclusive }))
}

fn outer_wall_distance() -> IntProvider {
    uniform(4, 5)
}

fn distribution_points() -> IntProvider {
    uniform(3, 4)
}

fn point_offset() -> IntProvider {
    uniform(1, 2)
}

fn min_gen_offset() -> i32 {
    -16
}

fn max_gen_offset() -> i32 {
    16
}

fn noise_multiplier() -> f64 {
    0.05
}

/// `sculk_patch`: the sculk spreader run once at generation time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SculkPatchConfig {
    pub charge_count: i32,
    pub amount_per_charge: i32,
    pub spread_attempts: i32,
    pub growth_rounds: i32,
    pub spread_rounds: i32,
    pub extra_rare_growths: IntProvider,
    pub catalyst_chance: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::ConfiguredFeature;

    #[test]
    fn omitted_fields_take_the_codec_defaults() {
        let geode: ConfiguredFeature = serde_json::from_str(
            r##"{"type":"minecraft:geode","config":{"blocks":{
            "filling_provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:air"}},
            "inner_layer_provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:air"}},
            "alternate_inner_layer_provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:air"}},
            "middle_layer_provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:air"}},
            "outer_layer_provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:air"}},
            "inner_placements":[],"cannot_replace":"#minecraft:a","invalid_blocks":"#minecraft:b"},
            "crack":{},"layers":{},"invalid_blocks_threshold":1}}"##,
        )
        .unwrap();
        let ConfiguredFeature::Geode(config) = geode else { panic!("not a geode") };
        assert_eq!(config.layers.outer_layer, 4.2);
        assert_eq!(config.crack.crack_point_offset, 2);
        assert_eq!(config.use_potential_placements_chance, 0.35);
        assert!(config.placements_require_layer0_alternate);
        assert_eq!((config.min_gen_offset, config.max_gen_offset), (-16, 16));
    }

    #[test]
    fn a_large_dripstone_without_a_search_range_searches_thirty_blocks() {
        let feature: ConfiguredFeature = serde_json::from_str(
            r##"{"type":"minecraft:large_dripstone","config":{"column_radius":3,"height_scale":1.0,
            "max_column_radius_to_cave_height_ratio":0.33,"min_bluntness_for_wind":0.6,"min_radius_for_wind":4,
            "replaceable_blocks":"#minecraft:dripstone_replaceable_blocks","stalactite_bluntness":0.5,
            "stalagmite_bluntness":0.5,"wind_speed":0.1}}"##,
        )
        .unwrap();
        let ConfiguredFeature::LargeDripstone(config) = feature else { panic!("not a large dripstone") };
        assert_eq!(config.floor_to_ceiling_search_range, 30);
    }
}
