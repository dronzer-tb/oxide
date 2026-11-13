//! Tree tests. The shape tests run on a small synthetic block table and flat ground, so every
//! expectation is a bound worked out from the config (an oak with `base_height 4`,
//! `height_rand_a 2` is 4 to 6 logs). The last two run against the local 26.2 export when it is
//! present and skip silently otherwise.

use std::collections::HashMap;
use std::path::Path;

use oxide_core::{BlockPos, BlockState, ChunkPos, ResourceLocation, Xoroshiro128PlusPlus};
use oxide_datapack::feature::ConfiguredFeature;
use oxide_datapack::{load_tags, BlockFacts, Registry, TagSet};

use super::super::context::{Environment, FeatureContext, NoBlockBehaviours, UnsupportedTally};
use super::super::feature::Compiler;
use super::{FallenTreeFeature, TreeFeature};
use crate::region::testutil::{flat_ring, refs};
use crate::region::{BlockWrite, DecorationRegion};

/// Air, stone, dirt, and oak and spruce logs and leaves with every property combination.
fn synthetic_facts() -> BlockFacts {
    let mut table = String::from(
        "minecraft:air\tDarc\nminecraft:void_air\tDarc\nminecraft:stone\tDsmoudf1234\nminecraft:dirt\tDsmoudf1234\n",
    );
    for wood in ["oak", "spruce"] {
        for axis in ["x", "y", "z"] {
            let default = if axis == "y" { "D" } else { "" };
            table.push_str(&format!("minecraft:{wood}_log[axis={axis}]\t{default}smoudf1234\n"));
        }
        for distance in 1..=7 {
            for persistent in [true, false] {
                for waterlogged in [true, false] {
                    let default = if distance == 7 && !persistent && !waterlogged { "D" } else { "" };
                    let water = if waterlogged { "WS" } else { "" };
                    table.push_str(&format!(
                        "minecraft:{wood}_leaves[distance={distance},persistent={persistent},waterlogged={waterlogged}]\t{default}smf123{water}\n"
                    ));
                }
            }
        }
    }
    let dir = std::env::temp_dir().join(format!("oxide-tree-facts-{}-{:?}", std::process::id(), std::thread::current().id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("block_facts.tsv"), table).expect("write facts");
    let facts = BlockFacts::load(&dir).expect("facts load");
    let _ = std::fs::remove_dir_all(dir);
    facts
}

fn synthetic_tags() -> TagSet {
    let root = std::env::temp_dir().join(format!("oxide-tree-tags-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("data/minecraft/tags/block");
    std::fs::create_dir_all(&dir).expect("tag dir");
    let logs = r#"{"values":["minecraft:oak_log","minecraft:spruce_log"]}"#;
    let leaves = r#"{"values":["minecraft:oak_leaves","minecraft:spruce_leaves"]}"#;
    std::fs::write(dir.join("logs.json"), logs).expect("tag");
    std::fs::write(dir.join("leaves.json"), leaves).expect("tag");
    std::fs::write(dir.join("replaceable_by_trees.json"), leaves).expect("tag");
    let tags = load_tags(&[&root], "block").expect("tags load");
    let _ = std::fs::remove_dir_all(root);
    tags
}

fn environment(facts: BlockFacts, block_tags: TagSet) -> Environment {
    Environment {
        block_tags,
        fluid_tags: TagSet::default(),
        biome_tags: TagSet::default(),
        facts,
        seed: 0,
        sea_level: 63,
        behaviours: Box::new(NoBlockBehaviours),
        placed: HashMap::new(),
        biome_climate: HashMap::new(),
        biome_features: HashMap::new(),
    }
}

fn synthetic_env() -> Environment {
    environment(synthetic_facts(), synthetic_tags())
}

struct Placed {
    writes: Vec<BlockWrite>,
    tally: UnsupportedTally,
    placed: bool,
}

/// Runs `place` against a flat world (stone below y=64) seeded with `seed`.
fn run_on_flat(
    env: &Environment,
    seed: i64,
    origin: BlockPos,
    place: impl FnOnce(&mut FeatureContext<'_, '_, '_>) -> bool,
) -> Placed {
    let chunks = flat_ring(ChunkPos::new(0, 0), 64);
    let mut region = DecorationRegion::new(ChunkPos::new(0, 0), refs(&chunks), &env.facts);
    let mut random = Xoroshiro128PlusPlus::new(seed);
    let mut tally = UnsupportedTally::default();
    let placed = {
        let mut cx = FeatureContext {
            env,
            region: &mut region,
            random: &mut random,
            origin,
            unsupported: &mut tally,
            top_feature: None,
        };
        place(&mut cx)
    };
    Placed { writes: region.into_writes(), tally, placed }
}

fn tree(env: &Environment, json: &str) -> TreeFeature {
    let ir: ConfiguredFeature = serde_json::from_str(json).expect("configured feature json");
    let ConfiguredFeature::Tree(config) = ir else { panic!("not a tree") };
    TreeFeature::compile(&config, &env.facts, &mut |_| Ok(None)).expect("tree compiles")
}

fn fallen(env: &Environment, json: &str) -> FallenTreeFeature {
    let ir: ConfiguredFeature = serde_json::from_str(json).expect("configured feature json");
    let ConfiguredFeature::FallenTree(config) = ir else { panic!("not a fallen tree") };
    FallenTreeFeature::compile(&config, &env.facts, &mut |_| Ok(None)).expect("fallen tree compiles")
}

const OAK: &str = r#"{"type":"minecraft:tree","config":{
  "trunk_provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:oak_log","Properties":{"axis":"y"}}},
  "below_trunk_provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:dirt"}},
  "foliage_provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:oak_leaves",
    "Properties":{"distance":"7","persistent":"false","waterlogged":"false"}}},
  "decorators":[],"ignore_vines":true,
  "minimum_size":{"type":"minecraft:two_layers_feature_size"},
  "foliage_placer":{"type":"minecraft:blob_foliage_placer","height":3,"offset":0,"radius":2},
  "trunk_placer":{"type":"minecraft:straight_trunk_placer","base_height":4,"height_rand_a":2,"height_rand_b":0}}}"#;

