//! Tests for the placement chain, predicates, providers and feature families, on small flat
//! regions where every expected value can be worked out by hand, plus one end-to-end run of a
//! decorator built from the local 26.2 reference tree when it is present.

use std::collections::HashMap;
use std::path::Path;

use oxide_core::{BlockPos, BlockState, ChunkPos, RandomSource, ResourceLocation, WorldgenRandom};
use oxide_datapack::placement::PlacedFeature as PlacedFeatureIr;
use oxide_datapack::{load_tags, BlockFacts, Registry, TagSet};

use super::*;
use crate::decorate::Decorator;
use crate::region::testutil::{block, facts, flat_ring, refs};
use crate::region::{BlockWrite, DecorationRegion};

struct AlwaysSurvives;

impl BlockBehaviours for AlwaysSurvives {
    fn can_survive(&self, _: &BlockState, _: &DecorationRegion<'_>, _: BlockPos) -> Option<bool> {
        Some(true)
    }
}

fn env(tags: &[(&str, &str)], behaviours: Box<dyn BlockBehaviours>) -> Environment {
    let root = std::env::temp_dir().join(format!(
        "oxide-decorator-tags-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for (name, body) in tags {
        let path = root.join("data/minecraft/tags/block").join(format!("{name}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    let block_tags = if tags.is_empty() { TagSet::default() } else { load_tags(&[&root], "block").unwrap() };
    let _ = std::fs::remove_dir_all(&root);
    Environment {
        block_tags,
        fluid_tags: TagSet::default(),
        biome_tags: TagSet::default(),
        facts: facts(),
        seed: 0,
        sea_level: 63,
        behaviours,
        placed: HashMap::new(),
        biome_climate: HashMap::new(),
        biome_features: HashMap::new(),
    }
}

fn compile(json: &str, facts: &BlockFacts) -> Placed {
    let ir: PlacedFeatureIr = serde_json::from_str(json).expect("placed feature json");
    let (placed, configured) = (Registry::default(), Registry::default());
    Compiler::new(facts, &placed, &configured).compile_placed(&ir).expect("compiles")
}

struct Outcome {
    writes: Vec<BlockWrite>,
    tally: UnsupportedTally,
    placed: bool,
}

/// Places `json` at `origin` in a flat stone world (surface y=64) seeded with `seed`, after
/// letting `prepare` edit the region.
fn place_in_flat(
    env: &Environment,
    json: &str,
    seed: i64,
    origin: BlockPos,
    prepare: impl FnOnce(&mut DecorationRegion<'_>),
) -> Outcome {
    let placed = compile(json, &env.facts);
    let chunks = flat_ring(ChunkPos::new(0, 0), 64);
    let mut region = DecorationRegion::new(ChunkPos::new(0, 0), refs(&chunks), &env.facts);
    prepare(&mut region);
    let mut random = WorldgenRandom::new(seed);
    let mut tally = UnsupportedTally::default();
    let placed_any = {
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
    Outcome { writes: region.into_writes(), tally, placed: placed_any }
}

const LOG_FEATURE: &str = r#"{"type":"minecraft:simple_block","config":{"to_place":
    {"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:oak_log"}}}}"#;

fn with_chain(chain: &str) -> String {
    format!(r#"{{"feature":{LOG_FEATURE},"placement":[{chain}]}}"#)
}

fn positions(outcome: &Outcome) -> Vec<(i32, i32, i32)> {
    outcome.writes.iter().map(|(pos, _)| (pos.x, pos.y, pos.z)).collect()
}

#[test]
fn the_chain_runs_depth_first_drawing_in_vanillas_order() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let json = with_chain(
        r#"{"type":"minecraft:count","count":3},{"type":"minecraft:in_square"},
           {"type":"minecraft:heightmap","heightmap":"MOTION_BLOCKING"}"#,
    );
    let outcome = place_in_flat(&env, &json, 42, BlockPos::new(0, -64, 0), |_| {});

    // Three copies, each drawing x then z before the next copy draws anything.
    let mut mirror = WorldgenRandom::new(42);
    let mut expected = Vec::new();
    for _ in 0..3 {
        let x = mirror.next_int_bounded(16);
        let z = mirror.next_int_bounded(16);
        let at = (x, 64, z);
        if !expected.contains(&at) {
            expected.push(at);
        }
    }
    assert_eq!(positions(&outcome), expected);
    assert!(outcome.placed);
    assert!(outcome.writes.iter().all(|(_, state)| *state == block("oak_log")));
}

#[test]
fn random_offset_draws_x_then_y_then_z() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let json = with_chain(
        r#"{"type":"minecraft:random_offset",
            "xz_spread":{"type":"minecraft:uniform","min_inclusive":0,"max_inclusive":9},
            "y_spread":{"type":"minecraft:uniform","min_inclusive":0,"max_inclusive":4}}"#,
    );
    let outcome = place_in_flat(&env, &json, 8, BlockPos::new(2, 70, 3), |_| {});
    let mut mirror = WorldgenRandom::new(8);
    let x = 2 + mirror.next_int_bounded(10);
    let y = 70 + mirror.next_int_bounded(5);
    let z = 3 + mirror.next_int_bounded(10);
    assert_eq!(positions(&outcome), vec![(x, y, z)]);
}

#[test]
fn rarity_filter_passes_when_the_float_is_under_one_over_chance() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let json = with_chain(r#"{"type":"minecraft:rarity_filter","chance":4}"#);
    for seed in 0..40 {
        let outcome = place_in_flat(&env, &json, seed, BlockPos::new(5, 70, 5), |_| {});
        let passes = WorldgenRandom::new(seed).next_float() < 0.25;
        assert_eq!(outcome.writes.len(), usize::from(passes), "seed {seed}");
    }
}

#[test]
fn heightmap_surface_filters_and_relative_threshold() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let json = with_chain(
        r#"{"type":"minecraft:heightmap","heightmap":"WORLD_SURFACE_WG"},
           {"type":"minecraft:surface_water_depth_filter","max_water_depth":0},
           {"type":"minecraft:surface_relative_threshold_filter","heightmap":"OCEAN_FLOOR_WG",
            "min_inclusive":0,"max_inclusive":0}"#,
    );
    let outcome = place_in_flat(&env, &json, 1, BlockPos::new(9, 10, 9), |_| {});
    assert_eq!(positions(&outcome), vec![(9, 64, 9)]);

    // A relative window that excludes the surface rejects it.
    let json = with_chain(
        r#"{"type":"minecraft:heightmap","heightmap":"WORLD_SURFACE_WG"},
           {"type":"minecraft:surface_relative_threshold_filter","heightmap":"OCEAN_FLOOR_WG",
            "min_inclusive":1,"max_inclusive":5}"#,
    );
    assert!(place_in_flat(&env, &json, 1, BlockPos::new(9, 10, 9), |_| {}).writes.is_empty());
}

