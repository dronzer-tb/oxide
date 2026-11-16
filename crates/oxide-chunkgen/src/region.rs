//! The 3x3-chunk window one chunk's decoration reads and writes.
//!
//! Vanilla hands a feature a `WorldGenRegion`: the chunk being decorated plus the ring around
//! it, because a tree rooted near a border has branches next door. [`DecorationRegion`] is that
//! window over *undecorated* terrain, with every write kept in an overlay instead of applied.
//! The overlay is the point: a chunk's decoration becomes a list of block writes that is a pure
//! function of the terrain around it, which is what lets `crate::decorate` hand out chunks in
//! any order and still agree (see that module).

use oxide_core::{
    BiomeId, BlockPos, BlockState, ChunkData, ChunkPos, FastHashMap, Heightmap, HeightmapType,
    ResourceLocation,
};
use oxide_datapack::{BlockFact, BlockFacts};

use crate::fill::local_index;

/// One block write a decoration made, in world coordinates.
pub type BlockWrite = (BlockPos, BlockState);

/// A block entity a feature filled in, as the block-entity SNBT vanilla would save
/// (`id`, `x`, `y`, `z` and the type's own fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockEntityData {
    pub pos: BlockPos,
    /// The block it belongs to. Vanilla drops a block entity when its block is replaced by a
    /// different block, so this is checked against the block finally standing at `pos`.
    pub block: ResourceLocation,
    pub snbt: String,
}

/// What vanilla's feature pass leaves in a chunk besides block states.
///
/// From one region this holds everything its features left anywhere in the 3x3 window; for a
/// finished chunk (`crate::decorate::decorated_chunk_with_extras`) only what lands in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChunkExtras {
    /// Block entities with data. In a finished chunk this also lists every other block-entity
    /// block decoration wrote, as vanilla's `{id:"DUMMY",x,y,z}` placeholder.
    pub block_entities: Vec<BlockEntityData>,
    /// `scheduleTick(pos, fluid, ..)`: position and fluid id (`minecraft:water`,
    /// `minecraft:flowing_lava`, ..).
    pub fluid_ticks: Vec<(BlockPos, ResourceLocation)>,
    /// `scheduleTick(pos, block, ..)`: position and block id.
    pub block_ticks: Vec<(BlockPos, ResourceLocation)>,
    /// `ChunkAccess.markPosForPostProcessing` positions.
    pub post_process: Vec<BlockPos>,
}

impl ChunkExtras {
    pub fn is_empty(&self) -> bool {
        self.block_entities.is_empty()
            && self.fluid_ticks.is_empty()
            && self.block_ticks.is_empty()
            && self.post_process.is_empty()
    }
}

/// `BlockBehaviour.Properties.postProcess` as 26.2 registers it: the position `setBlock` marks
/// for post-processing when it writes `state` without `UPDATE_KNOWN_SHAPE`. Only four blocks
/// set it (`Blocks.postProcessSelf` / `postProcessAbove`), none copied by `ofFullCopy`.
fn post_process_pos(state: &BlockState, pos: BlockPos) -> Option<BlockPos> {
    if state.name.namespace() != "minecraft" {
        return None;
    }
    match state.name.path() {
        "brown_mushroom" | "red_mushroom" => Some(pos),
        "soul_sand" | "magma_block" => Some(BlockPos::new(pos.x, pos.y + 1, pos.z)),
        _ => None,
    }
}

