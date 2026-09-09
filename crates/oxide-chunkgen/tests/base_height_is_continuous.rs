//! `base_height` must describe one continuous world, not one answer per chunk.
//!
//! It once did the latter: it asked for its density caches with `chunk_caches(x >> 4 << 4, ...)`
//! — a block coordinate where the method wants a chunk coordinate, so the cache origin landed 16x
//! too far out. Cell lookups clamp to the grid rather than fail, so every column of a chunk read
//! the same clamped corner and returned the same height. Seed 1234 at z=8 gave 10 for all of
//! chunk -1, 76 for all of chunk 1, and 256 — the world ceiling — for all of chunk 3, while chunk
//! 0 alone looked right because `0 >> 4 << 4` is 0. Bukkit asks this per column when it places
//! structures, so villages sat on stilts or inside hillsides everywhere except chunk 0.
//!
//! Needs the extracted vanilla export at `reference/`; skips without it, like the other
//! reference-backed tests in this workspace.

use oxide_core::HeightmapType;
use oxide_datapack::{load_datapack, ResourceLocation};
use oxide_noise::NoiseRouterEvaluator;
use std::path::Path;
use std::str::FromStr;

fn reference() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../reference"))
}

#[test]
fn base_height_varies_within_a_chunk_and_does_not_jump_at_its_borders() {
    if !reference().is_dir() {
        eprintln!("skipping: no extracted vanilla export at {}", reference().display());
        return;
    }

    let pack = load_datapack(reference()).unwrap();
    let dimension = pack
        .dimensions
        .get(&ResourceLocation::from_str("minecraft:overworld").unwrap())
        .unwrap();
    let settings = pack
        .noise_settings
        .get(dimension.generator.settings.as_ref().unwrap())
        .unwrap()
        .clone();
    let router = NoiseRouterEvaluator::new(
        1234,
        &settings,
        &pack.density_functions,
        &pack.noise_params,
    );

    let z = 8;
    let heights: Vec<i32> = (-64..64)
        .map(|x| oxide_chunkgen::base_height(x, z, &settings, &router, HeightmapType::OceanFloorWg))
        .collect();

    // Terrain changes between neighbouring columns of the same chunk. Flat ground exists, so
    // this asks the strip as a whole rather than every chunk -- but with the bug *no* interior
    // step ever moved, because a chunk's 16 columns all read one clamped cell and every change
    // in the world landed exactly on a chunk border.
    let interior_steps = heights
        .windows(2)
        .enumerate()
        .filter(|(i, _)| (*i as i32 - 64).rem_euclid(16) != 15)
        .filter(|(_, pair)| pair[0] != pair[1])
        .count();
    assert!(
        interior_steps > 0,
        "every height change in 128 columns fell on a chunk border -- caches are not per-chunk"
    );

    // Nothing special may happen at x % 16 == 0. Overworld terrain has cliffs, so this bounds
    // the step rather than forbidding one; the bug produced steps of 40 to 220.
    let (worst_x, worst_step) = heights
        .windows(2)
        .enumerate()
        .map(|(i, pair)| (i as i32 - 64, (pair[1] - pair[0]).abs()))
        .max_by_key(|(_, step)| *step)
        .unwrap();
    assert!(
        worst_step < 32,
        "height jumps {worst_step} blocks between x={worst_x} and x={}",
        worst_x + 1
    );
}
