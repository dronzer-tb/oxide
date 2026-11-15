//! Tests for the Nether features, on synthetic terrain with fixed seeds: a netherrack floor and
//! ceiling with air between them. They check what each feature places, that the same seed gives
//! the same writes, that nothing lands outside the region, and that every 26.2 configured
//! feature of these types compiles when the reference export is present.

use std::collections::HashMap;
use std::path::Path;

use oxide_core::{BlockPos, ChunkPos, RandomSource, WorldgenRandom};
use oxide_datapack::placement::PlacedFeature as PlacedFeatureIr;
use oxide_datapack::{BlockFacts, Registry, TagSet};

use super::super::*;
use super::offset;
use crate::region::testutil::{flat_ring, refs};
use crate::region::{BlockWrite, DecorationRegion};

const FLOOR_TOP: i32 = 64;
const CEILING_BOTTOM: i32 = 80;

/// Facts for the blocks these tests touch, in the exporter's format.
fn facts() -> BlockFacts {
    let dir = std::env::temp_dir().join(format!(
        "oxide-nether-facts-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let full = "Dsmoudf1234";
    let mut text = String::from(
        "minecraft:air\tDarc\nminecraft:cave_air\tDarc\nminecraft:void_air\tDarc\n\
         minecraft:lava[level=0]\tDlLS\n\
         minecraft:weeping_vines[age=0]\tDrc\n\
         minecraft:weeping_vines_plant\tDrc\n\
         minecraft:twisting_vines[age=0]\tDrc\n\
         minecraft:twisting_vines_plant\tDrc\n\
         minecraft:crimson_roots\tDrc\nminecraft:crimson_fungus\tDrc\nminecraft:warped_fungus\tDrc\n\
         minecraft:poppy\tDrc\n\
         minecraft:basalt[axis=y]\tDsmoudf1234\n\
         minecraft:crimson_stem[axis=y]\tDsmoudf1234\n\
         minecraft:warped_stem[axis=y]\tDsmoudf1234\n",
    );
    for name in [
        "stone",
        "netherrack",
        "magma_block",
        "crimson_nylium",
        "warped_nylium",
        "nether_wart_block",
        "warped_wart_block",
        "shroomlight",
        "chest",
        "glowstone",
        "blackstone",
        "soul_sand",
    ] {
        text.push_str(&format!("minecraft:{name}\t{full}\n"));
    }
    std::fs::write(dir.join("block_facts.tsv"), text).unwrap();
    let facts = BlockFacts::load(&dir).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    facts
}

fn env() -> Environment {
    env_with(TagSet::default(), Box::new(NoBlockBehaviours))
}

fn env_with(block_tags: TagSet, behaviours: Box<dyn BlockBehaviours>) -> Environment {
    Environment {
        block_tags,
        fluid_tags: TagSet::default(),
        biome_tags: TagSet::default(),
        facts: facts(),
        seed: 0,
        sea_level: 32,
        behaviours,
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
    /// The positions the feature itself wrote, i.e. every write of a block in `paths`.
    fn of(&self, paths: &[&str]) -> Vec<BlockPos> {
        self.writes
            .iter()
            .filter(|(_, s)| paths.contains(&s.name.path()))
            .map(|(p, _)| *p)
            .collect()
    }

    fn count(&self, path: &str) -> usize {
        self.of(&[path]).len()
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
    let chunks = flat_ring(ChunkPos::new(0, 0), 64);
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

fn fill(region: &mut DecorationRegion<'_>, y: (i32, i32), path: &str, name_props: &[(&str, &str)]) {
    let mut state = oxide_core::BlockState::new(oxide_core::ResourceLocation::minecraft(path));
    for (key, value) in name_props {
        state = state.with_property(*key, *value);
    }
    for x in -16..32 {
        for z in -16..32 {
            for py in y.0..y.1 {
                assert!(region.set_block(BlockPos::new(x, py, z), state.clone()));
            }
        }
    }
}

/// A netherrack floor below y = 64 and ceiling from y = 80, air between.
fn cavern(region: &mut DecorationRegion<'_>) {
    fill(region, (56, FLOOR_TOP), "netherrack", &[]);
    fill(region, (CEILING_BOTTOM, 84), "netherrack", &[]);
    fill(region, (FLOOR_TOP, CEILING_BOTTOM), "air", &[]);
}

fn inside_window(positions: &[BlockPos]) -> bool {
    positions
        .iter()
        .all(|pos| (-16..32).contains(&pos.x) && (-16..32).contains(&pos.z))
}

const DELTA: &str = r#"{"type":"minecraft:delta_feature","config":{"contents":{"Name":"minecraft:lava","Properties":{"level":"0"}},"rim":{"Name":"minecraft:magma_block"},"rim_size":{"type":"minecraft:uniform","max_inclusive":2,"min_inclusive":0},"size":{"type":"minecraft:uniform","max_inclusive":7,"min_inclusive":3}}}"#;

/// The delta's draw that decides whether the rim is rolled.
fn rim_rolled(seed: i64) -> bool {
    WorldgenRandom::new(seed).next_double() < 0.9
}

#[test]
fn a_delta_patch_lies_on_the_floor_surface_and_is_deterministic() {
    let env = env();
    let origin = BlockPos::new(8, FLOOR_TOP - 1, 8);
    let mut saw_rim = false;
    for seed in 0..24 {
        let first = place(&env, DELTA, seed, origin, cavern);
        let again = place(&env, DELTA, seed, origin, cavern);
        assert_eq!(first.writes, again.writes, "seed {seed}");
        assert!(first.placed, "seed {seed}");
        let mine = first.of(&["lava", "magma_block"]);
        assert!(!mine.is_empty() && inside_window(&mine));
        assert!(mine.iter().all(|pos| pos.y == FLOOR_TOP - 1), "seed {seed}");
        assert!(first.count("lava") > 0, "seed {seed}");
        if rim_rolled(seed) {
            saw_rim |= first.count("magma_block") > 0;
        } else {
            // No rim: the contents are not shifted, so they stay inside the Manhattan diamond.
            assert_eq!(first.count("magma_block"), 0, "seed {seed}");
            let lava = first.of(&["lava"]);
            assert!(
                lava.iter()
                    .all(|pos| (pos.x - 8).abs() + (pos.z - 8).abs() <= 7),
                "seed {seed}"
            );
        }
    }
    assert!(saw_rim, "no seed rolled a rim");
}

#[test]
fn a_delta_needs_air_above_and_solid_around_each_cell() {
    let env = env();
    // The floor reaches y = 65, so no cell near the origin has air directly above it.
    let buried = |region: &mut DecorationRegion<'_>| {
        fill(region, (56, 66), "netherrack", &[]);
        fill(region, (66, CEILING_BOTTOM), "air", &[]);
    };
    let outcome = place(&env, DELTA, 3, BlockPos::new(8, 62, 8), buried);
    assert!(!outcome.placed);
    assert!(outcome.of(&["lava", "magma_block"]).is_empty());
    // A chest is on the blocked list even where its surroundings are right.
    let chests = |region: &mut DecorationRegion<'_>| {
        cavern(region);
        fill(region, (FLOOR_TOP - 1, FLOOR_TOP), "chest", &[]);
    };
    let outcome = place(&env, DELTA, 3, BlockPos::new(8, FLOOR_TOP - 1, 8), chests);
    assert!(!outcome.placed);
}

fn blobs(target: &str, state: &str, props: &str) -> String {
    format!(
        r#"{{"type":"minecraft:netherrack_replace_blobs","config":{{"radius":{{"type":"minecraft:uniform","max_inclusive":7,"min_inclusive":3}},"state":{{"Name":"minecraft:{state}"{props}}},"target":{{"Name":"minecraft:{target}"}}}}}}"#
    )
}

#[test]
fn blobs_sink_to_the_target_and_replace_only_it() {
    let env = env();
    let config = blobs("netherrack", "basalt", r#","Properties":{"axis":"y"}"#);
    let origin = BlockPos::new(8, 70, 8);
    for seed in 0..8 {
        let first = place(&env, &config, seed, origin, cavern);
        assert_eq!(
            first.writes,
            place(&env, &config, seed, origin, cavern).writes
        );
        assert!(first.placed, "seed {seed}");
        let basalt = first.of(&["basalt"]);
        assert!(inside_window(&basalt));
        // The origin sank to the floor's top block, y = 63; the diamond is at most 7 wide.
        assert!(basalt
            .iter()
            .all(|pos| (pos.x - 8).abs() + (pos.y - 63).abs() + (pos.z - 8).abs() <= 7));
        assert!(basalt.iter().all(|pos| pos.y < FLOOR_TOP), "seed {seed}");
        assert!(
            basalt.iter().any(|pos| pos.y == FLOOR_TOP - 1),
            "seed {seed}"
        );
    }
}

#[test]
fn blobs_without_a_target_below_give_up_and_a_high_origin_is_clamped() {
    let env = env();
    let config = blobs("netherrack", "basalt", r#","Properties":{"axis":"y"}"#);
    let none = place(&env, &config, 1, BlockPos::new(8, 70, 8), |_| {});
    assert!(!none.placed && none.writes.is_empty());
    let high = place(&env, &config, 1, BlockPos::new(8, 5000, 8), cavern);
    assert!(high.placed);
}

fn fungus(stem: &str, hat: &str, base: &str, planted: bool) -> String {
    let planted = if planted { r#","planted":true"# } else { "" };
    format!(
        r#"{{"type":"minecraft:huge_fungus","config":{{"decor_state":{{"Name":"minecraft:shroomlight"}},"hat_state":{{"Name":"minecraft:{hat}"}},"replaceable_blocks":{{"type":"minecraft:matching_blocks","blocks":["minecraft:poppy"]}},"stem_state":{{"Name":"minecraft:{stem}","Properties":{{"axis":"y"}}}},"valid_base_block":{{"Name":"minecraft:{base}"}}{planted}}}}}"#
    )
}

fn nylium_floor(region: &mut DecorationRegion<'_>) {
    cavern(region);
    fill(region, (FLOOR_TOP - 1, FLOOR_TOP), "crimson_nylium", &[]);
}

#[test]
fn a_crimson_fungus_grows_a_stem_and_a_wart_hat_on_nylium() {
    let env = env();
    let config = fungus("crimson_stem", "nether_wart_block", "crimson_nylium", false);
    let origin = BlockPos::new(8, FLOOR_TOP, 8);
    let mut vines_seen = false;
    for seed in 0..40 {
        let first = place(&env, &config, seed, origin, nylium_floor);
        assert_eq!(
            first.writes,
            place(&env, &config, seed, origin, nylium_floor).writes
        );
        assert!(first.placed, "seed {seed}");
        let stems = first.of(&["crimson_stem"]);
        assert!(stems.len() >= 4 && inside_window(&stems), "seed {seed}");
        assert!(first.count("nether_wart_block") > 0, "seed {seed}");
        // Nothing grows below the origin or out of the cavern except weeping vines.
        let grown = first.of(&["crimson_stem", "nether_wart_block", "shroomlight"]);
        assert!(grown.iter().all(|pos| pos.y >= FLOOR_TOP), "seed {seed}");
        for (pos, state) in &first.writes {
            if state.name.path() == "weeping_vines" {
                vines_seen = true;
                let age: i32 = state.properties["age"].parse().unwrap();
                assert!((23..=25).contains(&age), "seed {seed} at {pos:?}");
            }
        }
    }
    assert!(vines_seen, "no seed grew weeping vines");
}

#[test]
fn a_warped_fungus_grows_no_vines_and_needs_its_base() {
    let env = env();
    let config = fungus("warped_stem", "warped_wart_block", "warped_nylium", false);
    let origin = BlockPos::new(8, FLOOR_TOP, 8);
    let on_warped = |region: &mut DecorationRegion<'_>| {
        cavern(region);
        fill(region, (FLOOR_TOP - 1, FLOOR_TOP), "warped_nylium", &[]);
    };
    for seed in 0..20 {
        let outcome = place(&env, &config, seed, origin, on_warped);
        assert!(
            outcome.placed && outcome.count("weeping_vines") == 0,
            "seed {seed}"
        );
    }
    // Plain netherrack under the origin is not the valid base.
    let wrong = place(&env, &config, 1, origin, cavern);
    assert!(!wrong.placed && wrong.of(&["warped_stem", "warped_wart_block"]).is_empty());
}

#[test]
fn a_planted_fungus_stays_unsupported() {
    let env = env();
    let origin = BlockPos::new(8, FLOOR_TOP, 8);
    let planted = place(
        &env,
        &fungus("crimson_stem", "nether_wart_block", "crimson_nylium", true),
        1,
        origin,
        nylium_floor,
    );
    assert!(!planted.placed);
    assert_eq!(planted.tally.get("feature:minecraft:huge_fungus"), 1);
}

/// Every 26.2 configured feature of the Nether types compiles to [`Feature::Nether`], but for the
/// planted fungi, which compile to [`Feature::Unsupported`].
#[test]
fn every_reference_nether_feature_compiles() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference");
    if !root
        .join("data/minecraft/worldgen/configured_feature")
        .is_dir()
        || !root.join("block_facts.tsv").is_file()
    {
        eprintln!(
            "skipping: no extracted vanilla export at {}",
            root.display()
        );
        return;
    }
    let datapack = oxide_datapack::load_datapack(&root).expect("reference datapack loads");
    const TYPES: [&str; 9] = [
        "basalt_columns",
        "basalt_pillar",
        "delta_feature",
        "glowstone_blob",
        "huge_fungus",
        "nether_forest_vegetation",
        "netherrack_replace_blobs",
        "twisting_vines",
        "weeping_vines",
    ];
    let (mut ported, mut unsupported) = (0, 0);
    for (id, ir) in datapack.configured_features.iter() {
        let value = serde_json::to_value(ir).unwrap();
        let tag = value["type"].as_str().unwrap().to_string();
        let Some(kind) = tag
            .strip_prefix("minecraft:")
            .filter(|kind| TYPES.contains(kind))
        else {
            continue;
        };
        let wrapper: PlacedFeatureIr = serde_json::from_value(
            serde_json::json!({ "feature": id.to_string(), "placement": [] }),
        )
        .unwrap();
        let placed = Compiler::new(
            &datapack.block_facts,
            &datapack.placed_features,
            &datapack.configured_features,
        )
        .compile_placed(&wrapper)
        .unwrap_or_else(|error| panic!("{id}: {error:#}"));
        let is_ported =
            !(kind == "huge_fungus" && value["config"]["planted"] == serde_json::json!(true));
        match (&*placed.feature, is_ported) {
            (Feature::Nether(_), true) => ported += 1,
            (Feature::Unsupported(_), false) => unsupported += 1,
            (other, _) => panic!("{id} ({kind}) compiled to {other:?}"),
        }
    }
    assert!(ported >= 18, "only {ported} ported Nether features found");
    assert_eq!(unsupported, 2, "the two planted fungi stay unsupported");
}

/// The `age` of a vine head.
fn age(state: &oxide_core::BlockState) -> i32 {
    state.properties["age"].parse().unwrap()
}

/// The last write at `pos`, when there is one.
fn written(outcome: &Outcome, pos: BlockPos) -> Option<&str> {
    outcome
        .writes
        .iter()
        .rev()
        .find(|(p, _)| *p == pos)
        .map(|(_, s)| s.name.path())
}

const LARGE_COLUMNS: &str = r#"{"type":"minecraft:basalt_columns","config":{"height":{"type":"minecraft:uniform","max_inclusive":10,"min_inclusive":5},"reach":{"type":"minecraft:uniform","max_inclusive":3,"min_inclusive":2}}}"#;

#[test]
fn basalt_columns_rise_from_the_floor_and_refuse_a_blocked_base() {
    let env = env();
    let origin = BlockPos::new(8, FLOOR_TOP, 8);
    for seed in 0..10 {
        let first = place(&env, LARGE_COLUMNS, seed, origin, cavern);
        assert_eq!(
            first.writes,
            place(&env, LARGE_COLUMNS, seed, origin, cavern).writes,
            "seed {seed}"
        );
        assert!(first.placed, "seed {seed}");
        let basalt = first.of(&["basalt"]);
        assert!(inside_window(&basalt), "seed {seed}");
        // Every column starts on the floor and stays below the ceiling.
        assert!(
            basalt
                .iter()
                .all(|pos| (FLOOR_TOP..CEILING_BOTTOM).contains(&pos.y)),
            "seed {seed}"
        );
        assert!(
            basalt
                .iter()
                .all(|pos| pos.y == FLOOR_TOP || basalt.contains(&offset(*pos, 0, -1, 0))),
            "seed {seed}"
        );
    }
    let on_soul_sand = |region: &mut DecorationRegion<'_>| {
        cavern(region);
        fill(region, (FLOOR_TOP - 1, FLOOR_TOP), "soul_sand", &[]);
    };
    let refused = place(&env, LARGE_COLUMNS, 1, origin, on_soul_sand);
    assert!(!refused.placed && refused.count("basalt") == 0);
}

#[test]
fn basalt_columns_fill_a_lava_sea_but_not_lava_above_it() {
    let origin = BlockPos::new(8, FLOOR_TOP, 8);
    let flooded = |region: &mut DecorationRegion<'_>| {
        cavern(region);
        fill(
            region,
            (FLOOR_TOP, FLOOR_TOP + 2),
            "lava",
            &[("level", "0")],
        );
    };
    // With the sea level above the lava, the origin counts as open sea.
    let mut sea = env();
    sea.sea_level = FLOOR_TOP + 4;
    let outcome = place(&sea, LARGE_COLUMNS, 3, origin, flooded);
    assert!(outcome.placed);
    assert!(outcome.of(&["basalt"]).iter().any(|pos| pos.y == FLOOR_TOP));
    // At the Nether's sea level of 32 the same lava is a solid obstacle at the origin.
    let dry = place(&env(), LARGE_COLUMNS, 3, origin, flooded);
    assert!(!dry.placed && dry.count("basalt") == 0);
}

#[test]
fn a_basalt_pillar_spans_ceiling_to_floor_with_a_skirt() {
    let env = env();
    let config = r#"{"type":"minecraft:basalt_pillar","config":{}}"#;
    let origin = BlockPos::new(8, CEILING_BOTTOM - 1, 8);
    for seed in 0..10 {
        let first = place(&env, config, seed, origin, cavern);
        assert_eq!(
            first.writes,
            place(&env, config, seed, origin, cavern).writes,
            "seed {seed}"
        );
        assert!(first.placed, "seed {seed}");
        for y in FLOOR_TOP..CEILING_BOTTOM {
            assert_eq!(
                written(&first, BlockPos::new(8, y, 8)),
                Some("basalt"),
                "seed {seed} y {y}"
            );
        }
        let basalt = first.of(&["basalt"]);
        assert!(inside_window(&basalt), "seed {seed}");
        // The skirt replaces floor blocks within three of the pillar, nothing lower.
        assert!(basalt
            .iter()
            .all(|pos| pos.y >= FLOOR_TOP - 1 && (pos.x - 8).abs() <= 3 && (pos.z - 8).abs() <= 3));
    }
    // Needs air at the origin and a block above it.
    let hanging_free = place(&env, config, 1, BlockPos::new(8, 70, 8), cavern);
    assert!(!hanging_free.placed && hanging_free.count("basalt") == 0);
    let buried = place(&env, config, 1, BlockPos::new(8, CEILING_BOTTOM, 8), cavern);
    assert!(!buried.placed && buried.count("basalt") == 0);
}

#[test]
fn a_glowstone_blob_hangs_below_its_ceiling() {
    let env = env();
    let config = r#"{"type":"minecraft:glowstone_blob","config":{}}"#;
    let origin = BlockPos::new(8, CEILING_BOTTOM - 1, 8);
    for seed in 0..10 {
        let first = place(&env, config, seed, origin, cavern);
        assert_eq!(
            first.writes,
            place(&env, config, seed, origin, cavern).writes,
            "seed {seed}"
        );
        assert!(first.placed, "seed {seed}");
        let glow = first.of(&["glowstone"]);
        assert!(glow.len() > 1 && glow.contains(&origin), "seed {seed}");
        assert!(glow.iter().all(|pos| {
            (pos.x - 8).abs() <= 7
                && (pos.z - 8).abs() <= 7
                && (origin.y - 11..=origin.y).contains(&pos.y)
        }));
    }
    let on_chest = |region: &mut DecorationRegion<'_>| {
        cavern(region);
        fill(region, (CEILING_BOTTOM, CEILING_BOTTOM + 1), "chest", &[]);
    };
    assert!(!place(&env, config, 1, origin, on_chest).placed);
    assert!(!place(&env, config, 1, BlockPos::new(8, 70, 8), cavern).placed);
}

fn warped_floor(region: &mut DecorationRegion<'_>) {
    cavern(region);
    fill(region, (FLOOR_TOP - 1, FLOOR_TOP), "warped_nylium", &[]);
}

#[test]
fn twisting_vines_grow_up_from_a_warped_floor() {
    let env = env();
    let config = r#"{"type":"minecraft:twisting_vines","config":{"max_height":8,"spread_height":4,"spread_width":8}}"#;
    let origin = BlockPos::new(8, FLOOR_TOP, 8);
    for seed in 0..10 {
        let first = place(&env, config, seed, origin, warped_floor);
        assert_eq!(
            first.writes,
            place(&env, config, seed, origin, warped_floor).writes,
            "seed {seed}"
        );
        assert!(first.placed, "seed {seed}");
        let vines = first.of(&["twisting_vines", "twisting_vines_plant"]);
        assert!(!vines.is_empty() && inside_window(&vines), "seed {seed}");
        for (pos, state) in &first.writes {
            match state.name.path() {
                "twisting_vines" => {
                    assert!((17..=25).contains(&age(state)), "seed {seed} at {pos:?}")
                }
                // A plant block always carries more vine above it, and stands on the floor or vine.
                "twisting_vines_plant" => {
                    assert!(
                        vines.contains(&offset(*pos, 0, 1, 0)),
                        "seed {seed} at {pos:?}"
                    );
                    assert!(
                        pos.y == FLOOR_TOP || vines.contains(&offset(*pos, 0, -1, 0)),
                        "seed {seed} at {pos:?}"
                    );
                }
                _ => {}
            }
        }
    }
    // Crimson nylium is not a twisting-vine ground.
    let crimson = place(&env, config, 1, origin, nylium_floor);
    assert!(
        !crimson.placed
            && crimson
                .of(&["twisting_vines", "twisting_vines_plant"])
                .is_empty()
    );
}

#[test]
fn weeping_vines_hang_from_a_wart_covered_roof() {
    let env = env();
    let config = r#"{"type":"minecraft:weeping_vines","config":{}}"#;
    let origin = BlockPos::new(8, CEILING_BOTTOM - 1, 8);
    let mut heads = 0;
    for seed in 0..10 {
        let first = place(&env, config, seed, origin, cavern);
        assert_eq!(
            first.writes,
            place(&env, config, seed, origin, cavern).writes,
            "seed {seed}"
        );
        assert!(first.placed, "seed {seed}");
        let wart = first.of(&["nether_wart_block"]);
        assert!(wart.contains(&origin), "seed {seed}");
        assert!(
            wart.iter()
                .all(|pos| (origin.y - 4..=origin.y + 1).contains(&pos.y)),
            "seed {seed}"
        );
        let vines = first.of(&["weeping_vines", "weeping_vines_plant"]);
        assert!(inside_window(&vines), "seed {seed}");
        for (pos, state) in &first.writes {
            match state.name.path() {
                "weeping_vines" => {
                    heads += 1;
                    assert!((17..=25).contains(&age(state)), "seed {seed} at {pos:?}");
                }
                "weeping_vines_plant" => assert!(
                    vines.contains(&offset(*pos, 0, -1, 0)),
                    "seed {seed} at {pos:?}"
                ),
                _ => {}
            }
        }
    }
    assert!(heads > 0, "no seed hung a vine");
    // A roof of anything else, or no roof, places nothing.
    let on_stone = |region: &mut DecorationRegion<'_>| {
        cavern(region);
        fill(region, (CEILING_BOTTOM, CEILING_BOTTOM + 1), "stone", &[]);
    };
    assert!(!place(&env, config, 1, origin, on_stone).placed);
    assert!(!place(&env, config, 1, BlockPos::new(8, 70, 8), cavern).placed);
}

