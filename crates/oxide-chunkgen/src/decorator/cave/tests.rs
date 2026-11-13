//! Tests for the cave features, on synthetic terrain with fixed seeds: a stone mass with an air
//! pocket carved into it. They check what each places, that the same seed gives the same writes,
//! that nothing lands outside the region, and that every 26.2 configured feature of these types
//! compiles when the reference export is present.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use oxide_core::{BlockPos, BlockState, ChunkPos, ResourceLocation, Xoroshiro128PlusPlus};
use oxide_datapack::placement::PlacedFeature as PlacedFeatureIr;
use oxide_datapack::{load_tags, BlockFacts, Registry, TagSet};

use super::super::*;
use crate::region::testutil::{block, flat_ring, refs};
use crate::region::{BlockWrite, DecorationRegion};

const SOLID: &str = "Dsmoudf1234";

/// Facts for the blocks these tests touch, in the exporter's format: every state a feature may
/// look up or build, since the table is exact.
fn facts() -> BlockFacts {
    let dir = std::env::temp_dir().join(format!("oxide-cave-facts-{}-{:?}", std::process::id(), std::thread::current().id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut text = String::from(
        "minecraft:air\tDarc\nminecraft:cave_air\tDarc\nminecraft:void_air\tDarc\n\
         minecraft:water[level=0]\tDlcWS\n\
         minecraft:pointed_dripstone[thickness=tip,vertical_direction=up,waterlogged=false]\tDsm\n\
         minecraft:sculk_catalyst[bloom=false]\tDsmoudf\n\
         minecraft:sculk_sensor[power=0,sculk_sensor_phase=inactive,waterlogged=false]\tDsm\n\
         minecraft:sculk_shrieker[can_summon=false,shrieking=false,waterlogged=false]\tDsm\n",
    );
    for name in [
        "stone", "deepslate", "dripstone_block", "sulfur", "calcite", "smooth_basalt", "amethyst_block",
        "budding_amethyst", "bedrock", "sculk",
    ] {
        writeln!(text, "minecraft:{name}\t{SOLID}").unwrap();
    }
    for thickness in ["tip_merge", "tip", "frustum", "middle", "base"] {
        for direction in ["up", "down"] {
            for water in ["true", "false"] {
                writeln!(
                    text,
                    "minecraft:pointed_dripstone[thickness={thickness},vertical_direction={direction},waterlogged={water}]\tsm"
                )
                .unwrap();
            }
        }
    }
    for bud in ["small_amethyst_bud", "medium_amethyst_bud", "large_amethyst_bud", "amethyst_cluster"] {
        for facing in ["down", "up", "north", "south", "west", "east"] {
            for water in ["true", "false"] {
                let default = if facing == "up" && water == "false" { "D" } else { "" };
                writeln!(text, "minecraft:{bud}[facing={facing},waterlogged={water}]\t{default}sm").unwrap();
            }
        }
    }
    for bits in 0..128u32 {
        let faces = ["down", "up", "north", "south", "west", "east", "waterlogged"];
        let props: Vec<String> =
            faces.iter().enumerate().map(|(index, face)| format!("{face}={}", bits >> index & 1 == 1)).collect();
        let default = if bits == 0 { "D" } else { "" };
        writeln!(text, "minecraft:sculk_vein[{}]\t{default}c", props.join(",")).unwrap();
    }
    std::fs::write(dir.join("block_facts.tsv"), text).unwrap();
    let facts = BlockFacts::load(&dir).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    facts
}

fn tags(entries: &[(&str, &[&str])]) -> TagSet {
    let root = std::env::temp_dir().join(format!("oxide-cave-tags-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&root);
    for (name, values) in entries {
        let path = root.join("data/minecraft/tags/block").join(format!("{name}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let list: Vec<String> = values.iter().map(|value| format!("\"minecraft:{value}\"")).collect();
        std::fs::write(path, format!("{{\"values\":[{}]}}", list.join(","))).unwrap();
    }
    let set = load_tags(&[&root], "block").unwrap();
    let _ = std::fs::remove_dir_all(&root);
    set
}

fn env() -> Environment {
    Environment {
        block_tags: tags(&[
            ("dripstone_replaceable_blocks", &["stone", "deepslate"]),
            ("base_stone_overworld", &["stone", "deepslate"]),
            ("features_cannot_replace", &["bedrock"]),
            ("geode_invalid_blocks", &["bedrock", "water"]),
            ("sculk_replaceable", &["stone", "deepslate"]),
            ("sculk_replaceable_world_gen", &["stone", "deepslate", "sculk"]),
            ("fire", &[]),
        ]),
        fluid_tags: TagSet::default(),
        biome_tags: TagSet::default(),
        facts: facts(),
        seed: 4242,
        sea_level: 63,
        behaviours: Box::new(NoBlockBehaviours),
        placed: HashMap::new(),
        biome_climate: HashMap::new(),
        biome_features: HashMap::new(),
    }
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

    /// The last write at `(x, y, z)`.
    fn at(&self, x: i32, y: i32, z: i32) -> Option<&BlockState> {
        self.writes.iter().rev().find(|(pos, _)| *pos == BlockPos::new(x, y, z)).map(|(_, state)| state)
    }

    fn only(&self, allowed: &[&str]) -> bool {
        self.writes.iter().all(|(_, state)| allowed.contains(&state.name.path()))
    }
}

/// Places the configured feature `config` at `origin` in a flat stone world (stone below y = 64,
/// chunk (0, 0)) seeded with `seed`, after `prepare` has edited the region. The outcome's writes
/// include what `prepare` wrote.
fn place(env: &Environment, config: &str, seed: i64, origin: BlockPos, prepare: impl FnOnce(&mut DecorationRegion<'_>)) -> Outcome {
    let ir: PlacedFeatureIr = serde_json::from_str(&format!(r#"{{"feature":{config},"placement":[]}}"#)).expect("json");
    let (placed_registry, configured) = (Registry::default(), Registry::default());
    let placed = Compiler::new(&env.facts, &placed_registry, &configured).compile_placed(&ir).expect("compiles");
    let chunks = flat_ring(ChunkPos::new(0, 0), 64);
    let mut region = DecorationRegion::new(ChunkPos::new(0, 0), refs(&chunks), &env.facts);
    prepare(&mut region);
    let mut random = Xoroshiro128PlusPlus::new(seed);
    let mut tally = UnsupportedTally::default();
    let did_place = {
        let mut cx = FeatureContext { env, region: &mut region, random: &mut random, origin, unsupported: &mut tally, top_feature: None };
        place_placed(&placed, &mut cx, origin, None)
    };
    Outcome { writes: region.into_writes(), tally, placed: did_place }
}

/// Fills the box `[x0, x1) x [y0, y1) x [z0, z1)` with the block `path`.
fn fill(region: &mut DecorationRegion<'_>, x: (i32, i32), y: (i32, i32), z: (i32, i32), path: &str) {
    for px in x.0..x.1 {
        for py in y.0..y.1 {
            for pz in z.0..z.1 {
                assert!(region.set_block(BlockPos::new(px, py, pz), block(path)));
            }
        }
    }
}

/// A stone mass `y` in `[60, 90)` over the window's middle with a pocket of air `y` in `[floor,
/// ceiling)` across `[0, 16)` on both horizontal axes.
fn cave(region: &mut DecorationRegion<'_>, floor: i32, ceiling: i32) {
    fill(region, (-12, 28), (60, 90), (-12, 28), "stone");
    fill(region, (0, 16), (floor, ceiling), (0, 16), "air");
}

const POINTED: &str = r#"{"Name":"minecraft:pointed_dripstone","Properties":{"thickness":"tip","vertical_direction":"up","waterlogged":"false"}}"#;

fn speleothem() -> String {
    format!(
        r##"{{"type":"minecraft:speleothem","config":{{"base_block":{{"Name":"minecraft:dripstone_block"}},
        "pointed_block":{POINTED},"replaceable_blocks":"#minecraft:dripstone_replaceable_blocks"}}}}"##
    )
}

fn cluster() -> String {
    format!(
        r##"{{"type":"minecraft:speleothem_cluster","config":{{"base_block":{{"Name":"minecraft:dripstone_block"}},
        "pointed_block":{POINTED},"replaceable_blocks":"#minecraft:dripstone_replaceable_blocks",
        "floor_to_ceiling_search_range":12,"height":{{"type":"minecraft:uniform","min_inclusive":2,"max_inclusive":4}},
        "radius":{{"type":"minecraft:uniform","min_inclusive":2,"max_inclusive":4}},
        "max_stalagmite_stalactite_height_diff":1,"height_deviation":3,
        "speleothem_block_layer_thickness":{{"type":"minecraft:uniform","min_inclusive":1,"max_inclusive":2}},
        "density":{{"type":"minecraft:uniform","min_inclusive":0.4,"max_exclusive":0.9}},"wetness":0.0,
        "chance_of_speleothem_at_max_distance_from_center":0.2,
        "max_distance_from_edge_affecting_chance_of_speleothem":3,
        "max_distance_from_center_affecting_height_bias":6}}}}"##
    )
}

