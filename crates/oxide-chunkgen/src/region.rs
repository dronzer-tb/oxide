//! The 3x3-chunk window one chunk's decoration reads and writes.
//!
//! Vanilla hands a feature a `WorldGenRegion`: the chunk being decorated plus the ring around
//! it, because a tree rooted near a border has branches next door. [`DecorationRegion`] is that
//! window over *undecorated* terrain, with every write kept in an overlay instead of applied.
//! The overlay is the point: a chunk's decoration becomes a list of block writes that is a pure
//! function of the terrain around it, which is what lets `crate::decorate` hand out chunks in
//! any order and still agree (see that module).

use std::cell::{Cell, OnceCell};

use ahash::AHashMap;

use oxide_core::{BiomeId, BlockPos, BlockState, ChunkData, ChunkPos, Heightmap, HeightmapType};
use oxide_datapack::{BlockFact, BlockFacts};

use crate::fill::local_index;

/// Columns across the 3x3-chunk window.
const WINDOW: usize = 48;
/// `written_top` for a column decoration has not written.
const NEVER_WRITTEN: i32 = i32::MIN;
/// Writes to reserve room for up front: a region averages about 7.5k at radius 16.
const RESERVED_WRITES: usize = 8192;

/// The six heightmap kinds as indices into a per-chunk table.
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

const KINDS: [HeightmapType; 6] = [
    HeightmapType::WorldSurface,
    HeightmapType::WorldSurfaceWg,
    HeightmapType::OceanFloor,
    HeightmapType::OceanFloorWg,
    HeightmapType::MotionBlocking,
    HeightmapType::MotionBlockingNoLeaves,
];

/// One block write a decoration made, in world coordinates.
pub type BlockWrite = (BlockPos, BlockState);

/// A region's writes with every distinct state stored once: the same list as
/// [`DecorationRegion::into_writes`], in the same order, but each write names its state by
/// index into `states` instead of owning a copy.
///
/// Prefer it wherever the writes are only read: cloning a `BlockState` per write (two heap
/// strings each, into memory that stays live) is about a quarter of a region's CPU.
pub struct InternedWrites {
    /// Each write's position and its state's index into `states`, in first-write order.
    pub writes: Vec<(BlockPos, u32)>,
    /// Every state some write names (and possibly a few none does), in first-seen order.
    pub states: Vec<BlockState>,
}

impl InternedWrites {
    /// The writes in order, each with its state.
    pub fn iter(&self) -> impl Iterator<Item = (BlockPos, &BlockState)> + '_ {
        self.writes
            .iter()
            .map(|&(pos, id)| (pos, &self.states[id as usize]))
    }
}

/// Names the state at a position for the life of a region: two positions with equal keys hold
/// equal states. A written position's key is its interned id; a terrain position's key is its
/// section and palette entry (terrain is read-only during decoration, so its palettes never
/// change). Lets a feature cache an answer about a state without hashing the state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateKey(u64);

impl StateKey {
    /// A small hash of the key, for direct-mapped caches.
    pub fn slot(self) -> usize {
        (self.0 ^ self.0 >> 29) as usize
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
    overlay: AHashMap<BlockPos, usize>,
    /// Every write in the order its position was first written, each with the id (into
    /// `states`) of its latest state: `into_writes` hands this list back with the ids resolved.
    writes: Vec<(BlockPos, u32)>,
    /// Every distinct state written so far, in first-written order. A region writes a few dozen
    /// distinct states thousands of times, so writes keep a `u32` instead of an owned
    /// `BlockState` (two heap strings and a map) each.
    states: Vec<BlockState>,
    /// `states` the other way round.
    state_ids: AHashMap<BlockState, u32>,
    /// Per entry of `states`, the facts already answered for it (see [`FactCache`]).
    state_facts: Vec<FactCache>,
    /// Per terrain section (`chunk * sections + section`), per palette entry, the facts already
    /// answered for that entry; made on the section's first fact query.
    terrain_facts: Vec<OnceCell<Box<[FactCache]>>>,
    /// Per chunk (as `chunks`), per kind (as [`kind_index`]), the terrain's stored heightmap.
    heightmaps: [[Option<&'a Heightmap>; 6]; 9],
    /// Highest written Y per column of the window, `(z - min_z) * 48 + (x - min_x)`;
    /// [`NEVER_WRITTEN`] for a column that still has its terrain heights.
    written_top: Box<[i32]>,
    /// The highest Y any write so far reached, [`NEVER_WRITTEN`] before the first.
    max_written: i32,
    /// Per kind, the highest stored height over all nine chunks (`max_y` if a chunk lacks that
    /// map), worked out on first use.
    stored_max: [Cell<Option<i32>>; 6],
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
            overlay: AHashMap::with_capacity(RESERVED_WRITES),
            writes: Vec::with_capacity(RESERVED_WRITES),
            states: Vec::new(),
            state_ids: AHashMap::default(),
            state_facts: Vec::new(),
            terrain_facts: (0..9 * middle.sections.len())
                .map(|_| OnceCell::new())
                .collect(),
            heightmaps: chunks.map(|chunk| KINDS.map(|kind| chunk.heightmaps.get(&kind))),
            written_top: vec![NEVER_WRITTEN; WINDOW * WINDOW].into_boxed_slice(),
            max_written: NEVER_WRITTEN,
            stored_max: Default::default(),
        }
    }

    /// `written_top`'s index for a column inside the window.
    fn column(&self, x: i32, z: i32) -> usize {
        let dx = x - (self.center.x - 1) * 16;
        let dz = z - (self.center.z - 1) * 16;
        debug_assert!((0..WINDOW as i32).contains(&dx) && (0..WINDOW as i32).contains(&dz));
        dz as usize * WINDOW + dx as usize
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
            Some(&i) => Some(&self.states[self.writes[i].1 as usize]),
            None => self.terrain_block(pos),
        }
    }

