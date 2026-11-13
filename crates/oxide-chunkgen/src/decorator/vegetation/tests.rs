//! Tests for the vegetation family on synthetic terrain: flat stone (surface at y=64) edited
//! per test, fixed seeds, and a `BlockBehaviours` that lets every block survive.

use std::collections::HashMap;
use std::path::Path;

use oxide_core::{BlockPos, BlockState, ChunkPos, RandomSource, Xoroshiro128PlusPlus};
use oxide_datapack::placement::PlacedFeature as PlacedFeatureIr;
use oxide_datapack::{load_tags, BlockFacts, Registry, TagSet};

use crate::decorator::*;
use crate::region::testutil::{block, flat_ring, refs};
use crate::region::{BlockWrite, DecorationRegion};

struct AlwaysSurvives;

impl BlockBehaviours for AlwaysSurvives {
    fn can_survive(&self, _: &BlockState, _: &DecorationRegion<'_>, _: BlockPos) -> Option<bool> {
        Some(true)
    }
}

const FULL: &str = "Dsmoudf1234";

fn facts() -> BlockFacts {
    let mut text = String::new();
    let mut line = |state: &str, letters: &str| text.push_str(&format!("{state}\t{letters}\n"));
    line("minecraft:air", "Darc");
    line("minecraft:cave_air", "arc");
    line("minecraft:void_air", "arc");
    for solid in ["stone", "moss_block", "clay", "rooted_dirt", "oak_log"] {
        line(&format!("minecraft:{solid}"), FULL);
    }
    line("minecraft:podzol[snowy=false]", FULL);
    line("minecraft:water[level=0]", "DlWS");
    line("minecraft:short_grass", "Drc1");
    line("minecraft:small_dripleaf[waterlogged=false]", "D");
    line("minecraft:hanging_roots[waterlogged=false]", "Dc");
    line("minecraft:bamboo[age=0,leaves=none,stage=0]", "D");
    let faces = |value: &str| {
        ["down", "east", "north", "south", "up", "west"]
            .iter()
            .map(|face| format!("{face}={value}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    line(&format!("minecraft:vine[{}]", faces("false")), "D");
    line(&format!("minecraft:glow_lichen[{},waterlogged=false]", faces("false")), "D");
    line(&format!("minecraft:sculk_vein[{},waterlogged=false]", faces("false")), "D");
    for cap in ["red_mushroom_block", "brown_mushroom_block"] {
        line(&format!("minecraft:{cap}[{}]", faces("true")), "D");
        line(
            &format!("minecraft:{cap}[down=false,east=true,north=true,south=true,up=true,west=true]"),
            "",
        );
    }
    line(&format!("minecraft:mushroom_stem[{}]", faces("true")), "D");
    line("minecraft:mushroom_stem[down=false,east=true,north=true,south=true,up=false,west=true]", "");
    let dir = std::env::temp_dir().join(format!("oxide-vegetation-facts-{}-{:?}", std::process::id(), std::thread::current().id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("block_facts.tsv"), text).expect("write facts");
    let facts = BlockFacts::load(&dir).expect("facts parse");
    let _ = std::fs::remove_dir_all(dir);
    facts
}

fn tag_set(tags: &[(&str, &[&str])]) -> TagSet {
    if tags.is_empty() {
        return TagSet::default();
    }
    let root = std::env::temp_dir().join(format!("oxide-vegetation-tags-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&root);
    for (name, values) in tags {
        let path = root.join("data/minecraft/tags/block").join(format!("{name}.json"));
        std::fs::create_dir_all(path.parent().expect("parent")).expect("tag dir");
        let list = values.iter().map(|v| format!("\"minecraft:{v}\"")).collect::<Vec<_>>().join(",");
        std::fs::write(path, format!("{{\"values\":[{list}]}}")).expect("write tag");
    }
    let set = load_tags(&[&root], "block").expect("tags load");
    let _ = std::fs::remove_dir_all(&root);
    set
}

fn env(tags: &[(&str, &[&str])], behaviours: Box<dyn BlockBehaviours>) -> Environment {
    Environment {
        block_tags: tag_set(tags),
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

fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
    let mut state = block(name);
    for (key, value) in properties {
        state.properties.insert((*key).into(), (*value).into());
    }
    state
}

/// Terrain edits a test makes before placing; they are not counted as the feature's writes.
struct Prep<'a, 'r> {
    region: &'a mut DecorationRegion<'r>,
    edits: HashMap<BlockPos, BlockState>,
}

impl Prep<'_, '_> {
    fn set(&mut self, x: i32, y: i32, z: i32, name: &str) {
        let (pos, state) = (BlockPos::new(x, y, z), block(name));
        self.region.set_block(pos, state.clone());
        self.edits.insert(pos, state);
    }

    fn fill(&mut self, x: (i32, i32), y: (i32, i32), z: (i32, i32), name: &str) {
        for xx in x.0..=x.1 {
            for yy in y.0..=y.1 {
                for zz in z.0..=z.1 {
                    self.set(xx, yy, zz, name);
                }
            }
        }
    }
}

struct Outcome {
    /// What the feature wrote, by position (last write wins), excluding terrain edits.
    writes: HashMap<BlockPos, BlockState>,
    tally: UnsupportedTally,
    placed: bool,
}

impl Outcome {
    fn at(&self, x: i32, y: i32, z: i32) -> Option<&BlockState> {
        self.writes.get(&BlockPos::new(x, y, z))
    }

    fn named(&self, name: &str) -> Vec<BlockPos> {
        let mut found: Vec<_> =
            self.writes.iter().filter(|(_, state)| state.name.path() == name).map(|(pos, _)| *pos).collect();
        found.sort();
        found
    }
}

fn place(
    env: &Environment,
    configured: &str,
    seed: i64,
    origin: BlockPos,
    surface: i32,
    prepare: impl FnOnce(&mut Prep<'_, '_>),
) -> Outcome {
    let json = format!(r#"{{"feature":{configured},"placement":[]}}"#);
    let ir: PlacedFeatureIr = serde_json::from_str(&json).expect("placed feature json");
    let (placed_registry, configured_registry) = (Registry::default(), Registry::default());
    let placed = Compiler::new(&env.facts, &placed_registry, &configured_registry)
        .compile_placed(&ir)
        .expect("compiles");
    let chunks = flat_ring(ChunkPos::new(0, 0), surface);
    let mut region = DecorationRegion::new(ChunkPos::new(0, 0), refs(&chunks), &env.facts);
    let mut prep = Prep { region: &mut region, edits: HashMap::new() };
    prepare(&mut prep);
    let edits = prep.edits;
    let mut random = Xoroshiro128PlusPlus::new(seed);
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
    let writes: Vec<BlockWrite> = region.into_writes();
    let writes = writes.into_iter().filter(|(pos, state)| edits.get(pos) != Some(state)).collect();
    Outcome { writes, tally, placed: placed_any }
}

fn assert_in_window(outcome: &Outcome) {
    for pos in outcome.writes.keys() {
        assert!((-16..32).contains(&pos.x) && (-16..32).contains(&pos.z), "write outside the region at {pos:?}");
    }
}

const VINES: &str = r#"{"type":"minecraft:vines","config":{}}"#;
const BAMBOO_PLAIN: &str = r#"{"type":"minecraft:bamboo","config":{"probability":0.0}}"#;
const BAMBOO_PODZOL: &str = r#"{"type":"minecraft:bamboo","config":{"probability":1.0}}"#;

#[test]
fn vines_cling_to_the_first_face_in_vanillas_order() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let outcome = place(&env, VINES, 1, BlockPos::new(0, 65, 0), 64, |prep| {
        prep.set(1, 65, 0, "stone");
        prep.set(0, 65, 1, "stone");
    });
    // South is tried before east, so the south wall wins.
    let vine = outcome.at(0, 65, 0).expect("vine placed");
    assert_eq!(vine.name.path(), "vine");
    assert_eq!(vine.properties.get("south").map(String::as_str), Some("true"));
    assert_eq!(vine.properties.values().filter(|v| *v == "true").count(), 1);
    assert_eq!(outcome.writes.len(), 1);
    assert!(outcome.placed);

    let hanging = place(&env, VINES, 1, BlockPos::new(0, 65, 0), 64, |prep| prep.set(0, 66, 0, "stone"));
    assert_eq!(hanging.at(0, 65, 0).and_then(|s| s.properties.get("up")).map(String::as_str), Some("true"));
}

#[test]
fn vines_need_air_and_something_to_hold() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let open = place(&env, VINES, 1, BlockPos::new(0, 66, 0), 64, |_| {});
    assert!(!open.placed && open.writes.is_empty());
    let buried = place(&env, VINES, 1, BlockPos::new(0, 62, 0), 64, |_| {});
    assert!(!buried.placed && buried.writes.is_empty());
}

#[test]
fn bamboo_builds_a_stalk_with_a_three_block_crown() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let outcome = place(&env, BAMBOO_PLAIN, 77, BlockPos::new(2, 64, 3), 64, |_| {});
    let mut mirror = Xoroshiro128PlusPlus::new(77);
    let height = mirror.next_int_bounded(12) + 5;
    let top = 64 + height;
    assert!(outcome.placed);
    assert_in_window(&outcome);
    for y in 64..top - 2 {
        assert_eq!(outcome.at(2, y, 3), Some(&state("bamboo", &[("age", "1"), ("leaves", "none"), ("stage", "0")])), "trunk at {y}");
    }
    assert_eq!(outcome.at(2, top, 3), Some(&state("bamboo", &[("age", "1"), ("leaves", "large"), ("stage", "1")])));
    assert_eq!(outcome.at(2, top - 1, 3), Some(&state("bamboo", &[("age", "1"), ("leaves", "large"), ("stage", "0")])));
    assert_eq!(outcome.at(2, top - 2, 3), Some(&state("bamboo", &[("age", "1"), ("leaves", "small"), ("stage", "0")])));
    assert!(outcome.named("podzol").is_empty());
    let again = place(&env, BAMBOO_PLAIN, 77, BlockPos::new(2, 64, 3), 64, |_| {});
    assert_eq!(outcome.writes, again.writes);
}

#[test]
fn bamboo_lays_a_podzol_disc_where_the_tag_allows() {
    let env = env(&[("beneath_bamboo_podzol_replaceable", &["stone"])], Box::new(AlwaysSurvives));
    let outcome = place(&env, BAMBOO_PODZOL, 5, BlockPos::new(0, 64, 0), 64, |_| {});
    let mut mirror = Xoroshiro128PlusPlus::new(5);
    mirror.next_int_bounded(12);
    mirror.next_float();
    let radius = mirror.next_int_bounded(4) + 1;
    for x in -radius..=radius {
        for z in -radius..=radius {
            let expected = (x * x + z * z <= radius * radius).then(|| block("podzol"));
            let podzol = state("podzol", &[("snowy", "false")]);
            let found = outcome.at(x, 63, z).cloned();
            assert_eq!(found, expected.map(|_| podzol), "podzol at ({x}, 63, {z}) with radius {radius}");
        }
    }
}

#[test]
fn bamboo_with_unknown_survival_is_tallied_and_places_nothing() {
    let env = env(&[], Box::new(NoBlockBehaviours));
    let outcome = place(&env, BAMBOO_PLAIN, 77, BlockPos::new(0, 64, 0), 64, |_| {});
    assert!(!outcome.placed && outcome.writes.is_empty());
    assert_eq!(outcome.tally.get("survival:minecraft:bamboo"), 1);
}

fn mushroom(kind: &str, radius: Option<i32>) -> String {
    let radius = radius.map(|r| format!(r#""foliage_radius":{r},"#)).unwrap_or_default();
    let cap = if kind == "red" { "red_mushroom_block" } else { "brown_mushroom_block" };
    format!(
        r#"{{"type":"minecraft:huge_{kind}_mushroom","config":{{{radius}
        "can_place_on":{{"type":"minecraft:matching_block_tag","tag":"minecraft:huge_{kind}_mushroom_can_place_on"}},
        "cap_provider":{{"type":"minecraft:simple_state_provider","state":{{"Name":"minecraft:{cap}","Properties":
            {{"down":"false","east":"true","north":"true","south":"true","up":"true","west":"true"}}}}}},
        "stem_provider":{{"type":"minecraft:simple_state_provider","state":{{"Name":"minecraft:mushroom_stem","Properties":
            {{"down":"false","east":"true","north":"true","south":"true","up":"false","west":"true"}}}}}}}}}}"#
    )
}

fn mushroom_height(seed: i64) -> i32 {
    let mut mirror = Xoroshiro128PlusPlus::new(seed);
    let mut height = mirror.next_int_bounded(3) + 4;
    if mirror.next_int_bounded(12) == 0 {
        height *= 2;
    }
    height
}

fn faces_set(state: &BlockState) -> Vec<&str> {
    ["down", "east", "north", "south", "up", "west"]
        .into_iter()
        .filter(|face| state.properties.get(*face).is_some_and(|v| v == "true"))
        .collect()
}

#[test]
fn huge_red_mushroom_caps_a_stem_with_a_domed_ring() {
    let env = env(&[("huge_red_mushroom_can_place_on", &["stone"])], Box::new(AlwaysSurvives));
    let json = mushroom("red", None);
    let seed = 9;
    let height = mushroom_height(seed);
    let outcome = place(&env, &json, seed, BlockPos::new(0, 64, 0), 64, |_| {});
    assert!(outcome.placed);
    assert_in_window(&outcome);
    for y in 64..64 + height {
        assert_eq!(outcome.at(0, y, 0).map(|s| s.name.path()), Some("mushroom_stem"), "stem at {y}");
    }
    assert_eq!(outcome.at(0, 64 + height, 0).map(faces_set), Some(vec!["up"]));
    // The top layer is a 3x3 plate, the layers under it hollow rings of radius 2.
    assert!(outcome.at(1, 64 + height, 1).is_some());
    assert!(outcome.at(2, 64 + height, 0).is_none());
    assert_eq!(outcome.at(2, 64 + height - 1, 0).map(|s| s.name.path()), Some("red_mushroom_block"));
    assert!(outcome.at(2, 64 + height - 1, 2).is_none(), "corners of a ring are empty");
    for pos in outcome.writes.keys() {
        assert!(pos.x.abs() <= 2 && pos.z.abs() <= 2 && (64..=64 + height).contains(&pos.y));
    }
    let again = place(&env, &json, seed, BlockPos::new(0, 64, 0), 64, |_| {});
    assert_eq!(outcome.writes, again.writes);
}

#[test]
fn huge_brown_mushroom_caps_a_flat_plate_with_outward_faces() {
    let env = env(&[("huge_brown_mushroom_can_place_on", &["stone"])], Box::new(AlwaysSurvives));
    let json = mushroom("brown", Some(3));
    let seed = 4;
    let height = mushroom_height(seed);
    let outcome = place(&env, &json, seed, BlockPos::new(0, 64, 0), 64, |_| {});
    assert!(outcome.placed);
    assert_in_window(&outcome);
    let top = 64 + height;
    assert!(outcome.at(-3, top, -3).is_none(), "plate corners are empty");
    assert_eq!(outcome.at(-3, top, 0).map(faces_set), Some(vec!["up", "west"]));
    assert_eq!(outcome.at(-3, top, -2).map(faces_set), Some(vec!["north", "up", "west"]));
    assert_eq!(outcome.at(0, top, 0).map(faces_set), Some(vec!["up"]));
    assert_eq!(outcome.at(0, top, 0).map(|s| s.name.path()), Some("brown_mushroom_block"));
}

#[test]
fn huge_mushrooms_refuse_bad_ground_and_blocked_volumes() {
    let env = env(&[("huge_red_mushroom_can_place_on", &["stone"])], Box::new(AlwaysSurvives));
    let json = mushroom("red", None);
    let floating = place(&env, &json, 9, BlockPos::new(0, 70, 0), 64, |_| {});
    assert!(!floating.placed && floating.writes.is_empty());
    let blocked = place(&env, &json, 9, BlockPos::new(0, 64, 0), 64, |prep| prep.set(0, 66, 0, "stone"));
    assert!(!blocked.placed && blocked.writes.is_empty());
}

fn lichen(extra: &str, block: &str) -> String {
    format!(
        r#"{{"type":"minecraft:multiface_growth","config":{{"block":"minecraft:{block}",
        "can_be_placed_on":["minecraft:stone"],"search_range":4{extra}}}}}"#
    )
}

