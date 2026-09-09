//! Assembles real vanilla structures from real vanilla data.
//!
//! The unit tests in `jigsaw.rs` drive the placer with three-block toy rooms, which proves the
//! algorithm but not that it survives contact with Mojang's own pools: 1200-odd templates,
//! weights up to 150, fallback pools, `aligned` joints, selection and placement priorities, and
//! connector names that only match in one direction.
//!
//! Needs the extracted export at `reference/` — both `worldgen/template_pool` and the `.nbt`
//! templates under `data/<ns>/structure/`. Skips without them, like every other
//! reference-backed test here; the data is Mojang's and never enters the repo.

use oxide_core::{BlockPos, LegacyRandom, ResourceLocation};
use oxide_structures::jigsaw::{assemble_jigsaw, JigsawSettings, SurfaceHeights};
use oxide_structures::JigsawData;
use std::path::Path;

fn reference() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../reference"))
}

struct FlatGround(i32);
impl SurfaceHeights for FlatGround {
    fn first_free_height(&self, _x: i32, _z: i32) -> i32 {
        self.0
    }
}

fn load() -> Option<JigsawData> {
    if !reference().join("data/minecraft/structure").is_dir() {
        eprintln!(
            "skipping: no extracted structure templates under {}",
            reference().display()
        );
        return None;
    }
    let (data, skipped) = JigsawData::load(reference()).unwrap();
    eprintln!(
        "loaded {} templates, {} pools ({skipped} unparsable)",
        data.templates.len(),
        data.pools.len()
    );
    Some(data)
}

fn village_settings() -> JigsawSettings {
    // `minecraft:village_plains`: start pool village/plains/town_centers, size 6, and the
    // expansion hack on -- the values from the structure definition, not invented ones.
    JigsawSettings {
        max_depth: 6,
        max_distance_horizontal: 80,
        max_distance_vertical: 80,
        do_expansion_hack: true,
        project_start_to_heightmap: true,
        min_y: -64,
        max_y: 319,
    }
}

#[test]
fn a_plains_village_grows_into_many_connected_pieces() {
    let Some(data) = load() else { return };
    let start = ResourceLocation::minecraft("village/plains/town_centers");
    assert!(
        data.pools.contains_key(&start),
        "the export has no plains town_centers pool"
    );

    let mut placed_counts = Vec::new();
    for seed in [1i64, 1234, 99_999, -4242, 4489057056054590644] {
        let mut random = LegacyRandom::new(seed);
        let structure = assemble_jigsaw(
            &start,
            BlockPos::new(0, 0, 0),
            &village_settings(),
            &mut random,
            &data,
            &FlatGround(64),
        )
        .expect("plains town centers must assemble");

        assert!(
            structure.pieces.len() > 5,
            "seed {seed} produced only {} pieces -- a village is a town centre plus streets \
             plus houses, so a handful means connectors are not matching",
            structure.pieces.len()
        );

        // Every piece must actually be a template the pack ships, placed somewhere.
        for piece in &structure.pieces {
            assert!(
                data.templates.contains_key(&piece.template),
                "seed {seed}: placed a template that does not exist: {}",
                piece.template
            );
        }

        // Two pieces may never *partially* overlap: they are either disjoint, or one sits
        // wholly inside the other. Nesting is legitimate and load-bearing -- a connector whose
        // target block falls inside its own piece shares that piece's free space, which is how
        // a town centre gets its cats and villagers -- but two buildings sharing a corner is
        // the collision failure the free-space machinery exists to prevent, and the one the old
        // stub produced on every structure it built.
        for (i, a) in structure.pieces.iter().enumerate() {
            for b in &structure.pieces[i + 1..] {
                if !a.bbox.intersects(&b.bbox) {
                    continue;
                }
                assert!(
                    a.bbox.contains_box(&b.bbox) || b.bbox.contains_box(&a.bbox),
                    "seed {seed}: {} at {:?} partially overlaps {} at {:?}",
                    a.template,
                    a.bbox,
                    b.template,
                    b.bbox
                );
            }
        }

        placed_counts.push(structure.pieces.len());
    }

    // Different seeds must build different villages, not the same one over and over.
    assert!(
        placed_counts.windows(2).any(|w| w[0] != w[1]),
        "every seed produced the same piece count {placed_counts:?}"
    );
}

#[test]
fn a_village_sits_on_the_ground_it_was_given() {
    let Some(data) = load() else { return };
    let ground = 71;
    let mut random = LegacyRandom::new(31337);
    let structure = assemble_jigsaw(
        &ResourceLocation::minecraft("village/plains/town_centers"),
        BlockPos::new(0, 0, 0),
        &village_settings(),
        &mut random,
        &data,
        &FlatGround(ground),
    )
    .unwrap();

    let start = structure.pieces[0].bbox;
    // `projectStartToHeightmap` puts the start piece's ground level on the surface, one block
    // below it being the floor the fountain stands on.
    assert!(
        (start.min_y - (ground - 1)).abs() <= 1,
        "the town centre floor is at {} for ground {ground}",
        start.min_y
    );
    assert!(
        structure.pieces.iter().all(|p| p.bbox.min_y > ground - 40),
        "a piece fell far below the surface"
    );
}

#[test]
fn an_outpost_and_an_ancient_city_also_assemble() {
    let Some(data) = load() else { return };

    // The outpost is a single tower plus features: a much smaller graph than a village, and it
    // uses `aligned` joints, so it exercises the top-facing half of canAttach.
    let mut random = LegacyRandom::new(7);
    let outpost = assemble_jigsaw(
        &ResourceLocation::minecraft("pillager_outpost/base_plates"),
        BlockPos::new(0, 0, 0),
        &JigsawSettings {
            max_depth: 7,
            do_expansion_hack: false,
            ..village_settings()
        },
        &mut random,
        &data,
        &FlatGround(70),
    );
    assert!(outpost.is_some(), "the outpost start pool must resolve");

    // The ancient city is deep underground and rigid throughout, so nothing is projected to a
    // heightmap -- a different path through the Y arithmetic.
    let mut random = LegacyRandom::new(11);
    let city = assemble_jigsaw(
        &ResourceLocation::minecraft("ancient_city/city_center"),
        BlockPos::new(0, -51, 0),
        &JigsawSettings {
            max_depth: 3,
            project_start_to_heightmap: false,
            max_distance_horizontal: 116,
            ..village_settings()
        },
        &mut random,
        &data,
        &FlatGround(64),
    )
    .expect("the ancient city start pool must resolve");
    assert!(
        city.pieces.len() > 1,
        "the ancient city centre placed nothing around itself"
    );
    assert!(
        city.pieces.iter().all(|p| p.bbox.min_y < 0),
        "the ancient city must stay underground"
    );
}