#[test]
fn environment_scan_walks_down_to_the_first_solid_block() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let json = with_chain(
        r#"{"type":"minecraft:environment_scan","direction_of_search":"down","max_steps":32,
            "target_condition":{"type":"minecraft:solid"},
            "allowed_search_condition":{"type":"minecraft:replaceable"}}"#,
    );
    let outcome = place_in_flat(&env, &json, 1, BlockPos::new(5, 70, 5), |_| {});
    // Air down to y=64, then stone at y=63: the scan stops on it.
    assert_eq!(positions(&outcome), vec![(5, 63, 5)]);
    // Too few steps to reach the ground finds nothing.
    let short = json.replace("\"max_steps\":32", "\"max_steps\":2");
    assert!(place_in_flat(&env, &short, 1, BlockPos::new(5, 70, 5), |_| {}).writes.is_empty());
}

#[test]
fn count_on_every_layer_finds_the_surface_layer() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let json = with_chain(r#"{"type":"minecraft:count_on_every_layer","count":2}"#);
    let outcome = place_in_flat(&env, &json, 6, BlockPos::new(0, 0, 0), |_| {});
    // One layer in a flat world: two attempts land on the surface, then layer 1 finds nothing.
    assert!(!outcome.writes.is_empty() && outcome.writes.len() <= 2);
    assert!(outcome.writes.iter().all(|(pos, _)| pos.y == 64));
}