const SPRUCE: &str = r#"{"type":"minecraft:tree","config":{
  "trunk_provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:spruce_log","Properties":{"axis":"y"}}},
  "below_trunk_provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:dirt"}},
  "foliage_provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:spruce_leaves",
    "Properties":{"distance":"7","persistent":"false","waterlogged":"false"}}},
  "decorators":[],"ignore_vines":false,
  "minimum_size":{"type":"minecraft:two_layers_feature_size","limit":2,"lower_size":0,"upper_size":2},
  "foliage_placer":{"type":"minecraft:spruce_foliage_placer",
    "radius":{"type":"minecraft:uniform","min_inclusive":2,"max_inclusive":3},
    "offset":{"type":"minecraft:uniform","min_inclusive":0,"max_inclusive":2},
    "trunk_height":{"type":"minecraft:uniform","min_inclusive":1,"max_inclusive":2}},
  "trunk_placer":{"type":"minecraft:straight_trunk_placer","base_height":5,"height_rand_a":2,"height_rand_b":1}}}"#;

const FALLEN_OAK: &str = r#"{"type":"minecraft:fallen_tree","config":{
  "trunk_provider":{"type":"minecraft:simple_state_provider","state":{"Name":"minecraft:oak_log","Properties":{"axis":"y"}}},
  "log_length":{"type":"minecraft:uniform","min_inclusive":4,"max_inclusive":7},
  "stump_decorators":[],"log_decorators":[]}}"#;

fn named<'a>(writes: &'a [BlockWrite], suffix: &'a str) -> impl Iterator<Item = &'a BlockWrite> {
    writes.iter().filter(move |(_, state)| state.name.path().ends_with(suffix))
}

#[test]
fn an_oak_is_a_straight_log_column_under_a_blob_of_leaves() {
    let env = synthetic_env();
    let oak = tree(&env, OAK);
    let origin = BlockPos::new(8, 64, 8);
    let outcome = run_on_flat(&env, 7, origin, |cx| oak.place(cx));
    assert!(outcome.placed && outcome.tally.is_empty());

    let mut logs: Vec<i32> = named(&outcome.writes, "_log").map(|(pos, _)| {
        assert_eq!((pos.x, pos.z), (8, 8), "an oak trunk is one column");
        pos.y
    }).collect();
    logs.sort_unstable();
    assert!((4..=6).contains(&logs.len()), "{} logs", logs.len());
    assert_eq!(logs[0], 64);
    assert!(logs.windows(2).all(|pair| pair[1] == pair[0] + 1), "contiguous: {logs:?}");

    let top = *logs.last().expect("a log");
    let leaves: Vec<&BlockWrite> = named(&outcome.writes, "_leaves").collect();
    assert!(leaves.len() > 20, "{} leaves", leaves.len());
    assert!(leaves.iter().all(|(pos, _)| pos.y >= top - 2 && pos.y <= top + 1), "a blob around the crown");
    let above_top = leaves.iter().find(|(pos, _)| *pos == BlockPos::new(8, top + 1, 8)).expect("a leaf above the trunk");
    assert_eq!(above_top.1.properties.get("distance").map(String::as_str), Some("1"));
    assert!(
        leaves.iter().all(|(_, state)| state.properties["distance"] != "7"),
        "the distance pass reaches every leaf of a lone oak"
    );

    let dirt = outcome.writes.iter().find(|(pos, _)| *pos == BlockPos::new(8, 63, 8)).expect("dirt under the trunk");
    assert_eq!(dirt.1.name, ResourceLocation::minecraft("dirt"));
}

