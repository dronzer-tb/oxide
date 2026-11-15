//! Tests for the water features and the desert well, on synthetic terrain with fixed seeds: a
//! water column from y = 64 to 80 over a sand floor for the underwater features, flat sand under
//! air for the well. They check what each feature places and with which properties, that the
//! same seed gives the same writes, that nothing lands outside the region, and that every 26.2
//! configured feature of these types compiles when the reference export is present.

use std::collections::HashMap;
use std::path::Path;

use oxide_core::{BlockPos, BlockState, ChunkPos, ResourceLocation, WorldgenRandom};
use oxide_datapack::placement::PlacedFeature as PlacedFeatureIr;
use oxide_datapack::{load_tags, BlockFacts, Registry, TagSet};

use super::super::*;
use crate::region::testutil::{flat_ring, refs};
use crate::region::{BlockWrite, DecorationRegion};

const FLOOR_TOP: i32 = 64;
const WATER_TOP: i32 = 80;
const COLOURS: [&str; 5] = ["tube", "brain", "bubble", "fire", "horn"];

fn facts() -> BlockFacts {
    let dir = std::env::temp_dir().join(format!(
        "oxide-water-facts-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let full = "Dsmoudf1234";
    let mut text = String::from(
        "minecraft:air\tDarc\nminecraft:cave_air\tDarc\nminecraft:void_air\tDarc\n\
         minecraft:water[level=0]\tDlWS\n\
         minecraft:kelp[age=0]\tDcWS\n\
         minecraft:kelp_plant\tDcWS\n\
         minecraft:seagrass\tDrcWS\n\
         minecraft:tall_seagrass[half=lower]\tDrcWS\n\
         minecraft:tall_seagrass[half=upper]\trcWS\n\
         minecraft:sea_pickle[pickles=1,waterlogged=true]\tDsm\n\
         minecraft:sandstone_slab[type=bottom,waterlogged=false]\tDsm1234\n\
         minecraft:suspicious_sand[dusted=0]\tDsmoudf1234\n",
    );
    for name in ["stone", "sand", "sandstone", "magma_block"] {
        text.push_str(&format!("minecraft:{name}\t{full}\n"));
    }
    for colour in COLOURS {
        text.push_str(&format!("minecraft:{colour}_coral_block\t{full}\n"));
        text.push_str(&format!(
            "minecraft:{colour}_coral[waterlogged=true]\tDcWS\n"
        ));
        text.push_str(&format!(
            "minecraft:{colour}_coral_fan[waterlogged=true]\tDcWS\n"
        ));
        text.push_str(&format!(
            "minecraft:{colour}_coral_wall_fan[facing=north,waterlogged=true]\tDcWS\n"
        ));
    }
    std::fs::write(dir.join("block_facts.tsv"), text).unwrap();
    let facts = BlockFacts::load(&dir).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    facts
}

/// The coral tags in the shape 26.2 gives them (`corals` nests `coral_plants`), and the two
/// survival tags the plants consult.
fn tags() -> TagSet {
    let root = std::env::temp_dir().join(format!(
        "oxide-water-tags-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let list = |suffix: &str| {
        COLOURS
            .map(|c| format!("\"minecraft:{c}{suffix}\""))
            .join(",")
    };
    let files = [
        ("coral_blocks", list("_coral_block")),
        ("coral_plants", list("_coral")),
        (
            "corals",
            format!("\"#minecraft:coral_plants\",{}", list("_coral_fan")),
        ),
        ("wall_corals", list("_coral_wall_fan")),
        (
            "cannot_support_kelp",
            String::from("\"minecraft:magma_block\""),
        ),
        (
            "cannot_support_seagrass",
            String::from("\"minecraft:magma_block\""),
        ),
    ];
    for (name, values) in files {
        let path = root
            .join("data/minecraft/tags/block")
            .join(format!("{name}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("{{\"values\":[{values}]}}")).unwrap();
    }
    let set = load_tags(&[&root], "block").unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    set
}

fn env() -> Environment {
    let block_tags = tags();
    Environment {
        behaviours: Box::new(SurvivalRules::new(&block_tags)),
        block_tags,
        fluid_tags: TagSet::default(),
        biome_tags: TagSet::default(),
        facts: facts(),
        seed: 0,
        sea_level: 63,
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
    /// The final writes of blocks whose path satisfies `keep`.
    fn matching(&self, keep: impl Fn(&str) -> bool) -> Vec<(BlockPos, BlockState)> {
        self.writes
            .iter()
            .filter(|(_, s)| keep(s.name.path()))
            .cloned()
            .collect()
    }

    fn count(&self, path: &str) -> usize {
        self.matching(|p| p == path).len()
    }
}

/// Places `config` (`{"type":..,"config":..}`) at `origin` with `seed` after `prepare` edited
/// the region (a stone world with its surface at y = 64, chunk (0, 0)).
fn place(
    env: &Environment,
    config: &str,
    seed: i64,
    origin: BlockPos,
    prepare: impl FnOnce(&mut DecorationRegion<'_>),
) -> Outcome {
    let ir: PlacedFeatureIr =
        serde_json::from_str(&format!(r#"{{"feature":{config},"placement":[]}}"#)).expect("json");
    let (placed_registry, configured) = (Registry::default(), Registry::default());
    let placed = Compiler::new(&env.facts, &placed_registry, &configured)
        .compile_placed(&ir)
        .expect("compiles");
    assert!(
        matches!(&*placed.feature, Feature::Water(_)),
        "{config} compiled to {:?}",
        placed.feature
    );
    let chunks = flat_ring(ChunkPos::new(0, 0), FLOOR_TOP);
    let mut region = DecorationRegion::new(ChunkPos::new(0, 0), refs(&chunks), &env.facts);
    prepare(&mut region);
    let mut random = WorldgenRandom::new(seed);
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
    Outcome {
        writes: region.into_writes(),
        tally,
        placed: did_place,
    }
}

fn fill(region: &mut DecorationRegion<'_>, y: (i32, i32), state: &BlockState) {
    for x in -16..32 {
        for z in -16..32 {
            for py in y.0..y.1 {
                assert!(region.set_block(BlockPos::new(x, py, z), state.clone()));
            }
        }
    }
}

fn state(path: &str, properties: &[(&str, &str)]) -> BlockState {
    properties.iter().fold(
        BlockState::new(ResourceLocation::minecraft(path)),
        |s, (k, v)| s.with_property(*k, *v),
    )
}

/// Sand from y = 60 to the floor, water from the floor to y = 80.
fn seabed(region: &mut DecorationRegion<'_>) {
    fill(region, (60, FLOOR_TOP), &state("sand", &[]));
    fill(
        region,
        (FLOOR_TOP, WATER_TOP),
        &state("water", &[("level", "0")]),
    );
}

/// Sand from y = 56 to the surface, air above (the flat world's own).
fn dunes(region: &mut DecorationRegion<'_>) {
    fill(region, (56, FLOOR_TOP), &state("sand", &[]));
}

fn inside_window(writes: &[(BlockPos, BlockState)]) -> bool {
    writes
        .iter()
        .all(|(pos, _)| (-16..32).contains(&pos.x) && (-16..32).contains(&pos.z))
}

fn int_property(state: &BlockState, key: &str) -> i32 {
    state.properties[key].parse().unwrap()
}

const ORIGIN: BlockPos = BlockPos {
    x: 8,
    y: FLOOR_TOP,
    z: 8,
};

#[test]
fn kelp_grows_a_plant_column_under_an_aged_head() {
    let env = env();
    let kelp = r#"{"type":"minecraft:kelp","config":{}}"#;
    for seed in 0..16 {
        let outcome = place(&env, kelp, seed, ORIGIN, seabed);
        assert!(outcome.placed, "seed {seed}");
        let column = outcome.matching(|p| p == "kelp" || p == "kelp_plant");
        assert!(column.iter().all(|(pos, _)| pos.x == 8 && pos.z == 8));
        let mut ys: Vec<i32> = column.iter().map(|(pos, _)| pos.y).collect();
        ys.sort_unstable();
        assert_eq!(
            ys,
            (FLOOR_TOP..FLOOR_TOP + ys.len() as i32).collect::<Vec<_>>(),
            "seed {seed}"
        );
        assert!(
            (2..=11).contains(&ys.len()),
            "1 + nextInt(10) plants and a head, seed {seed}"
        );
        let heads: Vec<_> = column
            .iter()
            .filter(|(_, s)| s.name.path() == "kelp")
            .collect();
        assert_eq!(heads.len(), 1, "seed {seed}");
        assert_eq!(
            heads[0].0.y,
            *ys.last().unwrap(),
            "the head tops the column"
        );
        assert!((20..=23).contains(&int_property(&heads[0].1, "age")));
        assert_eq!(
            outcome.writes,
            place(&env, kelp, seed, ORIGIN, seabed).writes,
            "deterministic"
        );
    }
    let dry = place(&env, kelp, 1, ORIGIN, |_| {});
    assert!(!dry.placed && dry.writes.is_empty());
}

#[test]
fn kelp_stops_under_the_surface_with_a_head() {
    let env = env();
    let shallow = |region: &mut DecorationRegion<'_>| {
        fill(region, (60, FLOOR_TOP), &state("sand", &[]));
        fill(
            region,
            (FLOOR_TOP, FLOOR_TOP + 3),
            &state("water", &[("level", "0")]),
        );
    };
    for seed in 0..16 {
        let outcome = place(
            &env,
            r#"{"type":"minecraft:kelp","config":{}}"#,
            seed,
            ORIGIN,
            shallow,
        );
        let column = outcome.matching(|p| p == "kelp" || p == "kelp_plant");
        assert!(
            column.iter().all(|(pos, _)| pos.y < FLOOR_TOP + 2),
            "needs water above, seed {seed}"
        );
        let top = column.iter().max_by_key(|(pos, _)| pos.y).unwrap();
        assert_eq!(top.1.name.path(), "kelp", "seed {seed}");
    }
}

#[test]
fn seagrass_places_short_or_two_tall_halves() {
    let env = env();
    let short = r#"{"type":"minecraft:seagrass","config":{"probability":0.0}}"#;
    let tall = r#"{"type":"minecraft:seagrass","config":{"probability":1.0}}"#;
    for seed in 0..16 {
        let outcome = place(&env, short, seed, ORIGIN, seabed);
        assert!(outcome.placed);
        let grass = outcome.matching(|p| p.contains("seagrass"));
        assert_eq!(grass.len(), 1);
        let (pos, block) = &grass[0];
        assert_eq!((block.name.path(), pos.y), ("seagrass", FLOOR_TOP));
        assert!((pos.x - 8).abs() <= 7 && (pos.z - 8).abs() <= 7);

        let outcome = place(&env, tall, seed, ORIGIN, seabed);
        let halves = outcome.matching(|p| p == "tall_seagrass");
        assert_eq!(halves.len(), 2, "seed {seed}");
        let (lower, upper) = (&halves[0], &halves[1]);
        assert_eq!(lower.1.properties["half"], "lower");
        assert_eq!(upper.1.properties["half"], "upper");
        assert_eq!(BlockPos::new(lower.0.x, lower.0.y + 1, lower.0.z), upper.0);
        assert_eq!(
            outcome.writes,
            place(&env, tall, seed, ORIGIN, seabed).writes,
            "deterministic"
        );
    }
}

#[test]
fn seagrass_on_magma_does_not_survive() {
    let env = env();
    let magma = |region: &mut DecorationRegion<'_>| {
        seabed(region);
        fill(
            region,
            (FLOOR_TOP - 1, FLOOR_TOP),
            &state("magma_block", &[]),
        );
    };
    let outcome = place(
        &env,
        r#"{"type":"minecraft:seagrass","config":{"probability":0.0}}"#,
        3,
        ORIGIN,
        magma,
    );
    assert!(!outcome.placed);
    assert_eq!(outcome.count("seagrass"), 0);
}

#[test]
fn sea_pickles_scatter_with_one_to_four_pickles() {
    let env = env();
    let config = r#"{"type":"minecraft:sea_pickle","config":{"count":20}}"#;
    let mut total = 0;
    for seed in 0..8 {
        let outcome = place(&env, config, seed, ORIGIN, seabed);
        assert!(outcome.placed);
        let pickles = outcome.matching(|p| p == "sea_pickle");
        assert!(!pickles.is_empty() && pickles.len() <= 20);
        for (pos, block) in &pickles {
            assert_eq!(pos.y, FLOOR_TOP);
            assert!((pos.x - 8).abs() <= 7 && (pos.z - 8).abs() <= 7);
            assert!((1..=4).contains(&int_property(block, "pickles")));
            assert_eq!(block.properties["waterlogged"], "true");
        }
        total += pickles.len();
        assert_eq!(
            outcome.writes,
            place(&env, config, seed, ORIGIN, seabed).writes,
            "deterministic"
        );
    }
    assert!(total > 40);
}

/// Wall fans face away from the coral block they hang on, every coral block of one feature is
/// the same colour, and only coral and pickle blocks are written.
fn check_reef(outcome: &Outcome, seed: i64) {
    let reef = outcome.matching(|p| p != "water" && p != "sand");
    assert!(
        reef.iter()
            .all(|(_, s)| s.name.path().contains("coral") || s.name.path() == "sea_pickle"),
        "seed {seed}"
    );
    assert!(inside_window(&reef));
    let blocks: Vec<_> = reef
        .iter()
        .filter(|(_, s)| s.name.path().ends_with("_coral_block"))
        .collect();
    assert!(!blocks.is_empty(), "seed {seed}");
    assert!(
        blocks.iter().all(|(_, s)| s.name == blocks[0].1.name),
        "one colour per feature"
    );
    let at: HashMap<BlockPos, &BlockState> = reef.iter().map(|(p, s)| (*p, s)).collect();
    for (pos, fan) in reef
        .iter()
        .filter(|(_, s)| s.name.path().ends_with("_wall_fan"))
    {
        let (dx, dz) = match fan.properties["facing"].as_str() {
            "north" => (0, -1),
            "south" => (0, 1),
            "west" => (-1, 0),
            "east" => (1, 0),
            other => panic!("facing {other}"),
        };
        let support = BlockPos::new(pos.x - dx, pos.y, pos.z - dz);
        assert!(
            at.get(&support)
                .is_some_and(|s| s.name.path().ends_with("_coral_block")),
            "seed {seed}"
        );
    }
}

#[test]
fn coral_shapes_build_reefs() {
    let env = env();
    let mut fans = 0;
    for kind in ["coral_tree", "coral_claw", "coral_mushroom"] {
        let config = format!(r#"{{"type":"minecraft:{kind}","config":{{}}}}"#);
        for seed in 0..12 {
            let outcome = place(&env, &config, seed, ORIGIN, seabed);
            assert!(outcome.placed, "{kind} seed {seed}");
            check_reef(&outcome, seed);
            fans += outcome.matching(|p| p.ends_with("_wall_fan")).len();
            assert_eq!(
                outcome.writes,
                place(&env, &config, seed, ORIGIN, seabed).writes,
                "{kind} deterministic"
            );
        }
        let dry = place(&env, &config, 1, ORIGIN, |_| {});
        assert!(
            dry.matching(|p| p.contains("coral")).is_empty(),
            "{kind} needs water"
        );
    }
    assert!(fans > 0);
}

#[test]
fn a_coral_without_its_tags_places_nothing() {
    let mut env = env();
    env.block_tags = TagSet::default();
    let outcome = place(
        &env,
        r#"{"type":"minecraft:coral_tree","config":{}}"#,
        1,
        ORIGIN,
        seabed,
    );
    assert!(!outcome.placed);
    assert!(outcome.matching(|p| p.contains("coral")).is_empty());
}

#[test]
fn the_desert_well_is_built_on_sand() {
    let env = env();
    let well = r#"{"type":"minecraft:desert_well","config":{}}"#;
    for seed in 0..8 {
        let outcome = place(&env, well, seed, ORIGIN, dunes);
        assert!(outcome.placed);
        let center = BlockPos::new(8, FLOOR_TOP - 1, 8);
        let at: HashMap<BlockPos, BlockState> = outcome.writes.iter().cloned().collect();
        let name = |dx: i32, dy: i32, dz: i32| {
            at.get(&BlockPos::new(center.x + dx, center.y + dy, center.z + dz))
                .map(|s| s.name.path().to_string())
        };
        for (dx, dz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
            assert_eq!(name(dx, 0, dz).as_deref(), Some("water"));
        }
        assert_eq!(name(2, 1, 0).as_deref(), Some("sandstone_slab"));
        assert_eq!(name(2, 1, 2).as_deref(), Some("sandstone"));
        assert_eq!(name(0, 4, 0).as_deref(), Some("sandstone"));
        assert_eq!(name(1, 4, 1).as_deref(), Some("sandstone_slab"));
        for dy in 1..=3 {
            assert_eq!(name(-1, dy, 1).as_deref(), Some("sandstone"));
        }
        assert_eq!(outcome.count("sandstone_slab"), 12);
        assert_eq!(outcome.count("suspicious_sand"), 2);
        assert_eq!(
            outcome.tally.get("block_entity:minecraft:brushable_block"),
            2
        );
        let sus = outcome.matching(|p| p == "suspicious_sand");
        assert_eq!(sus.iter().filter(|(p, _)| p.y == center.y - 1).count(), 1);
        assert_eq!(sus.iter().filter(|(p, _)| p.y == center.y - 2).count(), 1);
        assert!(inside_window(&outcome.matching(|_| true)));
        assert_eq!(
            outcome.writes,
            place(&env, well, seed, ORIGIN, dunes).writes,
            "deterministic"
        );
    }
    let on_stone = place(&env, well, 1, ORIGIN, |_| {});
    assert!(!on_stone.placed && on_stone.writes.is_empty());
    let undermined = place(&env, well, 1, ORIGIN, |region| {
        dunes(region);
        for y in [FLOOR_TOP - 3, FLOOR_TOP - 2] {
            region.set_block(BlockPos::new(10, y, 10), state("air", &[]));
        }
    });
    assert!(!undermined.placed);
    assert_eq!(undermined.count("sandstone"), 0);
}

/// Every 26.2 configured feature of these types compiles to [`Feature::Water`]; the corals,
/// which exist only inline in `warm_ocean_vegetation`, are checked through it.
#[test]
fn every_reference_water_feature_compiles() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference");
    if !root
        .join("data/minecraft/worldgen/configured_feature")
        .is_dir()
        || !root.join("block_facts.tsv").is_file()
    {
        return;
    }
    let datapack = oxide_datapack::load_datapack(&root).expect("reference datapack loads");
    const TYPES: [&str; 4] = ["kelp", "seagrass", "sea_pickle", "desert_well"];
    let compile = |id: &str| {
        let wrapper: PlacedFeatureIr =
            serde_json::from_value(serde_json::json!({ "feature": id, "placement": [] })).unwrap();
        Compiler::new(
            &datapack.block_facts,
            &datapack.placed_features,
            &datapack.configured_features,
        )
        .compile_placed(&wrapper)
        .unwrap_or_else(|error| panic!("{id}: {error:#}"))
    };
    let mut ported = 0;
    for (id, ir) in datapack.configured_features.iter() {
        let value = serde_json::to_value(ir).unwrap();
        let tag = value["type"].as_str().unwrap();
        if tag
            .strip_prefix("minecraft:")
            .is_some_and(|kind| TYPES.contains(&kind))
        {
            let placed = compile(&id.to_string());
            assert!(
                matches!(&*placed.feature, Feature::Water(_)),
                "{id} compiled to {:?}",
                placed.feature
            );
            ported += 1;
        }
    }
    assert!(ported >= 7, "only {ported} water features found");
    let reef = compile("minecraft:warm_ocean_vegetation");
    let Feature::SimpleRandomSelector(shapes) = &*reef.feature else {
        panic!("warm_ocean_vegetation compiled to {:?}", reef.feature);
    };
    assert_eq!(shapes.len(), 3);
    assert!(shapes
        .iter()
        .all(|shape| matches!(&*shape.feature, Feature::Water(_))));
    let corals = datapack
        .block_tags
        .ordered(&ResourceLocation::minecraft("coral_blocks"))
        .expect("coral_blocks");
    assert_eq!(corals.len(), 5);
}
