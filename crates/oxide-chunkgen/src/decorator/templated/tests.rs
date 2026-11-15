//! Template placement on hand-built templates, and (when the local 26.2 reference tree is
//! present) the real `fossil` and `template` features compiled and placed.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use oxide_core::rotation::Rotation;
use oxide_core::{BlockPos, BlockState, ChunkPos, RandomSource, ResourceLocation, WorldgenRandom};
use oxide_datapack::structure_template::{RawBlockEntry, RawBlockState, RawStructureNbt};
use oxide_datapack::{load_tags, BlockFacts, TagSet};

use super::*;
use crate::decorator::context::{Environment, NoBlockBehaviours, UnsupportedTally};
use crate::decorator::feature::{Compiler, Feature};
use crate::region::testutil::{flat_ring, refs};
use crate::region::{BlockWrite, DecorationRegion};

const SURFACE: i32 = 64;

fn state(path: &str, properties: &[(&str, &str)]) -> BlockState {
    properties.iter().fold(
        BlockState::new(ResourceLocation::minecraft(path)),
        |s, (k, v)| s.with_property(*k, *v),
    )
}

fn temp_dir(what: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "oxide-templated-{what}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ))
}

fn facts() -> BlockFacts {
    let dir = temp_dir("facts");
    std::fs::create_dir_all(&dir).unwrap();
    let full = "smoudf1234k";
    let mut text = String::from(
        "minecraft:air\tDarck\nminecraft:void_air\tDarck\nminecraft:water[level=0]\tDlWSk\n\
         minecraft:bone_block[axis=x]\tsmoudf1234k\nminecraft:bone_block[axis=y]\tDsmoudf1234k\n\
         minecraft:bone_block[axis=z]\tsmoudf1234k\n\
         minecraft:sulfur_spike[thickness=tip,vertical_direction=up,waterlogged=false]\tDsmy\n\
         minecraft:sulfur_spike[thickness=tip,vertical_direction=up,waterlogged=true]\tsmWSy\n",
    );
    for name in [
        "stone",
        "coal_ore",
        "deepslate_diamond_ore",
        "magma_block",
        "bedrock",
    ] {
        text.push_str(&format!("minecraft:{name}\tD{full}\n"));
    }
    for (value, default) in [
        ("dry", "D"),
        ("wet", ""),
        ("dormant", ""),
        ("erupting", ""),
        ("continuous", ""),
    ] {
        text.push_str(&format!(
            "minecraft:potent_sulfur[potent_sulfur_state={value}]\t{default}smoudf1234e\n"
        ));
    }
    std::fs::write(dir.join("block_facts.tsv"), text).unwrap();
    let facts = BlockFacts::load(&dir).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    facts
}