    /// [`Self::block`] together with a [`StateKey`] for the state, or `None` outside the window.
    pub fn block_keyed(&self, pos: BlockPos) -> Option<(StateKey, &BlockState)> {
        if let Some(&i) = self.overlay.get(&pos) {
            let id = self.writes[i].1;
            return Some((StateKey(1 << 63 | u64::from(id)), &self.states[id as usize]));
        }
        if pos.y < self.min_y || pos.y >= self.max_y() {
            return None;
        }
        let dx = (pos.x >> 4) - self.center.x;
        let dz = (pos.z >> 4) - self.center.z;
        if !(-1..=1).contains(&dx) || !(-1..=1).contains(&dz) {
            return None;
        }
        let chunk_index = ((dz + 1) * 3 + (dx + 1)) as usize;
        let chunk = self.chunks[chunk_index];
        let section_index = ((pos.y - self.min_y) >> 4) as usize;
        let section = chunk.sections.get(section_index)?;
        let slot = local_index(
            (pos.x & 15) as usize,
            ((pos.y - self.min_y) & 15) as usize,
            (pos.z & 15) as usize,
        );
        let states = &section.block_states;
        let entry = states.indices()[slot] as usize;
        let section_key = (chunk_index * chunk.sections.len() + section_index) as u64;
        Some((
            StateKey(section_key << 32 | entry as u64),
            &states.palette()[entry],
        ))
    }

    /// Whether `fact` holds at `pos`. Outside the window nothing holds.
    pub fn is(&self, pos: BlockPos, fact: BlockFact) -> bool {
        self.fact_at(pos, fact).unwrap_or(false)
    }

    /// Whether `fact` holds for the block at `pos`, or `None` outside the window: the same
    /// answer as `facts().is(block(pos)?, fact)`, but each palette entry and each written state
    /// asks the table at most once per fact.
    pub fn fact_at(&self, pos: BlockPos, fact: BlockFact) -> Option<bool> {
        if let Some(&i) = self.overlay.get(&pos) {
            let id = self.writes[i].1 as usize;
            return Some(self.state_facts[id].is(self.facts, &self.states[id], fact));
        }
        if pos.y < self.min_y || pos.y >= self.max_y() {
            return None;
        }
        let dx = (pos.x >> 4) - self.center.x;
        let dz = (pos.z >> 4) - self.center.z;
        if !(-1..=1).contains(&dx) || !(-1..=1).contains(&dz) {
            return None;
        }
        let chunk_index = ((dz + 1) * 3 + (dx + 1)) as usize;
        let chunk = self.chunks[chunk_index];
        let section_index = ((pos.y - self.min_y) >> 4) as usize;
        let section = chunk.sections.get(section_index)?;
        let slot = local_index(
            (pos.x & 15) as usize,
            ((pos.y - self.min_y) & 15) as usize,
            (pos.z & 15) as usize,
        );
        let states = &section.block_states;
        let entry = states.indices()[slot] as usize;
        let cache = self.terrain_facts[chunk_index * chunk.sections.len() + section_index]
            .get_or_init(|| (0..states.palette().len()).map(|_| FactCache::default()).collect());
        Some(cache[entry].is(self.facts, &states.palette()[entry], fact))
    }

