//! The 3x3-chunk window one chunk's decoration reads and writes.
//!
//! Vanilla hands a feature a `WorldGenRegion`: the chunk being decorated plus the ring around
//! it, because a tree rooted near a border has branches next door. [`DecorationRegion`] is that
//! window over *undecorated* terrain, with every write kept in an overlay instead of applied.
//! The overlay is the point: a chunk's decoration becomes a list of block writes that is a pure
//! function of the terrain around it, which is what lets `crate::decorate` hand out chunks in
//! any order and still agree (see that module).

use std::collections::HashMap;

use oxide_core::{BlockPos, BlockState, ChunkData, ChunkPos, HeightmapType};
use oxide_datapack::{BlockFact, BlockFacts};

use crate::fill::local_index;

/// One block write a decoration made, in world coordinates.
pub type BlockWrite = (BlockPos, BlockState);

pub struct DecorationRegion<'a> {
    center: ChunkPos,
    /// Row-major from the north-west: index `(dz + 1) * 3 + (dx + 1)`.
    chunks: [&'a ChunkData; 9],
    facts: &'a BlockFacts,
    min_y: i32,
    height: i32,
    overlay: HashMap<BlockPos, BlockState>,
    /// Positions in the order they were first written, so the write list is deterministic.
    order: Vec<BlockPos>,
    /// Highest written Y per column: a column with no entry still has its terrain heights.
    written_top: HashMap<(i32, i32), i32>,
}

impl<'a> DecorationRegion<'a> {
    /// `chunks` are the nine chunks around `center`, row-major from `(center.x - 1,
    /// center.z - 1)`. They must share one vertical range.
    pub fn new(center: ChunkPos, chunks: [&'a ChunkData; 9], facts: &'a BlockFacts) -> Self {
        let middle = chunks[4];
        debug_assert_eq!(middle.pos, center);
        Self {
            center,
            chunks,
            facts,
            min_y: middle.min_y,
            height: middle.height,
            overlay: HashMap::new(),
            order: Vec::new(),
            written_top: HashMap::new(),
        }
    }

    pub fn center(&self) -> ChunkPos {
        self.center
    }

    pub fn min_y(&self) -> i32 {
        self.min_y
    }

    /// One past the highest block Y.
    pub fn max_y(&self) -> i32 {
        self.min_y + self.height
    }

    pub fn facts(&self) -> &'a BlockFacts {
        self.facts
    }