#[test]
fn glow_lichen_attaches_to_the_face_that_has_stone() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let json = lichen(r#","can_place_on_ceiling":true,"can_place_on_wall":true,"chance_of_spreading":0.0"#, "glow_lichen");
    let outcome = place(&env, &json, 3, BlockPos::new(0, 66, 0), 64, |prep| prep.set(0, 67, 0, "stone"));
    assert!(outcome.placed);
    assert_eq!(outcome.writes.len(), 1);
    let vein = outcome.at(0, 66, 0).expect("lichen at origin");
    assert_eq!(vein.name.path(), "glow_lichen");
    assert_eq!(faces_set(vein), vec!["up"]);
}

#[test]
fn glow_lichen_searches_along_a_face_when_the_origin_has_nothing() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let json = lichen(r#","can_place_on_floor":true,"chance_of_spreading":0.0"#, "glow_lichen");
    let outcome = place(&env, &json, 3, BlockPos::new(0, 65, 0), 64, |_| {});
    assert!(outcome.placed);
    assert_eq!(faces_set(outcome.at(0, 64, 0).expect("lichen below the origin")), vec!["down"]);
    let nowhere = place(&env, &json, 3, BlockPos::new(0, 70, 0), 64, |_| {});
    assert!(!nowhere.placed && nowhere.writes.is_empty());
}

