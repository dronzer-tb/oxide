//! Tests for the End features, on synthetic terrain with fixed seeds: what each places, that the
//! same seed gives the same writes, that nothing lands outside the region, and that every 26.2
//! configured feature of these types compiles when the reference export is present.

use std::collections::HashMap;
use std::path::Path;

use oxide_core::{BlockPos, BlockState, ChunkPos, WorldgenRandom};
use oxide_datapack::placement::PlacedFeature as PlacedFeatureIr;
use oxide_datapack::{load_tags, BlockFacts, Registry, TagSet};

use super::super::*;
use super::spike::spikes_for_seed;
use crate::region::testutil::{flat_ring, refs};
use crate::region::{BlockWrite, DecorationRegion};

struct AlwaysSurvives;

impl BlockBehaviours for AlwaysSurvives {
    fn can_survive(&self, _: &BlockState, _: &DecorationRegion<'_>, _: BlockPos) -> Option<bool> {
        Some(true)
    }
}

/// Facts for the blocks these tests touch, in the exporter's format.
fn facts() -> BlockFacts {
    let dir = std::env::temp_dir().join(format!(
        "oxide-end-facts-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let full = "Dsmoudf1234";
    let mut text = String::from(
        "minecraft:air\tDarc\nminecraft:cave_air\tDarc\nminecraft:void_air\tDarc\n\
         minecraft:chorus_plant[down=false,east=false,north=false,south=false,up=false,west=false]\tDsmd1234\n\
         minecraft:chorus_flower[age=0]\tDsm1234\n\
         minecraft:iron_bars[east=false,north=false,south=false,waterlogged=false,west=false]\tDsmd1234\n\
         minecraft:fire[age=0,east=false,north=false,south=false,up=false,west=false]\tDrc1\n\
         minecraft:chest[facing=north,type=single,waterlogged=false]\tDsm1234\n\
         minecraft:torch\tDrc1\n\
         minecraft:end_gateway\tDc1\n",
    );
    for name in ["stone", "cobblestone", "end_stone", "obsidian", "bedrock"] {
        text.push_str(&format!("minecraft:{name}\t{full}\n"));
    }
    std::fs::write(dir.join("block_facts.tsv"), text).unwrap();
    let facts = BlockFacts::load(&dir).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    facts
}

fn tags() -> TagSet {
    let root = std::env::temp_dir().join(format!(
        "oxide-end-tags-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let path = root.join("data/minecraft/tags/block/supports_chorus_plant.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, r#"{"values":["minecraft:end_stone"]}"#).unwrap();
    let set = load_tags(&[&root], "block").unwrap();
    let _ = std::fs::remove_dir_all(&root);
    set
}

fn env(behaviours: Box<dyn BlockBehaviours>) -> Environment {
    Environment {
        block_tags: tags(),
        fluid_tags: TagSet::default(),
        biome_tags: TagSet::default(),
        facts: facts(),
        seed: 0,
        sea_level: 63,
        behaviours,
        placed: HashMap::new(),
        biome_climate: HashMap::new(),
        biome_features: Default::default(),
    }
}

struct Outcome {
    writes: Vec<BlockWrite>,
    tally: UnsupportedTally,
    placed: bool,
}

impl Outcome {
    fn count(&self, path: &str) -> usize {
        self.writes
            .iter()
            .filter(|(_, state)| state.name.path() == path)
            .count()
    }

    fn at(&self, x: i32, y: i32, z: i32) -> Option<&BlockState> {
        self.writes
            .iter()
            .rev()
            .find(|(pos, _)| *pos == BlockPos::new(x, y, z))
            .map(|(_, state)| state)
    }

    fn inside_window(&self) -> bool {
        self.writes
            .iter()
            .all(|(pos, _)| (-16..32).contains(&pos.x) && (-16..32).contains(&pos.z))
    }
}

/// Places the configured feature `config` at `origin` over flat stone (surface y = 64, chunk
/// (0, 0)) seeded with `seed`, after `prepare` has edited the region.
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
        !matches!(&*placed.feature, Feature::Unsupported(_)),
        "{config} did not compile to a family"
    );
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

fn state(path: &str) -> BlockState {
    BlockState::new(oxide_core::ResourceLocation::minecraft(path))
}

/// An end-stone island: a 16x16 slab at y = 63, the rest of the world above it air.
fn island(region: &mut DecorationRegion<'_>) {
    for x in 0..16 {
        for z in 0..16 {
            assert!(region.set_block(BlockPos::new(x, 63, z), state("end_stone")));
        }
    }
}

const CHORUS: &str = r#"{"type":"minecraft:chorus_plant","config":{}}"#;

#[test]
fn a_chorus_plant_grows_from_end_stone_and_ends_in_flowers() {
    let env = env(Box::new(NoBlockBehaviours));
    let origin = BlockPos::new(8, 64, 8);
    let outcome = place(&env, CHORUS, 3, origin, island);
    assert!(outcome.placed);
    assert!(
        outcome.count("chorus_plant") >= 3,
        "a root and at least the first stem"
    );
    let flowers: Vec<_> = outcome
        .writes
        .iter()
        .filter(|(_, s)| s.name.path() == "chorus_flower")
        .collect();
    assert!(!flowers.is_empty());
    assert!(flowers
        .iter()
        .all(|(pos, s)| s.properties["age"] == "5" && pos.y > origin.y));
    let root = outcome.at(8, 64, 8).expect("root written");
    assert_eq!(root.name.path(), "chorus_plant");
    assert_eq!(root.properties["down"], "true");
    assert_eq!(root.properties["up"], "true");
    assert!(outcome
        .writes
        .iter()
        .all(|(pos, _)| (pos.x - 8).abs() <= 8 && (pos.z - 8).abs() <= 8));
    assert!(outcome.inside_window());
    let again = place(&env, CHORUS, 3, origin, island);
    assert_eq!(outcome.writes, again.writes);
    assert!((0..20).any(|seed| place(&env, CHORUS, seed, origin, island).writes != outcome.writes));
}

#[test]
fn a_chorus_plant_needs_air_and_a_supporting_block() {
    let env = env(Box::new(NoBlockBehaviours));
    // Flat stone: stone is not in the support tag.
    let unsupported = place(&env, CHORUS, 3, BlockPos::new(8, 64, 8), |_| {});
    assert!(!unsupported.placed && unsupported.writes.is_empty());
    let blocked = place(&env, CHORUS, 3, BlockPos::new(8, 64, 8), |region| {
        island(region);
        region.set_block(BlockPos::new(8, 64, 8), state("stone"));
    });
    assert!(!blocked.placed && blocked.count("chorus_plant") == 0);
}

const ISLAND: &str = r#"{"type":"minecraft:end_island","config":{}}"#;

#[test]
fn an_end_island_is_shrinking_end_stone_discs_below_the_origin() {
    let env = env(Box::new(NoBlockBehaviours));
    let origin = BlockPos::new(8, 100, 8);
    let outcome = place(&env, ISLAND, 9, origin, |_| {});
    assert!(outcome.placed);
    assert_eq!(outcome.count("end_stone"), outcome.writes.len());
    assert!(outcome.writes.iter().all(|(pos, _)| pos.y <= 100));
    let layer = |y: i32| outcome.writes.iter().filter(|(pos, _)| pos.y == y).count();
    // The top disc has a radius of 4, 5 or 6; count its cells the way vanilla does.
    let disc = |radius: f32| {
        let (low, high) = ((-radius).floor() as i32, radius.ceil() as i32);
        (low..=high)
            .flat_map(|x| (low..=high).map(move |z| (x, z)))
            .filter(|(x, z)| ((x * x + z * z) as f32) <= (radius + 1.0) * (radius + 1.0))
            .count()
    };
    assert!([4.0, 5.0, 6.0].iter().any(|r| disc(*r) == layer(100)));
    assert!(layer(99) <= layer(100) && layer(100) > 0);
    assert!(outcome.inside_window());
    assert_eq!(
        outcome.writes,
        place(&env, ISLAND, 9, origin, |_| {}).writes
    );
}

const SPIKE: &str = r#"{"type":"minecraft:end_spike","config":{"spikes":[
    {"centerX":8,"centerZ":8,"radius":2,"height":80,"guarded":false},
    {"centerX":24,"centerZ":8,"radius":3,"height":85,"guarded":true}]}}"#;