    fn chunk_at(&self, x: i32, z: i32) -> Option<&'a ChunkData> {
        let dx = (x >> 4) - self.center.x;
        let dz = (z >> 4) - self.center.z;
        if !(-1..=1).contains(&dx) || !(-1..=1).contains(&dz) {
            return None;
        }
        Some(self.chunks[((dz + 1) * 3 + (dx + 1)) as usize])
    }

    /// Whether `pos` is inside the window and the world's height.
    pub fn contains(&self, pos: BlockPos) -> bool {
        pos.y >= self.min_y && pos.y < self.max_y() && self.chunk_at(pos.x, pos.z).is_some()
    }

    fn terrain_block(&self, pos: BlockPos) -> Option<&'a BlockState> {
        if pos.y < self.min_y || pos.y >= self.max_y() {
            return None;
        }
        let chunk = self.chunk_at(pos.x, pos.z)?;
        let section = chunk.sections.get(((pos.y - self.min_y) >> 4) as usize)?;
        let slot = local_index(
            (pos.x & 15) as usize,
            ((pos.y - self.min_y) & 15) as usize,
            (pos.z & 15) as usize,
        );
        Some(section.block_states.get(slot))
    }

    /// The block at `pos` as decoration so far left it, or `None` outside the window.
    pub fn block(&self, pos: BlockPos) -> Option<&BlockState> {
        match self.overlay.get(&pos) {
            Some(state) => Some(state),
            None => self.terrain_block(pos),
        }
    }

    /// Whether `fact` holds at `pos`. Outside the window nothing holds.
    pub fn is(&self, pos: BlockPos, fact: BlockFact) -> bool {
        self.block(pos).is_some_and(|state| self.facts.is(state, fact))
    }

    /// Writes `state` at `pos`. Returns `false`, writing nothing, outside the window -- the
    /// same refusal vanilla's region gives a feature that reaches past its write radius.
    pub fn set_block(&mut self, pos: BlockPos, state: BlockState) -> bool {
        if !self.contains(pos) {
            return false;
        }
        if self.overlay.insert(pos, state).is_none() {
            self.order.push(pos);
        }
        let top = self.written_top.entry((pos.x, pos.z)).or_insert(pos.y);
        *top = (*top).max(pos.y);
        true
    }

    /// The first free Y above the highest block `kind` counts in the column at `(x, z)`: what
    /// vanilla's `getHeight` returns. `None` outside the window.
    ///
    /// A column decoration has not written to answers from the terrain's stored heightmap; a
    /// written column is rescanned, since vanilla keeps its heightmaps current block by block
    /// and a later feature must see the tree an earlier one planted.
    pub fn height(&self, kind: HeightmapType, x: i32, z: i32) -> Option<i32> {
        let chunk = self.chunk_at(x, z)?;
        let stored = chunk
            .heightmaps
            .get(&kind)
            .map(|map| map.get((x & 15) as usize, (z & 15) as usize) + self.min_y);
        let Some(&written_top) = self.written_top.get(&(x, z)) else {
            return Some(stored.unwrap_or(self.min_y));
        };
        let fact = height_fact(kind);
        let start = stored.unwrap_or(self.max_y()).max(written_top + 1).min(self.max_y());
        let mut y = start;
        while y > self.min_y {
            if self.is(BlockPos::new(x, y - 1, z), fact) {
                return Some(y);
            }
            y -= 1;
        }
        Some(self.min_y)
    }

    /// Every write, in the order positions were first written, each with its final state.
    pub fn into_writes(mut self) -> Vec<BlockWrite> {
        let mut writes = Vec::with_capacity(self.order.len());
        for pos in self.order {
            if let Some(state) = self.overlay.remove(&pos) {
                writes.push((pos, state));
            }
        }
        writes
    }
}