fn large() -> String {
    r##"{"type":"minecraft:large_dripstone","config":{
        "column_radius":{"type":"minecraft:uniform","min_inclusive":3,"max_inclusive":8},
        "height_scale":{"type":"minecraft:uniform","min_inclusive":0.4,"max_exclusive":2.0},
        "max_column_radius_to_cave_height_ratio":0.33,"min_bluntness_for_wind":0.6,"min_radius_for_wind":4,
        "replaceable_blocks":"#minecraft:dripstone_replaceable_blocks",
        "stalactite_bluntness":{"type":"minecraft:uniform","min_inclusive":0.3,"max_exclusive":0.9},
        "stalagmite_bluntness":{"type":"minecraft:uniform","min_inclusive":0.4,"max_exclusive":1.0},
        "wind_speed":{"type":"minecraft:uniform","min_inclusive":0.0,"max_exclusive":0.3}}}"##
        .to_string()
}

fn simple(path: &str) -> String {
    format!(r#"{{"type":"minecraft:simple_state_provider","state":{{"Name":"minecraft:{path}"}}}}"#)
}

fn geode() -> String {
    format!(
        r##"{{"type":"minecraft:geode","config":{{"blocks":{{"filling_provider":{},"inner_layer_provider":{},
        "alternate_inner_layer_provider":{},"middle_layer_provider":{},"outer_layer_provider":{},
        "inner_placements":[{{"Name":"minecraft:small_amethyst_bud","Properties":{{"facing":"up","waterlogged":"false"}}}},
        {{"Name":"minecraft:amethyst_cluster","Properties":{{"facing":"up","waterlogged":"false"}}}}],
        "cannot_replace":"#minecraft:features_cannot_replace","invalid_blocks":"#minecraft:geode_invalid_blocks"}},
        "crack":{{"generate_crack_chance":0.9}},"invalid_blocks_threshold":1,"layers":{{}},
        "outer_wall_distance":{{"type":"minecraft:uniform","min_inclusive":4,"max_inclusive":6}},
        "use_alternate_layer0_chance":0.5}}}}"##,
        simple("air"),
        simple("amethyst_block"),
        simple("budding_amethyst"),
        simple("calcite"),
        simple("smooth_basalt"),
    )
}