#[test]
fn fixed_placement_keeps_only_positions_in_the_same_chunk() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let json = with_chain(
        r#"{"type":"minecraft:fixed_placement","positions":[[3,70,4],[20,70,4],[1,71,2]]}"#,
    );
    let outcome = place_in_flat(&env, &json, 1, BlockPos::new(0, -64, 0), |_| {});
    assert_eq!(positions(&outcome), vec![(3, 70, 4), (1, 71, 2)]);
}

#[test]
fn block_predicates_read_blocks_tags_and_fluids() {
    let env = env(
        &[("stoneish", r#"{"values":["minecraft:stone","minecraft:deepslate"]}"#)],
        Box::new(AlwaysSurvives),
    );
    let at = BlockPos::new(4, 63, 4);
    let check = |predicate: &str| -> bool {
        let json = with_chain(&format!(
            r#"{{"type":"minecraft:block_predicate_filter","predicate":{predicate}}}"#
        ));
        !place_in_flat(&env, &json, 1, at, |_| {}).writes.is_empty()
    };
    assert!(check(r#"{"type":"minecraft:matching_blocks","blocks":"minecraft:stone"}"#));
    assert!(!check(r#"{"type":"minecraft:matching_blocks","blocks":["minecraft:dirt"]}"#));
    assert!(check(
        r#"{"type":"minecraft:matching_blocks","blocks":"minecraft:air","offset":[0,1,0]}"#
    ));
    assert!(check(r#"{"type":"minecraft:matching_block_tag","tag":"minecraft:stoneish"}"#));
    assert!(check(r##"{"type":"minecraft:matching_blocks","blocks":"#minecraft:stoneish"}"##));
    assert!(check(r#"{"type":"minecraft:not","predicate":{"type":"minecraft:replaceable"}}"#));
    assert!(check(
        r#"{"type":"minecraft:all_of","predicates":[{"type":"minecraft:solid"},{"type":"minecraft:true"}]}"#
    ));
    assert!(!check(
        r#"{"type":"minecraft:any_of","predicates":[{"type":"minecraft:replaceable"},
            {"type":"minecraft:matching_blocks","blocks":"minecraft:oak_log"}]}"#
    ));
    assert!(check(r#"{"type":"minecraft:matching_fluids","fluids":"minecraft:empty"}"#));
    assert!(!check(r#"{"type":"minecraft:matching_fluids","fluids":["minecraft:water","minecraft:flowing_water"]}"#));
    assert!(check(r#"{"type":"minecraft:inside_world_bounds","offset":[0,0,0]}"#));
    assert!(!check(r#"{"type":"minecraft:inside_world_bounds","offset":[0,-200,0]}"#));
    assert!(check(r#"{"type":"minecraft:has_sturdy_face","direction":"up"}"#));
}

#[test]
fn would_survive_is_unknown_without_behaviours_and_is_tallied() {
    let env = env(&[], Box::new(NoBlockBehaviours));
    let json = with_chain(
        r#"{"type":"minecraft:block_predicate_filter","predicate":
            {"type":"minecraft:would_survive","state":{"Name":"minecraft:oak_log"}}}"#,
    );
    let outcome = place_in_flat(&env, &json, 1, BlockPos::new(4, 63, 4), |_| {});
    assert!(outcome.writes.is_empty());
    assert_eq!(outcome.tally.get("survival:minecraft:oak_log"), 1);
}

#[test]
fn simple_block_without_survival_rules_places_nothing_and_says_so() {
    let env = env(&[], Box::new(NoBlockBehaviours));
    let outcome = place_in_flat(&env, &with_chain(""), 1, BlockPos::new(4, 70, 4), |_| {});
    assert!(!outcome.placed && outcome.writes.is_empty());
    assert_eq!(outcome.tally.get("survival:minecraft:oak_log"), 1);

    let env = self::env(&[], Box::new(AlwaysSurvives));
    let outcome = place_in_flat(&env, &with_chain(""), 1, BlockPos::new(4, 70, 4), |_| {});
    assert!(outcome.placed);
    assert_eq!(positions(&outcome), vec![(4, 70, 4)]);
}

fn feature_json(provider: &str) -> String {
    format!(
        r#"{{"feature":{{"type":"minecraft:simple_block","config":{{"to_place":{provider}}}}},"placement":[]}}"#
    )
}

#[test]
fn weighted_provider_draws_one_bounded_value() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let json = feature_json(
        r#"{"type":"minecraft:weighted_state_provider","entries":[
            {"data":{"Name":"minecraft:stone"},"weight":1},
            {"data":{"Name":"minecraft:oak_log"},"weight":3}]}"#,
    );
    for seed in 0..30 {
        let outcome = place_in_flat(&env, &json, seed, BlockPos::new(2, 70, 2), |_| {});
        let expected = if WorldgenRandom::new(seed).next_int_bounded(4) < 1 {
            block("stone")
        } else {
            block("oak_log")
        };
        assert_eq!(outcome.writes[0].1, expected, "seed {seed}");
    }
}

#[test]
fn rule_based_provider_takes_the_first_matching_rule_else_the_fallback() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let json = feature_json(
        r#"{"type":"minecraft:rule_based_state_provider",
            "fallback":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:stone"}},
            "rules":[{"if_true":{"type":"minecraft:matching_blocks","blocks":"minecraft:stone",
                                  "offset":[0,-1,0]},
                      "then":{"type":"minecraft:simple_state_provider",
                              "state":{"Name":"minecraft:oak_log"}}}]}"#,
    );
    // Above the surface the block below is stone: the rule fires.
    let on_ground = place_in_flat(&env, &json, 1, BlockPos::new(2, 64, 2), |_| {});
    assert_eq!(on_ground.writes[0].1, block("oak_log"));
    // In the air the rule misses and the fallback answers.
    let in_air = place_in_flat(&env, &json, 1, BlockPos::new(2, 80, 2), |_| {});
    assert_eq!(in_air.writes[0].1, block("stone"));
}

#[test]
fn noise_providers_are_deterministic_and_stay_in_their_state_lists() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let noise = r#"{"firstOctave":-3,"amplitudes":[1.0]}"#;
    let states = r#"[{"Name":"minecraft:stone"},{"Name":"minecraft:oak_log"},{"Name":"minecraft:oak_leaves"}]"#;
    let providers = [
        format!(r#"{{"type":"minecraft:noise_provider","seed":2345,"noise":{noise},"scale":1.3,"states":{states}}}"#),
        format!(
            r#"{{"type":"minecraft:dual_noise_provider","seed":2345,"noise":{noise},"scale":1.3,
                "states":{states},"variety":[1,3],"slow_noise":{noise},"slow_scale":2.1}}"#
        ),
        format!(
            r#"{{"type":"minecraft:noise_threshold_provider","seed":2345,"noise":{noise},"scale":1.3,
                "threshold":-0.2,"high_chance":0.5,"default_state":{{"Name":"minecraft:stone"}},
                "low_states":[{{"Name":"minecraft:oak_log"}}],
                "high_states":[{{"Name":"minecraft:oak_leaves"}}]}}"#
        ),
    ];
    for provider in &providers {
        let json = feature_json(provider);
        let mut seen = std::collections::HashSet::new();
        for x in 0..16 {
            let origin = BlockPos::new(x, 65 + x * 4, x * 7 % 16);
            let first = place_in_flat(&env, &json, 5, origin, |_| {});
            let second = place_in_flat(&env, &json, 5, origin, |_| {});
            assert_eq!(first.writes, second.writes);
            let state = first.writes[0].1.clone();
            assert!(
                [block("stone"), block("oak_log"), block("oak_leaves")].contains(&state),
                "{state}"
            );
            seen.insert(state);
        }
        assert!(seen.len() > 1, "a noise provider that never varies: {provider}");
    }
}

#[test]
fn rotated_provider_is_unsupported_not_invented() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let json = feature_json(
        r#"{"type":"minecraft:rotated_block_provider","state":{"Name":"minecraft:oak_log"}}"#,
    );
    let outcome = place_in_flat(&env, &json, 1, BlockPos::new(2, 70, 2), |_| {});
    assert!(outcome.writes.is_empty());
    assert_eq!(outcome.tally.get("provider:rotated_block_provider"), 1);
}

const COAL: &str = r#"{"feature":{"type":"minecraft:ore","config":{"size":9,
    "discard_chance_on_air_exposure":0.0,"targets":[
      {"state":{"Name":"minecraft:coal_ore"},
       "target":{"predicate_type":"minecraft:tag_match","tag":"minecraft:stone_ore_replaceables"}},
      {"state":{"Name":"minecraft:coal_ore"},
       "target":{"predicate_type":"minecraft:block_match","block":"minecraft:deepslate"}}]}},
    "placement":[]}"#;

#[test]
fn an_ore_blob_replaces_only_its_targets_inside_a_small_neighbourhood() {
    let env = env(
        &[("stone_ore_replaceables", r#"{"values":["minecraft:stone"]}"#)],
        Box::new(NoBlockBehaviours),
    );
    let origin = BlockPos::new(8, 20, 8);
    let outcome = place_in_flat(&env, COAL, 77, origin, |_| {});
    assert!(outcome.placed);
    assert!(!outcome.writes.is_empty());
    for (pos, state) in &outcome.writes {
        assert_eq!(*state, block("coal_ore"));
        assert!((pos.x - origin.x).abs() <= 6 && (pos.z - origin.z).abs() <= 6, "{pos:?}");
        assert!((pos.y - origin.y).abs() <= 6, "{pos:?}");
    }
    // The same seed gives the same blob; another seed gives another.
    let again = place_in_flat(&env, COAL, 77, origin, |_| {});
    assert_eq!(outcome.writes, again.writes);
    let other = place_in_flat(&env, COAL, 78, origin, |_| {});
    assert_ne!(outcome.writes, other.writes);
}

#[test]
fn an_ore_blob_in_open_air_places_nothing() {
    let env = env(
        &[("stone_ore_replaceables", r#"{"values":["minecraft:stone"]}"#)],
        Box::new(NoBlockBehaviours),
    );
    // Far above the surface there is no ground for the height scan to find under the blob.
    let outcome = place_in_flat(&env, COAL, 77, BlockPos::new(8, 200, 8), |_| {});
    assert!(outcome.writes.is_empty());
}

#[test]
fn air_exposure_discard_keeps_ore_out_of_cave_walls() {
    let env = env(
        &[("stone_ore_replaceables", r#"{"values":["minecraft:stone"]}"#)],
        Box::new(NoBlockBehaviours),
    );
    let json = COAL.replace("\"discard_chance_on_air_exposure\":0.0", "\"discard_chance_on_air_exposure\":1.0");
    let origin = BlockPos::new(8, 62, 8);
    // Chance 1 refuses every block touching air, so only blocks at least two deep survive.
    let outcome = place_in_flat(&env, &json, 77, origin, |_| {});
    for (pos, _) in &outcome.writes {
        assert!(pos.y <= 62, "an ore block on the exposed surface: {pos:?}");
    }
}

#[test]
fn scattered_ore_places_single_blocks_near_the_origin() {
    let env = env(
        &[("stone_ore_replaceables", r#"{"values":["minecraft:stone"]}"#)],
        Box::new(NoBlockBehaviours),
    );
    let json = r#"{"feature":{"type":"minecraft:scattered_ore","config":{"size":20,
        "discard_chance_on_air_exposure":0.0,"targets":[
          {"state":{"Name":"minecraft:coal_ore"},
           "target":{"predicate_type":"minecraft:tag_match","tag":"minecraft:stone_ore_replaceables"}}]}},
        "placement":[]}"#;
    let origin = BlockPos::new(8, 30, 8);
    let outcome = place_in_flat(&env, json, 3, origin, |_| {});
    assert!(!outcome.writes.is_empty());
    for (pos, _) in &outcome.writes {
        assert!((pos.x - origin.x).abs() <= 7 && (pos.y - origin.y).abs() <= 7 && (pos.z - origin.z).abs() <= 7);
    }
}

const SPRING: &str = r#"{"feature":{"type":"minecraft:spring_feature","config":{
    "state":{"Name":"minecraft:water","Properties":{"falling":"true"}},
    "valid_blocks":["minecraft:stone"]}},"placement":[]}"#;

#[test]
fn a_spring_needs_four_rocks_and_one_hole() {
    let env = env(&[], Box::new(NoBlockBehaviours));
    let origin = BlockPos::new(8, 30, 8);

    // Solid stone: no hole, so no spring.
    assert!(place_in_flat(&env, SPRING, 1, origin, |_| {}).writes.is_empty());

    // One air neighbour: the cavity vanilla wants.
    let outcome = place_in_flat(&env, SPRING, 1, origin, |region| {
        region.set_block(BlockPos::new(7, 30, 8), block("air"));
    });
    assert!(outcome.placed);
    assert_eq!(
        outcome.writes,
        vec![
            (BlockPos::new(7, 30, 8), block("air")),
            (origin, block("water").with_property("level", "8")),
        ]
    );

    // Two air neighbours is one hole too many.
    let crowded = place_in_flat(&env, SPRING, 1, origin, |region| {
        region.set_block(BlockPos::new(7, 30, 8), block("air"));
        region.set_block(BlockPos::new(9, 30, 8), block("air"));
    });
    assert!(crowded.writes.iter().all(|(_, state)| *state == block("air")), "no spring");
}

#[test]
fn selectors_choose_with_vanillas_draws() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let log = r#"{"feature":{"type":"minecraft:simple_block","config":{"to_place":
        {"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:oak_log"}}}},"placement":[]}"#;
    let stone = log.replace("oak_log", "stone");
    let boolean = format!(
        r#"{{"feature":{{"type":"minecraft:random_boolean_selector","config":{{
            "feature_true":{log},"feature_false":{stone}}}}},"placement":[]}}"#
    );
    let simple = format!(
        r#"{{"feature":{{"type":"minecraft:simple_random_selector","config":{{
            "features":[{log},{stone}]}}}},"placement":[]}}"#
    );
    let chance = format!(
        r#"{{"feature":{{"type":"minecraft:random_selector","config":{{
            "features":[{{"feature":{log},"chance":0.5}}],"default":{stone}}}}},"placement":[]}}"#
    );
    for seed in 0..30 {
        let at = BlockPos::new(2, 70, 2);
        let wrote = |json: &str| place_in_flat(&env, json, seed, at, |_| {}).writes[0].1.clone();

        let want = if WorldgenRandom::new(seed).next_boolean() { block("oak_log") } else { block("stone") };
        assert_eq!(wrote(&boolean), want, "boolean, seed {seed}");

        let want = if WorldgenRandom::new(seed).next_int_bounded(2) == 0 { block("oak_log") } else { block("stone") };
        assert_eq!(wrote(&simple), want, "simple, seed {seed}");

        let want = if WorldgenRandom::new(seed).next_float() < 0.5 { block("oak_log") } else { block("stone") };
        assert_eq!(wrote(&chance), want, "chance, seed {seed}");
    }
}

