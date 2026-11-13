//! Tests for the ground features, on flat terrain with fixed seeds: what each places, that the
//! same seed gives the same writes, that nothing lands outside the region, and that every 26.2
//! configured feature of these types compiles when the reference export is present.

use std::collections::HashMap;
use std::path::Path;

use oxide_core::{BlockPos, BlockState, ChunkPos, RandomSource, ResourceLocation, Xoroshiro128PlusPlus};
use oxide_datapack::placement::PlacedFeature as PlacedFeatureIr;
use oxide_datapack::{load_tags, Biome, BlockFacts, Registry, TagSet};

use super::super::*;
use super::BiomeClimate;
use crate::region::testutil::{block, flat_ring, refs};
use crate::region::{BlockWrite, DecorationRegion};

struct AlwaysSurvives;

impl BlockBehaviours for AlwaysSurvives {
    fn can_survive(&self, _: &BlockState, _: &DecorationRegion<'_>, _: BlockPos) -> Option<bool> {
        Some(true)
    }
}

/// Facts for the blocks these tests touch, in the exporter's format.
fn facts() -> BlockFacts {
    let dir = std::env::temp_dir().join(format!("oxide-ground-facts-{}-{:?}", std::process::id(), std::thread::current().id()));
    std::fs::create_dir_all(&dir).unwrap();
    let full = "Dsmoudf1234";
    let mut text = String::from(
        "minecraft:air\tDarc\nminecraft:cave_air\tDarc\nminecraft:void_air\tDarc\n\
         minecraft:grass_block[snowy=true]\tsmoudf1234\n\
         minecraft:water[level=0]\tDlrcWS134\n\
         minecraft:ice\tDsmudf1234\n\
         minecraft:snow[layers=1]\tDrdc1\n\
         minecraft:sugar_cane[age=0]\tDc1\n\
         minecraft:spawner\tDsmudf1234\n\
         minecraft:dirt_path\tDsmd1234\n\
         minecraft:chest[facing=east,type=single,waterlogged=false]\tsm1234\n\
         minecraft:chest[facing=south,type=single,waterlogged=false]\tsm1234\n\
         minecraft:chest[facing=west,type=single,waterlogged=false]\tsm1234\n\
         minecraft:chest[facing=north,type=single,waterlogged=false]\tDsm1234\n\
         minecraft:grass_block[snowy=false]\tDsmoudf1234\n",
    );
    for name in [
        "stone", "dirt", "sand", "sandstone", "snow_block", "packed_ice", "blue_ice", "magma_block",
        "cobblestone", "mossy_cobblestone", "pumpkin", "oak_log",
    ] {
        text.push_str(&format!("minecraft:{name}\t{full}\n"));
    }
    std::fs::write(dir.join("block_facts.tsv"), text).unwrap();
    let facts = BlockFacts::load(&dir).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    facts
}

