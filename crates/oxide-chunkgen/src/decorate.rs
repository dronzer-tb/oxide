//! Decoration as a pure function of position, so chunks can be generated in any order.
//!
//! Vanilla decorates through shared mutable chunks: a chunk's features write into its eight
//! neighbours, and what they find there depends on which neighbour was decorated first. That
//! order comes from the chunk scheduler, and it is why two vanilla servers disagree on a few
//! blocks around the same spot. Oxide cannot adopt it: `generate_chunk` is called per chunk, in
//! whatever order players walk, on several Folia region threads at once, and the same chunk must
//! come out the same every time.
//!
//! So decoration is split in two pure steps:
//!
//! 1. **A chunk's writes.** Run every feature of chunk `N` against the *undecorated* terrain of
//!    the 3x3 chunks around it, and record the block writes instead of applying them
//!    ([`DecorationRegion`]). That list depends on the seed and on terrain only.
//! 2. **A finished chunk.** Chunk `C` is its terrain plus, from each of the nine chunks around
//!    it, the writes that land inside `C`, applied in one fixed order.
//!
//! Both steps are memoised ([`DecorationCache`]); evicting an entry or racing two threads into
//! the same computation costs work and cannot change output.
//!
//! PARITY-CHECK: this is deliberately not vanilla's result wherever two chunks' features touch
//! the same blocks. A feature never sees a neighbour's decoration (a tree does not grow around
//! the tree next door, it overlaps it), and overlaps resolve by the fixed order below rather
//! than by scheduler order. Features that stay inside one chunk's own writes are unaffected.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use oxide_core::{ChunkData, ChunkPos, ChunkStatus, Heightmap, HeightmapType};
use oxide_datapack::BlockFacts;

use crate::fill::local_index;
use crate::region::{height_fact, DecorationRegion, InternedWrites};

/// Runs one chunk's features. The region's centre is the chunk being decorated.
pub trait Decorator: Sync {
    fn decorate(&self, region: &mut DecorationRegion<'_>);
}

/// A bounded map that forgets its oldest entry first. Only ever a memoisation.
struct Bounded<K, V> {
    capacity: usize,
    entries: HashMap<K, V>,
    age: VecDeque<K>,
}

impl<K: Hash + Eq + Clone, V: Clone> Bounded<K, V> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            age: VecDeque::new(),
        }
    }

    fn get(&self, key: &K) -> Option<V> {
        self.entries.get(key).cloned()
    }

    fn insert(&mut self, key: K, value: V) {
        if self.entries.insert(key.clone(), value).is_none() {
            self.age.push_back(key);
            while self.age.len() > self.capacity {
                if let Some(oldest) = self.age.pop_front() {
                    self.entries.remove(&oldest);
                }
            }
        }
    }
}

/// Memoised terrain and decoration writes, shared by every thread generating one world.
pub struct DecorationCache {
    terrain: Mutex<Bounded<ChunkPos, Arc<ChunkData>>>,
    writes: Mutex<Bounded<ChunkPos, Arc<InternedWrites>>>,
}

impl DecorationCache {
    /// `terrain_chunks` bounds memory: an undecorated chunk is a few hundred kilobytes, and
    /// finishing one chunk touches the 5x5 around it.
    pub fn new(terrain_chunks: usize, write_lists: usize) -> Self {
        Self {
            terrain: Mutex::new(Bounded::new(terrain_chunks.max(25))),
            writes: Mutex::new(Bounded::new(write_lists.max(9))),
        }
    }

    fn terrain(
        &self,
        pos: ChunkPos,
        generate: &(dyn Fn(ChunkPos) -> ChunkData + Sync),
    ) -> Arc<ChunkData> {
        if let Some(found) = self
            .terrain
            .lock()
            .expect("terrain cache poisoned")
            .get(&pos)
        {
            return found;
        }
        // Outside the lock: generation is slow, and two threads racing here produce the same
        // chunk -- the loser's copy is dropped.
        let made = Arc::new(generate(pos));
        self.terrain
            .lock()
            .expect("terrain cache poisoned")
            .insert(pos, made.clone());
        made
    }