pub struct DecorationRegion<'a> {
    center: ChunkPos,
    /// Row-major from the north-west: index `(dz + 1) * 3 + (dx + 1)`.
    chunks: [&'a ChunkData; 9],
    facts: &'a BlockFacts,
    min_y: i32,
    height: i32,
    /// Index into `writes` of each written position.
    overlay: FastHashMap<BlockPos, u32>,
    /// Every written position once, in the order it was first written (so the write list is
    /// deterministic), with its current state and that state's fact bits.
    writes: Vec<(BlockPos, BlockState, u32)>,
    /// Fact bits per terrain palette entry, per chunk and section: `[chunk][section][entry]`.
    terrain_bits: [Vec<Vec<u32>>; 9],
    /// The facts of `void_air`, what the level answers outside the window.
    void_bits: u32,
    /// Each chunk's stored heightmaps, by `HeightmapType` index.
    heightmaps: [[Option<&'a Heightmap>; 6]; 9],
    /// Highest written Y per column: a column with no entry still has its terrain heights.
    written_top: FastHashMap<(i32, i32), i32>,
    extras: ChunkExtras,
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
            overlay: FastHashMap::default(),
            writes: Vec::new(),
            terrain_bits: chunks.map(|chunk| {
                chunk
                    .sections
                    .iter()
                    .map(|section| {
                        let palette = section.block_states.palette();
                        palette.iter().map(|state| facts.bits(state)).collect()
                    })
                    .collect()
            }),
            void_bits: facts.bits(&BlockState::new(ResourceLocation::minecraft("void_air"))),
            heightmaps: chunks.map(|chunk| HEIGHTMAP_KINDS.map(|kind| chunk.heightmaps.get(&kind))),
            written_top: FastHashMap::default(),
            extras: ChunkExtras::default(),
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

    /// Which of the nine chunks holds column `(x, z)`, or `None` outside the window.
    fn chunk_index(&self, x: i32, z: i32) -> Option<usize> {
        let dx = (x >> 4) - self.center.x;
        let dz = (z >> 4) - self.center.z;
        if !(-1..=1).contains(&dx) || !(-1..=1).contains(&dz) {
            return None;
        }
        Some(((dz + 1) * 3 + (dx + 1)) as usize)
    }

    fn chunk_at(&self, x: i32, z: i32) -> Option<&'a ChunkData> {
        Some(self.chunks[self.chunk_index(x, z)?])
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
            Some(&index) => Some(&self.writes[index as usize].1),
            None => self.terrain_block(pos),
        }
    }

    /// The fact bits of the block at `pos` (`BlockFacts::bits`), or `None` outside the window.
    pub fn bits(&self, pos: BlockPos) -> Option<u32> {
        if let Some(&index) = self.overlay.get(&pos) {
            return Some(self.writes[index as usize].2);
        }
        if pos.y < self.min_y || pos.y >= self.max_y() {
            return None;
        }
        let chunk = self.chunk_index(pos.x, pos.z)?;
        let section_index = ((pos.y - self.min_y) >> 4) as usize;
        let section = self.chunks[chunk].sections.get(section_index)?;
        let slot = local_index(
            (pos.x & 15) as usize,
            ((pos.y - self.min_y) & 15) as usize,
            (pos.z & 15) as usize,
        );
        let entry = section.block_states.indices()[slot] as usize;
        Some(self.terrain_bits[chunk][section_index][entry])
    }

    /// [`Self::bits`], with `void_air`'s outside the window: the facts of `block_in(pos)`.
    pub fn bits_or_void(&self, pos: BlockPos) -> u32 {
        self.bits(pos).unwrap_or(self.void_bits)
    }

    /// Whether `fact` holds at `pos`. Outside the window nothing holds.
    pub fn is(&self, pos: BlockPos, fact: BlockFact) -> bool {
        self.bits(pos).is_some_and(|bits| bits & fact as u32 != 0)
    }

    /// Writes `state` at `pos`. Returns `false`, writing nothing, outside the window -- the
    /// same refusal vanilla's region gives a feature that reaches past its write radius.
    ///
    /// This is `WorldGenRegion.setBlock` without `UPDATE_KNOWN_SHAPE`, the flags nearly every
    /// feature uses: a block with a post-process rule ([`post_process_pos`]) is marked.
    pub fn set_block(&mut self, pos: BlockPos, state: BlockState) -> bool {
        let mark = post_process_pos(&state, pos);
        if !self.write(pos, state) {
            return false;
        }
        if let Some(mark) = mark {
            self.mark_post_process(mark);
        }
        true
    }

    /// [`Self::set_block`] for writes vanilla makes with `UPDATE_KNOWN_SHAPE` (trees) or straight
    /// into the section (`OreFeature`'s `BulkSectionAccess`): nothing is marked.
    pub fn set_block_known_shape(&mut self, pos: BlockPos, state: BlockState) -> bool {
        self.write(pos, state)
    }

    fn write(&mut self, pos: BlockPos, state: BlockState) -> bool {
        if !self.contains(pos) {
            return false;
        }
        // Vanilla (Paper's `WorldGenRegion.setBlock`, as `LevelChunk.setBlockState`) drops the
        // block entity when its block changes to another block.
        let block = &state.name;
        self.extras
            .block_entities
            .retain(|entity| entity.pos != pos || entity.block == *block);
        let bits = self.facts.bits(&state);
        match self.overlay.get(&pos) {
            Some(&index) => self.writes[index as usize] = (pos, state, bits),
            None => {
                self.overlay.insert(pos, self.writes.len() as u32);
                self.writes.push((pos, state, bits));
            }
        }
        let top = self.written_top.entry((pos.x, pos.z)).or_insert(pos.y);
        *top = (*top).max(pos.y);
        true
    }

    /// Fills in the block entity of the block now at `pos` with `snbt` (see
    /// [`BlockEntityData::snbt`]), replacing what an earlier call left there. Outside the window
    /// nothing is kept.
    pub fn put_block_entity(&mut self, pos: BlockPos, snbt: String) {
        let Some(block) = self.block(pos).map(|state| state.name.clone()) else {
            return;
        };
        self.extras
            .block_entities
            .retain(|entity| entity.pos != pos);
        self.extras
            .block_entities
            .push(BlockEntityData { pos, block, snbt });
    }

    /// `LevelAccessor.scheduleTick(pos, fluid, ..)`.
    pub fn schedule_fluid_tick(&mut self, pos: BlockPos, fluid: ResourceLocation) {
        self.extras.fluid_ticks.push((pos, fluid));
    }

    /// `LevelAccessor.scheduleTick(pos, block, ..)`.
    pub fn schedule_block_tick(&mut self, pos: BlockPos, block: ResourceLocation) {
        self.extras.block_ticks.push((pos, block));
    }

    /// `ChunkAccess.markPosForPostProcessing`; `ProtoChunk` ignores a position outside the
    /// build height.
    pub fn mark_post_process(&mut self, pos: BlockPos) {
        if self.contains(pos) {
            self.extras.post_process.push(pos);
        }
    }

    /// `Feature.markAboveForPostProcessing`: marks up to two blocks above `pos`, stopping at the
    /// first air (`void_air` outside the window counts as air).
    pub fn mark_above_for_post_processing(&mut self, pos: BlockPos) {
        for dy in 1..=2 {
            let above = BlockPos::new(pos.x, pos.y + dy, pos.z);
            if !self.contains(above) || self.is(above, BlockFact::Air) {
                return;
            }
            self.extras.post_process.push(above);
        }
    }

    /// The first free Y above the highest block `kind` counts in the column at `(x, z)`: what
    /// vanilla's `getHeight` returns. `None` outside the window.
    ///
    /// A column decoration has not written to answers from the terrain's stored heightmap; a
    /// written column is rescanned, since vanilla keeps its heightmaps current block by block
    /// and a later feature must see the tree an earlier one planted.
    pub fn height(&self, kind: HeightmapType, x: i32, z: i32) -> Option<i32> {
        let chunk = self.chunk_index(x, z)?;
        let stored = self.heightmaps[chunk][kind_index(kind)]
            .map(|map| map.get((x & 15) as usize, (z & 15) as usize) + self.min_y);
        let Some(&written_top) = self.written_top.get(&(x, z)) else {
            return Some(stored.unwrap_or(self.min_y));
        };
        let fact = height_fact(kind);
        let start = stored
            .unwrap_or(self.max_y())
            .max(written_top + 1)
            .min(self.max_y());
        let mut y = start;
        while y > self.min_y {
            if self.is(BlockPos::new(x, y - 1, z), fact) {
                return Some(y);
            }
            y -= 1;
        }
        Some(self.min_y)
    }

    /// The biome at `pos`, read from the nearest stored 4x4x4 quart, or `None` outside the
    /// window horizontally.
    ///
    /// Vertically the quart row is clamped into the world, as vanilla's
    /// `ChunkAccess.getNoiseBiome` does, so a position above or below the world reads the
    /// topmost or bottommost row instead of failing.
    pub fn biome(&self, pos: BlockPos) -> Option<&BiomeId> {
        let chunk = self.chunk_at(pos.x, pos.z)?;
        let min_quart = self.min_y >> 2;
        let max_quart = min_quart + (self.height >> 2) - 1;
        let quart_y = (pos.y >> 2).clamp(min_quart, max_quart) - min_quart;
        let section = chunk.sections.get((quart_y >> 2) as usize)?;
        let slot = (((quart_y & 3) * 4 + ((pos.z >> 2) & 3)) * 4 + ((pos.x >> 2) & 3)) as usize;
        Some(section.biomes.get(slot))
    }

    /// Every distinct biome actually stored in the nine chunks, in first-seen order.
    ///
    /// Reads the quarts in use rather than each section's palette: a palette can keep an entry
    /// nothing references any more, and vanilla collects the biomes the storage holds.
    pub fn biomes_present(&self) -> Vec<&BiomeId> {
        let mut found: Vec<&BiomeId> = Vec::new();
        for chunk in self.chunks {
            for section in &chunk.sections {
                let palette = section.biomes.palette();
                let mut used = vec![false; palette.len()];
                for &index in section.biomes.indices() {
                    used[index as usize] = true;
                }
                for (entry, in_use) in palette.iter().zip(used) {
                    if in_use && !found.contains(&entry) {
                        found.push(entry);
                    }
                }
            }
        }
        found
    }

    /// Every write, in the order positions were first written, each with its final state.
    pub fn into_writes(self) -> Vec<BlockWrite> {
        self.into_parts().0
    }

    /// The writes split by the chunk they land in (`[chunk index]`, row-major from the
    /// north-west as [`Self::new`] takes the chunks), each in first-write order with its state's
    /// fact bits, and everything else the features left.
    pub(crate) fn into_buckets(mut self) -> ([Vec<(BlockPos, BlockState, u32)>; 9], ChunkExtras) {
        let mut buckets: [Vec<(BlockPos, BlockState, u32)>; 9] = Default::default();
        for write in std::mem::take(&mut self.writes) {
            if let Some(index) = self.chunk_index(write.0.x, write.0.z) {
                buckets[index].push(write);
            }
        }
        (buckets, self.extras)
    }

    /// [`Self::into_writes`] and everything else the features left, in the order they left it.
    pub fn into_parts(self) -> (Vec<BlockWrite>, ChunkExtras) {
        let writes = self
            .writes
            .into_iter()
            .map(|(pos, state, _)| (pos, state))
            .collect();
        (writes, self.extras)
    }
}

