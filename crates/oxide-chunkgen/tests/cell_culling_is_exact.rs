//! Cell culling must be a pure speedup: every block it decides without evaluating density has
//! to match exactly what the per-block path would have written.
//!
//! The culling reads the min/max of a cell's eight corners out of the `interpolated` cache and
//! concludes "all solid" or "all air" for every position inside, skipping the density
//! evaluation and the aquifer query. That rests on trilinear interpolation never leaving the
//! range of its corners. This test does not take it on trust -- it generates real chunks both
//! ways, `fill_chunk_culling(.., cull = true)` against `cull = false`, and compares every
//! block. A shortcut that silently changed terrain would be far worse than a slow one.

use std::path::Path;

use oxide_chunkgen::fill_chunk_culling;
use oxide_core::{ChunkPos, ResourceLocation};
use oxide_datapack::{load_datapack, BiomeSource};

fn reference() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../reference"))
}

#[test]
fn culled_and_exact_fills_agree_block_for_block() {
    if !reference().is_dir() {
        eprintln!(
            "skipping: no extracted vanilla export at {}",
            reference().display()
        );
        return;
    }

    let datapack = load_datapack(reference()).expect("load reference datapack");
    let dim = ResourceLocation::minecraft("overworld");
    let dimension = datapack.dimensions.get(&dim).expect("overworld dimension");
    let settings_id = dimension
        .generator
        .settings
        .as_ref()
        .expect("dimension has noise settings");
    let settings = datapack
        .noise_settings
        .get(settings_id)
        .expect("settings present")
        .clone();

    let router = oxide_noise::NoiseRouterEvaluator::new(
        1234,
        &settings,
        &datapack.density_functions,
        &datapack.noise_params,
    );

    let biome_tree = match dimension.generator.biome_source.as_ref() {
        Some(BiomeSource::MultiNoise(source)) => oxide_biome::BiomeSearchTree::from_source(source),
        _ => None,
    };

    // Culling only bites where a cell is uniform, so this spans several coordinates -- ocean,
    // inland and far-out terrain -- rather than trusting one chunk to exercise both branches.
    for (cx, cz) in [(0, 0), (7, -3), (-12, 5), (100, 100), (-64, -64), (2000, -2000)] {
        let pos = ChunkPos::new(cx, cz);

        // Each side gets its own caches and aquifer: the aquifer is `&mut` and order-sensitive,
        // so sharing one between the two runs would not be a fair comparison.
        let caches_a = router.chunk_caches(cx, cz);
        let mut aq_a = oxide_chunkgen::aquifer_for_settings(pos, &settings, &router, &caches_a);
        let culled = fill_chunk_culling(
            pos,
            &settings,
            &router,
            biome_tree.as_ref(),
            &caches_a,
            aq_a.as_mut(),
            true,
        );

        let caches_b = router.chunk_caches(cx, cz);
        let mut aq_b = oxide_chunkgen::aquifer_for_settings(pos, &settings, &router, &caches_b);
        let exact = fill_chunk_culling(
            pos,
            &settings,
            &router,
            biome_tree.as_ref(),
            &caches_b,
            aq_b.as_mut(),
            false,
        );

        assert_eq!(
            culled.sections.len(),
            exact.sections.len(),
            "chunk {cx},{cz}: section count differs"
        );

        let mut compared = 0usize;
        for (si, (sa, sb)) in culled.sections.iter().zip(exact.sections.iter()).enumerate() {
            assert_eq!(
                sa.block_states.len(),
                sb.block_states.len(),
                "chunk {cx},{cz} section {si}: block count differs"
            );
            for i in 0..sa.block_states.len() {
                assert_eq!(
                    sa.block_states.get(i),
                    sb.block_states.get(i),
                    "chunk {cx},{cz} section {si} slot {i}: culled path wrote a different block \
                     than the exact path"
                );
                compared += 1;
            }
        }
        assert!(
            compared > 90_000,
            "chunk {cx},{cz}: only {compared} blocks compared, expected a full column"
        );

        assert_eq!(
            culled.heightmaps, exact.heightmaps,
            "chunk {cx},{cz}: heightmaps differ, so the culled fill produced different terrain"
        );
    }
}
