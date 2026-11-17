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
//! Both steps are memoised ([`DecorationCache`]), each value computed once while it is cached (a
//! second thread asking waits for the first); evicting an entry costs work and cannot change
//! output.
//!
//! PARITY-CHECK: this is deliberately not vanilla's result wherever two chunks' features touch
//! the same blocks. A feature never sees a neighbour's decoration (a tree does not grow around
//! the tree next door, it overlaps it), and overlaps resolve by the fixed order below rather
//! than by scheduler order. Features that stay inside one chunk's own writes are unaffected.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::{Arc, Mutex, OnceLock};

use oxide_core::{
    BlockPos, BlockState, ChunkData, ChunkPos, ChunkStatus, FastHashMap, FastHashSet, Heightmap,
    HeightmapType,
};
use oxide_datapack::{BlockFact, BlockFacts};

use crate::deco_stats::{self, MemoStats, Stopwatch};
use crate::decorator::block_entity;
use crate::fill::local_index;
use crate::region::{height_fact, BlockEntityData, Bucket, ChunkExtras, DecorationRegion};

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

/// Lock shards per memo: neighbouring chunks land in different shards, so threads finishing
/// adjacent chunks rarely meet on one lock.
const SHARDS: usize = 16;

/// A sharded, bounded memo of one value per chunk that computes each value once.
///
/// A thread asking for a value another thread is still computing waits for it instead of
/// computing it again: the slot is a `OnceLock` handed out under the shard's lock and filled
/// outside it.
struct Memo<V> {
    shards: Vec<Mutex<Bounded<ChunkPos, Slot<V>>>>,
    stats: &'static MemoStats,
}

/// One memoised value: filled once, by whichever thread asked first.
type Slot<V> = Arc<OnceLock<Arc<V>>>;

impl<V> Memo<V> {
    fn new(capacity: usize, stats: &'static MemoStats) -> Self {
        Self {
            shards: (0..SHARDS)
                .map(|_| Mutex::new(Bounded::new(capacity.div_ceil(SHARDS))))
                .collect(),
            stats,
        }
    }

    fn get_or_compute(&self, pos: ChunkPos, compute: impl FnOnce() -> V) -> Arc<V> {
        let shard = (pos.x.wrapping_mul(31).wrapping_add(pos.z)).rem_euclid(SHARDS as i32);
        let slot = {
            let locking = Stopwatch::start();
            let mut map = self.shards[shard as usize]
                .lock()
                .expect("decoration cache poisoned");
            locking.stop(&self.stats.lock);
            match map.get(&pos) {
                Some(slot) => slot,
                None => {
                    let slot = Arc::new(OnceLock::new());
                    map.insert(pos, slot.clone());
                    slot
                }
            }
        };
        if let Some(value) = slot.get() {
            deco_stats::hit(self.stats);
            return value.clone();
        }
        let timer = Stopwatch::start();
        let mut computed = false;
        let value = slot
            .get_or_init(|| {
                computed = true;
                Arc::new(compute())
            })
            .clone();
        timer.stop(if computed {
            &self.stats.computed
        } else {
            &self.stats.waited
        });
        value
    }
}

/// Memoised terrain and decoration writes, shared by every thread generating one world.
pub struct DecorationCache {
    terrain: Memo<ChunkData>,
    writes: Memo<SourceOutput>,
}

/// One chunk's features' output: block writes and the extras that came with them.
///
/// The writes are split by the chunk they land in (index `(dz + 1) * 3 + (dx + 1)` from the
/// source chunk), each in first-write order with its state's fact bits.
type SourceOutput = ([Bucket; 9], ChunkExtras);

impl DecorationCache {
    /// `terrain_chunks` bounds memory: an undecorated chunk is a few hundred kilobytes, and
    /// finishing one chunk touches the 5x5 around it.
    pub fn new(terrain_chunks: usize, write_lists: usize) -> Self {
        Self {
            terrain: Memo::new(terrain_chunks.max(25), &deco_stats::TERRAIN),
            writes: Memo::new(write_lists.max(9), &deco_stats::WRITES),
        }
    }

    /// Computes and caches `pos`'s terrain now, so a later chunk finds it.
    pub fn prepare_terrain(
        &self,
        pos: ChunkPos,
        generate: &(dyn Fn(ChunkPos) -> ChunkData + Sync),
    ) {
        self.terrain(pos, generate);
    }

    /// Computes and caches `pos`'s feature writes now, so a later chunk finds them.
    pub fn prepare_writes(
        &self,
        pos: ChunkPos,
        generate: &(dyn Fn(ChunkPos) -> ChunkData + Sync),
        decorator: &dyn Decorator,
        facts: &BlockFacts,
    ) {
        self.writes(pos, generate, decorator, facts);
    }

    fn terrain(
        &self,
        pos: ChunkPos,
        generate: &(dyn Fn(ChunkPos) -> ChunkData + Sync),
    ) -> Arc<ChunkData> {
        self.terrain.get_or_compute(pos, || generate(pos))
    }