const HEIGHTMAP_KINDS: [HeightmapType; 6] = [
    HeightmapType::WorldSurface,
    HeightmapType::WorldSurfaceWg,
    HeightmapType::OceanFloor,
    HeightmapType::OceanFloorWg,
    HeightmapType::MotionBlocking,
    HeightmapType::MotionBlockingNoLeaves,
];

/// `kind`'s index in [`HEIGHTMAP_KINDS`].
fn kind_index(kind: HeightmapType) -> usize {
    match kind {
        HeightmapType::WorldSurface => 0,
        HeightmapType::WorldSurfaceWg => 1,
        HeightmapType::OceanFloor => 2,
        HeightmapType::OceanFloorWg => 3,
        HeightmapType::MotionBlocking => 4,
        HeightmapType::MotionBlockingNoLeaves => 5,
    }
}

/// The block fact a heightmap kind counts.
pub(crate) fn height_fact(kind: HeightmapType) -> BlockFact {
    match kind {
        HeightmapType::WorldSurface | HeightmapType::WorldSurfaceWg => {
            BlockFact::HeightWorldSurface
        }
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
             minecraft:short_grass\tDrc1\n\
             minecraft:void_air\tDarc\n\
             minecraft:deepslate\tDsmoudf1234\n\
             minecraft:coal_ore\tDsmoudf1234\n\
             minecraft:water[level=0]\tDlWS\n\
             minecraft:water[level=8]\tlWS\n\
             minecraft:chest\tDe\n\
             minecraft:spawner\tDsmoudf1234e\n",
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
                chunks.push(flat_chunk(
                    ChunkPos::new(center.x + dx, center.z + dz),
                    surface,
                ));
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
        assert_eq!(
            region.block(BlockPos::new(inside.x, 63, inside.z)),
            Some(&block("stone"))
        );

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
        assert_eq!(
            region.height(HeightmapType::MotionBlockingNoLeaves, 3, 3),
            Some(65)
        );
        assert_eq!(
            region.height(HeightmapType::WorldSurface, 4, 3),
            Some(64),
            "untouched column"
        );

        // Digging below the stored surface lowers it.
        region.set_block(BlockPos::new(8, 63, 8), block("air"));
        assert_eq!(region.height(HeightmapType::OceanFloor, 8, 8), Some(63));
        assert_eq!(region.height(HeightmapType::WorldSurface, 99, 0), None);
    }

    #[test]
    fn post_processing_marks_follow_the_blocks_rule_and_the_write_flags() {
        let facts = facts();
        let chunks = flat_ring(ChunkPos::new(0, 0), 64);
        let mut region = DecorationRegion::new(ChunkPos::new(0, 0), refs(&chunks), &facts);
        region.set_block(BlockPos::new(1, 64, 1), block("brown_mushroom"));
        region.set_block(BlockPos::new(2, 63, 2), block("magma_block"));
        region.set_block(BlockPos::new(3, 63, 3), block("soul_sand"));
        region.set_block(BlockPos::new(4, 64, 4), block("oak_log"));
        // An ore's bulk section write and a tree's known-shape write mark nothing.
        region.set_block_known_shape(BlockPos::new(5, 40, 5), block("magma_block"));
        // Outside the window: no write, no mark.
        assert!(!region.set_block(BlockPos::new(99, 64, 0), block("red_mushroom")));
        // Stone above stone is marked twice; air right above stops it.
        region.mark_above_for_post_processing(BlockPos::new(6, 50, 6));
        region.mark_above_for_post_processing(BlockPos::new(7, 63, 7));
        let (_, extras) = region.into_parts();
        assert_eq!(
            extras.post_process,
            vec![
                BlockPos::new(1, 64, 1),
                BlockPos::new(2, 64, 2),
                BlockPos::new(3, 64, 3),
                BlockPos::new(6, 51, 6),
                BlockPos::new(6, 52, 6),
            ]
        );
    }

    #[test]
    fn a_block_entity_goes_with_its_block() {
        let facts = facts();
        let chunks = flat_ring(ChunkPos::new(0, 0), 64);
        let mut region = DecorationRegion::new(ChunkPos::new(0, 0), refs(&chunks), &facts);
        let (kept, replaced, same) = (
            BlockPos::new(1, 64, 1),
            BlockPos::new(2, 64, 2),
            BlockPos::new(3, 64, 3),
        );
        for pos in [kept, replaced, same] {
            region.set_block(pos, block("chest"));
            region.put_block_entity(pos, format!("data {}", pos.x));
        }
        region.set_block(replaced, block("stone"));
        // The same block again keeps its data, as Paper's `setBlock` does.
        region.set_block(same, block("chest").with_property("waterlogged", "true"));
        // Nothing is kept outside the window.
        region.put_block_entity(BlockPos::new(99, 64, 0), "outside".into());
        let (_, extras) = region.into_parts();
        let kept_at: Vec<(BlockPos, &str)> = extras
            .block_entities
            .iter()
            .map(|e| (e.pos, e.snbt.as_str()))
            .collect();
        assert_eq!(kept_at, vec![(kept, "data 1"), (same, "data 3")]);
    }
}