fn sculk() -> String {
    r#"{"type":"minecraft:sculk_patch","config":{"amount_per_charge":32,"catalyst_chance":0.5,"charge_count":10,
        "extra_rare_growths":{"type":"minecraft:uniform","min_inclusive":1,"max_inclusive":3},
        "growth_rounds":0,"spread_attempts":64,"spread_rounds":1}}"#
        .to_string()
}

/// Every write lies inside the nine-chunk window around chunk (0, 0).
fn inside_window(outcome: &Outcome) -> bool {
    outcome.writes.iter().all(|(pos, _)| (-16..32).contains(&pos.x) && (-16..32).contains(&pos.z))
}

fn prop<'a>(state: &'a BlockState, key: &str) -> &'a str {
    state.properties.get(key).map(String::as_str).unwrap_or("")
}

#[test]
fn a_speleothem_hangs_from_a_ceiling_and_grows_from_a_floor() {
    let env = env();
    // Ceiling at y = 74: the origin is the air cell under it.
    let down = place(&env, &speleothem(), 3, BlockPos::new(8, 73, 8), |region| cave(region, 66, 74));
    assert!(down.placed);
    let tip = down.at(8, 73, 8).expect("a pointed block at the origin");
    assert_eq!(tip.name.path(), "pointed_dripstone");
    assert_eq!(prop(tip, "vertical_direction"), "down");
    assert_eq!(down.at(8, 74, 8).map(|state| state.name.path()), Some("dripstone_block"));
    assert!(down.only(&["stone", "air", "dripstone_block", "pointed_dripstone"]));
    assert!(inside_window(&down));

    // Floor top at y = 65: the origin is the air cell on it.
    let up = place(&env, &speleothem(), 3, BlockPos::new(8, 66, 8), |region| cave(region, 66, 74));
    assert!(up.placed);
    assert_eq!(prop(up.at(8, 66, 8).expect("a pointed block"), "vertical_direction"), "up");
    assert_eq!(up.at(8, 65, 8).map(|state| state.name.path()), Some("dripstone_block"));
}

