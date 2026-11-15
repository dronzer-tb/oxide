//! Rule-family tests on hand-built neighbourhoods, plus the replay of the game's own answers.
//!
//! The unit tests use a small fact table and tag set so they run anywhere. The replay reads
//! `reference/block_survival.tsv` and `reference/block_survival_classes.tsv`, written by
//! `scripts/BlockSurvivalOracle.java` from the real 26.2 jar, and is skipped when they are absent.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use oxide_core::{BlockPos, BlockState, ChunkData, ChunkPos, ResourceLocation};
use oxide_datapack::{load_tags, BlockFacts, TagSet};

use super::rules::{classify, Rule};
use super::SurvivalRules;
use crate::decorator::context::BlockBehaviours;
use crate::region::testutil::{block, flat_ring, refs};
use crate::region::DecorationRegion;

const FACTS: &str = "\
minecraft:air\tDarc
minecraft:stone\tDsmoudf1234
minecraft:dirt\tDsmoudf1234
minecraft:grass_block[snowy=false]\tDsmoudf1234
minecraft:sand\tDsmoudf1234
minecraft:glass\tDsmudf1234
minecraft:magma_block\tDsmoudf1234
minecraft:ice\tDsmoudf1234
minecraft:oak_leaves[distance=7,persistent=false,waterlogged=false]\tDsmf123
minecraft:oak_slab[type=bottom,waterlogged=false]\tDsmd1234
minecraft:water[level=0]\tDlrcWS134
minecraft:water[level=8]\tlrcW134
minecraft:short_grass\tDrc1
minecraft:cactus[age=0]\tDsm134
";