#[test]
fn the_same_seed_gives_the_same_tree_and_seeds_vary_its_height() {
    let env = synthetic_env();
    let oak = tree(&env, OAK);
    let origin = BlockPos::new(8, 64, 8);
    let first = run_on_flat(&env, 99, origin, |cx| oak.place(cx));
    let second = run_on_flat(&env, 99, origin, |cx| oak.place(cx));
    assert_eq!(first.writes, second.writes);

    let heights: std::collections::BTreeSet<usize> = (0..40)
        .map(|seed| named(&run_on_flat(&env, seed, origin, |cx| oak.place(cx)).writes, "_log").count())
        .collect();
    assert_eq!(heights.iter().copied().min(), Some(4));
    assert_eq!(heights.iter().copied().max(), Some(6));
}

#[test]
fn a_tree_at_the_region_edge_is_clipped_not_refused() {
    let env = synthetic_env();
    let oak = tree(&env, OAK);
    // x = 31 is the last column of the 3x3 chunk window around chunk (0, 0).
    let outcome = run_on_flat(&env, 3, BlockPos::new(31, 64, 8), |cx| oak.place(cx));
    assert!(outcome.placed);
    assert!(outcome.writes.iter().all(|(pos, _)| (-16..32).contains(&pos.x) && (-16..32).contains(&pos.z)));
    assert!(named(&outcome.writes, "_leaves").any(|(pos, _)| pos.x == 31));
    assert!(named(&outcome.writes, "_leaves").any(|(pos, _)| pos.x == 29));
}

#[test]
fn a_blocked_column_clips_or_refuses_the_tree() {
    let env = synthetic_env();
    let oak = tree(&env, OAK);
    let origin = BlockPos::new(8, 64, 8);
    // Stone two blocks up leaves a free height of 0, below the default minimum of the tree's
    // own height, and `two_layers_feature_size` has no `min_clipped_height`.
    let outcome = run_on_flat(&env, 1, origin, |cx| {
        cx.region.set_block(BlockPos::new(8, 66, 8), BlockState::new(ResourceLocation::minecraft("stone")));
        oak.place(cx)
    });
    assert!(!outcome.placed);
    assert_eq!(named(&outcome.writes, "_log").count(), 0);
}

#[test]
fn a_spruce_narrows_toward_the_top() {
    let env = synthetic_env();
    let spruce = tree(&env, SPRUCE);
    let outcome = run_on_flat(&env, 11, BlockPos::new(8, 64, 8), |cx| spruce.place(cx));
    assert!(outcome.placed);
    let logs = named(&outcome.writes, "_log").count();
    assert!((5..=8).contains(&logs), "{logs} logs");

    let mut radius_at: std::collections::BTreeMap<i32, i32> = std::collections::BTreeMap::new();
    for (pos, _) in named(&outcome.writes, "_leaves") {
        let reach = (pos.x - 8).abs().max((pos.z - 8).abs());
        let entry = radius_at.entry(pos.y).or_insert(0);
        *entry = (*entry).max(reach);
    }
    let rows: Vec<(i32, i32)> = radius_at.into_iter().collect();
    assert!(rows.len() >= 3, "{rows:?}");
    let (top_row, bottom_widest) = (rows.last().expect("row").1, rows.iter().map(|row| row.1).max().expect("row"));
    assert!(top_row < bottom_widest, "the crown comes to a point: {rows:?}");
}