fn tags(entries: &[(&str, &[&str])]) -> TagSet {
    let root = std::env::temp_dir().join(format!("oxide-ground-tags-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&root);
    for (name, values) in entries {
        let path = root.join("data/minecraft/tags/block").join(format!("{name}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let list: Vec<String> = values.iter().map(|value| format!("\"minecraft:{value}\"")).collect();
        std::fs::write(path, format!("{{\"values\":[{}]}}", list.join(","))).unwrap();
    }
    let set = if entries.is_empty() { TagSet::default() } else { load_tags(&[&root], "block").unwrap() };
    let _ = std::fs::remove_dir_all(&root);
    set
}

/// A biome with the given base temperature and precipitation, called `minecraft:plains`.
fn climate(temperature: f32, has_precipitation: bool) -> HashMap<ResourceLocation, BiomeClimate> {
    let json = format!(
        r#"{{"has_precipitation":{has_precipitation},"temperature":{temperature},"downfall":0.4,"effects":{{}}}}"#
    );
    let biome: Biome = serde_json::from_str(&json).expect("biome json");
    HashMap::from([(ResourceLocation::minecraft("plains"), BiomeClimate::from_biome(&biome).unwrap())])
}

fn env(tag_entries: &[(&str, &[&str])], behaviours: Box<dyn BlockBehaviours>, climates: HashMap<ResourceLocation, BiomeClimate>) -> Environment {
    Environment {
        block_tags: tags(tag_entries),
        fluid_tags: TagSet::default(),
        biome_tags: TagSet::default(),
        facts: facts(),
        seed: 0,
        sea_level: 63,
        behaviours,
        placed: HashMap::new(),
        biome_climate: climates,
        biome_features: HashMap::new(),
    }
}

fn plain_env(tag_entries: &[(&str, &[&str])]) -> Environment {
    env(tag_entries, Box::new(NoBlockBehaviours), HashMap::new())
}

struct Outcome {
    writes: Vec<BlockWrite>,
    tally: UnsupportedTally,
    placed: bool,
}

impl Outcome {
    fn count(&self, path: &str) -> usize {
        self.writes.iter().filter(|(_, state)| state.name.path() == path).count()
    }

    fn at(&self, x: i32, y: i32, z: i32) -> Option<&str> {
        self.writes
            .iter()
            .rev()
            .find(|(pos, _)| *pos == BlockPos::new(x, y, z))
            .map(|(_, state)| state.name.path())
    }
}

/// Places the configured feature `config` (`{"type":..,"config":..}`) at `origin` in a flat stone
/// world (surface y = 64, chunk (0, 0)) seeded with `seed`, after `prepare` has edited the region.
/// The outcome's writes include what `prepare` wrote; tests look for the feature's own blocks.
fn place(
    env: &Environment,
    config: &str,
    seed: i64,
    origin: BlockPos,
    prepare: impl FnOnce(&mut DecorationRegion<'_>),
) -> Outcome {
    let ir: PlacedFeatureIr = serde_json::from_str(&format!(r#"{{"feature":{config},"placement":[]}}"#)).expect("json");
    let (placed_registry, configured) = (Registry::default(), Registry::default());
    let placed = Compiler::new(&env.facts, &placed_registry, &configured).compile_placed(&ir).expect("compiles");
    let chunks = flat_ring(ChunkPos::new(0, 0), 64);
    let mut region = DecorationRegion::new(ChunkPos::new(0, 0), refs(&chunks), &env.facts);
    prepare(&mut region);
    let mut random = Xoroshiro128PlusPlus::new(seed);
    let mut tally = UnsupportedTally::default();
    let did_place = {
        let mut cx = FeatureContext {
            env,
            region: &mut region,
            random: &mut random,
            origin,
            unsupported: &mut tally,
            top_feature: None,
        };
        place_placed(&placed, &mut cx, origin, None)
    };
    Outcome { writes: region.into_writes(), tally, placed: did_place }
}

/// The state of the vanilla block `path`; water is the source block the facts know.
fn state(path: &str) -> BlockState {
    let mut state = block(path);
    if path == "water" {
        state.properties.insert("level".into(), "0".into());
    }
    state
}

/// Fills the box `[x0, x1) x [y0, y1) x [z0, z1)` with the block `path`.
fn fill(region: &mut DecorationRegion<'_>, x: (i32, i32), y: (i32, i32), z: (i32, i32), path: &str) {
    for px in x.0..x.1 {
        for py in y.0..y.1 {
            for pz in z.0..z.1 {
                assert!(region.set_block(BlockPos::new(px, py, pz), state(path)));
            }
        }
    }
}

fn simple(path: &str) -> String {
    format!(r#"{{"type":"minecraft:simple_state_provider","state":{{"Name":"minecraft:{path}"}}}}"#)
}

/// Every write lies inside the nine-chunk window around chunk (0, 0).
fn inside_window(outcome: &Outcome) -> bool {
    outcome.writes.iter().all(|(pos, _)| (-16..32).contains(&pos.x) && (-16..32).contains(&pos.z))
}

#[test]
fn a_disk_replaces_only_target_blocks_inside_its_circle() {
    let env = plain_env(&[]);
    let config = format!(
        r#"{{"type":"minecraft:disk","config":{{"half_height":2,"radius":3,
        "state_provider":{},"target":{{"type":"minecraft:matching_blocks","blocks":["minecraft:dirt"]}}}}}}"#,
        simple("sand")
    );
    let outcome = place(&env, &config, 5, BlockPos::new(8, 64, 8), |region| {
        fill(region, (0, 16), (62, 64), (0, 16), "dirt")
    });
    assert!(outcome.placed);
    // The circle of radius 3 holds 29 columns; the two dirt layers in each are replaced.
    assert_eq!(outcome.count("sand"), 29 * 2);
    for (pos, state) in outcome.writes.iter().filter(|(_, state)| state.name.path() == "sand") {
        let (dx, dz) = (pos.x - 8, pos.z - 8);
        assert!(dx * dx + dz * dz <= 9 && (62..64).contains(&pos.y), "{pos:?} {state}");
    }
    assert_eq!(outcome.count("dirt"), 16 * 16 * 2 - 29 * 2);
    assert!(inside_window(&outcome));
}

#[test]
fn a_disk_with_a_random_radius_is_deterministic_and_a_missing_target_places_nothing() {
    let env = plain_env(&[]);
    let config = format!(
        r#"{{"type":"minecraft:disk","config":{{"half_height":1,
        "radius":{{"type":"minecraft:uniform","min_inclusive":2,"max_inclusive":6}},
        "state_provider":{},"target":{{"type":"minecraft:matching_blocks","blocks":["minecraft:stone"]}}}}}}"#,
        simple("sand")
    );
    let run = |seed| place(&env, &config, seed, BlockPos::new(8, 63, 8), |_| {});
    let (first, again) = (run(11), run(11));
    assert_eq!(first.writes, again.writes);
    assert!(first.placed && first.count("sand") > 0);
    assert!((0..20).any(|seed| run(seed).writes != first.writes), "the radius never varies");

    let no_target = place(&env, &config, 11, BlockPos::new(8, 90, 8), |_| {});
    assert!(!no_target.placed && no_target.writes.is_empty());
}