    /// Writes `state` at `pos`. Returns `false`, writing nothing, outside the window -- the
    /// same refusal vanilla's region gives a feature that reaches past its write radius.
    pub fn set_block(&mut self, pos: BlockPos, state: BlockState) -> bool {
        if !self.contains(pos) {
            return false;
        }
        let id = match self.state_ids.get(&state) {
            Some(&id) => id,
            None => self.add_state(state),
        };
        self.write_id(pos, id);
        true
    }

    /// [`Self::set_block`] for a state the caller keeps, such as one from a feature's config:
    /// the state is cloned only the first time this region sees it.
    pub fn set_block_ref(&mut self, pos: BlockPos, state: &BlockState) -> bool {
        if !self.contains(pos) {
            return false;
        }
        let id = match self.state_ids.get(state) {
            Some(&id) => id,
            None => self.add_state(state.clone()),
        };
        self.write_id(pos, id);
        true
    }

    /// The id [`Self::set_block_id`] writes `state` with. Interning a state that is then never
    /// written leaves it in the table unused; the writes are unaffected.
    pub fn intern(&mut self, state: &BlockState) -> u32 {
        match self.state_ids.get(state) {
            Some(&id) => id,
            None => self.add_state(state.clone()),
        }
    }

    /// [`Self::set_block`] for a state already interned with [`Self::intern`]: no hashing of the
    /// state per write.
    pub fn set_block_id(&mut self, pos: BlockPos, id: u32) -> bool {
        debug_assert!((id as usize) < self.states.len());
        if !self.contains(pos) {
            return false;
        }
        self.write_id(pos, id);
        true
    }

    /// Interns a state `state_ids` does not hold yet.
    fn add_state(&mut self, state: BlockState) -> u32 {
        let id = self.states.len() as u32;
        self.state_ids.insert(state.clone(), id);
        self.states.push(state);
        self.state_facts.push(FactCache::default());
        id
    }

    /// Records the write of state `id` at `pos`, which must be inside the window.
    fn write_id(&mut self, pos: BlockPos, id: u32) {
        match self.overlay.entry(pos) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                self.writes[*entry.get()].1 = id;
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(self.writes.len());
                self.writes.push((pos, id));
            }
        }
        let column = self.column(pos.x, pos.z);
        let top = &mut self.written_top[column];
        *top = if *top == NEVER_WRITTEN { pos.y } else { (*top).max(pos.y) };
        self.max_written = self.max_written.max(pos.y);
    }

    /// The first free Y above the highest block `kind` counts in the column at `(x, z)`: what
    /// vanilla's `getHeight` returns. `None` outside the window.
    ///
    /// A column decoration has not written to answers from the terrain's stored heightmap; a
    /// written column is rescanned, since vanilla keeps its heightmaps current block by block
    /// and a later feature must see the tree an earlier one planted.
    pub fn height(&self, kind: HeightmapType, x: i32, z: i32) -> Option<i32> {
        let dx = (x >> 4) - self.center.x;
        let dz = (z >> 4) - self.center.z;
        if !(-1..=1).contains(&dx) || !(-1..=1).contains(&dz) {
            return None;
        }
        let chunk_index = ((dz + 1) * 3 + (dx + 1)) as usize;
        let stored = self.heightmaps[chunk_index][kind_index(kind)]
            .map(|map| map.get((x & 15) as usize, (z & 15) as usize) + self.min_y);
        let written_top = self.written_top[self.column(x, z)];
        if written_top == NEVER_WRITTEN {
            return Some(stored.unwrap_or(self.min_y));
        }
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

    /// A bound no column of the window exceeds: `height(kind, x, z) <= height_bound(kind)` for
    /// every `(x, z)` inside it, as decoration has left it so far. `height` answers an unwritten
    /// column from its stored map, and rescans a written one from at most
    /// `max(stored, written_top + 1)`, so the stored maps' maximum and the highest write bound it.
    pub fn height_bound(&self, kind: HeightmapType) -> i32 {
        let k = kind_index(kind);
        let stored = match self.stored_max[k].get() {
            Some(v) => v,
            None => {
                let mut v = self.min_y;
                for maps in &self.heightmaps {
                    v = v.max(match maps[k] {
                        Some(map) => {
                            (0..16)
                                .flat_map(|z| (0..16).map(move |x| map.get(x, z)))
                                .max()
                                .unwrap_or(0)
                                + self.min_y
                        }
                        None => self.max_y(),
                    });
                }
                self.stored_max[k].set(Some(v));
                v
            }
        };
        let written = if self.max_written == NEVER_WRITTEN {
            self.min_y
        } else {
            (self.max_written + 1).min(self.max_y())
        };
        stored.max(written)
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
        let interned = self.into_interned();
        interned
            .iter()
            .map(|(pos, state)| (pos, state.clone()))
            .collect()
    }

    /// [`Self::into_writes`] without a copy of the state per write.
    pub fn into_interned(self) -> InternedWrites {
        InternedWrites {
            writes: self.writes,
            states: self.states,
        }
    }
}