const TAGS: &[(&str, &str)] = &[
    (
        "supports_vegetation",
        r#"{"values":["minecraft:dirt","minecraft:grass_block"]}"#,
    ),
    (
        "supports_dry_vegetation",
        r##"{"values":["minecraft:sand","#minecraft:supports_vegetation"]}"##,
    ),
    ("supports_cactus", r#"{"values":["minecraft:sand"]}"#),
    (
        "supports_sugar_cane",
        r#"{"values":["minecraft:sand","minecraft:dirt"]}"#,
    ),
    (
        "cannot_support_kelp",
        r#"{"values":["minecraft:magma_block"]}"#,
    ),
    (
        "cannot_support_snow_layer",
        r#"{"values":["minecraft:ice"]}"#,
    ),
    ("supports_lily_pad", r#"{"values":["minecraft:ice"]}"#),
    ("supports_crops", r#"{"values":["minecraft:dirt"]}"#),
    (
        "overrides_mushroom_light_requirement",
        r#"{"values":["minecraft:dirt"]}"#,
    ),
];

fn temp_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "oxide-survival-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp dir");
    root
}

/// The rules and facts the unit tests share: the tags above and the facts of a few blocks.
fn fixture() -> (SurvivalRules, BlockFacts) {
    let root = temp_root("fixture");
    std::fs::write(root.join("block_facts.tsv"), FACTS).expect("facts file");
    for (name, body) in TAGS {
        let path = root
            .join("data/minecraft/tags/block")
            .join(format!("{name}.json"));
        std::fs::create_dir_all(path.parent().expect("parent")).expect("tag dir");
        std::fs::write(path, body).expect("tag file");
    }
    let tags = load_tags(&[&root], "block").expect("tags load");
    let facts = BlockFacts::load(&root).expect("facts load");
    let _ = std::fs::remove_dir_all(root);
    (SurvivalRules::new(&tags), facts)
}

fn air_ring() -> Vec<ChunkData> {
    flat_ring(ChunkPos::new(0, 0), -64)
}

/// Asks the rules whether `state` survives at the origin of an air world holding `placed`.
fn survives(
    (rules, facts): &(SurvivalRules, BlockFacts),
    state: &BlockState,
    placed: &[((i32, i32, i32), BlockState)],
) -> Option<bool> {
    let chunks = air_ring();
    let mut region = DecorationRegion::new(ChunkPos::new(0, 0), refs(&chunks), facts);
    for ((x, y, z), at) in placed {
        assert!(region.set_block(BlockPos::new(*x, *y, *z), at.clone()));
    }
    rules.can_survive(state, &region, BlockPos::new(0, 0, 0))
}

fn on(ground: &str, state: &BlockState, fixture: &(SurvivalRules, BlockFacts)) -> Option<bool> {
    survives(fixture, state, &[((0, -1, 0), block(ground))])
}

fn with_props(path: &str, props: &[(&str, &str)]) -> BlockState {
    props.iter().fold(block(path), |state, (key, value)| {
        state.with_property(*key, *value)
    })
}

#[test]
fn plants_follow_the_tag_of_their_class() {
    let f = fixture();
    let grass = with_props("grass_block", &[("snowy", "false")]);
    let place = |ground: &BlockState, plant: &str| {
        survives(&f, &block(plant), &[((0, -1, 0), ground.clone())])
    };
    assert_eq!(place(&grass, "poppy"), Some(true));
    assert_eq!(place(&block("stone"), "poppy"), Some(false));
    assert_eq!(place(&block("sand"), "poppy"), Some(false));
    assert_eq!(
        place(&block("sand"), "dead_bush"),
        Some(true),
        "dry vegetation also takes sand"
    );
    assert_eq!(place(&block("dirt"), "oak_sapling"), Some(true));
}

#[test]
fn two_block_plants_stack_on_their_lower_half() {
    let f = fixture();
    let lower = with_props("tall_grass", &[("half", "lower")]);
    let upper = with_props("tall_grass", &[("half", "upper")]);
    assert_eq!(on("dirt", &lower, &f), Some(true));
    assert_eq!(on("stone", &lower, &f), Some(false));
    assert_eq!(
        survives(&f, &upper, &[((0, -1, 0), lower.clone())]),
        Some(true)
    );
    assert_eq!(
        on("dirt", &upper, &f),
        Some(false),
        "an upper half needs its lower half"
    );
}

#[test]
fn blocks_without_an_override_always_survive_and_unported_ones_are_unknown() {
    let f = fixture();
    assert_eq!(on("stone", &block("stone"), &f), Some(true));
    assert_eq!(on("stone", &block("oak_door"), &f), None);
    assert_eq!(on("stone", &block("wall_torch"), &f), None);
}

#[test]
fn cactus_needs_sand_open_sides_and_no_liquid_above() {
    let f = fixture();
    let cactus = with_props("cactus", &[("age", "0")]);
    assert_eq!(on("sand", &cactus, &f), Some(true));
    assert_eq!(on("dirt", &cactus, &f), Some(false));
    let walled = [((0, -1, 0), block("sand")), ((1, 0, 0), block("stone"))];
    assert_eq!(survives(&f, &cactus, &walled), Some(false));
    let wet = [
        ((0, -1, 0), block("sand")),
        ((0, 1, 0), with_props("water", &[("level", "0")])),
    ];
    assert_eq!(survives(&f, &cactus, &wet), Some(false));
}

#[test]
fn sugar_cane_needs_water_beside_its_base() {
    let f = fixture();
    let cane = with_props("sugar_cane", &[("age", "0")]);
    assert_eq!(on("sand", &cane, &f), Some(false));
    let by_water = [
        ((0, -1, 0), block("sand")),
        ((1, -1, 0), with_props("water", &[("level", "0")])),
    ];
    assert_eq!(survives(&f, &cane, &by_water), Some(true));
    assert_eq!(
        survives(&f, &cane, &[((0, -1, 0), cane.clone())]),
        Some(true),
        "cane stacks on cane"
    );
}

#[test]
fn mushrooms_take_opaque_ground_or_a_light_override() {
    let f = fixture();
    assert_eq!(on("stone", &block("red_mushroom"), &f), Some(true));
    assert_eq!(
        on("glass", &block("red_mushroom"), &f),
        Some(false),
        "glass is not a solid render"
    );
    assert_eq!(on("dirt", &block("brown_mushroom"), &f), Some(true));
}

#[test]
fn snow_sits_on_full_collision_but_not_on_ice_or_a_slab() {
    let f = fixture();
    let snow = with_props("snow", &[("layers", "1")]);
    assert_eq!(on("stone", &snow, &f), Some(true));
    assert_eq!(on("ice", &snow, &f), Some(false));
    let leaves = with_props(
        "oak_leaves",
        &[
            ("distance", "7"),
            ("persistent", "false"),
            ("waterlogged", "false"),
        ],
    );
    assert_eq!(survives(&f, &snow, &[((0, -1, 0), leaves)]), Some(true));
    let slab = with_props("oak_slab", &[("type", "bottom"), ("waterlogged", "false")]);
    assert_eq!(
        survives(&f, &snow, &[((0, -1, 0), slab)]),
        None,
        "a partial shape is unknown"
    );
}

#[test]
fn growing_plants_hang_from_or_stand_on_a_sturdy_face() {
    let f = fixture();
    let kelp = with_props("kelp", &[("age", "0")]);
    assert_eq!(on("stone", &kelp, &f), Some(true));
    assert_eq!(
        on("magma_block", &kelp, &f),
        Some(false),
        "cannot_support_kelp"
    );
    let vines = with_props("cave_vines", &[("age", "0"), ("berries", "false")]);
    assert_eq!(
        survives(&f, &vines, &[((0, 1, 0), block("stone"))]),
        Some(true)
    );
    assert_eq!(
        on("stone", &vines, &f),
        Some(false),
        "cave vines hang, they do not stand"
    );
}

#[test]
fn lily_pads_float_on_water_and_ice() {
    let f = fixture();
    let pad = block("lily_pad");
    let water = with_props("water", &[("level", "0")]);
    assert_eq!(
        survives(&f, &pad, &[((0, -1, 0), water.clone())]),
        Some(true)
    );
    assert_eq!(
        survives(&f, &pad, &[((0, -1, 0), water.clone()), ((0, 0, 0), water)]),
        Some(false)
    );
    assert_eq!(on("ice", &pad, &f), Some(true));
    assert_eq!(on("stone", &pad, &f), Some(false));
}

#[test]
fn crops_read_the_unlit_world_decoration_runs_in() {
    let f = fixture();
    assert_eq!(
        on("dirt", &with_props("wheat", &[("age", "0")]), &f),
        Some(false)
    );
}

#[test]
fn a_tag_the_data_leaves_out_has_no_members() {
    let rules = SurvivalRules::new(&TagSet::default());
    let facts = BlockFacts::default();
    let chunks = air_ring();
    let region = DecorationRegion::new(ChunkPos::new(0, 0), refs(&chunks), &facts);
    assert_eq!(
        rules.can_survive(&block("poppy"), &region, BlockPos::new(0, 0, 0)),
        Some(false)
    );
}

#[test]
fn other_namespaces_are_unknown() {
    let f = fixture();
    let foreign = BlockState::new(ResourceLocation::new("mod", "flower"));
    assert_eq!(on("dirt", &foreign, &f), None);
}

fn reference_root() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference");
    root.join("block_facts.tsv").is_file().then_some(root)
}