#[test]
fn a_rule_based_disk_reads_the_world_as_the_walk_left_it() {
    let env = plain_env(&[]);
    // Grass where the block above is air, else sandstone: the walk goes top-down, so a layer
    // already converted is what the next layer's rule sees above it.
    let config = r#"{"type":"minecraft:disk","config":{"half_height":2,"radius":1,
        "state_provider":{"type":"minecraft:rule_based_state_provider",
          "fallback":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:sand"}},
          "rules":[{"if_true":{"type":"minecraft:matching_blocks","blocks":"minecraft:air","offset":[0,1,0]},
                    "then":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:sandstone"}}}]},
        "target":{"type":"minecraft:matching_blocks","blocks":["minecraft:dirt"]}}}"#;
    let outcome = place(&env, config, 1, BlockPos::new(8, 64, 8), |region| {
        fill(region, (0, 16), (62, 64), (0, 16), "dirt")
    });
    assert_eq!(outcome.at(8, 63, 8), Some("sandstone"), "air above");
    assert_eq!(outcome.at(8, 62, 8), Some("sand"), "converted block above");
}

#[test]
fn a_block_pile_stays_two_layers_high_on_sturdy_ground() {
    let env = plain_env(&[]);
    let config = format!(r#"{{"type":"minecraft:block_pile","config":{{"state_provider":{}}}}}"#, simple("pumpkin"));
    let run = |seed| place(&env, &config, seed, BlockPos::new(8, 64, 8), |_| {});
    let outcome = run(3);
    assert!(outcome.placed && outcome.count("pumpkin") > 0);
    assert_eq!(outcome.writes, run(3).writes);
    for (pos, _) in &outcome.writes {
        assert!((64..66).contains(&pos.y) && (pos.x - 8).abs() <= 3 && (pos.z - 8).abs() <= 3, "{pos:?}");
    }
    // Nothing floats: a pumpkin at y=65 sits on one at y=64.
    for (pos, _) in outcome.writes.iter().filter(|(pos, _)| pos.y == 65) {
        assert_eq!(outcome.at(pos.x, 64, pos.z), Some("pumpkin"));
    }
    assert!(inside_window(&outcome));
}

#[test]
fn a_block_pile_refuses_to_start_near_the_bottom_of_the_world() {
    let env = plain_env(&[]);
    let config = format!(r#"{{"type":"minecraft:block_pile","config":{{"state_provider":{}}}}}"#, simple("pumpkin"));
    let outcome = place(&env, &config, 3, BlockPos::new(8, -60, 8), |_| {});
    assert!(!outcome.placed && outcome.writes.is_empty());
}

#[test]
fn a_dirt_path_carries_a_pile_only_half_the_time() {
    let env = plain_env(&[]);
    let config = format!(r#"{{"type":"minecraft:block_pile","config":{{"state_provider":{}}}}}"#, simple("pumpkin"));
    let (mut on_path, mut on_stone) = (0, 0);
    for seed in 0..40 {
        let path = place(&env, &config, seed, BlockPos::new(8, 64, 8), |region| {
            fill(region, (0, 16), (63, 64), (0, 16), "dirt_path")
        });
        on_path += path.count("pumpkin");
        on_stone += place(&env, &config, seed, BlockPos::new(8, 64, 8), |_| {}).count("pumpkin");
    }
    assert!(on_path > 0 && on_path < on_stone, "{on_path} on a path, {on_stone} on stone");
}

#[test]
fn a_forest_rock_sinks_to_its_support_and_is_three_lumps() {
    let env = plain_env(&[("forest_rock_can_place_on", &["stone"])]);
    let config = r#"{"type":"minecraft:block_blob","config":{
        "can_place_on":{"type":"minecraft:matching_block_tag","tag":"minecraft:forest_rock_can_place_on"},
        "state":{"Name":"minecraft:mossy_cobblestone"}}}"#;
    let run = |seed| place(&env, config, seed, BlockPos::new(8, 70, 8), |_| {});
    let outcome = run(2);
    assert!(outcome.placed && outcome.count("mossy_cobblestone") >= 3);
    assert_eq!(outcome.writes, run(2).writes);
    // The origin came down to y=64 (stone below it); lumps drift down at most 2 more and reach
    // one block either side.
    for (pos, _) in &outcome.writes {
        assert!((61..=65).contains(&pos.y) && (pos.x - 8).abs() <= 4 && (pos.z - 8).abs() <= 4, "{pos:?}");
    }
    assert!(inside_window(&outcome));
}

#[test]
fn a_forest_rock_with_nothing_to_stand_on_gives_up() {
    let env = plain_env(&[]);
    let config = r#"{"type":"minecraft:block_blob","config":{
        "can_place_on":{"type":"minecraft:matching_block_tag","tag":"minecraft:forest_rock_can_place_on"},
        "state":{"Name":"minecraft:mossy_cobblestone"}}}"#;
    let outcome = place(&env, config, 2, BlockPos::new(8, 70, 8), |_| {});
    assert!(!outcome.placed && outcome.writes.is_empty());
}