#[test]
fn an_end_spike_builds_the_pillars_centred_in_the_origin_chunk() {
    let env = env(Box::new(NoBlockBehaviours));
    let outcome = place(&env, SPIKE, 4, BlockPos::new(0, 64, 0), |_| {});
    assert!(outcome.placed);
    assert_eq!(
        outcome.at(8, 70, 8).map(|s| s.name.path()),
        Some("obsidian")
    );
    assert_eq!(
        outcome.at(8, 79, 8).map(|s| s.name.path()),
        Some("obsidian")
    );
    assert_eq!(outcome.at(8, 80, 8).map(|s| s.name.path()), Some("bedrock"));
    assert_eq!(outcome.at(8, 81, 8).map(|s| s.name.path()), Some("fire"));
    assert_eq!(outcome.at(8, 90, 8).map(|s| s.name.path()), Some("air"));
    // Corner of the 5x5 box is outside the radius-2 disc (4 + 4 > 5).
    assert_eq!(outcome.at(10, 70, 10).map(|s| s.name.path()), Some("air"));
    assert_eq!(
        outcome.at(10, 70, 8).map(|s| s.name.path()),
        Some("obsidian")
    );
    // The second pillar is in chunk 1 and not this chunk's to build; no cage here either.
    assert_eq!(outcome.count("iron_bars"), 0);
    assert_eq!(outcome.tally.get("entity:minecraft:end_crystal"), 1);
    assert!(outcome.inside_window());
    assert_eq!(
        outcome.writes,
        place(&env, SPIKE, 4, BlockPos::new(0, 64, 0), |_| {}).writes
    );
}

