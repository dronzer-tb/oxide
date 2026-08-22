//! Per-chunk Merkle tree over placed block state, matching `docs/ARCHITECTURE.md`'s validation
//! harness design (`chunk -> section -> sub-region -> leaf`, leaf granularity configurable).
//!
//! This builds and compares trees; it does not itself diff against vanilla — that needs a real
//! reference chunk dump, which is a gitignored, manually-extracted, per-`docs/REFERENCE_DATA.md`
//! artifact nobody has produced for 26.2 yet (see `docs/ROADMAP.md`'s "Known unknowns"). Once
//! one exists, feed both chunks' trees to [`diverging_sections`].

use oxide_core::{ChunkData, ChunkSection};

/// `(y*16+z)*16+x` — same flattened section-array convention as `oxide-chunkgen`'s
/// `local_index` and `oxide-pregen`'s (see their own `// PARITY-CHECK`s); duplicated here
/// rather than shared across a crate boundary that shouldn't otherwise exist between them.
fn local_index(x: usize, y: usize, z: usize) -> usize {
    (y * 16 + z) * 16 + x
}

#[derive(Debug, Clone)]
pub struct MerkleTree {
    pub chunk_hash: blake3::Hash,
    /// One hash per placed section, in the chunk's section order (bottom to top).
    pub section_hashes: Vec<(i8, blake3::Hash)>,
}

/// `leaf_size` is the cube side length of a sub-region within a 16x16x16 section (e.g. `4` =>
/// 64 leaves of 4x4x4 blocks each); must evenly divide 16.
pub fn build_merkle(chunk: &ChunkData, leaf_size: usize) -> MerkleTree {
    assert!(
        leaf_size > 0 && 16 % leaf_size == 0,
        "leaf_size must evenly divide 16, got {leaf_size}"
    );
    let steps = 16 / leaf_size;

    let mut section_hashes = Vec::with_capacity(chunk.sections.len());
    for section in &chunk.sections {
        let mut section_hasher = blake3::Hasher::new();
        for sub_y in 0..steps {
            for sub_z in 0..steps {
                for sub_x in 0..steps {
                    let leaf = hash_sub_region(section, sub_x, sub_y, sub_z, leaf_size);
                    section_hasher.update(leaf.as_bytes());
                }
            }
        }
        section_hashes.push((section.y, section_hasher.finalize()));
    }

    let mut chunk_hasher = blake3::Hasher::new();
    for (_, hash) in &section_hashes {
        chunk_hasher.update(hash.as_bytes());
    }

    MerkleTree {
        chunk_hash: chunk_hasher.finalize(),
        section_hashes,
    }
}

fn hash_sub_region(
    section: &ChunkSection,
    sub_x: usize,
    sub_y: usize,
    sub_z: usize,
    leaf_size: usize,
) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    for ly in 0..leaf_size {
        for lz in 0..leaf_size {
            for lx in 0..leaf_size {
                let block = section.block_states.get(local_index(
                    sub_x * leaf_size + lx,
                    sub_y * leaf_size + ly,
                    sub_z * leaf_size + lz,
                ));
                hasher.update(block.to_string().as_bytes());
                hasher.update(&[0u8]); // separator, avoids "stone" + "" colliding with "sto" + "ne"
            }
        }
    }
    hasher.finalize()
}

/// Section `y` values whose hash diverges between two trees for (presumably) the same chunk
/// position — localizes a root mismatch without re-hashing every leaf. Descending further
/// (sub-region by sub-region) needs the original `ChunkData`, not just the tree, so that's left
/// to the caller.
pub fn diverging_sections(a: &MerkleTree, b: &MerkleTree) -> Vec<i8> {
    a.section_hashes
        .iter()
        .zip(&b.section_hashes)
        .filter(|((_, ha), (_, hb))| ha != hb)
        .map(|((y, _), _)| *y)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::{BlockState, ChunkPos, ChunkStatus, ResourceLocation};

    fn uniform_chunk(block_path: &str) -> ChunkData {
        let air = BlockState::new(ResourceLocation::minecraft("air"));
        let biome = ResourceLocation::minecraft("plains");
        let mut chunk = ChunkData::new(ChunkPos::new(0, 0), -64, 32);
        for section_y in -4..-2i32 {
            let mut section = ChunkSection::new(section_y as i8, air.clone(), biome.clone());
            if block_path != "air" {
                let block = BlockState::new(ResourceLocation::minecraft(block_path));
                for i in 0..4096 {
                    section.block_states.set(i, block.clone());
                }
            }
            chunk.sections.push(section);
        }
        chunk.status = ChunkStatus::Full;
        chunk
    }

    #[test]
    fn identical_chunks_hash_identically() {
        let a = build_merkle(&uniform_chunk("stone"), 4);
        let b = build_merkle(&uniform_chunk("stone"), 4);
        assert_eq!(a.chunk_hash, b.chunk_hash);
        assert!(diverging_sections(&a, &b).is_empty());
    }

    #[test]
    fn different_blocks_hash_differently() {
        let a = build_merkle(&uniform_chunk("stone"), 4);
        let b = build_merkle(&uniform_chunk("dirt"), 4);
        assert_ne!(a.chunk_hash, b.chunk_hash);
    }

    #[test]
    fn a_change_in_one_section_only_diverges_that_section() {
        let mut chunk = uniform_chunk("stone");
        let dirt = BlockState::new(ResourceLocation::minecraft("dirt"));
        chunk.sections[1].block_states.set(0, dirt);
        let a = build_merkle(&uniform_chunk("stone"), 4);
        let b = build_merkle(&chunk, 4);
        let diverging = diverging_sections(&a, &b);
        assert_eq!(diverging, vec![chunk.sections[1].y]);
    }

    #[test]
    fn leaf_size_must_evenly_divide_16() {
        let result = std::panic::catch_unwind(|| build_merkle(&uniform_chunk("stone"), 5));
        assert!(result.is_err());
    }
}