    fn writes(
        &self,
        pos: ChunkPos,
        generate: &(dyn Fn(ChunkPos) -> ChunkData + Sync),
        decorator: &dyn Decorator,
        facts: &BlockFacts,
    ) -> Arc<InternedWrites> {
        if let Some(found) = self.writes.lock().expect("write cache poisoned").get(&pos) {
            return found;
        }
        let mut ring = Vec::with_capacity(9);
        for dz in -1..=1 {
            for dx in -1..=1 {
                ring.push(self.terrain(ChunkPos::new(pos.x + dx, pos.z + dz), generate));
            }
        }
        let chunks: [&ChunkData; 9] = std::array::from_fn(|index| ring[index].as_ref());
        let mut region = DecorationRegion::new(pos, chunks, facts);
        decorator.decorate(&mut region);
        let made = Arc::new(region.into_interned());
        self.writes
            .lock()
            .expect("write cache poisoned")
            .insert(pos, made.clone());
        made
    }
}

/// The finished chunk at `pos`: its terrain with every decoration write that lands in it.
///
/// `generate` produces a chunk's undecorated terrain (noise, surface, carvers) and must be a
/// pure function of position.
pub fn decorated_chunk(
    pos: ChunkPos,
    generate: &(dyn Fn(ChunkPos) -> ChunkData + Sync),
    decorator: &dyn Decorator,
    facts: &BlockFacts,
    cache: &DecorationCache,
) -> ChunkData {
    let mut chunk = (*cache.terrain(pos, generate)).clone();
    // West to east, then north to south within a column of chunks: fixed, so overlapping
    // writes from two neighbours resolve the same way whoever asks.
    for dx in -1..=1 {
        for dz in -1..=1 {
            let source = ChunkPos::new(pos.x + dx, pos.z + dz);
            let writes = cache.writes(source, generate, decorator, facts);
            for (at, state) in writes.iter() {
                if at.chunk_pos() != pos || at.y < chunk.min_y || at.y >= chunk.min_y + chunk.height
                {
                    continue;
                }
                let Some(section) = chunk.sections.get_mut(((at.y - chunk.min_y) >> 4) as usize)
                else {
                    continue;
                };
                let slot = local_index(
                    (at.x & 15) as usize,
                    ((at.y - chunk.min_y) & 15) as usize,
                    (at.z & 15) as usize,
                );
                section.block_states.set(slot, state.clone());
            }
        }
    }
    chunk.heightmaps = heightmaps(&chunk, facts);
    chunk.status = ChunkStatus::Features;
    chunk
}