const SUGAR_CANE: &str = r#"{"type":"minecraft:block_column","config":{
    "allowed_placement":{"type":"minecraft:matching_block_tag","tag":"minecraft:air"},
    "direction":"up","prioritize_tip":false,
    "layers":[{"height":{"type":"minecraft:biased_to_bottom","min_inclusive":2,"max_inclusive":4},
               "provider":{"type":"minecraft:simple_state_provider",
                           "state":{"Name":"minecraft:sugar_cane","Properties":{"age":"0"}}}}]}}"#;

#[test]
fn a_block_column_grows_upward_two_to_four_blocks() {
    let env = plain_env(&[("air", &["air", "cave_air", "void_air"])]);
    for seed in 0..20 {
        let outcome = place(&env, SUGAR_CANE, seed, BlockPos::new(8, 64, 8), |_| {});
        assert!(outcome.placed);
        let height = outcome.count("sugar_cane");
        assert!((2..=4).contains(&height), "height {height}");
        for y in 64..64 + height as i32 {
            assert_eq!(outcome.at(8, y, 8), Some("sugar_cane"));
        }
        assert_eq!(outcome.writes.len(), height);
    }
}

#[test]
fn a_blocked_column_is_cut_short() {
    let env = plain_env(&[("air", &["air", "cave_air", "void_air"])]);
    // The test starts one block past the origin, so stone at y=66 allows only the origin block.
    let outcome = place(&env, SUGAR_CANE, 4, BlockPos::new(8, 64, 8), |region| {
        region.set_block(BlockPos::new(8, 66, 8), block("stone"));
    });
    assert!(outcome.placed);
    assert_eq!(outcome.count("sugar_cane"), 1);
    assert_eq!(outcome.at(8, 64, 8), Some("sugar_cane"));
}

#[test]
fn a_block_column_with_two_layers_lays_them_in_order_downward() {
    let env = plain_env(&[("air", &["air", "cave_air", "void_air"])]);
    let config = r#"{"type":"minecraft:block_column","config":{
        "allowed_placement":{"type":"minecraft:matching_block_tag","tag":"minecraft:air"},
        "direction":"down","prioritize_tip":true,
        "layers":[{"height":2,"provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:oak_log"}}},
                  {"height":1,"provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:pumpkin"}}}]}}"#;
    let outcome = place(&env, config, 1, BlockPos::new(8, 70, 8), |_| {});
    assert_eq!(outcome.at(8, 70, 8), Some("oak_log"));
    assert_eq!(outcome.at(8, 69, 8), Some("oak_log"));
    assert_eq!(outcome.at(8, 68, 8), Some("pumpkin"));
}

#[test]
fn a_column_with_zero_height_places_nothing() {
    let env = plain_env(&[("air", &["air"])]);
    let config = r#"{"type":"minecraft:block_column","config":{
        "allowed_placement":{"type":"minecraft:matching_block_tag","tag":"minecraft:air"},
        "direction":"up","prioritize_tip":false,
        "layers":[{"height":0,"provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:oak_log"}}}]}}"#;
    let outcome = place(&env, config, 1, BlockPos::new(8, 70, 8), |_| {});
    assert!(!outcome.placed && outcome.writes.is_empty());
}

fn lake_config(fluid: &str) -> String {
    format!(
        r#"{{"type":"minecraft:lake","config":{{"barrier":{},"fluid":{},
        "can_place_feature":{{"type":"minecraft:true"}},
        "can_replace_with_air_or_fluid":{{"type":"minecraft:not","predicate":{{"type":"minecraft:matching_block_tag","tag":"minecraft:features_cannot_replace"}}}},
        "can_replace_with_barrier":{{"type":"minecraft:not","predicate":{{"type":"minecraft:matching_block_tag","tag":"minecraft:lava_pool_stone_cannot_replace"}}}}}}}}"#,
        simple("stone"),
        fluid
    )
}