#[test]
fn an_unsupported_feature_places_nothing_and_is_tallied() {
    let env = env(&[], Box::new(AlwaysSurvives));
    // A fossil compiled without a datapack tree to read its templates from.
    let json = r#"{"feature":{"type":"minecraft:fossil","config":{"fossil_processors":"minecraft:fossil_rot",
        "fossil_structures":["minecraft:fossil/spine_1"],"max_empty_corners_allowed":4,
        "overlay_processors":"minecraft:fossil_coal","overlay_structures":["minecraft:fossil/spine_1_coal"]}},
        "placement":[{"type":"minecraft:count","count":3}]}"#;
    let outcome = place_in_flat(&env, json, 1, BlockPos::new(4, 64, 4), |_| {});
    assert!(!outcome.placed && outcome.writes.is_empty());
    assert_eq!(outcome.tally.get("feature:minecraft:fossil (no datapack tree)"), 3);
}

#[test]
fn a_nested_feature_outside_the_writable_chunks_is_refused() {
    let env = env(&[], Box::new(AlwaysSurvives));
    // Two chunks east of the centre: the region may not write there.
    let outcome = place_in_flat(&env, &with_chain(""), 1, BlockPos::new(40, 70, 4), |_| {});
    assert!(!outcome.placed && outcome.writes.is_empty());
}