/// The facts already looked up for one block state: bit `f` of the low half says fact `f` has
/// been asked, the same bit of the high half holds the answer. Every `BlockFact` is one bit.
#[derive(Default)]
struct FactCache(Cell<u64>);

impl FactCache {
    #[inline]
    fn is(&self, facts: &BlockFacts, state: &BlockState, fact: BlockFact) -> bool {
        let bit = u64::from(fact as u32);
        let known = self.0.get();
        if known & bit != 0 {
            return known & (bit << 32) != 0;
        }
        let answer = facts.is(state, fact);
        self.0
            .set(known | bit | if answer { bit << 32 } else { 0 });
        answer
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
             minecraft:water[level=8]\tlWS\n",
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
        assert!(region.set_block_ref(inside, &block("air")));
        assert!(region.set_block_ref(inside, &block("oak_log")));
        let interned = region.into_interned();
        let expected = vec![(next_door, block("stone")), (inside, block("oak_log"))];
        let resolved: Vec<BlockWrite> = interned
            .iter()
            .map(|(pos, state)| (pos, state.clone()))
            .collect();
        assert_eq!(resolved, expected);
    }

    #[test]
    fn state_keys_name_states_and_ids_write_them() {
        let facts = facts();
        let chunks = flat_ring(ChunkPos::new(0, 0), 64);
        let mut region = DecorationRegion::new(ChunkPos::new(0, 0), refs(&chunks), &facts);
        let stone_a = BlockPos::new(1, 10, 1);
        let stone_b = BlockPos::new(20, 30, -7);
        let air = BlockPos::new(1, 100, 1);
        let key = |region: &DecorationRegion<'_>, pos| region.block_keyed(pos).unwrap().0;
        assert_eq!(region.block_keyed(stone_a).unwrap().1, &block("stone"));
        assert_ne!(key(&region, stone_a), key(&region, air));
        assert!(region.block_keyed(BlockPos::new(100, 10, 1)).is_none());

        let log = region.intern(&block("oak_log"));
        assert_eq!(region.intern(&block("oak_log")), log);
        assert!(region.set_block_id(stone_a, log));
        assert!(region.set_block_id(stone_b, log));
        assert!(!region.set_block_id(BlockPos::new(100, 10, 1), log));
        assert_eq!(key(&region, stone_a), key(&region, stone_b));
        assert_eq!(region.block_keyed(stone_b).unwrap().1, &block("oak_log"));
        assert_eq!(region.height(HeightmapType::WorldSurface, 1, 1), Some(64));
        let writes: Vec<BlockWrite> = region
            .into_interned()
            .iter()
            .map(|(pos, state)| (pos, state.clone()))
            .collect();
        assert_eq!(
            writes,
            vec![(stone_a, block("oak_log")), (stone_b, block("oak_log"))]
        );
    }

    #[test]
    fn height_bound_covers_every_column_as_writes_go() {
        let facts = facts();
        let chunks = flat_ring(ChunkPos::new(0, 0), 64);
        let mut region = DecorationRegion::new(ChunkPos::new(0, 0), refs(&chunks), &facts);
        let all = |region: &DecorationRegion<'_>, kind| {
            (-16..32)
                .flat_map(|z| (-16..32).map(move |x| (x, z)))
                .map(|(x, z)| region.height(kind, x, z).unwrap())
                .max()
                .unwrap()
        };
        for kind in KINDS {
            assert!(all(&region, kind) <= region.height_bound(kind));
        }
        region.set_block(BlockPos::new(3, 80, 3), block("oak_log"));
        region.set_block(BlockPos::new(-5, 70, 20), block("short_grass"));
        for kind in KINDS {
            assert!(all(&region, kind) <= region.height_bound(kind));
        }
        assert_eq!(region.height_bound(HeightmapType::WorldSurface), 81);
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
}