fn parse_state(text: &str) -> BlockState {
    let (name, props) = match text.split_once('[') {
        Some((name, rest)) => (name, rest.trim_end_matches(']')),
        None => (text, ""),
    };
    let mut state = BlockState::new(name.parse().expect("state id"));
    for pair in props.split(',').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').expect("property");
        state = state.with_property(key, value);
    }
    state
}

/// The set of blocks the game says override `canSurvive` is exactly the set the classifier calls
/// anything but [`Rule::Always`].
#[test]
fn the_override_set_matches_the_game() {
    let Some(root) = reference_root() else { return };
    let Ok(classes) = std::fs::read_to_string(root.join("block_survival_classes.tsv")) else {
        return;
    };
    let overriding: Vec<&str> = classes
        .lines()
        .filter_map(|line| line.split('\t').next())
        .collect();
    let facts = std::fs::read_to_string(root.join("block_facts.tsv")).expect("facts");
    let mut wrong = Vec::new();
    for line in facts.lines() {
        let Some((state, letters)) = line.split_once('\t') else {
            continue;
        };
        if !letters.contains('D') {
            continue;
        }
        let id = state.split('[').next().unwrap_or(state);
        let path = id.trim_start_matches("minecraft:");
        let classified = classify(path) != Rule::Always;
        if classified != overriding.contains(&id) {
            wrong.push(id.to_string());
        }
    }
    assert!(
        wrong.is_empty(),
        "classification disagrees with the game for {wrong:?}"
    );
}