/// The overworld's possible biomes in the order the parameter list first mentions them.
fn overworld_biomes() -> Vec<ResourceLocation> {
    let entries = oxide_datapack::preset_entries(&ResourceLocation::minecraft("overworld"))
        .expect("overworld preset");
    let mut biomes = Vec::new();
    for entry in entries {
        if !biomes.contains(&entry.biome) {
            biomes.push(entry.biome);
        }
    }
    biomes
}

#[test]
fn a_decorator_built_from_the_reference_tree_writes_ores_deterministically() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference");
    if !root.join("data/minecraft/worldgen/placed_feature").is_dir() || !root.join("block_facts.tsv").is_file() {
        eprintln!("skipping: no extracted vanilla export at {}", root.display());
        return;
    }
    let datapack = oxide_datapack::load_datapack(&root).expect("reference datapack loads");
    let biomes = overworld_biomes();
    let decorator = BiomeDecorator::new(&datapack, &biomes, 4242, 63).expect("decorator builds");

    let writes_for = |decorator: &BiomeDecorator| -> Vec<BlockWrite> {
        let center = ChunkPos::new(3, -2);
        let chunks = flat_ring(center, 64);
        let mut region = DecorationRegion::new(center, refs(&chunks), decorator.facts());
        decorator.decorate(&mut region);
        region.into_writes()
    };
    let first = writes_for(&decorator);
    let second = writes_for(&decorator);
    assert_eq!(first, second, "the same seed and terrain must give identical writes");

    let ores = first
        .iter()
        .filter(|(_, state)| state.name.path().ends_with("_ore"))
        .count();
    assert!(ores > 0, "no ore among {} writes", first.len());
    let lowest = first.iter().map(|(pos, _)| pos.y).min().unwrap();
    assert!(lowest < 0, "deepslate-level ores expected, lowest write at y={lowest}");

    // Another world seed decorates the same terrain differently.
    let other = BiomeDecorator::new(&datapack, &biomes, 4243, 63).expect("decorator builds");
    assert_ne!(first, writes_for(&other));

    let tally = decorator.unsupported();
    eprintln!("{} writes, unsupported: {:?}", first.len(), tally.entries().collect::<Vec<_>>());
}