fn tags() -> TagSet {
    let root = temp_dir("tags");
    let _ = std::fs::remove_dir_all(&root);
    for (name, values) in [
        ("features_cannot_replace", "\"minecraft:bedrock\""),
        ("speleothems", "\"minecraft:sulfur_spike\""),
        (
            "causes_periodic_geyser_eruptions",
            "\"minecraft:magma_block\"",
        ),
        ("causes_continuous_geyser_eruptions", "\"minecraft:lava\""),
    ] {
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

fn env_with(facts: BlockFacts, block_tags: TagSet) -> Environment {
    Environment {
        behaviours: Box::new(NoBlockBehaviours),
        block_tags,
        fluid_tags: TagSet::default(),
        biome_tags: TagSet::default(),
        facts,
        seed: 0,
        sea_level: 63,
        placed: HashMap::new(),
        biome_climate: HashMap::new(),
        biome_features: HashMap::new(),
    }
}

fn env() -> Environment {
    env_with(facts(), tags())
}

/// A template from `(pos, palette state)` pairs, all in one palette.
fn template(size: [i32; 3], blocks: &[([i32; 3], BlockState)], facts: &BlockFacts) -> Template {
    let mut palette: Vec<RawBlockState> = Vec::new();
    let mut entries = Vec::new();
    for (pos, block) in blocks {
        let raw = RawBlockState {
            name: block.name.to_string(),
            properties: block.properties.clone(),
        };
        let index = palette
            .iter()
            .position(|p| p.name == raw.name && p.properties == raw.properties)
            .unwrap_or_else(|| {
                palette.push(raw);
                palette.len() - 1
            });
        entries.push(RawBlockEntry {
            pos: *pos,
            state: index,
            nbt: None,
        });
    }
    let raw = RawStructureNbt {
        data_version: 0,
        size,
        palette,
        palettes: None,
        blocks: entries,
        entities: Vec::new(),
    };
    Template::from_raw(&raw, facts)
}

struct Outcome {
    writes: Vec<BlockWrite>,
    tally: UnsupportedTally,
    placed: bool,
}

impl Outcome {
    fn at(&self, pos: BlockPos) -> Option<&BlockState> {
        self.writes
            .iter()
            .rev()
            .find(|(p, _)| *p == pos)
            .map(|(_, s)| s)
    }
}

/// Runs `body` with a context over a flat stone world (surface y = 64, centre chunk (0, 0))
/// that `prepare` edited first.
fn with_context(
    env: &Environment,
    seed: i64,
    origin: BlockPos,
    prepare: impl FnOnce(&mut DecorationRegion<'_>),
    body: impl FnOnce(&mut FeatureContext<'_, '_, '_>) -> bool,
) -> Outcome {
    let chunks = flat_ring(ChunkPos::new(0, 0), SURFACE);
    let mut region = DecorationRegion::new(ChunkPos::new(0, 0), refs(&chunks), &env.facts);
    prepare(&mut region);
    let before = region.into_writes();
    let mut region = DecorationRegion::new(ChunkPos::new(0, 0), refs(&chunks), &env.facts);
    for (pos, state) in &before {
        region.set_block(*pos, state.clone());
    }
    let mut random = WorldgenRandom::new(seed);
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
        body(&mut cx)
    };
    // The region reports one final state per position, so a position `prepare` touched shows
    // up only if the body changed it.
    let writes = region
        .into_writes()
        .into_iter()
        .filter(|write| !before.contains(write))
        .collect();
    Outcome {
        writes,
        tally,
        placed,
    }
}

fn place(
    env: &Environment,
    template: &Template,
    position: BlockPos,
    settings: PlaceSettings<'_>,
    seed: i64,
    prepare: impl FnOnce(&mut DecorationRegion<'_>),
) -> Outcome {
    let id = ResourceLocation::minecraft("test");
    with_context(env, seed, position, prepare, |cx| {
        place_in_world(template, &id, cx, position, &settings)
    })
}

fn plain(rotation: Rotation) -> PlaceSettings<'static> {
    PlaceSettings {
        rotation,
        bounding_box: None,
        processors: &[],
    }
}

#[test]
fn blocks_are_ordered_full_cubes_first_then_by_y_x_z() {
    let facts = facts();
    let spike = state(
        "sulfur_spike",
        &[
            ("thickness", "tip"),
            ("vertical_direction", "up"),
            ("waterlogged", "false"),
        ],
    );
    let built = template(
        [2, 2, 2],
        &[
            ([1, 1, 0], state("stone", &[])),
            ([0, 0, 1], spike.clone()),
            ([1, 0, 0], state("stone", &[])),
            ([0, 0, 0], state("air", &[])),
            ([0, 1, 1], state("stone", &[])),
        ],
        &facts,
    );
    let order: Vec<[i32; 3]> = built.palettes[0]
        .iter()
        .map(|b| [b.pos.x, b.pos.y, b.pos.z])
        .collect();
    // Stone (full, not dynamic) sorted by y, x, z; then the spike (dynamic) and air (no
    // collision), sorted the same way.
    assert_eq!(
        order,
        vec![[1, 0, 0], [0, 1, 1], [1, 1, 0], [0, 0, 0], [0, 0, 1]]
    );
}

#[test]
fn places_under_each_rotation_and_turns_axes() {
    let env = env();
    let built = template(
        [3, 1, 2],
        &[
            ([0, 0, 0], state("bone_block", &[("axis", "x")])),
            ([2, 0, 1], state("stone", &[])),
        ],
        &env.facts,
    );
    let at = BlockPos::new(8, 80, 8);
    // Where (2, 0, 1) lands about the origin, worked by hand from `StructureTemplate.transform`.
    let cases = [
        (Rotation::None, [2, 0, 1], "x"),
        (Rotation::Clockwise90, [-1, 0, 2], "z"),
        (Rotation::Clockwise180, [-2, 0, -1], "x"),
        (Rotation::Counterclockwise90, [1, 0, -2], "z"),
    ];
    for (rotation, far, axis) in cases {
        let outcome = place(&env, &built, at, plain(rotation), 1, |_| {});
        assert!(outcome.placed, "{rotation:?}");
        assert_eq!(
            outcome.at(at),
            Some(&state("bone_block", &[("axis", axis)])),
            "{rotation:?}"
        );
        let far = BlockPos::new(at.x + far[0], at.y + far[1], at.z + far[2]);
        assert_eq!(outcome.at(far), Some(&state("stone", &[])), "{rotation:?}");
        assert_eq!(outcome.writes.len(), 2, "{rotation:?}");
        assert!(
            outcome.tally.is_empty(),
            "{rotation:?}: {:?}",
            outcome.tally
        );
    }
}

#[test]
fn block_rot_draws_one_float_per_block_after_the_palette_draw() {
    let env = env();
    let blocks: Vec<([i32; 3], BlockState)> =
        (0..12).map(|x| ([x, 0, 0], state("stone", &[]))).collect();
    let built = template([12, 1, 1], &blocks, &env.facts);
    let rot = Processor::compile(
        &serde_json::json!({"processor_type": "minecraft:block_rot", "integrity": 0.5}),
        &env.facts,
    )
    .unwrap()
    .unwrap();
    let processors = [rot];
    let at = BlockPos::new(0, 80, 0);
    let seed = 99;
    let outcome = place(
        &env,
        &built,
        at,
        PlaceSettings {
            processors: &processors,
            ..plain(Rotation::None)
        },
        seed,
        |_| {},
    );

    let mut replay = WorldgenRandom::new(seed);
    replay.next_int_bounded(1);
    let expected: Vec<i32> = (0..12).filter(|_| replay.next_float() <= 0.5).collect();
    let kept: Vec<i32> = outcome.writes.iter().map(|(pos, _)| pos.x).collect();
    assert_eq!(kept, expected);
    assert!(
        !kept.is_empty() && kept.len() < 12,
        "seed {seed} should keep some and drop some: {kept:?}"
    );
}

#[test]
fn protected_blocks_and_rules_act_on_the_world_and_the_template() {
    let env = env();
    let built = template(
        [2, 1, 1],
        &[
            ([0, 0, 0], state("coal_ore", &[])),
            ([1, 0, 0], state("coal_ore", &[])),
        ],
        &env.facts,
    );
    let processors = [
        serde_json::json!({"processor_type": "minecraft:rule", "rules": [{
            "input_predicate": {"predicate_type": "minecraft:block_match", "block": "minecraft:coal_ore"},
            "location_predicate": {"predicate_type": "minecraft:always_true"},
            "output_state": {"Name": "minecraft:deepslate_diamond_ore"}}]}),
        serde_json::json!({"processor_type": "minecraft:protected_blocks", "value": "#minecraft:features_cannot_replace"}),
    ]
    .map(|value| Processor::compile(&value, &env.facts).unwrap().unwrap());
    let at = BlockPos::new(4, 40, 4);
    let bedrock = BlockPos::new(5, 40, 4);
    let outcome = place(
        &env,
        &built,
        at,
        PlaceSettings {
            processors: &processors,
            ..plain(Rotation::None)
        },
        3,
        |region| {
            region.set_block(bedrock, state("bedrock", &[]));
        },
    );
    assert_eq!(outcome.at(at), Some(&state("deepslate_diamond_ore", &[])));
    assert_eq!(outcome.at(bedrock), None, "the bedrock cell is protected");
}

#[test]
fn an_unknown_processor_type_is_reported_by_name() {
    let compiled = Processor::compile(
        &serde_json::json!({"processor_type": "minecraft:gravity"}),
        &facts(),
    )
    .unwrap();
    assert_eq!(compiled.unwrap_err(), "minecraft:gravity");
}

#[test]
fn nothing_is_written_outside_the_region_or_the_bounding_box() {
    let env = env();
    let blocks: Vec<([i32; 3], BlockState)> = (0..40)
        .flat_map(|x| (0..3).map(move |z| ([x, 0, z], state("stone", &[]))))
        .collect();
    let built = template([40, 1, 3], &blocks, &env.facts);
    // From x = 20 the template runs to x = 59, past the window's east edge at x = 31.
    let at = BlockPos::new(20, 80, 0);
    let outcome = place(&env, &built, at, plain(Rotation::None), 5, |_| {});
    assert!(outcome.placed);
    assert!(
        outcome.writes.iter().all(|(pos, _)| pos.x <= 31),
        "a write left the 3x3 window"
    );
    assert_eq!(outcome.writes.len(), 12 * 3);

    let bounds = BoundingBox {
        min: BlockPos::new(0, -64, 0),
        max: BlockPos::new(25, 319, 1),
    };
    let outcome = place(
        &env,
        &built,
        at,
        PlaceSettings {
            bounding_box: Some(bounds),
            ..plain(Rotation::None)
        },
        5,
        |_| {},
    );
    assert!(outcome.writes.iter().all(|(pos, _)| bounds.is_inside(*pos)));
    assert_eq!(outcome.writes.len(), 6 * 2);
}

#[test]
fn containers_take_water_where_they_land_and_from_unlocked_neighbours() {
    let env = env();
    let dry_spike = state(
        "sulfur_spike",
        &[
            ("thickness", "tip"),
            ("vertical_direction", "up"),
            ("waterlogged", "false"),
        ],
    );
    let wet_spike = dry_spike.clone().with_property("waterlogged", "true");
    let water = state("water", &[("level", "0")]);
    let built = template(
        [2, 1, 1],
        &[
            ([0, 0, 0], dry_spike.clone()),
            ([1, 0, 0], dry_spike.clone()),
        ],
        &env.facts,
    );
    let at = BlockPos::new(4, 64, 4);
    let east = BlockPos::new(5, 64, 4);
    // The first lands in a water source; the second lands in air beside world water to the
    // north, which is not one of the template's own (locked) sources.
    let outcome = place(&env, &built, at, plain(Rotation::None), 7, |region| {
        region.set_block(at, water.clone());
        region.set_block(BlockPos::new(5, 64, 3), water.clone());
    });
    assert_eq!(outcome.at(at), Some(&wet_spike));
    assert_eq!(outcome.at(east), Some(&wet_spike));
}

#[test]
fn shape_updates_dry_out_a_potent_sulfur_without_water_above() {
    let env = env();
    let dormant = state("potent_sulfur", &[("potent_sulfur_state", "dormant")]);
    let built = template(
        [1, 2, 1],
        &[
            ([0, 0, 0], state("magma_block", &[])),
            ([0, 1, 0], dormant.clone()),
        ],
        &env.facts,
    );
    let at = BlockPos::new(4, 64, 4);
    let top = BlockPos::new(4, 65, 4);
    let dry = place(&env, &built, at, plain(Rotation::None), 1, |_| {});
    assert_eq!(
        dry.at(top),
        Some(&state("potent_sulfur", &[("potent_sulfur_state", "dry")]))
    );
    assert_eq!(dry.tally.get("block_entity:minecraft:potent_sulfur"), 1);

    let wet = place(&env, &built, at, plain(Rotation::None), 1, |region| {
        region.set_block(BlockPos::new(4, 66, 4), state("water", &[("level", "0")]));
    });
    assert_eq!(
        wet.at(top),
        Some(&dormant),
        "magma below keeps a periodic geyser dormant"
    );
}

#[test]
fn a_missing_template_places_nothing_and_is_tallied() {
    let env = env();
    let id = ResourceLocation::minecraft("nowhere");
    let feature = TemplateFeature {
        entries: vec![(
            TemplateRef {
                id: id.clone(),
                template: None,
            },
            Rotation::VALUES.to_vec(),
            1,
        )],
        total_weight: 1,
    };
    let origin = BlockPos::new(8, 70, 8);
    let outcome = with_context(&env, 11, origin, |_| {}, |cx| feature.place(cx));
    assert!(!outcome.placed);
    assert!(outcome.writes.is_empty());
    assert_eq!(outcome.tally.get("template:minecraft:nowhere"), 1);
}

#[test]
fn a_fossil_sinks_below_the_surface_and_places_base_then_overlay() {
    let env = env();
    let base = template(
        [2, 1, 1],
        &[
            ([0, 0, 0], state("bone_block", &[("axis", "y")])),
            ([1, 0, 0], state("bone_block", &[("axis", "y")])),
        ],
        &env.facts,
    );
    let overlay = template(
        [2, 1, 1],
        &[([1, 0, 0], state("coal_ore", &[]))],
        &env.facts,
    );
    let reference = |name: &str, built: Template| TemplateRef {
        id: ResourceLocation::minecraft(name),
        template: Some(Arc::new(built)),
    };
    let feature = FossilFeature {
        pairs: vec![(reference("base", base), reference("overlay", overlay))],
        fossil_processors: Vec::new(),
        overlay_processors: Vec::new(),
        max_empty_corners: 0,
    };
    let origin = BlockPos::new(8, 100, 8);
    let seed = 21;
    let outcome = with_context(&env, seed, origin, |_| {}, |cx| feature.place(cx));
    assert!(outcome.placed);

    let mut replay = WorldgenRandom::new(seed);
    let rotation = Rotation::get_random(&mut replay);
    replay.next_int_bounded(1);
    let y = SURFACE - 15 - replay.next_int_bounded(10);
    assert!(
        outcome.writes.iter().all(|(pos, _)| pos.y == y),
        "{rotation:?}: {:?}",
        outcome.writes
    );
    assert_eq!(
        outcome
            .writes
            .iter()
            .filter(|(_, s)| s.name.path() == "coal_ore")
            .count(),
        1
    );
    assert_eq!(
        outcome.writes.len(),
        2,
        "two bones, the overlay's ore replacing one of them"
    );
}

fn reference_root() -> Option<std::path::PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference");
    (root.join("data/minecraft/structure/fossil").is_dir()
        && root.join("block_facts.tsv").is_file())
    .then_some(root)
}

/// The templated features under a compiled feature, through selectors and sequences.
fn templated_in(feature: &Feature, out: &mut Vec<*const TemplatedFeature>) {
    match feature {
        Feature::Templated(templated) => out.push(templated),
        Feature::WeightedRandomSelector(entries) => entries
            .iter()
            .for_each(|(placed, _)| templated_in(&placed.feature, out)),
        Feature::Sequence(entries) => entries
            .iter()
            .for_each(|placed| templated_in(&placed.feature, out)),
        _ => {}
    }
}

#[test]
fn the_reference_fossil_and_template_features_compile_and_place() {
    let Some(root) = reference_root() else { return };
    let datapack = oxide_datapack::load_datapack(&root).expect("reference datapack loads");
    let mut compiler = Compiler::new(
        &datapack.block_facts,
        &datapack.placed_features,
        &datapack.configured_features,
    )
    .with_templates(&datapack);
    let mut found = Vec::new();
    for id in ["fossil_coal", "fossil_diamonds", "sulfur_spring"] {
        let feature = compiler
            .configured_by_id(&ResourceLocation::minecraft(id))
            .unwrap()
            .expect("registered");
        templated_in(&feature, &mut found);
        assert!(
            !format!("{feature:?}").contains("Unsupported(\"minecraft:fossil")
                && !format!("{feature:?}").contains("Unsupported(\"minecraft:template")
        );
    }
    assert_eq!(
        found.len(),
        6,
        "two fossils and the sulfur spring's four template features"
    );

    let env = env_with(datapack.block_facts.clone(), datapack.block_tags.clone());
    let fossil = compiler
        .configured_by_id(&ResourceLocation::minecraft("fossil_coal"))
        .unwrap()
        .unwrap();
    let Feature::Templated(TemplatedFeature::Fossil(fossil)) = &*fossil else {
        panic!("fossil_coal compiled to {fossil:?}")
    };
    let outcome = with_context(
        &env,
        5,
        BlockPos::new(8, 40, 8),
        |_| {},
        |cx| fossil.place(cx),
    );
    assert!(outcome.placed);
    assert!(outcome
        .writes
        .iter()
        .any(|(_, s)| s.name.path() == "bone_block"));
    assert!(outcome.writes.iter().all(|(pos, _)| pos.y < SURFACE - 14));
    assert!(outcome.tally.is_empty(), "{:?}", outcome.tally);

    let spring = compiler
        .configured_by_id(&ResourceLocation::minecraft("sulfur_spring"))
        .unwrap()
        .unwrap();
    let mut templates = Vec::new();
    templated_in(&spring, &mut templates);
    // SAFETY-free: the pointers came from `spring`, which is alive for this whole scope.
    let template = templates.iter().find_map(|&ptr| match unsafe { &*ptr } {
        TemplatedFeature::Template(template) => Some(template),
        TemplatedFeature::Fossil(_) => None,
    });
    let template = template.expect("a template feature");
    let outcome = with_context(
        &env,
        5,
        BlockPos::new(8, 40, 8),
        |_| {},
        |cx| template.place(cx),
    );
    assert!(outcome.placed);
    assert!(outcome
        .writes
        .iter()
        .any(|(_, s)| s.name.path() == "potent_sulfur"));
    assert!(outcome.tally.get("block_entity:minecraft:potent_sulfur") > 0);
}