#[test]
fn a_speleothem_needs_a_base_and_is_waterlogged_in_water() {
    let env = env();
    let floating = place(&env, &speleothem(), 3, BlockPos::new(8, 70, 8), |region| cave(region, 66, 74));
    assert!(!floating.placed);
    assert_eq!(floating.count("pointed_dripstone"), 0);
    assert_eq!(floating.count("dripstone_block"), 0);

    let wet = place(&env, &speleothem(), 3, BlockPos::new(8, 73, 8), |region| {
        cave(region, 66, 74);
        region.set_block(BlockPos::new(8, 73, 8), block("water").with_property("level", "0"));
    });
    assert!(wet.placed);
    assert_eq!(prop(wet.at(8, 73, 8).unwrap(), "waterlogged"), "true");
}

#[test]
fn a_speleothem_is_deterministic_per_seed() {
    let env = env();
    let run = |seed| place(&env, &speleothem(), seed, BlockPos::new(8, 73, 8), |region| cave(region, 66, 74)).writes;
    assert_eq!(run(11), run(11));
    assert!((1..40).any(|seed| run(seed) != run(11)), "the seed never changes the result");
}

#[test]
fn a_cluster_fills_a_pocket_with_formations() {
    let env = env();
    let outcome = place(&env, &cluster(), 5, BlockPos::new(8, 70, 8), |region| cave(region, 64, 76));
    assert!(outcome.placed);
    assert!(outcome.count("pointed_dripstone") > 0);
    assert!(outcome.count("dripstone_block") > 0);
    assert!(outcome.only(&["stone", "air", "dripstone_block", "pointed_dripstone", "water"]));
    assert!(inside_window(&outcome));
    // Stalactites hang, stalagmites rise; both occur in a pocket this tall.
    let directions: Vec<&str> =
        outcome.writes.iter().filter(|(_, state)| state.name.path() == "pointed_dripstone").map(|(_, state)| prop(state, "vertical_direction")).collect();
    assert!(directions.contains(&"up") && directions.contains(&"down"));
    // Nothing is written outside the pocket's rock.
    assert!(outcome.writes.iter().all(|(pos, _)| (60..90).contains(&pos.y)));
    let again = place(&env, &cluster(), 5, BlockPos::new(8, 70, 8), |region| cave(region, 64, 76));
    assert_eq!(outcome.writes, again.writes);
}