#[test]
fn a_guarded_spike_is_caged_in_iron_bars() {
    let env = env(Box::new(NoBlockBehaviours));
    let outcome = place(&env, SPIKE, 4, BlockPos::new(16, 64, 0), |_| {});
    // The 5x5 cage: sides on layers 0..=2 (16 columns) plus the 25-block roof.
    assert_eq!(outcome.count("iron_bars"), 16 * 3 + 25);
    let corner = outcome.at(22, 85, 6).expect("cage corner");
    assert_eq!(corner.properties["north"], "false");
    assert_eq!(corner.properties["south"], "true");
    assert_eq!(corner.properties["west"], "false");
    assert_eq!(corner.properties["east"], "true");
    assert_eq!(outcome.tally.get("entity:minecraft:end_crystal"), 1);
    assert!(outcome.inside_window());
}

#[test]
fn spikes_default_to_the_seed_derived_layout() {
    let spikes = spikes_for_seed(0);
    assert_eq!(spikes.len(), 10);
    let mut heights: Vec<i32> = spikes.iter().map(|s| s.height).collect();
    heights.sort_unstable();
    assert_eq!(
        heights,
        (0..10).map(|size| 76 + size * 3).collect::<Vec<_>>()
    );
    assert_eq!(spikes.iter().filter(|s| s.guarded).count(), 2);
    assert_eq!((spikes[0].center_x, spikes[0].center_z), (42, 0));
    assert_eq!(spikes, spikes_for_seed(0));
    assert_ne!(spikes, spikes_for_seed(12345));
    // With an empty config the world seed decides; none of seed 0's pillars is in chunk (0, 0).
    let env = env(Box::new(NoBlockBehaviours));
    let outcome = place(
        &env,
        r#"{"type":"minecraft:end_spike","config":{"spikes":[]}}"#,
        1,
        BlockPos::new(0, 64, 0),
        |_| {},
    );
    assert!(outcome.placed && outcome.writes.is_empty());
}

#[test]
fn an_end_gateway_is_a_portal_in_a_bedrock_frame() {
    let env = env(Box::new(NoBlockBehaviours));
    let origin = BlockPos::new(8, 100, 8);
    let delayed = place(
        &env,
        r#"{"type":"minecraft:end_gateway","config":{"exact":false}}"#,
        1,
        origin,
        |_| {},
    );
    assert!(delayed.placed);
    assert_eq!(delayed.count("end_gateway"), 1);
    // Two cross-shaped middle layers of five and a single block on each cap.
    assert_eq!(delayed.count("bedrock"), 5 + 5 + 1 + 1);
    assert_eq!(
        delayed.at(8, 100, 8).map(|s| s.name.path()),
        Some("end_gateway")
    );
    assert_eq!(
        delayed.at(8, 102, 8).map(|s| s.name.path()),
        Some("bedrock")
    );
    assert_eq!(delayed.at(9, 102, 8).map(|s| s.name.path()), Some("air"));
    assert_eq!(delayed.at(9, 100, 8).map(|s| s.name.path()), Some("air"));
    assert!(delayed.tally.is_empty());
    let exact = place(
        &env,
        r#"{"type":"minecraft:end_gateway","config":{"exact":true,"exit":[100,50,0]}}"#,
        1,
        origin,
        |_| {},
    );
    assert_eq!(exact.writes, delayed.writes);
    assert_eq!(exact.tally.get("block_entity:minecraft:end_gateway"), 1);
}