/// Survives where the block below is crimson or warped nylium: the vegetation's own rule,
/// enough to tell placement from refusal.
struct OnNylium;

impl BlockBehaviours for OnNylium {
    fn can_survive(
        &self,
        _: &oxide_core::BlockState,
        region: &DecorationRegion<'_>,
        pos: BlockPos,
    ) -> Option<bool> {
        let below = region.block(offset(pos, 0, -1, 0))?;
        Some(matches!(
            below.name.path(),
            "crimson_nylium" | "warped_nylium"
        ))
    }
}

fn nylium_tags() -> TagSet {
    let root = std::env::temp_dir().join(format!(
        "oxide-nether-tags-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let path = root.join("data/minecraft/tags/block/nylium.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"values":["minecraft:crimson_nylium","minecraft:warped_nylium"]}"#,
    )
    .unwrap();
    let tags = oxide_datapack::load_tags(&[&root], "block").unwrap();
    std::fs::remove_dir_all(root).unwrap();
    tags
}

const CRIMSON_VEGETATION: &str = r#"{"type":"minecraft:nether_forest_vegetation","config":{"spread_height":4,"spread_width":8,"state_provider":{"type":"minecraft:weighted_state_provider","entries":[{"data":{"Name":"minecraft:crimson_roots"},"weight":87},{"data":{"Name":"minecraft:crimson_fungus"},"weight":11},{"data":{"Name":"minecraft:warped_fungus"},"weight":1}]}}}"#;