#[test]
fn a_cluster_refuses_an_origin_in_rock() {
    let env = env();
    let outcome = place(&env, &cluster(), 5, BlockPos::new(8, 62, 8), |region| cave(region, 64, 76));
    assert!(!outcome.placed);
    assert_eq!(outcome.count("pointed_dripstone"), 0);
}

#[test]
fn large_dripstone_spans_a_cave_with_dripstone_blocks() {
    let env = env();
    let outcome = place(&env, &large(), 9, BlockPos::new(8, 70, 8), |region| cave(region, 64, 78));
    assert!(outcome.placed);
    let written: Vec<&BlockWrite> =
        outcome.writes.iter().filter(|(_, state)| state.name.path() == "dripstone_block").collect();
    assert!(!written.is_empty());
    assert!(written.iter().any(|(pos, _)| pos.y >= 64 && pos.y < 70), "no stalagmite");
    assert!(written.iter().any(|(pos, _)| pos.y > 70 && pos.y < 78), "no stalactite");
    assert!(outcome.only(&["stone", "air", "dripstone_block"]));
    assert!(inside_window(&outcome));
    let again = place(&env, &large(), 9, BlockPos::new(8, 70, 8), |region| cave(region, 64, 78));
    assert_eq!(outcome.writes, again.writes);
}

#[test]
fn large_dripstone_needs_four_blocks_of_cave() {
    let env = env();
    let outcome = place(&env, &large(), 9, BlockPos::new(8, 65, 8), |region| cave(region, 64, 67));
    assert!(!outcome.placed);
    assert_eq!(outcome.count("dripstone_block"), 0);
}

#[test]
fn a_geode_layers_its_blocks_inside_solid_rock() {
    let env = env();
    let outcome = place(&env, &geode(), 7, BlockPos::new(8, 40, 8), |_| {});
    assert!(outcome.placed);
    for layer in ["amethyst_block", "calcite", "smooth_basalt", "air"] {
        assert!(outcome.count(layer) > 0, "no {layer}");
    }
    assert!(outcome.only(&[
        "air", "amethyst_block", "budding_amethyst", "calcite", "smooth_basalt", "small_amethyst_bud",
        "amethyst_cluster",
    ]));
    assert!(inside_window(&outcome));
    // The shell encloses the hollow: calcite lies farther out than amethyst on average.
    let mean = |path: &str| {
        let hits: Vec<f64> = outcome
            .writes
            .iter()
            .filter(|(_, state)| state.name.path() == path)
            .map(|(pos, _)| f64::from(pos.x - 8).powi(2) + f64::from(pos.y - 40).powi(2) + f64::from(pos.z - 8).powi(2))
            .collect();
        hits.iter().sum::<f64>() / hits.len() as f64
    };
    assert!(mean("smooth_basalt") > mean("calcite") && mean("calcite") > mean("amethyst_block"));
    let again = place(&env, &geode(), 7, BlockPos::new(8, 40, 8), |_| {});
    assert_eq!(outcome.writes, again.writes);
}

#[test]
fn a_geode_is_refused_in_open_air_and_clipped_at_the_window() {
    let env = env();
    let in_air = place(&env, &geode(), 7, BlockPos::new(8, 70, 8), |_| {});
    assert!(!in_air.placed);
    assert!(in_air.writes.is_empty());

    // Near the window's edge the geode reaches past what the region may write.
    let edge = place(&env, &geode(), 7, BlockPos::new(-12, 40, 8), |_| {});
    assert!(edge.placed);
    assert!(inside_window(&edge));
}

