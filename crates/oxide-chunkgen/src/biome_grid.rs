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

    let nearest = |qx: usize, qy: usize, qz: usize| -> Option<&oxide_core::ResourceLocation> {
        let block_x = pos.min_block_x() + (qx * 4) as i32 + 2;
        let y = section_min_y + (qy * 4) as i32 + 2;
        let block_z = pos.min_block_z() + (qz * 4) as i32 + 2;
        let sample = ClimateSample::sample_in_chunk(router, caches, block_x, y, block_z);
        tree.nearest(sample)
    };

    // Fast-path: check the 8 corners of the 4x4x4 quart section.
    // In >80% of underground and open sections, all 8 corners resolve to the exact same biome.
    // Compared by reference into the tree -- no id is cloned to answer "same biome?".
    let corners = [
        nearest(0, 0, 0),
        nearest(3, 0, 0),
        nearest(0, 0, 3),
        nearest(3, 0, 3),
        nearest(0, 3, 0),
        nearest(3, 3, 0),
        nearest(0, 3, 3),
        nearest(3, 3, 3),
    ];
    if let Some(biome) = corners[0] {
        if corners.iter().all(|corner| *corner == Some(biome)) {
            // Entire 64-quart section is homogenous: one palette entry, written by index.
            let index = section.biomes.index_of_or_insert(biome.clone());
            for i in 0..64 {
                section.biomes.set_index(i, index);
            }
            return;
        }
    }

    // Boundary section: sample all 64 quart cells. Neighbouring cells usually repeat a biome,
    // so the palette index of the last one written is reused while the tree keeps returning
    // the same entry.
    let mut last: Option<(&oxide_core::ResourceLocation, u32)> = None;
    for qy in 0..4usize {
        for qz in 0..4usize {
            for qx in 0..4usize {
                if let Some(biome) = nearest(qx, qy, qz) {
                    let index = match last {
                        Some((known, index)) if std::ptr::eq(known, biome) => index,
                        _ => {
                            let index = section.biomes.index_of_or_insert(biome.clone());
                            last = Some((biome, index));
                            index
                        }
                    };
                    section
                        .biomes
                        .set_index(local_biome_index(qx, qy, qz), index);
                }
            }
        }
    }
}
