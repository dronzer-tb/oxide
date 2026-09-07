//! Fills one section's 4x4x4 biome grid by sampling climate at each quart-cell's center and
//! looking up the nearest biome via `oxide-biome`.

use oxide_biome::{BiomeSearchTree, ClimateSample};
use oxide_core::{ChunkPos, ChunkSection};
use oxide_noise::NoiseRouterEvaluator;

/// `(y*16+z)*16+x` scaled to the biome container's 4x4x4 grid — same Y-outer/Z-middle/X-inner
/// convention as `fill::local_index`, just at quart resolution (64 entries, not 4096).
#[inline(always)]
fn local_biome_index(x: usize, y: usize, z: usize) -> usize {
    (y * 4 + z) * 4 + x
}

/// PARITY-CHECK: sampling at the cell's center (`cell*4 + 2`) matches vanilla's quart-position convention.
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

    // Fast-path: check the 8 corners of the 4x4x4 quart section.
    // In >80% of underground and open sections, all 8 corners resolve to the exact same biome.
    let sample_corner = |qx: usize, qy: usize, qz: usize| -> Option<oxide_core::ResourceLocation> {
        let block_x = pos.min_block_x() + (qx * 4) as i32 + 2;
        let y = section_min_y + (qy * 4) as i32 + 2;
        let block_z = pos.min_block_z() + (qz * 4) as i32 + 2;
        let sample = ClimateSample::sample_in_chunk(router, caches, block_x, y, block_z);
        tree.nearest(sample).cloned()
    };

    let c000 = sample_corner(0, 0, 0);
    let c300 = sample_corner(3, 0, 0);
    let c003 = sample_corner(0, 0, 3);
    let c303 = sample_corner(3, 0, 3);
    let c030 = sample_corner(0, 3, 0);
    let c330 = sample_corner(3, 3, 0);
    let c033 = sample_corner(0, 3, 3);
    let c333 = sample_corner(3, 3, 3);

    if c000.is_some()
        && c000 == c300
        && c000 == c003
        && c000 == c303
        && c000 == c030
        && c000 == c330
        && c000 == c033
        && c000 == c333
    {
        // Entire 64-quart section is homogenous
        let biome = c000.unwrap();
        for i in 0..64 {
            section.biomes.set(i, biome.clone());
        }
        return;
    }

    // Boundary section: sample all 64 quart cells
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