/// The block fact a heightmap kind counts.
pub(crate) fn height_fact(kind: HeightmapType) -> BlockFact {
    match kind {
        HeightmapType::WorldSurface | HeightmapType::WorldSurfaceWg => BlockFact::HeightWorldSurface,
        HeightmapType::OceanFloor | HeightmapType::OceanFloorWg => BlockFact::HeightOceanFloor,
        HeightmapType::MotionBlocking => BlockFact::HeightMotionBlocking,
        HeightmapType::MotionBlockingNoLeaves => BlockFact::HeightMotionBlockingNoLeaves,
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use oxide_core::{BiomeId, ChunkSection, Heightmap, ResourceLocation};
    use std::path::Path;

    pub fn block(path: &str) -> BlockState {
        BlockState::new(ResourceLocation::minecraft(path))
    }

    /// Facts for the handful of blocks the tests use, in the exporter's own format.
    pub fn facts() -> BlockFacts {
        let dir = std::env::temp_dir().join(format!(
            "oxide-region-facts-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("block_facts.tsv"),
            "minecraft:air\tDarc\n\
             minecraft:stone\tDsmoudf1234\n\
             minecraft:oak_log\tDsmoudf1234\n\
             minecraft:oak_leaves\tDsmf123\n\
             minecraft:short_grass\tDrc1\n",
        )
        .unwrap();
        let facts = BlockFacts::load(Path::new(&dir)).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        facts
    }

    /// A chunk of stone up to (not including) `surface`, air above, in a -64..320 world.
    pub fn flat_chunk(pos: ChunkPos, surface: i32) -> ChunkData {
        let (min_y, height) = (-64, 384);
        let mut chunk = ChunkData::new(pos, min_y, height);
        let biome: BiomeId = ResourceLocation::minecraft("plains");
        for index in 0..(height / 16) {
            let section_min = min_y + index * 16;
            let mut section =
                ChunkSection::new((section_min >> 4) as i8, block("air"), biome.clone());
            for y in 0..16 {
                if section_min + y < surface {
                    for z in 0..16 {
                        for x in 0..16 {
                            section
                                .block_states
                                .set(local_index(x, y as usize, z), block("stone"));
                        }
                    }
                }
            }
            chunk.sections.push(section);
        }
        let mut map = Heightmap::new(height);
        for z in 0..16 {
            for x in 0..16 {
                map.set(x, z, surface - min_y);
            }
        }
        for kind in [
            HeightmapType::WorldSurface,
            HeightmapType::WorldSurfaceWg,
            HeightmapType::OceanFloor,
            HeightmapType::OceanFloorWg,
            HeightmapType::MotionBlocking,
            HeightmapType::MotionBlockingNoLeaves,
        ] {
            chunk.heightmaps.insert(kind, map.clone());
        }
        chunk
    }

    /// The nine flat chunks around `center`, row-major.
    pub fn flat_ring(center: ChunkPos, surface: i32) -> Vec<ChunkData> {
        let mut chunks = Vec::new();
        for dz in -1..=1 {
            for dx in -1..=1 {
                chunks.push(flat_chunk(ChunkPos::new(center.x + dx, center.z + dz), surface));
            }
        }
        chunks
    }

    pub fn refs(chunks: &[ChunkData]) -> [&ChunkData; 9] {
        std::array::from_fn(|index| &chunks[index])
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn reads_terrain_takes_writes_and_refuses_outside_the_window() {
        let facts = facts();
        let chunks = flat_ring(ChunkPos::new(2, -3), 64);
        let mut region = DecorationRegion::new(ChunkPos::new(2, -3), refs(&chunks), &facts);

        let inside = BlockPos::new(2 * 16 + 5, 64, -3 * 16 + 9);
        assert_eq!(region.block(inside), Some(&block("air")));
        assert_eq!(region.block(BlockPos::new(inside.x, 63, inside.z)), Some(&block("stone")));

        // A neighbouring chunk is writable; two chunks away is not.
        let next_door = BlockPos::new(3 * 16 + 1, 64, -3 * 16);
        assert!(region.set_block(next_door, block("oak_log")));
        assert_eq!(region.block(next_door), Some(&block("oak_log")));
        let too_far = BlockPos::new(4 * 16, 64, -3 * 16);
        assert!(!region.set_block(too_far, block("oak_log")));
        assert_eq!(region.block(too_far), None);
        assert!(!region.set_block(BlockPos::new(inside.x, 320, inside.z), block("stone")));

        // Rewriting a position keeps its place in the order and its last state.
        assert!(region.set_block(inside, block("oak_log")));
        assert!(region.set_block(next_door, block("stone")));
        assert_eq!(
            region.into_writes(),
            vec![(next_door, block("stone")), (inside, block("oak_log"))]
        );
    }

    #[test]
    fn heights_follow_writes_per_heightmap_kind() {
        let facts = facts();
        let chunks = flat_ring(ChunkPos::new(0, 0), 64);
        let mut region = DecorationRegion::new(ChunkPos::new(0, 0), refs(&chunks), &facts);
        assert_eq!(region.height(HeightmapType::WorldSurface, 3, 3), Some(64));

        region.set_block(BlockPos::new(3, 64, 3), block("oak_log"));
        region.set_block(BlockPos::new(3, 65, 3), block("oak_leaves"));
        region.set_block(BlockPos::new(3, 66, 3), block("short_grass"));
        // Grass is not air, so the world surface counts it; nothing else does.
        assert_eq!(region.height(HeightmapType::WorldSurface, 3, 3), Some(67));
        assert_eq!(region.height(HeightmapType::MotionBlocking, 3, 3), Some(66));
        assert_eq!(region.height(HeightmapType::MotionBlockingNoLeaves, 3, 3), Some(65));
        assert_eq!(region.height(HeightmapType::WorldSurface, 4, 3), Some(64), "untouched column");

        // Digging below the stored surface lowers it.
        region.set_block(BlockPos::new(8, 63, 8), block("air"));
        assert_eq!(region.height(HeightmapType::OceanFloor, 8, 8), Some(63));
        assert_eq!(region.height(HeightmapType::WorldSurface, 99, 0), None);
    }
}