#[test]
fn sculk_veins_spread_deterministically_and_only_place_veins() {
    let env = env(&[], Box::new(AlwaysSurvives));
    let json = lichen(
        r#","can_place_on_floor":true,"can_place_on_ceiling":true,"can_place_on_wall":true,"chance_of_spreading":1.0"#,
        "sculk_vein",
    );
    let outcome = place(&env, &json, 11, BlockPos::new(0, 64, 0), 64, |prep| prep.fill((-2, 2), (65, 65), (-2, 2), "stone"));
    assert!(outcome.placed);
    assert_in_window(&outcome);
    assert!(outcome.writes.values().all(|s| s.name.path() == "sculk_vein" && !faces_set(s).is_empty()));
    assert!(outcome.writes.len() > 1, "full spreading chance should reach more than one block");
    let again = place(&env, &json, 11, BlockPos::new(0, 64, 0), 64, |prep| prep.fill((-2, 2), (65, 65), (-2, 2), "stone"));
    assert_eq!(outcome.writes, again.writes);
}

const GRASS_PLACED: &str = r#"{"feature":{"type":"minecraft:simple_block","config":{"to_place":
    {"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:short_grass"}}}},"placement":[]}"#;
const DRIPLEAF_PLACED: &str = r#"{"feature":{"type":"minecraft:simple_block","config":{"to_place":
    {"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:small_dripleaf","Properties":{"waterlogged":"false"}}}}},"placement":[]}"#;

