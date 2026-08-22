//! Chunk section and chunk-level data model.

use super::{Heightmap, HeightmapType, PalettedContainer};
use crate::ident::{BiomeId, BlockState};
use crate::pos::ChunkPos;
use std::collections::HashMap;

/// Vanilla's chunk generation status ladder.
///
/// // PARITY-CHECK: this is the post-1.18 "flattened" ladder (noise-based generation merged
/// // the old `liquid_carvers`/`noise` split). Mojang has adjusted status names/counts across
/// // versions before; verify the exact stage list and ordering against 26.2's
/// // `ChunkStatus` registry before relying on ordinal comparisons for generation gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChunkStatus {
    Empty,
    StructureStarts,
    StructureReferences,
    Biomes,
    Noise,
    Surface,
    Carvers,
    Features,
    InitializeLight,
    Light,
    Spawn,
    Full,
}

/// One 16x16x16 vertical slice of a chunk column.
#[derive(Debug, Clone)]
pub struct ChunkSection {
    /// Section Y index (section coordinates, not blocks — can be negative below y=0).
    pub y: i8,
    pub block_states: PalettedContainer<BlockState>,
    pub biomes: PalettedContainer<BiomeId>,
}

impl ChunkSection {
    /// Block-state container: 4096 entries (16x16x16), minimum 4 bits per entry.
    const BLOCK_COUNT: usize = 4096;
    /// Biome container: 64 entries (4x4x4, one per biome-quart), minimum 1 bit per entry.
    const BIOME_COUNT: usize = 64;
    const BLOCK_MIN_BITS: u8 = 4;
    const BIOME_MIN_BITS: u8 = 1;

    pub fn new(y: i8, default_block: BlockState, default_biome: BiomeId) -> Self {
        Self {
            y,
            block_states: PalettedContainer::new(
                Self::BLOCK_COUNT,
                Self::BLOCK_MIN_BITS,
                default_block,
            ),
            biomes: PalettedContainer::new(Self::BIOME_COUNT, Self::BIOME_MIN_BITS, default_biome),
        }
    }
}

/// Full chunk column data: position, vertical section stack, heightmaps, and generation state.
#[derive(Debug, Clone)]
pub struct ChunkData {
    pub pos: ChunkPos,
    /// Lowest block Y in the world (can be negative, e.g. `-64`).
    pub min_y: i32,
    /// Total world height in blocks (e.g. `384`).
    pub height: i32,
    pub sections: Vec<ChunkSection>,
    pub heightmaps: HashMap<HeightmapType, Heightmap>,
    pub status: ChunkStatus,
    /// Set on every chunk Rust generates — lighting is computed separately, never carried
    /// through from Rust-side generation (see `docs/ARCHITECTURE.md` § "Chunk output
    /// contract").
    pub needs_relight: bool,
}

impl ChunkData {
    pub fn new(pos: ChunkPos, min_y: i32, height: i32) -> Self {
        Self {
            pos,
            min_y,
            height,
            sections: Vec::new(),
            heightmaps: HashMap::new(),
            status: ChunkStatus::Empty,
            needs_relight: true,
        }
    }

    pub fn section_count(&self) -> usize {
        (self.height / 16) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::ResourceLocation;

    #[test]
    fn status_ladder_orders_empty_before_full() {
        assert!(ChunkStatus::Empty < ChunkStatus::Full);
        assert!(ChunkStatus::Noise < ChunkStatus::Surface);
    }

    #[test]
    fn chunk_data_defaults_needs_relight() {
        let c = ChunkData::new(ChunkPos::new(0, 0), -64, 384);
        assert!(c.needs_relight);
        assert_eq!(c.status, ChunkStatus::Empty);
        assert_eq!(c.section_count(), 24);
    }

    #[test]
    fn chunk_section_default_palettes_are_single_value() {
        let air = BlockState::new(ResourceLocation::minecraft("air"));
        let plains = ResourceLocation::minecraft("plains");
        let s = ChunkSection::new(0, air, plains);
        assert_eq!(s.block_states.bits_per_entry(), 0);
        assert_eq!(s.biomes.bits_per_entry(), 0);
    }
}