const WATER: &str = r#"{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:water","Properties":{"level":"0"}}}"#;

#[test]
fn a_lake_fills_the_lower_four_layers_and_opens_the_upper_four() {
    let env = plain_env(&[]);
    let run = |seed| place(&env, &lake_config(WATER), seed, BlockPos::new(8, 64, 8), |_| {});
    let outcome = run(7);
    assert!(outcome.placed, "a lake on flat stone has nothing to refuse it");
    assert_eq!(outcome.writes, run(7).writes);
    assert!(outcome.count("water") > 0 && outcome.count("cave_air") > 0);
    for (pos, state) in &outcome.writes {
        assert!((0..16).contains(&pos.x) && (0..16).contains(&pos.z) && (60..68).contains(&pos.y), "{pos:?}");
        match state.name.path() {
            "water" => assert!(pos.y < 64, "water above the surface at {pos:?}"),
            "cave_air" => assert!(pos.y >= 64, "air below the surface at {pos:?}"),
            "stone" => {}
            other => panic!("unexpected block {other}"),
        }
    }
    assert!(inside_window(&outcome));
}

#[test]
fn a_lake_abandons_itself_when_its_edge_would_leak() {
    let env = plain_env(&[]);
    // The lower layers of the edge must be solid or fluid: open air under the surface refuses.
    let outcome = place(&env, &lake_config(WATER), 7, BlockPos::new(8, 64, 8), |region| {
        fill(region, (0, 16), (60, 64), (0, 16), "oak_log");
        fill(region, (0, 16), (60, 64), (0, 16), "air");
    });
    assert!(!outcome.placed);
    assert_eq!(outcome.count("water"), 0);
}

#[test]
fn a_lake_too_close_to_the_bottom_is_refused() {
    let env = plain_env(&[]);
    let outcome = place(&env, &lake_config(WATER), 7, BlockPos::new(8, -60, 8), |_| {});
    assert!(!outcome.placed && outcome.writes.is_empty());
}

#[test]
fn a_water_lake_freezes_water_beside_it_in_a_cold_biome_only() {
    let freezing = env(&[], Box::new(NoBlockBehaviours), climate(-0.5, true));
    let warm = env(&[], Box::new(NoBlockBehaviours), climate(0.8, true));
    // Column (0, 0) of the lake grid is never part of the pool; the ice pass looks at the layer
    // that is air inside the pool, so open water standing there is what can freeze.
    let prepare = |region: &mut DecorationRegion<'_>| {
        region.set_block(BlockPos::new(0, 64, 0), state("water"));
    };
    let cold = place(&freezing, &lake_config(WATER), 7, BlockPos::new(8, 64, 8), prepare);
    assert!(cold.placed);
    assert_eq!(cold.at(0, 64, 0), Some("ice"));
    let hot = place(&warm, &lake_config(WATER), 7, BlockPos::new(8, 64, 8), prepare);
    assert_eq!(hot.at(0, 64, 0), Some("water"));
}

const FREEZE: &str = r#"{"type":"minecraft:freeze_top_layer","config":{}}"#;

#[test]
fn freezing_ices_water_and_snows_the_ground_in_a_cold_biome() {
    let env = env(&[], Box::new(AlwaysSurvives), climate(-0.5, true));
    let outcome = place(&env, FREEZE, 1, BlockPos::new(0, 64, 0), |region| {
        region.set_block(BlockPos::new(3, 63, 3), state("water"));
        let mut grass = block("grass_block");
        grass.properties.insert("snowy".into(), "false".into());
        region.set_block(BlockPos::new(5, 63, 5), grass);
    });
    assert!(outcome.placed);
    assert_eq!(outcome.at(3, 63, 3), Some("ice"));
    assert_eq!(outcome.at(3, 64, 3), Some("snow"));
    let under_snow = outcome.writes.iter().find(|(pos, _)| *pos == BlockPos::new(5, 63, 5)).unwrap();
    assert_eq!(under_snow.1.properties.get("snowy").map(String::as_str), Some("true"));
    assert_eq!(outcome.count("snow"), 256, "one layer per column of the chunk");
    assert!(outcome.writes.iter().all(|(pos, _)| (0..16).contains(&pos.x) && (0..16).contains(&pos.z)));
}