fn patch(kind: &str, ground: &str, surface: &str, depth: &str, chance: f32, vegetation: &str) -> String {
    format!(
        r#"{{"type":"minecraft:{kind}","config":{{"depth":{depth},"extra_bottom_block_chance":0.0,
        "extra_edge_column_chance":0.3,"ground_state":{{"type":"minecraft:simple_state_provider","state":{{"Name":"minecraft:{ground}"}}}},
        "replaceable":"\u0023minecraft:patch_replaceable","surface":"{surface}","vegetation_chance":{chance},
        "vegetation_feature":{vegetation},"vertical_range":5,
        "xz_radius":{{"type":"minecraft:uniform","max_inclusive":7,"min_inclusive":4}}}}}}"#
    )
}

#[test]
fn a_floor_patch_lays_ground_and_plants_above_it() {
    let env = env(&[("patch_replaceable", &["stone"])], Box::new(AlwaysSurvives));
    let json = patch("vegetation_patch", "moss_block", "floor", "1", 0.8, GRASS_PLACED);
    let seed = 21;
    let outcome = place(&env, &json, seed, BlockPos::new(0, 64, 0), 64, |_| {});
    let mut mirror = Xoroshiro128PlusPlus::new(seed);
    let x_radius = mirror.next_int_bounded(4) + 4 + 1;
    let z_radius = mirror.next_int_bounded(4) + 4 + 1;
    assert!(outcome.placed);
    assert_in_window(&outcome);
    for x in -x_radius + 1..x_radius {
        for z in -z_radius + 1..z_radius {
            assert_eq!(outcome.at(x, 63, z).map(|s| s.name.path()), Some("moss_block"), "interior ({x}, {z})");
        }
    }
    assert!(outcome.at(x_radius, 63, z_radius).is_none(), "corners are never ground");
    let grass = outcome.named("short_grass");
    assert!(!grass.is_empty());
    for pos in grass {
        assert_eq!(pos.y, 64);
        assert_eq!(outcome.at(pos.x, 63, pos.z).map(|s| s.name.path()), Some("moss_block"));
    }
    let again = place(&env, &json, seed, BlockPos::new(0, 64, 0), 64, |_| {});
    assert_eq!(outcome.writes, again.writes);
}

