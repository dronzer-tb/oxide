//! Chunk-level divergence reporting: from "these two chunks differ" down to "this block, at
//! these world coordinates, is X in Oxide and Y in vanilla".
//!
//! The Merkle tree narrows the search; this does the last step. Reporting the block rather than
//! the chunk is the entire point -- "chunk 3,-7 differs" is not actionable, "y=62 is grass in
//! vanilla and dirt in Oxide" names the surface rule to go and read.

use oxide_core::{BlockState, ChunkData};

use crate::merkle::{build_merkle, diverging_leaves, diverging_sections, MerkleTree};

/// One block that differs.
#[derive(Debug, Clone)]
pub struct BlockDivergence {
    /// World coordinates, not section-local -- these are what you type into `/tp`.
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub oxide: BlockState,
    pub vanilla: BlockState,
}

/// Which side a section exists on, when only one does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnlyIn {
    Oxide,
    Vanilla,
}

impl std::fmt::Display for OnlyIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            OnlyIn::Oxide => "oxide",
            OnlyIn::Vanilla => "vanilla",
        })
    }
}

/// A section one side has and the other does not.
#[derive(Debug, Clone, Copy)]
pub struct SectionPresence {
    pub section_y: i8,
    pub only_in: OnlyIn,
}

/// What a single chunk comparison found.
#[derive(Debug, Clone, Default)]
pub struct ChunkComparison {
    pub identical: bool,
    /// Section Y values whose hashes differ, from the Merkle roots.
    pub diverging_sections: Vec<i8>,
    /// Sections that exist on only one side.
    pub section_presence: Vec<SectionPresence>,
    /// Individual differing blocks, capped -- see [`compare_chunks`].
    pub blocks: Vec<BlockDivergence>,
    /// Differing blocks found in total, which can exceed `blocks.len()`.
    pub total_differing_blocks: usize,
}

/// Compares one Oxide chunk against its vanilla counterpart.
///
/// `max_blocks_reported` caps the detail, not the count: a whole-chunk divergence is thousands
/// of blocks and printing them all buries the one line that would tell you why. The total is
/// always reported.
pub fn compare_chunks(
    oxide: &ChunkData,
    vanilla: &ChunkData,
    leaf_size: usize,
    max_blocks_reported: usize,
) -> ChunkComparison {
    let tree_oxide = build_merkle(oxide, leaf_size);
    let tree_vanilla = build_merkle(vanilla, leaf_size);

    let mut report = ChunkComparison {
        section_presence: section_presence(oxide, vanilla),
        ..Default::default()
    };

    if tree_oxide.chunk_hash == tree_vanilla.chunk_hash && report.section_presence.is_empty() {
        report.identical = true;
        return report;
    }

    report.diverging_sections = diverging_sections(&tree_oxide, &tree_vanilla);
    collect_blocks(
        oxide,
        vanilla,
        &tree_oxide,
        &tree_vanilla,
        leaf_size,
        max_blocks_reported,
        &mut report,
    );
    report
}

fn section_presence(oxide: &ChunkData, vanilla: &ChunkData) -> Vec<SectionPresence> {
    let mut out = Vec::new();
    for section in &oxide.sections {
        if !vanilla.sections.iter().any(|s| s.y == section.y) {
            out.push(SectionPresence {
                section_y: section.y,
                only_in: OnlyIn::Oxide,
            });
        }
    }
    for section in &vanilla.sections {
        if !oxide.sections.iter().any(|s| s.y == section.y) {
            out.push(SectionPresence {
                section_y: section.y,
                only_in: OnlyIn::Vanilla,
            });
        }
    }
    out
}