/// Every answer the game gave, replayed: a rule that answers must agree, and the unknowns are
/// counted so a regression to "unknown" shows.
#[test]
fn the_rules_agree_with_the_games_own_answers() {
    let Some(root) = reference_root() else { return };
    let Ok(rows) = std::fs::read_to_string(root.join("block_survival.tsv")) else {
        return;
    };
    let facts = BlockFacts::load(&root).expect("facts load");
    let tags = load_tags(&[&root], "block").expect("tags load");
    let rules = SurvivalRules::new(&tags);
    let chunks = air_ring();
    let default = |name: &str| {
        facts
            .default_state(&ResourceLocation::minecraft(name))
            .expect("default state")
            .clone()
    };
    let (stone, water, frosted, lava) = (
        default("stone"),
        default("water"),
        default("frosted_ice"),
        default("lava"),
    );

    let mut cache: HashMap<&str, BlockState> = HashMap::new();
    let (mut agree, mut unknown, mut unported) = (0u64, 0u64, 0u64);
    let mut unknown_blocks: BTreeMap<String, u64> = BTreeMap::new();
    let mut disagreements: Vec<String> = Vec::new();
    for row in rows.lines() {
        let mut cols = row.split('\t');
        let (Some(state_text), Some(ground_text), Some(env), Some(expected)) =
            (cols.next(), cols.next(), cols.next(), cols.next())
        else {
            continue;
        };
        let state = cache
            .entry(state_text)
            .or_insert_with(|| parse_state(state_text))
            .clone();
        let ground = cache
            .entry(ground_text)
            .or_insert_with(|| parse_state(ground_text))
            .clone();
        let origin = BlockPos::new(0, 0, 0);
        let mut region = DecorationRegion::new(ChunkPos::new(0, 0), refs(&chunks), &facts);
        region.set_block(BlockPos::new(0, -1, 0), ground);
        let horizontal = [(0, -1), (1, 0), (0, 1), (-1, 0)];
        match env {
            "sealed_sides" => {
                for (dx, dz) in horizontal {
                    region.set_block(BlockPos::new(dx, 0, dz), stone.clone());
                }
            }
            "ceiling" => {
                region.set_block(BlockPos::new(0, 1, 0), stone.clone());
            }
            "water_here" => {
                region.set_block(origin, water.clone());
            }
            "water_above" => {
                region.set_block(BlockPos::new(0, 1, 0), water.clone());
            }
            "ground_water_sides" | "ground_ice_sides" => {
                let side = if env == "ground_water_sides" {
                    &water
                } else {
                    &frosted
                };
                for (dx, dz) in horizontal {
                    region.set_block(BlockPos::new(dx, -1, dz), side.clone());
                }
            }
            "lava_side" => {
                region.set_block(BlockPos::new(0, 0, -1), lava.clone());
            }
            "cactus_above" => {
                let own = facts
                    .default_state(&state.name)
                    .expect("default state")
                    .clone();
                region.set_block(BlockPos::new(0, 1, 0), own);
            }
            _ => {}
        }
        match (rules.can_survive(&state, &region, origin), expected == "1") {
            (None, _) => {
                if classify(state.name.path()) == Rule::Unported {
                    unported += 1;
                } else {
                    unknown += 1;
                    *unknown_blocks.entry(state.name.to_string()).or_insert(0) += 1;
                }
            }
            (Some(got), want) if got == want => agree += 1,
            (Some(got), want) => {
                if disagreements.len() < 40 {
                    disagreements.push(format!(
                        "{state_text} on {ground_text} [{env}]: rust {got}, game {want}"
                    ));
                }
            }
        }
    }
    eprintln!(
        "oracle replay: {agree} agree, {unported} in unported classes, {unknown} unknown (shape facts) in {:?}, {} disagree",
        unknown_blocks,
        disagreements.len()
    );
    assert!(
        disagreements.is_empty(),
        "rules disagree with the game:\n{}",
        disagreements.join("\n")
    );
}