/// All six heightmaps from the game's own per-state predicates.
pub fn heightmaps(chunk: &ChunkData, facts: &BlockFacts) -> HashMap<HeightmapType, Heightmap> {
    const KINDS: [HeightmapType; 6] = [
        HeightmapType::WorldSurface,
        HeightmapType::WorldSurfaceWg,
        HeightmapType::OceanFloor,
        HeightmapType::OceanFloorWg,
        HeightmapType::MotionBlocking,
        HeightmapType::MotionBlockingNoLeaves,
    ];
    let mut maps: Vec<Heightmap> = KINDS.iter().map(|_| Heightmap::new(chunk.height)).collect();
    for (section_index, section) in chunk.sections.iter().enumerate().rev() {
        // Per palette entry, which kinds count it: a section has a handful of distinct states.
        let counted: Vec<[bool; 6]> = section
            .block_states
            .palette()
            .iter()
            .map(|state| KINDS.map(|kind| facts.is(state, height_fact(kind))))
            .collect();
        if counted
            .iter()
            .all(|kinds| kinds.iter().all(|&counts| !counts))
        {
            continue;
        }
        let indices = section.block_states.indices();
        for local_y in (0..16usize).rev() {
            let above = (section_index * 16 + local_y + 1) as i32;
            for local_z in 0..16usize {
                for local_x in 0..16usize {
                    let entry = indices[local_index(local_x, local_y, local_z)] as usize;
                    for (kind, map) in maps.iter_mut().enumerate() {
                        if counted[entry][kind] && map.get(local_x, local_z) == 0 {
                            map.set(local_x, local_z, above);
                        }
                    }
                }
            }
        }
    }
    KINDS.into_iter().zip(maps).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::region::testutil::{block, facts, flat_chunk};
    use oxide_core::BlockPos;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Plants a three-block pole at each chunk's own corner and one block across each border.
    struct Poles;

    impl Decorator for Poles {
        fn decorate(&self, region: &mut DecorationRegion<'_>) {
            let center = region.center();
            let (x, z) = (center.min_block_x(), center.min_block_z());
            let top = region.height(HeightmapType::WorldSurface, x, z).unwrap();
            for y in top..top + 3 {
                region.set_block(BlockPos::new(x, y, z), block("oak_log"));
            }
            region.set_block(BlockPos::new(x - 1, top, z), block("oak_leaves"));
            region.set_block(BlockPos::new(x, top, z - 1), block("oak_leaves"));
        }
    }

    fn block_at(chunk: &ChunkData, x: i32, y: i32, z: i32) -> oxide_core::BlockState {
        let section = &chunk.sections[((y - chunk.min_y) >> 4) as usize];
        section
            .block_states
            .get(local_index(
                (x & 15) as usize,
                ((y - chunk.min_y) & 15) as usize,
                (z & 15) as usize,
            ))
            .clone()
    }

    #[test]
    fn a_chunk_gets_its_own_writes_and_its_neighbours() {
        let facts = facts();
        let cache = DecorationCache::new(64, 64);
        let terrain = |pos: ChunkPos| flat_chunk(pos, 64);
        let chunk = decorated_chunk(ChunkPos::new(0, 0), &terrain, &Poles, &facts, &cache);

        assert_eq!(block_at(&chunk, 0, 64, 0), block("oak_log"));
        assert_eq!(block_at(&chunk, 0, 66, 0), block("oak_log"));
        // The chunk to the east wrote one leaf across the border into x = 15.
        assert_eq!(block_at(&chunk, 15, 64, 0), block("oak_leaves"));
        assert_eq!(block_at(&chunk, 0, 64, 15), block("oak_leaves"));
        assert_eq!(block_at(&chunk, 5, 64, 5), block("air"));
        assert_eq!(chunk.status, ChunkStatus::Features);

        let surface = &chunk.heightmaps[&HeightmapType::WorldSurface];
        assert_eq!(surface.get(0, 0), 64 + 3 + 64);
        assert_eq!(surface.get(5, 5), 64 + 64);
        let no_leaves = &chunk.heightmaps[&HeightmapType::MotionBlockingNoLeaves];
        assert_eq!(
            no_leaves.get(15, 0),
            64 + 64,
            "a leaf does not raise this map"
        );
    }

    #[test]
    fn output_does_not_depend_on_request_order_or_on_the_cache() {
        let facts = facts();
        let terrain = |pos: ChunkPos| flat_chunk(pos, 60 + (pos.x + pos.z).rem_euclid(3));
        let targets = [
            ChunkPos::new(0, 0),
            ChunkPos::new(1, 0),
            ChunkPos::new(0, 1),
            ChunkPos::new(-1, -1),
        ];

        let warm = DecorationCache::new(64, 64);
        let forward: Vec<ChunkData> = targets
            .iter()
            .map(|&pos| decorated_chunk(pos, &terrain, &Poles, &facts, &warm))
            .collect();
        // Reverse order, and a cache so small everything is evicted and recomputed.
        let cold = DecorationCache::new(1, 1);
        let mut backward: Vec<ChunkData> = targets
            .iter()
            .rev()
            .map(|&pos| decorated_chunk(pos, &terrain, &Poles, &facts, &cold))
            .collect();
        backward.reverse();

        for (a, b) in forward.iter().zip(&backward) {
            for (left, right) in a.sections.iter().zip(&b.sections) {
                for slot in 0..4096 {
                    assert_eq!(left.block_states.get(slot), right.block_states.get(slot));
                }
            }
        }
    }

    #[test]
    fn terrain_is_generated_once_per_chunk_while_cached() {
        let facts = facts();
        let cache = DecorationCache::new(64, 64);
        let calls = AtomicUsize::new(0);
        let terrain = |pos: ChunkPos| {
            calls.fetch_add(1, Ordering::Relaxed);
            flat_chunk(pos, 64)
        };
        decorated_chunk(ChunkPos::new(0, 0), &terrain, &Poles, &facts, &cache);
        assert_eq!(
            calls.load(Ordering::Relaxed),
            25,
            "the 5x5 around one chunk"
        );
        decorated_chunk(ChunkPos::new(1, 0), &terrain, &Poles, &facts, &cache);
        assert_eq!(calls.load(Ordering::Relaxed), 30, "one new column of five");
    }
}