#[test]
fn a_ceiling_patch_grows_downward_from_the_roof() {
    let env = env(&[("patch_replaceable", &["stone"])], Box::new(AlwaysSurvives));
    let depth = r#"{"type":"minecraft:uniform","min_inclusive":1,"max_inclusive":2}"#;
    let json = patch("vegetation_patch", "moss_block", "ceiling", depth, 0.0, GRASS_PLACED);
    let outcome = place(&env, &json, 8, BlockPos::new(0, 66, 0), 64, |prep| prep.fill((-12, 12), (70, 72), (-12, 12), "stone"));
    assert!(outcome.placed);
    assert_eq!(outcome.at(0, 70, 0).map(|s| s.name.path()), Some("moss_block"));
    assert!(outcome.writes.keys().all(|pos| pos.y >= 70), "ceiling ground only goes up into the roof");
    assert!(outcome.named("short_grass").is_empty());
}

#[test]
fn a_waterlogged_patch_floods_sealed_ground_and_waterlogs_the_plants() {
    let env = env(&[("patch_replaceable", &["stone"])], Box::new(AlwaysSurvives));
    let json = patch("waterlogged_vegetation_patch", "clay", "floor", "3", 1.0, DRIPLEAF_PLACED);
    let outcome = place(&env, &json, 6, BlockPos::new(0, 64, 0), 64, |_| {});
    assert!(outcome.placed);
    assert_in_window(&outcome);
    assert_eq!(outcome.at(0, 62, 0).map(|s| s.name.path()), Some("clay"));
    let leaves = outcome.named("small_dripleaf");
    assert!(!leaves.is_empty(), "every surviving surface block is planted at chance 1.0");
    for pos in leaves {
        assert_eq!(pos.y, 63);
        assert_eq!(outcome.at(pos.x, pos.y, pos.z).and_then(|s| s.properties.get("waterlogged")).map(String::as_str), Some("true"));
    }
}