#[test]
fn a_warm_biome_and_a_biome_without_precipitation_do_not_snow() {
    for climates in [climate(0.8, true), climate(-0.5, false), HashMap::new()] {
        let env = env(&[], Box::new(AlwaysSurvives), climates);
        let outcome = place(&env, FREEZE, 1, BlockPos::new(0, 64, 0), |region| {
            region.set_block(BlockPos::new(3, 63, 3), state("water"));
        });
        assert!(outcome.placed);
        assert_eq!(outcome.count("snow") + outcome.count("ice"), if outcome.at(3, 63, 3) == Some("ice") { 1 } else { 0 });
    }
}

#[test]
fn snow_with_an_unknown_survival_rule_is_tallied_not_placed() {
    let env = env(&[], Box::new(NoBlockBehaviours), climate(-0.5, true));
    let outcome = place(&env, FREEZE, 1, BlockPos::new(0, 64, 0), |region| {
        region.set_block(BlockPos::new(3, 63, 3), state("water"));
    });
    assert_eq!(outcome.count("snow"), 0);
    assert_eq!(outcome.at(3, 63, 3), Some("ice"), "ice needs no survival rule");
    assert_eq!(outcome.tally.get("survival:minecraft:snow"), 256);
}

#[test]
fn frozen_biome_temperature_is_deterministic_and_drops_with_height() {
    let json = r#"{"has_precipitation":true,"temperature":0.8,"temperature_modifier":"frozen","downfall":0.4,"effects":{}}"#;
    let biome: Biome = serde_json::from_str(json).unwrap();
    let climate = BiomeClimate::from_biome(&biome).unwrap();
    for x in (-200..200).step_by(37) {
        let low = BlockPos::new(x, 70, x * 3);
        assert_eq!(climate.temperature(low, 63).to_bits(), climate.temperature(low, 63).to_bits());
        // Inside a frozen patch the base is 0.2, outside it 0.8; either way a mountain top is
        // colder than the same column near the sea.
        let high = BlockPos::new(x, 200, x * 3);
        assert!(climate.temperature(high, 63) < climate.temperature(low, 63));
        let base = climate.temperature(BlockPos::new(x, 60, x * 3), 63);
        assert!(base == 0.2 || base == 0.8, "{base}");
    }
    let unknown: Biome = serde_json::from_str(&json.replace("frozen", "melting")).unwrap();
    assert!(BiomeClimate::from_biome(&unknown).is_err());
}

const ICE_SPIKE: &str = r#"{"type":"minecraft:spike","config":{
    "can_place_on":{"type":"minecraft:matching_blocks","blocks":"minecraft:snow_block"},
    "can_replace":{"type":"minecraft:matching_block_tag","tag":"minecraft:ice_spike_replaceable"},
    "state":{"Name":"minecraft:packed_ice"}}}"#;

#[test]
fn an_ice_spike_stands_on_snow_and_rises_from_its_base() {
    let env = plain_env(&[("ice_spike_replaceable", &["dirt", "snow_block", "ice"])]);
    let prepare = |region: &mut DecorationRegion<'_>| fill(region, (0, 16), (63, 64), (0, 16), "snow_block");
    let run = |seed| place(&env, ICE_SPIKE, seed, BlockPos::new(8, 70, 8), prepare);
    let outcome = run(12);
    assert!(outcome.placed);
    assert_eq!(outcome.writes, run(12).writes);
    assert!(outcome.count("packed_ice") >= 7, "a spike is at least seven layers");
    // The tip is on the centre column.
    let top = outcome.writes.iter().filter(|(_, s)| s.name.path() == "packed_ice").map(|(p, _)| p.y).max().unwrap();
    assert_eq!(outcome.at(8, top, 8), Some("packed_ice"));
    assert!(inside_window(&outcome));
}

#[test]
fn an_ice_spike_needs_snow_under_it() {
    let env = plain_env(&[("ice_spike_replaceable", &["dirt"])]);
    let outcome = place(&env, ICE_SPIKE, 12, BlockPos::new(8, 70, 8), |_| {});
    assert!(!outcome.placed && outcome.writes.is_empty());
}

fn seawater(region: &mut DecorationRegion<'_>) {
    fill(region, (-8, 24), (44, 64), (-8, 24), "water");
}

#[test]
fn an_iceberg_is_blue_ice_in_and_above_the_sea() {
    let env = plain_env(&[]);
    let config = r#"{"type":"minecraft:iceberg","config":{"state":{"Name":"minecraft:blue_ice"}}}"#;
    let run = |seed| place(&env, config, seed, BlockPos::new(8, 80, 8), seawater);
    let outcome = run(21);
    assert!(outcome.placed);
    assert_eq!(outcome.writes, run(21).writes);
    assert!(outcome.count("blue_ice") > 0);
    assert!(
        outcome.writes.iter().any(|(pos, state)| pos.y < 63 && state.name.path() == "blue_ice"),
        "no ice below the surface"
    );
    for (_, state) in &outcome.writes {
        assert!(
            matches!(state.name.path(), "blue_ice" | "snow_block" | "water" | "air" | "snow"),
            "unexpected block {state}"
        );
    }
    assert!(inside_window(&outcome));
    assert!((0..30).any(|seed| run(seed).writes != outcome.writes), "icebergs never vary");
}