#[test]
fn a_sculk_patch_spreads_over_a_cave_floor() {
    let env = env();
    let run = |seed| place(&env, &sculk(), seed, BlockPos::new(8, 64, 8), |region| cave(region, 64, 70));
    let outcome = (1..30).map(run).find(|outcome| outcome.placed && outcome.count("sculk") > 0).expect("sculk grows for some seed");
    assert!(outcome.count("sculk_vein") > 0);
    assert!(outcome.only(&["stone", "air", "sculk", "sculk_vein", "sculk_catalyst", "sculk_sensor", "sculk_shrieker", "water"]));
    assert!(inside_window(&outcome));
    // Sculk replaces rock only where the tag allows, so the floor stays under it.
    assert!(outcome.writes.iter().all(|(pos, state)| state.name.path() != "sculk" || pos.y <= 70));
}

#[test]
fn a_sculk_patch_is_deterministic_and_needs_a_surface() {
    let env = env();
    let first = place(&env, &sculk(), 12, BlockPos::new(8, 64, 8), |region| cave(region, 64, 70));
    let second = place(&env, &sculk(), 12, BlockPos::new(8, 64, 8), |region| cave(region, 64, 70));
    assert_eq!(first.writes, second.writes);
    assert_eq!(first.placed, second.placed);

    // Floating in the middle of a cave nothing is within reach to spread from.
    let floating = place(&env, &sculk(), 12, BlockPos::new(8, 67, 8), |region| cave(region, 64, 70));
    assert!(!floating.placed);
    // Inside solid rock there is no air to start in.
    let buried = place(&env, &sculk(), 12, BlockPos::new(8, 40, 8), |_| {});
    assert!(!buried.placed);
}

#[test]
fn fossils_stay_unsupported_and_are_tallied() {
    let env = env();
    let config = r#"{"type":"minecraft:fossil","config":{"fossil_processors":"minecraft:fossil_rot",
        "fossil_structures":["minecraft:fossil/spine_1"],"max_empty_corners_allowed":4,
        "overlay_processors":"minecraft:fossil_coal","overlay_structures":["minecraft:fossil/spine_1_coal"]}}"#;
    let outcome = place(&env, config, 1, BlockPos::new(8, 40, 8), |_| {});
    assert!(!outcome.placed);
    assert_eq!(outcome.tally.get("feature:minecraft:fossil"), 1);
    assert!(outcome.writes.is_empty());
}

/// Every 26.2 configured feature of the cave types compiles, and each type but the fossil
/// compiles to something that places.
#[test]
fn every_reference_cave_feature_compiles() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference");
    if !root.join("data/minecraft/worldgen/configured_feature").is_dir() || !root.join("block_facts.tsv").is_file() {
        eprintln!("skipping: no extracted vanilla export at {}", root.display());
        return;
    }
    let datapack = oxide_datapack::load_datapack(&root).expect("reference datapack loads");
    const TYPES: [&str; 6] = ["speleothem", "speleothem_cluster", "large_dripstone", "geode", "sculk_patch", "fossil"];
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
            (Feature::Unsupported(_), "fossil") => {}
            (Feature::Unsupported(why), _) => panic!("{id} ({kind}) compiled to unsupported: {why}"),
            _ => {}
        }
        compiled += 1;
    }
    // Two fossils, a geode, two sculk patches, a large dripstone and the two cluster configs.
    assert!(compiled >= 8, "only {compiled} cave features found");

    // The selectors that wrap a speleothem (`pointed_dripstone`, `sulfur_spike`) compile to
    // cave features, not to unsupported entries.
    for name in ["pointed_dripstone", "sulfur_spike"] {
        let id = ResourceLocation::minecraft(name);
        let wrapper: PlacedFeatureIr =
            serde_json::from_value(serde_json::json!({ "feature": id.to_string(), "placement": [] })).unwrap();
        let placed = Compiler::new(&datapack.block_facts, &datapack.placed_features, &datapack.configured_features)
            .compile_placed(&wrapper)
            .unwrap_or_else(|error| panic!("{id}: {error:#}"));
        let Feature::SimpleRandomSelector(entries) = &*placed.feature else {
            panic!("{id} is not a simple random selector");
        };
        assert!(entries.iter().all(|entry| matches!(&*entry.feature, Feature::Cave(_))), "{id} has an unsupported entry");
    }
}