/// Facts for the two-block plants the tests below place, in the exporter's own format.
fn plant_facts() -> BlockFacts {
    let dir = std::env::temp_dir().join(format!("oxide-plant-facts-{}-{:?}", std::process::id(), std::thread::current().id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("block_facts.tsv"),
        "minecraft:air\tDarc\n\
         minecraft:stone\tDsmoudf1234\n\
         minecraft:water[level=0]\tDlrcWS134\n\
         minecraft:tall_grass[half=lower]\tDrc1\n\
         minecraft:tall_grass[half=upper]\trc1\n\
         minecraft:small_dripleaf[facing=north,half=lower,waterlogged=false]\tDrc1\n\
         minecraft:small_dripleaf[facing=north,half=lower,waterlogged=true]\trcWS1\n\
         minecraft:small_dripleaf[facing=north,half=upper,waterlogged=false]\trc1\n\
         minecraft:small_dripleaf[facing=north,half=upper,waterlogged=true]\trcWS1\n",
    )
    .unwrap();
    let facts = BlockFacts::load(&dir).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    facts
}

fn two_block_env() -> Environment {
    let mut env = env(&[], Box::new(AlwaysSurvives));
    env.facts = plant_facts();
    env
}

fn plant_json(state: &str) -> String {
    feature_json(&format!(r#"{{"type":"minecraft:simple_state_provider","state":{state}}}"#))
}

#[test]
fn a_two_block_plant_is_placed_as_both_halves() {
    let env = two_block_env();
    let json = plant_json(r#"{"Name":"minecraft:tall_grass","Properties":{"half":"lower"}}"#);
    let outcome = place_in_flat(&env, &json, 1, BlockPos::new(4, 64, 4), |_| {});
    assert!(outcome.placed);
    assert_eq!(
        outcome.writes,
        vec![
            (BlockPos::new(4, 64, 4), block("tall_grass").with_property("half", "lower")),
            (BlockPos::new(4, 65, 4), block("tall_grass").with_property("half", "upper")),
        ]
    );
}

#[test]
fn a_two_block_plant_needs_the_cell_above_empty() {
    let env = two_block_env();
    let json = plant_json(r#"{"Name":"minecraft:tall_grass","Properties":{"half":"lower"}}"#);
    let outcome = place_in_flat(&env, &json, 1, BlockPos::new(4, 64, 4), |region| {
        region.set_block(BlockPos::new(4, 65, 4), block("stone"));
    });
    assert!(!outcome.placed);
    assert_eq!(outcome.writes, vec![(BlockPos::new(4, 65, 4), block("stone"))], "only the test's own block");
}

#[test]
fn each_half_copies_its_waterlogging_from_the_water_already_there() {
    let env = two_block_env();
    let json = plant_json(
        r#"{"Name":"minecraft:small_dripleaf","Properties":{"half":"lower","facing":"north","waterlogged":"false"}}"#,
    );
    let outcome = place_in_flat(&env, &json, 1, BlockPos::new(4, 64, 4), |region| {
        region.set_block(BlockPos::new(4, 64, 4), block("water").with_property("level", "0"));
    });
    assert!(outcome.placed);
    let dripleaf = |half: &str, waterlogged: &str| {
        block("small_dripleaf")
            .with_property("facing", "north")
            .with_property("half", half)
            .with_property("waterlogged", waterlogged)
    };
    let written: HashMap<_, _> = outcome.writes.into_iter().collect();
    assert_eq!(written[&BlockPos::new(4, 64, 4)], dripleaf("lower", "true"));
    assert_eq!(written[&BlockPos::new(4, 65, 4)], dripleaf("upper", "false"));
}

/// A decorator built from the real 26.2 export, on grass, no longer stops at "survival unknown"
/// for plants whose rule is ported.
#[test]
fn reference_decoration_on_grass_places_short_grass() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference");
    if !root.join("data/minecraft/worldgen/placed_feature").is_dir() || !root.join("block_facts.tsv").is_file() {
        eprintln!("skipping: no extracted vanilla export at {}", root.display());
        return;
    }
    let datapack = oxide_datapack::load_datapack(&root).expect("reference datapack loads");
    let decorator = BiomeDecorator::new(&datapack, &overworld_biomes(), 4242, 63).expect("decorator builds");
    let center = ChunkPos::new(3, -2);
    let mut chunks = flat_ring(center, 64);
    for chunk in &mut chunks {
        let top = &mut chunk.sections[(63 - chunk.min_y) as usize >> 4];
        for z in 0..16 {
            for x in 0..16 {
                let slot = crate::fill::local_index(x, ((63 - chunk.min_y) & 15) as usize, z);
                top.block_states.set(slot, block("grass_block").with_property("snowy", "false"));
            }
        }
    }
    let mut region = DecorationRegion::new(center, refs(&chunks), decorator.facts());
    decorator.decorate(&mut region);
    let writes = region.into_writes();
    let grass = writes.iter().filter(|(_, state)| state.name.path() == "short_grass").count();
    let tally = decorator.unsupported();
    assert_eq!(tally.get("survival:minecraft:short_grass"), 0, "{:?}", tally.entries().collect::<Vec<_>>());
    assert!(grass > 0, "no short_grass among {} writes", writes.len());
}