#[test]
fn a_root_system_plants_a_tree_on_the_surface_and_roots_the_column() {
    let env = env(&[("root_air", &["air"]), ("patch_replaceable", &["stone"])], Box::new(AlwaysSurvives));
    let json = r#"{"type":"minecraft:root_system","config":{
        "allowed_tree_position":{"type":"minecraft:matching_block_tag","tag":"minecraft:root_air"},
        "allowed_vertical_water_for_tree":2,
        "feature":{"feature":{"type":"minecraft:simple_block","config":{"to_place":
            {"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:oak_log"}}}},"placement":[]},
        "hanging_root_placement_attempts":20,"hanging_root_radius":3,
        "hanging_root_state_provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:hanging_roots","Properties":{"waterlogged":"false"}}},
        "hanging_roots_vertical_span":2,"level_test_distance":0,"max_level_deviation":0,
        "required_vertical_space_for_tree":3,"root_column_max_height":100,"root_placement_attempts":20,"root_radius":3,
        "root_replaceable":"\u0023minecraft:patch_replaceable",
        "root_state_provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:rooted_dirt"}}}}"#;
    let outcome = place(&env, json, 13, BlockPos::new(0, 64, 0), 80, |prep| prep.set(0, 64, 0, "air"));
    assert!(outcome.placed);
    assert_in_window(&outcome);
    assert_eq!(outcome.at(0, 80, 0).map(|s| s.name.path()), Some("oak_log"), "the tree stands on the surface");
    let dirt = outcome.named("rooted_dirt");
    assert!(!dirt.is_empty());
    assert!(dirt.iter().all(|pos| pos.x.abs() <= 2 && pos.z.abs() <= 2 && (64..79).contains(&pos.y)));
    let again = place(&env, json, 13, BlockPos::new(0, 64, 0), 80, |prep| prep.set(0, 64, 0, "air"));
    assert_eq!(outcome.writes, again.writes);

    let closed = place(&env, json, 13, BlockPos::new(0, 60, 0), 64, |_| {});
    assert!(!closed.placed, "origin inside stone is not air");
}