fn collect_blocks(
    oxide: &ChunkData,
    vanilla: &ChunkData,
    tree_oxide: &MerkleTree,
    tree_vanilla: &MerkleTree,
    leaf_size: usize,
    max_blocks_reported: usize,
    report: &mut ChunkComparison,
) {
    for leaf in diverging_leaves(tree_oxide, tree_vanilla) {
        let (Some(section_oxide), Some(section_vanilla)) = (
            oxide.sections.iter().find(|s| s.y == leaf.section_y),
            vanilla.sections.iter().find(|s| s.y == leaf.section_y),
        ) else {
            continue;
        };

        for ly in 0..leaf_size {
            for lz in 0..leaf_size {
                for lx in 0..leaf_size {
                    let (x, y, z) = (
                        leaf.sub_x * leaf_size + lx,
                        leaf.sub_y * leaf_size + ly,
                        leaf.sub_z * leaf_size + lz,
                    );
                    let index = (y * 16 + z) * 16 + x;
                    let a = section_oxide.block_states.get(index);
                    let b = section_vanilla.block_states.get(index);
                    if a == b {
                        continue;
                    }
                    report.total_differing_blocks += 1;
                    if report.blocks.len() < max_blocks_reported {
                        report.blocks.push(BlockDivergence {
                            x: oxide.pos.x * 16 + x as i32,
                            y: leaf.section_y as i32 * 16 + y as i32,
                            z: oxide.pos.z * 16 + z as i32,
                            oxide: a.clone(),
                            vanilla: b.clone(),
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::{ChunkPos, ChunkSection, ResourceLocation};

    fn chunk_of(block_path: &str) -> ChunkData {
        let air = BlockState::new(ResourceLocation::minecraft("air"));
        let biome = ResourceLocation::minecraft("plains");
        let mut chunk = ChunkData::new(ChunkPos::new(2, -3), -64, 32);
        for section_y in -4..-2i32 {
            let mut section = ChunkSection::new(section_y as i8, air.clone(), biome.clone());
            let block = BlockState::new(ResourceLocation::minecraft(block_path));
            for i in 0..4096 {
                section.block_states.set(i, block.clone());
            }
            chunk.sections.push(section);
        }
        chunk
    }

    #[test]
    fn identical_chunks_report_identical() {
        let report = compare_chunks(&chunk_of("stone"), &chunk_of("stone"), 4, 16);
        assert!(report.identical);
        assert_eq!(report.total_differing_blocks, 0);
    }

    /// A single changed block is found and reported at world coordinates, not section-local
    /// ones -- the whole point is that the output can be pasted into a teleport.
    #[test]
    fn one_changed_block_is_located_in_world_coordinates() {
        let oxide = chunk_of("stone");
        let mut vanilla = chunk_of("stone");
        // Section -4 covers y = -64..-49. Local (5, 3, 9) is world (2*16+5, -64+3, -3*16+9).
        let index = (3 * 16 + 9) * 16 + 5;
        vanilla.sections[0]
            .block_states
            .set(index, BlockState::new(ResourceLocation::minecraft("dirt")));

        let report = compare_chunks(&oxide, &vanilla, 4, 16);
        assert!(!report.identical);
        assert_eq!(report.total_differing_blocks, 1);
        assert_eq!(report.diverging_sections, vec![-4]);

        let found = &report.blocks[0];
        assert_eq!((found.x, found.y, found.z), (37, -61, -39));
        assert_eq!(found.oxide.name.to_string(), "minecraft:stone");
        assert_eq!(found.vanilla.name.to_string(), "minecraft:dirt");
    }

    /// The cap limits what is printed, never what is counted -- a run that says "3 differences"
    /// when there were 4096 would send you looking in the wrong place.
    #[test]
    fn the_report_cap_does_not_change_the_total() {
        let report = compare_chunks(&chunk_of("stone"), &chunk_of("dirt"), 4, 3);
        assert_eq!(report.blocks.len(), 3);
        assert_eq!(report.total_differing_blocks, 8192);
    }

    /// A section only one side has is reported rather than skipped: it is the largest possible
    /// difference and comparing only shared sections would hide it entirely.
    #[test]
    fn a_section_present_on_one_side_only_is_reported() {
        let oxide = chunk_of("stone");
        let mut vanilla = chunk_of("stone");
        vanilla.sections.pop();

        let report = compare_chunks(&oxide, &vanilla, 4, 16);
        assert!(!report.identical);
        assert_eq!(report.section_presence.len(), 1);
        assert_eq!(report.section_presence[0].only_in, OnlyIn::Oxide);
    }
}