const BLUE_ICE: &str = r#"{"type":"minecraft:blue_ice","config":{}}"#;

#[test]
fn blue_ice_grows_from_packed_ice_under_the_sea_only() {
    let env = plain_env(&[]);
    let prepare = |region: &mut DecorationRegion<'_>| {
        fill(region, (4, 13), (58, 63), (4, 13), "water");
        region.set_block(BlockPos::new(9, 60, 8), block("packed_ice"));
    };
    let run = |seed| place(&env, BLUE_ICE, seed, BlockPos::new(8, 60, 8), prepare);
    let outcome = run(30);
    assert!(outcome.placed);
    assert_eq!(outcome.at(8, 60, 8), Some("blue_ice"));
    assert!(outcome.count("blue_ice") > 1, "it should have grown");
    assert_eq!(outcome.writes, run(30).writes);

    assert!(!place(&env, BLUE_ICE, 30, BlockPos::new(8, 64, 8), prepare).placed, "above the sea");
    assert!(!place(&env, BLUE_ICE, 30, BlockPos::new(8, 60, 8), |_| {}).placed, "no water");
    let dry_ice = |region: &mut DecorationRegion<'_>| {
        region.set_block(BlockPos::new(8, 60, 8), state("water"));
    };
    assert!(!place(&env, BLUE_ICE, 30, BlockPos::new(8, 60, 8), dry_ice).placed, "no packed ice beside it");
}

fn magma(probability: &str) -> String {
    format!(
        r#"{{"type":"minecraft:underwater_magma","config":{{"floor_search_range":5,
        "placement_probability_per_valid_position":{probability},"placement_radius_around_floor":1}}}}"#
    )
}

#[test]
fn underwater_magma_covers_the_floor_cube_when_every_roll_passes() {
    let env = plain_env(&[]);
    let prepare = |region: &mut DecorationRegion<'_>| fill(region, (0, 16), (64, 70), (0, 16), "water");
    let outcome = place(&env, &magma("1.0"), 3, BlockPos::new(8, 66, 8), prepare);
    assert!(outcome.placed);
    // The floor is y=63; the 3x3x3 cube holds the floor layer and the one under it as stone
    // (water above is never replaced).
    assert_eq!(outcome.count("magma_block"), 18);
    for (pos, state) in outcome.writes.iter().filter(|(_, s)| s.name.path() == "magma_block") {
        assert!((62..=63).contains(&pos.y) && (7..=9).contains(&pos.x) && (7..=9).contains(&pos.z), "{pos:?} {state}");
    }
    let none = place(&env, &magma("0.0"), 3, BlockPos::new(8, 66, 8), prepare);
    assert!(!none.placed && none.count("magma_block") == 0);
}

#[test]
fn underwater_magma_needs_water_and_a_floor_in_range() {
    let env = plain_env(&[]);
    assert!(!place(&env, &magma("1.0"), 3, BlockPos::new(8, 66, 8), |_| {}).placed, "not in water");
    let deep = |region: &mut DecorationRegion<'_>| fill(region, (0, 16), (50, 70), (0, 16), "water");
    assert!(!place(&env, &magma("1.0"), 3, BlockPos::new(8, 66, 8), deep).placed, "floor out of range");
}

const MONSTER_ROOM: &str = r#"{"type":"minecraft:monster_room","config":{}}"#;

/// Solid stone with a two-block-high tunnel along x through the room's centre row: the walls it
/// crosses are the room's holes.
fn tunnelled(region: &mut DecorationRegion<'_>) {
    fill(region, (-16, 32), (30, 50), (-16, 32), "stone");
    fill(region, (0, 16), (40, 42), (8, 9), "air");
}

#[test]
fn a_monster_room_is_hollowed_floored_and_given_a_spawner() {
    let env = plain_env(&[]);
    let run = |seed| place(&env, MONSTER_ROOM, seed, BlockPos::new(8, 40, 8), tunnelled);
    let outcome = run(40);
    assert!(outcome.placed);
    assert_eq!(outcome.writes, run(40).writes);
    assert_eq!(outcome.at(8, 40, 8), Some("spawner"));
    assert!(outcome.count("cobblestone") + outcome.count("mossy_cobblestone") > 0);
    assert_eq!(outcome.tally.get("block_entity:minecraft:mob_spawner"), 1);
    assert_eq!(outcome.tally.get("block_entity:minecraft:chest") as usize, outcome.count("chest"));
    assert!(outcome.count("chest") <= 2);
    assert!(outcome.count("cave_air") > 0);
    // The interior is open between the floor and the ceiling.
    assert_eq!(outcome.at(8, 41, 8), Some("cave_air"));
    assert!(inside_window(&outcome));
}

