//! Fills one section's 4x4x4 biome grid by sampling climate at each quart-cell's center and
//! looking up the nearest biome via `oxide-biome`.

use oxide_biome::{BiomeSearchTree, ClimateSample};
use oxide_core::{ChunkPos, ChunkSection};
use oxide_noise::NoiseRouterEvaluator;

/// `(y*16+z)*16+x` scaled to the biome container's 4x4x4 grid — same Y-outer/Z-middle/X-inner
/// convention as `fill::local_index`, just at quart resolution (64 entries, not 4096).
fn local_biome_index(x: usize, y: usize, z: usize) -> usize {
    (y * 4 + z) * 4 + x
}

/// PARITY-CHECK: sampling at the cell's center (`cell*4 + 2`) is reconstructed from memory of
/// vanilla's quart-position convention; not verified against a real 26.2 biome-grid dump.
pub(crate) fn fill_biomes(
    section: &mut ChunkSection,
    pos: ChunkPos,
    section_min_y: i32,
    router: &NoiseRouterEvaluator,
    caches: &oxide_noise::ChunkCaches,
    tree: &BiomeSearchTree,
) {
    if tree.is_empty() {
        return;
    }
    for qy in 0..4usize {
        let y = section_min_y + (qy * 4) as i32 + 2;
        for qz in 0..4usize {
            let block_z = pos.min_block_z() + (qz * 4) as i32 + 2;
            for qx in 0..4usize {
                let block_x = pos.min_block_x() + (qx * 4) as i32 + 2;
                let sample = ClimateSample::sample_in_chunk(router, caches, block_x, y, block_z);
                if let Some(biome) = tree.nearest(sample) {
                    section
                        .biomes
                        .set(local_biome_index(qx, qy, qz), biome.clone());
                }
            }
        }
    }
}