#[test]
fn forest_vegetation_covers_nylium_where_the_plants_survive() {
    let env = env_with(nylium_tags(), Box::new(OnNylium));
    let origin = BlockPos::new(8, FLOOR_TOP, 8);
    let plants = ["crimson_roots", "crimson_fungus", "warped_fungus"];
    for seed in 0..10 {
        let first = place(&env, CRIMSON_VEGETATION, seed, origin, nylium_floor);
        assert_eq!(
            first.writes,
            place(&env, CRIMSON_VEGETATION, seed, origin, nylium_floor).writes,
            "seed {seed}"
        );
        assert!(first.placed && first.tally.is_empty(), "seed {seed}");
        let placed = first.of(&plants);
        assert!(placed
            .iter()
            .all(|pos| pos.y == FLOOR_TOP && (pos.x - 8).abs() < 8 && (pos.z - 8).abs() < 8));
        assert!(first.count("crimson_roots") > 0, "seed {seed}");
    }
    // Not over nylium: nothing, and no draw-dependent tally.
    let bare = place(&env, CRIMSON_VEGETATION, 1, origin, cavern);
    assert!(!bare.placed && bare.of(&plants).is_empty());
    // Survival unknown: nothing placed, every refusal tallied.
    let unknown = env_with(nylium_tags(), Box::new(NoBlockBehaviours));
    let outcome = place(&unknown, CRIMSON_VEGETATION, 1, origin, nylium_floor);
    assert!(!outcome.placed && outcome.of(&plants).is_empty());
    assert!(outcome
        .tally
        .entries()
        .any(|(key, count)| key.starts_with("survival:") && count > 0));
}