#[test]
fn a_fallen_oak_is_a_stump_and_a_log_lying_on_its_side() {
    let env = synthetic_env();
    let fallen = fallen(&env, FALLEN_OAK);
    let origin = BlockPos::new(8, 64, 8);
    let outcome = run_on_flat(&env, 5, origin, |cx| fallen.place(cx));
    assert!(outcome.placed && outcome.tally.is_empty());

    let logs: Vec<&BlockWrite> = named(&outcome.writes, "_log").collect();
    let stump = logs.iter().find(|(pos, _)| *pos == origin).expect("stump");
    assert_eq!(stump.1.properties["axis"], "y");
    let lying: Vec<&&BlockWrite> = logs.iter().filter(|(pos, _)| *pos != origin).collect();
    assert!((2..=5).contains(&lying.len()), "{} lying logs", lying.len());
    let axis = &lying[0].1.properties["axis"];
    assert_ne!(axis, "y");
    assert!(lying.iter().all(|(pos, state)| pos.y == 64 && &state.properties["axis"] == axis));
    let along = |pos: &BlockPos| if axis == "x" { (pos.z, pos.x) } else { (pos.x, pos.z) };
    assert!(lying.iter().all(|(pos, _)| along(pos).0 == along(&lying[0].0).0), "one straight line");
}

fn reference_root() -> Option<std::path::PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference");
    if root.join("data/minecraft/worldgen/configured_feature").is_dir() && root.join("block_facts.tsv").is_file() {
        Some(root)
    } else {
        eprintln!("skipping: no extracted vanilla export at {}", root.display());
        None
    }
}

#[test]
fn every_26_2_tree_compiles() {
    let Some(root) = reference_root() else { return };
    let datapack = oxide_datapack::load_datapack(&root).expect("reference datapack loads");
    let (placed, configured): (&Registry<_>, _) = (&datapack.placed_features, &datapack.configured_features);
    let mut compiler = Compiler::new(&datapack.block_facts, placed, configured);
    let mut compiled = 0;
    for (id, config) in configured.iter() {
        if matches!(config, ConfiguredFeature::Tree(_) | ConfiguredFeature::FallenTree(_)) {
            compiler
                .configured_by_id(id)
                .unwrap_or_else(|error| panic!("{id}: {error:#}"))
                .unwrap_or_else(|| panic!("{id} vanished"));
            compiled += 1;
        }
    }
    assert!(compiled >= 44, "only {compiled} tree features found");
}

#[test]
fn the_real_oak_and_spruce_place_on_flat_ground() {
    let Some(root) = reference_root() else { return };
    let datapack = oxide_datapack::load_datapack(&root).expect("reference datapack loads");
    let env = environment(datapack.block_facts.clone(), datapack.block_tags.clone());
    let mut compiler =
        Compiler::new(&datapack.block_facts, &datapack.placed_features, &datapack.configured_features);
    for (name, min_logs, max_logs) in [("oak", 4, 6), ("spruce", 6, 9)] {
        let feature = compiler
            .configured_by_id(&ResourceLocation::minecraft(name))
            .expect("compiles")
            .expect("registered");
        let outcome = run_on_flat(&env, 21, BlockPos::new(8, 64, 8), |cx| {
            super::super::feature::place_configured_feature(&feature, cx)
        });
        assert!(outcome.placed, "{name}: {:?}", outcome.tally.entries().collect::<Vec<_>>());
        let logs = named(&outcome.writes, "_log").count();
        assert!((min_logs..=max_logs).contains(&logs), "{name}: {logs} logs");
        assert!(named(&outcome.writes, "_leaves").count() > 10, "{name} has leaves");
        assert!(outcome.tally.is_empty(), "{name}: {:?}", outcome.tally.entries().collect::<Vec<_>>());
    }
}

#[test]
fn every_26_2_tree_places_without_panicking_on_flat_ground() {
    let Some(root) = reference_root() else { return };
    let datapack = oxide_datapack::load_datapack(&root).expect("reference datapack loads");
    let env = environment(datapack.block_facts.clone(), datapack.block_tags.clone());
    let mut compiler =
        Compiler::new(&datapack.block_facts, &datapack.placed_features, &datapack.configured_features);
    let mut placed = 0;
    for (id, config) in datapack.configured_features.iter() {
        if !matches!(config, ConfiguredFeature::Tree(_) | ConfiguredFeature::FallenTree(_)) {
            continue;
        }
        let feature = compiler.configured_by_id(id).expect("compiles").expect("registered");
        for seed in 0..3 {
            let outcome = run_on_flat(&env, seed, BlockPos::new(8, 64, 8), |cx| {
                super::super::feature::place_configured_feature(&feature, cx)
            });
            assert!(
                outcome.writes.iter().all(|(pos, _)| (-16..32).contains(&pos.x) && (-16..32).contains(&pos.z)),
                "{id} wrote outside the region"
            );
            placed += usize::from(outcome.placed);
        }
    }
    assert!(placed > 44, "only {placed} placements succeeded");
}