#[test]
fn types_without_a_faithful_port_stay_unsupported_and_tallied() {
    let env = env(&[], Box::new(AlwaysSurvives));
    for (config, tallied) in [
        (r#"{"type":"minecraft:kelp","config":{}}"#, "feature:minecraft:kelp"),
        (r#"{"type":"minecraft:seagrass","config":{"probability":0.3}}"#, "feature:minecraft:seagrass"),
        (r#"{"type":"minecraft:sea_pickle","config":{"count":20}}"#, "feature:minecraft:sea_pickle"),
        (r#"{"type":"minecraft:coral_tree","config":{}}"#, "feature:minecraft:coral_tree"),
        (r#"{"type":"minecraft:coral_claw","config":{}}"#, "feature:minecraft:coral_claw"),
        (r#"{"type":"minecraft:coral_mushroom","config":{}}"#, "feature:minecraft:coral_mushroom"),
    ] {
        let outcome = place(&env, config, 1, BlockPos::new(0, 64, 0), 64, |_| {});
        assert!(!outcome.placed && outcome.writes.is_empty());
        assert_eq!(outcome.tally.get(tallied), 1, "{tallied}");
    }
}

#[test]
fn every_vegetation_configured_feature_in_the_26_2_data_compiles() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference");
    if !root.join("data/minecraft/worldgen/configured_feature").is_dir() || !root.join("block_facts.tsv").is_file() {
        eprintln!("skipping: no extracted vanilla export at {}", root.display());
        return;
    }
    let datapack = oxide_datapack::load_datapack(&root).expect("reference datapack loads");
    let supported = [
        "minecraft:vegetation_patch",
        "minecraft:waterlogged_vegetation_patch",
        "minecraft:root_system",
        "minecraft:multiface_growth",
        "minecraft:vines",
        "minecraft:bamboo",
        "minecraft:huge_brown_mushroom",
        "minecraft:huge_red_mushroom",
    ];
    let mut compiled = 0;
    for (id, config) in datapack.configured_features.iter() {
        let type_id = serde_json::to_value(config).ok().and_then(|v| v["type"].as_str().map(str::to_owned));
        let Some(type_id) = type_id.filter(|t| supported.contains(&t.as_str())) else { continue };
        let ir: PlacedFeatureIr = serde_json::from_value(serde_json::json!({"feature": id.to_string(), "placement": []}))
            .expect("placed feature json");
        let mut compiler = Compiler::new(&datapack.block_facts, &datapack.placed_features, &datapack.configured_features);
        let placed = compiler.compile_placed(&ir).unwrap_or_else(|error| panic!("{id}: {error:#}"));
        assert!(matches!(&*placed.feature, Feature::Vegetation(_)), "{id} ({type_id}) did not compile to a vegetation feature");
        compiled += 1;
    }
    assert!(compiled >= 14, "expected the vanilla patches, root systems, lichens, vines, bamboo and mushrooms, got {compiled}");
}