#[test]
fn a_monster_room_in_solid_rock_has_no_holes_and_is_refused() {
    let env = plain_env(&[]);
    let outcome = place(&env, MONSTER_ROOM, 40, BlockPos::new(8, 40, 8), |region| {
        fill(region, (-16, 32), (30, 50), (-16, 32), "stone")
    });
    assert!(!outcome.placed);
    assert_eq!(outcome.count("spawner"), 0);
}

#[test]
fn a_monster_room_needs_a_solid_floor_and_ceiling() {
    let env = plain_env(&[]);
    let outcome = place(&env, MONSTER_ROOM, 40, BlockPos::new(8, 90, 8), |_| {});
    assert!(!outcome.placed && outcome.writes.is_empty());
}

fn selector(weights: [i32; 2]) -> String {
    let entry = |path: &str, weight: i32| {
        format!(
            r#"{{"data":{{"feature":{{"type":"minecraft:simple_block","config":{{"to_place":{}}}}},"placement":[]}},"weight":{weight}}}"#,
            simple(path)
        )
    };
    format!(
        r#"{{"type":"minecraft:weighted_random_selector","config":{{"features":[{},{}]}}}}"#,
        entry("oak_log", weights[0]),
        entry("pumpkin", weights[1])
    )
}

#[test]
fn a_weighted_selector_draws_once_and_walks_the_weights() {
    let env = env(&[], Box::new(AlwaysSurvives), HashMap::new());
    for seed in 0..12 {
        let draw = Xoroshiro128PlusPlus::new(seed).next_int_bounded(4);
        let expected = if draw < 1 { "oak_log" } else { "pumpkin" };
        let outcome = place(&env, &selector([1, 3]), seed, BlockPos::new(8, 64, 8), |_| {});
        assert!(outcome.placed);
        assert_eq!(outcome.at(8, 64, 8), Some(expected), "seed {seed}");
    }
    let none = place(&env, &selector([0, 0]), 1, BlockPos::new(8, 64, 8), |_| {});
    assert!(!none.placed && none.writes.is_empty());
}

#[test]
fn a_desert_well_stays_unsupported_and_is_tallied() {
    let env = plain_env(&[]);
    let outcome = place(&env, r#"{"type":"minecraft:desert_well","config":{}}"#, 1, BlockPos::new(8, 64, 8), |_| {});
    assert!(!outcome.placed && outcome.writes.is_empty());
    assert_eq!(outcome.tally.get("feature:minecraft:desert_well"), 1);
}

/// Every 26.2 configured feature of the ground types compiles, and each type but the desert well
/// compiles to something that places.
#[test]
fn every_reference_ground_feature_compiles() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference");
    if !root.join("data/minecraft/worldgen/configured_feature").is_dir() || !root.join("block_facts.tsv").is_file() {
        eprintln!("skipping: no extracted vanilla export at {}", root.display());
        return;
    }
    let datapack = oxide_datapack::load_datapack(&root).expect("reference datapack loads");
    const TYPES: [&str; 13] = [
        "disk", "block_pile", "block_blob", "block_column", "lake", "freeze_top_layer", "spike", "iceberg",
        "blue_ice", "desert_well", "weighted_random_selector", "underwater_magma", "monster_room",
    ];
    let mut compiled = 0;
    for (id, ir) in datapack.configured_features.iter() {
        let tag = serde_json::to_value(ir).unwrap()["type"].as_str().unwrap().to_string();
        let Some(kind) = tag.strip_prefix("minecraft:").filter(|kind| TYPES.contains(kind)) else {
            continue;
        };
        let wrapper: PlacedFeatureIr =
            serde_json::from_value(serde_json::json!({ "feature": id.to_string(), "placement": [] })).unwrap();
        let placed = Compiler::new(&datapack.block_facts, &datapack.placed_features, &datapack.configured_features)
            .compile_placed(&wrapper)
            .unwrap_or_else(|error| panic!("{id}: {error:#}"));
        match (&*placed.feature, kind) {
            (Feature::Unsupported(_), "desert_well") => {}
            (Feature::Unsupported(why), _) => panic!("{id} ({kind}) compiled to unsupported: {why}"),
            _ => {}
        }
        compiled += 1;
    }
    assert!(compiled >= 25, "only {compiled} ground features found");
}