#[test]
fn the_end_platform_is_obsidian_over_cleared_air() {
    let env = env(Box::new(NoBlockBehaviours));
    let outcome = place(
        &env,
        r#"{"type":"minecraft:end_platform","config":{}}"#,
        1,
        BlockPos::new(8, 100, 8),
        |_| {},
    );
    assert!(outcome.placed);
    // Air over air is left alone, so only the 5x5 obsidian floor is written.
    assert_eq!(outcome.count("obsidian"), 25);
    assert_eq!(outcome.writes.len(), 25);
    assert!(outcome
        .writes
        .iter()
        .all(|(pos, _)| pos.y == 99 && (6..=10).contains(&pos.x) && (6..=10).contains(&pos.z)));
    let cleared = place(
        &env,
        r#"{"type":"minecraft:end_platform","config":{}}"#,
        1,
        BlockPos::new(8, 100, 8),
        |region| {
            region.set_block(BlockPos::new(8, 101, 8), state("stone"));
        },
    );
    assert_eq!(cleared.at(8, 101, 8).map(|s| s.name.path()), Some("air"));
}

#[test]
fn the_void_start_platform_covers_the_origin_chunk_ring_only() {
    let env = env(Box::new(NoBlockBehaviours));
    let config = r#"{"type":"minecraft:void_start_platform","config":{}}"#;
    let outcome = place(&env, config, 1, BlockPos::new(0, 60, 0), |_| {});
    assert!(outcome.placed);
    assert_eq!(outcome.count("stone") + outcome.count("cobblestone"), 256);
    assert_eq!(outcome.count("cobblestone"), 1);
    assert_eq!(
        outcome.at(8, 63, 8).map(|s| s.name.path()),
        Some("cobblestone")
    );
    let far = place(&env, config, 1, BlockPos::new(48, 60, 0), |_| {});
    assert!(far.writes.is_empty());
}

const BONUS: &str = r#"{"type":"minecraft:bonus_chest","config":{}}"#;

#[test]
fn the_bonus_chest_lands_on_the_surface_with_torches_where_they_survive() {
    let origin = BlockPos::new(0, 64, 0);
    let lit = env(Box::new(AlwaysSurvives));
    let outcome = place(&lit, BONUS, 6, origin, |_| {});
    assert!(outcome.placed);
    assert_eq!(outcome.count("chest"), 1);
    assert_eq!(outcome.count("torch"), 4);
    let (chest, _) = outcome
        .writes
        .iter()
        .find(|(_, s)| s.name.path() == "chest")
        .unwrap();
    assert_eq!(chest.y, 64);
    assert!((0..16).contains(&chest.x) && (0..16).contains(&chest.z));
    assert_eq!(outcome.tally.get("block_entity:minecraft:chest"), 1);
    assert!(outcome.inside_window());
    assert_eq!(outcome.writes, place(&lit, BONUS, 6, origin, |_| {}).writes);
    assert!((0..20).any(|seed| place(&lit, BONUS, seed, origin, |_| {}).writes != outcome.writes));
    // With no survival rules known, torches are not guessed.
    let dark = place(&env(Box::new(NoBlockBehaviours)), BONUS, 6, origin, |_| {});
    assert_eq!(dark.count("chest"), 1);
    assert_eq!(dark.count("torch"), 0);
    assert_eq!(dark.tally.get("survival:minecraft:torch"), 4);
}

/// Every 26.2 configured feature of the End types compiles to a family of this module.
#[test]
fn every_reference_end_feature_compiles() {
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
    const TYPES: [&str; 7] = [
        "chorus_plant",
        "end_island",
        "end_spike",
        "end_gateway",
        "end_platform",
        "void_start_platform",
        "bonus_chest",
    ];
    let mut seen = std::collections::BTreeSet::new();
    for (id, ir) in datapack.configured_features.iter() {
        let tag = serde_json::to_value(ir).unwrap()["type"]
            .as_str()
            .unwrap()
            .to_string();
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
        assert!(
            matches!(&*placed.feature, Feature::End(_)),
            "{id} ({kind}) did not compile to an End feature"
        );
        seen.insert(kind.to_string());
    }
    assert_eq!(seen.len(), TYPES.len(), "found only {seen:?}");
}