    fn writes(
        &self,
        pos: ChunkPos,
        generate: &(dyn Fn(ChunkPos) -> ChunkData + Sync),
        decorator: &dyn Decorator,
        facts: &BlockFacts,
    ) -> Arc<SourceOutput> {
        self.writes.get_or_compute(pos, || {
            let mut ring = Vec::with_capacity(9);
            for dz in -1..=1 {
                for dx in -1..=1 {
                    ring.push(self.terrain(ChunkPos::new(pos.x + dx, pos.z + dz), generate));
                }
            }
            let chunks: [&ChunkData; 9] = std::array::from_fn(|index| ring[index].as_ref());
            let timer = Stopwatch::start();
            let mut region = DecorationRegion::new(pos, chunks, facts);
            timer.stop(&deco_stats::REGION_NEW);
            let timer = Stopwatch::start();
            decorator.decorate(&mut region);
            timer.stop(&deco_stats::DECORATE);
            let timer = Stopwatch::start();
            let buckets = region.into_buckets();
            timer.stop(&deco_stats::BUCKETS);
            buckets
        })
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
    decorated_chunk_with_extras(pos, generate, decorator, facts, cache).0
}

/// [`decorated_chunk`] and the extras that land in it: block entities, scheduled ticks and
/// post-processing marks, from the same nine chunks' features in the same fixed order.
///
/// Block entities follow the blocks: one is listed for every block-entity block decoration left
/// standing in the chunk, with the data a feature filled in when the block it was filled in for
/// is still the block there, and vanilla's `{id:"DUMMY",x,y,z}` placeholder otherwise. Ticks are
/// deduplicated per position and type, first kept, as `ProtoChunkTicks.schedule` does; marks per
/// position.
pub fn decorated_chunk_with_extras(
    pos: ChunkPos,
    generate: &(dyn Fn(ChunkPos) -> ChunkData + Sync),
    decorator: &dyn Decorator,
    facts: &BlockFacts,
    cache: &DecorationCache,
) -> (ChunkData, ChunkExtras) {
    let whole = Stopwatch::start();
    let terrain = cache.terrain(pos, generate);
    let timer = Stopwatch::start();
    let mut chunk = (*terrain).clone();
    timer.stop(&deco_stats::CLONE);
    let timer = Stopwatch::start();
    let inside = |at: &BlockPos| {
        at.chunk_pos() == pos && at.y >= chunk.min_y && at.y < chunk.min_y + chunk.height
    };
    // West to east, then north to south within a column of chunks: fixed, so overlapping
    // writes from two neighbours resolve the same way whoever asks.
    let mut sources = Vec::with_capacity(9);
    for dx in -1..=1 {
        for dz in -1..=1 {
            let source = ChunkPos::new(pos.x + dx, pos.z + dz);
            // This chunk as seen from the source: the opposite offset.
            let bucket = ((1 - dz) * 3 + (1 - dx)) as usize;
            sources.push((cache.writes(source, generate, decorator, facts), bucket));
        }
    }
    timer.stop(&deco_stats::SOURCES);
    let timer = Stopwatch::start();
    // Every write of a block-entity block, numbered in application order. The last write to a
    // position decides its block, so such a write stands only if the finished chunk holds its
    // state there and no later write of a block-entity block went to the same position.
    let mut entity_writes: Vec<(usize, BlockPos, &BlockState)> = Vec::new();
    let mut sequence = 0;
    for (output, bucket) in &sources {
        for (at, state, bits) in output.0[*bucket].iter() {
            if !inside(at) {
                continue;
            }
            let Some(section) = chunk.sections.get_mut(((at.y - chunk.min_y) >> 4) as usize) else {
                continue;
            };
            let slot = local_index(
                (at.x & 15) as usize,
                ((at.y - chunk.min_y) & 15) as usize,
                (at.z & 15) as usize,
            );
            section.block_states.set_cloned(slot, state);
            if bits & BlockFact::BlockEntity as u32 != 0 {
                entity_writes.push((sequence, *at, state));
            }
            sequence += 1;
        }
    }

    timer.stop(&deco_stats::APPLY);
    let timer = Stopwatch::start();
    let mut extras = ChunkExtras::default();
    let mut filled: FastHashMap<BlockPos, &BlockEntityData> = FastHashMap::default();
    for (output, _) in &sources {
        for entity in output.1.block_entities.iter().filter(|e| inside(&e.pos)) {
            filled.insert(entity.pos, entity);
        }
    }
    let mut latest: FastHashMap<BlockPos, usize> = FastHashMap::default();
    for (order, at, _) in &entity_writes {
        latest.insert(*at, *order);
    }
    let standing = entity_writes
        .iter()
        .filter(|(order, at, state)| latest[at] == *order && state_at(&chunk, *at) == *state);
    for &(_, at, state) in standing {
        extras.block_entities.push(match filled.get(&at) {
            Some(entity) if entity.block == state.name => (*entity).clone(),
            _ => BlockEntityData {
                pos: at,
                block: state.name.clone(),
                snbt: block_entity::dummy(at),
            },
        });
    }
    let (mut fluid_seen, mut block_seen, mut mark_seen) = (
        FastHashSet::default(),
        FastHashSet::default(),
        FastHashSet::default(),
    );
    for (output, _) in &sources {
        for tick in output.1.fluid_ticks.iter().filter(|(at, _)| inside(at)) {
            if fluid_seen.insert(tick) {
                extras.fluid_ticks.push(tick.clone());
            }
        }
        for tick in output.1.block_ticks.iter().filter(|(at, _)| inside(at)) {
            if block_seen.insert(tick) {
                extras.block_ticks.push(tick.clone());
            }
        }
        for at in output.1.post_process.iter().filter(|at| inside(at)) {
            if mark_seen.insert(at) {
                extras.post_process.push(*at);
            }
        }
    }

    timer.stop(&deco_stats::EXTRAS);
    let timer = Stopwatch::start();
    chunk.heightmaps = heightmaps(&chunk, facts);
    timer.stop(&deco_stats::HEIGHTMAPS);
    chunk.status = ChunkStatus::Features;
    whole.stop(&deco_stats::CHUNK);
    (chunk, extras)
}

/// The block at `pos`, which must lie inside `chunk`.
fn state_at(chunk: &ChunkData, pos: BlockPos) -> &BlockState {
    let y = pos.y - chunk.min_y;
    chunk.sections[(y >> 4) as usize]
        .block_states
        .get(local_index(
            (pos.x & 15) as usize,
            (y & 15) as usize,
            (pos.z & 15) as usize,
        ))
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
    use oxide_core::{BlockPos, ResourceLocation};
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

    fn block_at(chunk: &ChunkData, x: i32, y: i32, z: i32) -> BlockState {
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

    /// In each chunk: a filled chest, a spawner with no data, a filled chest the chunk to the
    /// east overwrites with stone, and ticks and marks every neighbour repeats.
    struct Entities;

    impl Decorator for Entities {
        fn decorate(&self, region: &mut DecorationRegion<'_>) {
            let center = region.center();
            let (x, z) = (center.min_block_x(), center.min_block_z());
            let filled = BlockPos::new(x + 1, 64, z + 1);
            region.set_block(filled, block("chest"));
            region.put_block_entity(filled, format!("chest {x}"));
            region.set_block(BlockPos::new(x + 2, 64, z + 2), block("spawner"));
            let lost = BlockPos::new(x + 3, 64, z + 3);
            region.set_block(lost, block("chest"));
            region.put_block_entity(lost, "lost".into());
            region.set_block(BlockPos::new(x - 13, 64, z + 3), block("stone"));
            region.schedule_fluid_tick(
                BlockPos::new(0, 70, 0),
                ResourceLocation::minecraft("water"),
            );
            region.schedule_block_tick(filled, ResourceLocation::minecraft("chest"));
            region.mark_post_process(BlockPos::new(1, 70, 1));
        }
    }

    #[test]
    fn extras_follow_the_finished_blocks_and_are_deduplicated() {
        let facts = facts();
        let cache = DecorationCache::new(64, 64);
        let terrain = |pos: ChunkPos| flat_chunk(pos, 64);
        let (chunk, extras) =
            decorated_chunk_with_extras(ChunkPos::new(0, 0), &terrain, &Entities, &facts, &cache);
        assert_eq!(block_at(&chunk, 3, 64, 3), block("stone"));
        let entities: Vec<(BlockPos, &str)> = extras
            .block_entities
            .iter()
            .map(|e| (e.pos, e.snbt.as_str()))
            .collect();
        assert_eq!(
            entities,
            vec![
                (BlockPos::new(1, 64, 1), "chest 0"),
                (BlockPos::new(2, 64, 2), r#"{id:"DUMMY",x:2,y:64,z:2}"#),
            ]
        );
        assert_eq!(
            extras.fluid_ticks,
            vec![(
                BlockPos::new(0, 70, 0),
                ResourceLocation::minecraft("water")
            )]
        );
        assert_eq!(
            extras.block_ticks,
            vec![(
                BlockPos::new(1, 64, 1),
                ResourceLocation::minecraft("chest")
            )]
        );
        assert_eq!(extras.post_process, vec![BlockPos::new(1, 70, 1)]);
        // A chunk the shared tick and mark do not land in gets neither.
        let (_, east) =
            decorated_chunk_with_extras(ChunkPos::new(1, 0), &terrain, &Entities, &facts, &cache);
        assert!(east.fluid_ticks.is_empty() && east.post_process.is_empty());
        assert_eq!(east.block_entities.len(), 2);
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
