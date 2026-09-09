//! Cross-chunk structure starts: which structures exist *near* a chunk, and stamping the
//! pieces of those structures that land inside it.
//!
//! This is the layer between placement (`placement.rs`/`set.rs`, which answer "is this chunk a
//! start chunk") and assembly (`jigsaw.rs`, which builds the pieces). It exists because the two
//! have different units: a start is decided per start-chunk, but a village spans ~10 chunks, so
//! generating chunk `(0, 0)` has to know about a start that was decided at `(-5, 3)`.
//!
//! **The ordering problem this solves.** `generate_chunk` is called per chunk, in whatever order
//! players walk, on several Folia region threads at once. A structure must not depend on which
//! chunk asked first. The rule enforced here is that a start is a *pure function* of
//! `(world_seed, start chunk, structure set)` — never of neighbours, never of generation order,
//! never of anything already in the cache. [`StructureStartCache`] is therefore only a
//! memoisation: evicting an entry, or racing two threads into computing the same start twice,
//! can cost work but cannot change output. Everything in this module is written to keep that
//! property, and [`tests::starts_are_order_independent`] is what holds it.
//!
//! **What this module deliberately does not do yet**, so a status report cannot confuse
//! wired-up with correct:
//!
//! - No biome gate. Vanilla checks the structure's `biomes` filter at the start position and
//!   retries the next weighted entry when it fails; here the first pick always wins. Structures
//!   will land in wrong biomes until [`StartAssembler`] implementations get a biome source.
//! - No heightmap update after stamping. The caller recomputes (`oxide-chunkgen`'s
//!   `compute_heightmaps`) — this crate cannot, it does not depend on `oxide-chunkgen`'s fill.
//! - No structure *references*: nothing here produces the `StructureStart`/`References` chunk
//!   NBT that `/locate` and structure-gated mob spawning read. That is not a Rust-side gap; the
//!   Bukkit `ChunkGenerator` API has no method to register one (see `docs/ARCHITECTURE.md`).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};

use oxide_core::{ChunkData, ChunkPos, LegacyRandom, RandomSource, ResourceLocation};
use oxide_datapack::{Registry, StructurePlacement, StructureSet};

use crate::jigsaw::AssembledStructure;
use crate::placement::{concentric_rings_chunks, potential_structure_chunk};
use crate::select::pick_weighted;
use crate::set::{is_structure_chunk, StructureSetLookup};

/// A structure decided at one start chunk: which set produced it, which structure was picked,
/// and the assembled pieces.
#[derive(Debug, Clone)]
pub struct StructureStart {
    pub set_id: ResourceLocation,
    pub structure_id: ResourceLocation,
    pub chunk: ChunkPos,
    pub structure: AssembledStructure,
}

/// Everything an assembler is allowed to look at. Deliberately closed: an assembler that could
/// read neighbouring chunks or previously generated starts would make output depend on
/// generation order, which is exactly what this module exists to prevent.
#[derive(Debug, Clone, Copy)]
pub struct StartContext<'a> {
    pub set_id: &'a ResourceLocation,
    pub structure_id: &'a ResourceLocation,
    pub chunk: ChunkPos,
    pub world_seed: i64,
}

/// Builds the pieces of one start.
///
/// Implementations must be pure — same context in, same structure out, no interior state — and
/// `Sync`, because several region threads assemble different starts concurrently.
pub trait StartAssembler: Sync {
    fn assemble(&self, ctx: &StartContext<'_>) -> Option<AssembledStructure>;
}

/// A rectangle of chunks, inclusive on both ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkArea {
    pub min_x: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_z: i32,
}

impl ChunkArea {
    pub fn around(center: ChunkPos, radius: i32) -> Self {
        Self {
            min_x: center.x - radius,
            min_z: center.z - radius,
            max_x: center.x + radius,
            max_z: center.z + radius,
        }
    }

    pub fn contains(&self, pos: ChunkPos) -> bool {
        pos.x >= self.min_x && pos.x <= self.max_x && pos.z >= self.min_z && pos.z <= self.max_z
    }
}

/// Vanilla's `WorldgenRandom.setLargeFeatureSeed` — the RNG a structure start is built from,
/// distinct from `setLargeFeatureWithSalt` which `placement.rs` uses to pick the start *chunk*.
///
/// Verified against real Minecraft 26.2: `javap -c` on the game's own `WorldgenRandom` shows
/// `setSeed(baseSeed)`, two `nextLong()` draws, then `setSeed(x * first ^ z * second ^ baseSeed)`,
/// and `tests/feature_seed_parity.rs` diffs this against 72 vectors produced by running that
/// class itself out of a Mojang-mapped 26.2 jar.
pub fn large_feature_seed(world_seed: i64, chunk_x: i32, chunk_z: i32) -> LegacyRandom {
    let mut rng = LegacyRandom::new(world_seed);
    let a = rng.next_long();
    let b = rng.next_long();
    let seed = (chunk_x as i64)
        .wrapping_mul(a)
        ^ (chunk_z as i64).wrapping_mul(b)
        ^ world_seed;
    LegacyRandom::new(seed)
}

/// Every `(set, start chunk)` pair whose start chunk falls inside `area`.
///
/// Walks the placement *region grid* rather than testing all `area` chunks one by one: a
/// `random_spread` set puts exactly one candidate in each `spacing`-sized region, so a 33x33
/// chunk search over 40 sets is a few hundred region probes instead of ~43k full gate
/// evaluations. The full [`is_structure_chunk`] gate still decides — frequency reduction and
/// exclusion zones are not re-implemented here.
///
/// Registry iteration is `BTreeMap`-ordered, so the returned order is a function of the ids
/// alone. Callers may rely on it.
pub fn candidate_starts_in_area(
    sets: &Registry<StructureSet>,
    lookup: &dyn StructureSetLookup,
    world_seed: i64,
    area: ChunkArea,
) -> Vec<(ResourceLocation, ChunkPos)> {
    let mut out = Vec::new();

    for (set_id, set) in sets.iter() {
        match &set.placement {
            StructurePlacement::RandomSpread {
                spacing,
                separation,
                salt,
                spread_type,
                ..
            } => {
                let spacing = (*spacing).max(1);
                let region_min_x = area.min_x.div_euclid(spacing);
                let region_max_x = area.max_x.div_euclid(spacing);
                let region_min_z = area.min_z.div_euclid(spacing);
                let region_max_z = area.max_z.div_euclid(spacing);

                for region_x in region_min_x..=region_max_x {
                    for region_z in region_min_z..=region_max_z {
                        let candidate = potential_structure_chunk(
                            world_seed,
                            spacing,
                            *separation,
                            *salt,
                            spread_type.unwrap_or_default(),
                            region_x * spacing,
                            region_z * spacing,
                        );
                        if !area.contains(candidate) {
                            continue;
                        }
                        if is_structure_chunk(
                            set,
                            lookup,
                            world_seed,
                            candidate.x,
                            candidate.z,
                            None,
                        ) {
                            out.push((set_id.clone(), candidate));
                        }
                    }
                }
            }
            StructurePlacement::ConcentricRings {
                distance,
                spread,
                count,
                ..
            } => {
                // Ring positions are global to the seed, not local to the search area, so this
                // recomputes the whole ring set per call. Strongholds are one set of ~128
                // positions; if a pack ever ships several concentric_rings sets this wants
                // memoising per (seed, set) alongside the start cache.
                for candidate in concentric_rings_chunks(world_seed, *distance, *spread, *count) {
                    if area.contains(candidate) {
                        out.push((set_id.clone(), candidate));
                    }
                }
            }
        }
    }

    out
}

/// Which structure a set places at one of its start chunks.
///
/// The RNG matches vanilla: `ChunkGenerator.createStructures` builds
/// `new WorldgenRandom(new LegacyRandomSource(0L))` and seeds it with
/// `setLargeFeatureSeed(levelSeed, chunk.x, chunk.z)` before drawing (confirmed from 26.2
/// bytecode).
///
/// Two vanilla behaviours are missing here, both consequences of having no biome gate:
/// vanilla copies the entry list and, when a pick *fails* to place, removes that entry, re-sums
/// the remaining weights and redraws until the list empties — with nothing able to fail, that
/// loop is unreachable, so adding the biome gate means adding the loop, not just a filter. And a
/// single-entry set skips the RNG entirely (vanilla takes `list.get(0)` without constructing the
/// random at all); this always draws, which picks the same entry but is worth knowing before
/// anything downstream starts sharing this RNG.
fn pick_structure(
    set: &StructureSet,
    world_seed: i64,
    chunk: ChunkPos,
) -> Option<ResourceLocation> {
    let mut rng = large_feature_seed(world_seed, chunk.x, chunk.z);
    pick_weighted(&mut rng, &set.structures).map(|entry| entry.structure.clone())
}

/// The starts a set decides at one start chunk. Pure in `(world_seed, chunk, set)`.
fn build_start<A: StartAssembler>(
    assembler: &A,
    set_id: &ResourceLocation,
    set: &StructureSet,
    world_seed: i64,
    chunk: ChunkPos,
) -> Option<StructureStart> {
    let structure_id = pick_structure(set, world_seed, chunk)?;
    let ctx = StartContext {
        set_id,
        structure_id: &structure_id,
        chunk,
        world_seed,
    };
    let structure = assembler.assemble(&ctx)?;
    Some(StructureStart {
        set_id: set_id.clone(),
        structure_id,
        chunk,
        structure,
    })
}

/// Memoises assembled starts by start chunk, and stamps them into chunks that intersect them.
///
/// The cache is bounded and evicts in insertion order. Eviction is always safe: entries are pure
/// functions of the seed and the start chunk, so a re-computed entry is bit-identical to the one
/// dropped. That is also why the assembler runs *outside* the lock — two threads racing the same
/// start duplicate the work and agree on the answer, which is cheaper than serialising every
/// region thread behind one mutex.
pub struct StructureStartCache<A: StartAssembler> {
    world_seed: i64,
    /// How far, in chunks, a start may reach from its start chunk. Chunks this far away are
    /// searched for starts that might overlap. Too small silently clips large structures
    /// (ancient cities, ocean monuments) at the edge of the search box; too large costs a
    /// bigger region sweep on every uncached chunk.
    search_radius_chunks: i32,
    capacity: usize,
    assembler: A,
    cache: RwLock<CacheInner>,
}

#[derive(Default)]
struct CacheInner {
    /// `None` records "this chunk was checked and starts nothing", so a barren chunk is not
    /// re-swept on every neighbour's generation.
    entries: HashMap<(ResourceLocation, ChunkPos), Option<Arc<StructureStart>>>,
    order: VecDeque<(ResourceLocation, ChunkPos)>,
}

impl<A: StartAssembler> StructureStartCache<A> {
    pub fn new(world_seed: i64, search_radius_chunks: i32, capacity: usize, assembler: A) -> Self {
        Self {
            world_seed,
            search_radius_chunks: search_radius_chunks.max(0),
            capacity: capacity.max(1),
            assembler,
            cache: RwLock::new(CacheInner::default()),
        }
    }

    /// Every start whose pieces could reach `chunk`, in registry order.
    pub fn starts_near(
        &self,
        chunk: ChunkPos,
        sets: &Registry<StructureSet>,
        lookup: &dyn StructureSetLookup,
    ) -> Vec<Arc<StructureStart>> {
        let area = ChunkArea::around(chunk, self.search_radius_chunks);
        let candidates = candidate_starts_in_area(sets, lookup, self.world_seed, area);

        let mut out = Vec::new();
        for (set_id, start_chunk) in candidates {
            let key = (set_id.clone(), start_chunk);
            if let Some(hit) = self.cache.read().expect("start cache poisoned").entries.get(&key) {
                if let Some(start) = hit {
                    out.push(Arc::clone(start));
                }
                continue;
            }

            // Assembled outside the lock; see the type doc for why the duplicate-work race is
            // the intended trade.
            let Some(set) = sets.get(&set_id) else {
                continue;
            };
            let built = build_start(
                &self.assembler,
                &set_id,
                set,
                self.world_seed,
                start_chunk,
            )
            .map(Arc::new);

            if let Some(start) = &built {
                out.push(Arc::clone(start));
            }
            self.insert(key, built);
        }
        out
    }

    /// Writes the parts of every nearby start that fall inside `chunk`.
    ///
    /// Heightmaps are left stale — stamping moves the surface, and only the caller knows whether
    /// it still needs them (see this module's doc).
    pub fn stamp(
        &self,
        chunk: &mut ChunkData,
        sets: &Registry<StructureSet>,
        lookup: &dyn StructureSetLookup,
    ) -> usize {
        let pos = chunk.pos;
        let starts = self.starts_near(pos, sets, lookup);
        let mut stamped = 0;
        for start in &starts {
            if start.structure.overall_bbox_intersects_chunk(pos) {
                start.structure.stamp_into_chunk(chunk, pos);
                stamped += 1;
            }
        }
        stamped
    }

    fn insert(&self, key: (ResourceLocation, ChunkPos), value: Option<Arc<StructureStart>>) {
        let mut guard = self.cache.write().expect("start cache poisoned");
        if guard.entries.insert(key.clone(), value).is_none() {
            guard.order.push_back(key);
        }
        while guard.order.len() > self.capacity {
            if let Some(oldest) = guard.order.pop_front() {
                guard.entries.remove(&oldest);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.cache.read().expect("start cache poisoned").entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::{BlockPos, BlockState, ChunkSection};
    use oxide_datapack::{SpreadType, StructureSetEntry};
    use std::sync::Arc as StdArc;

    /// A stand-in for the jigsaw assembler: one solid box of a marker block, deliberately wider
    /// than a chunk so cross-chunk stamping is what the tests actually exercise. Piece *layout*
    /// is `jigsaw.rs`'s problem; what is under test here is start bookkeeping.
    struct MarkerAssembler {
        half_width: i32,
        height: i32,
    }

    impl StartAssembler for MarkerAssembler {
        fn assemble(&self, ctx: &StartContext<'_>) -> Option<AssembledStructure> {
            let origin = BlockPos::new(
                ctx.chunk.min_block_x() + 8,
                64,
                ctx.chunk.min_block_z() + 8,
            );
            let mut structure = AssembledStructure::new(origin);
            let state = BlockState::new(ResourceLocation::minecraft("gold_block"));
            let mut blocks = Vec::new();
            for dx in -self.half_width..=self.half_width {
                for dz in -self.half_width..=self.half_width {
                    for dy in 0..self.height {
                        blocks.push((
                            BlockPos::new(origin.x + dx, origin.y + dy, origin.z + dz),
                            state.clone(),
                        ));
                    }
                }
            }
            structure.add_piece(crate::jigsaw::AssembledPiece {
                origin,
                bbox: crate::jigsaw::BoundingBox::new(
                    origin.x - self.half_width,
                    origin.y,
                    origin.z - self.half_width,
                    origin.x + self.half_width,
                    origin.y + self.height - 1,
                    origin.z + self.half_width,
                ),
                blocks,
            });
            Some(structure)
        }
    }

    struct NoOtherSets;
    impl StructureSetLookup for NoOtherSets {
        fn get(&self, _id: &ResourceLocation) -> Option<&StructureSet> {
            None
        }
    }

    fn village_set() -> StructureSet {
        StructureSet {
            structures: vec![StructureSetEntry {
                structure: ResourceLocation::minecraft("village_plains"),
                weight: 1,
            }],
            placement: StructurePlacement::RandomSpread {
                spacing: 8,
                separation: 4,
                salt: 10_387_312,
                frequency_reduction_method: None,
                frequency: None,
                locate_offset: None,
                exclusion_zone: None,
                spread_type: Some(SpreadType::Linear),
            },
        }
    }

    fn registry() -> Registry<StructureSet> {
        [(ResourceLocation::minecraft("villages"), village_set())]
            .into_iter()
            .collect()
    }

    fn empty_chunk(pos: ChunkPos) -> ChunkData {
        let mut chunk = ChunkData::new(pos, -64, 384);
        let air = BlockState::new(ResourceLocation::minecraft("air"));
        let biome = ResourceLocation::minecraft("plains");
        let count = chunk.section_count();
        for i in 0..count {
            chunk
                .sections
                .push(ChunkSection::new((-4 + i as i32) as i8, air.clone(), biome.clone()));
        }
        chunk
    }

    fn cache() -> StructureStartCache<MarkerAssembler> {
        StructureStartCache::new(
            1234,
            2,
            512,
            MarkerAssembler {
                half_width: 20,
                height: 4,
            },
        )
    }

    /// A start decided at one chunk must reach the neighbours its bounding box covers — this is
    /// the whole reason the cache exists.
    #[test]
    fn a_start_stamps_into_neighbouring_chunks() {
        let sets = registry();
        let cache = cache();
        let area = ChunkArea::around(ChunkPos::new(0, 0), 8);
        let candidates = candidate_starts_in_area(&sets, &NoOtherSets, 1234, area);
        assert!(
            !candidates.is_empty(),
            "spacing 8 must put at least one village start within 17x17 chunks"
        );

        let start_chunk = candidates[0].1;
        // half_width 20 spans past the start chunk's own 16 blocks in both directions.
        let neighbour = ChunkPos::new(start_chunk.x + 1, start_chunk.z);
        let mut chunk = empty_chunk(neighbour);
        let stamped = cache.stamp(&mut chunk, &sets, &NoOtherSets);
        assert_eq!(stamped, 1, "the neighbouring chunk must see the start");
        assert!(
            chunk_contains_marker(&chunk),
            "stamping reported a hit but wrote no blocks"
        );
    }

    fn chunk_contains_marker(chunk: &ChunkData) -> bool {
        chunk.sections.iter().any(|section| {
            (0..4096).any(|i| section.block_states.get(i).name.path() == "gold_block")
        })
    }

    /// The property the whole design rests on: what a chunk gets cannot depend on which chunk
    /// was generated first, or on how much of the cache survived eviction.
    #[test]
    fn starts_are_order_independent() {
        let sets = registry();
        // A chunk next to a real start, so the query has something to be order-independent
        // *about* -- an empty result is trivially stable and would prove nothing.
        let seed_area = ChunkArea::around(ChunkPos::new(0, 0), 8);
        let start_chunk = candidate_starts_in_area(&sets, &NoOtherSets, 1234, seed_area)[0].1;
        let target = ChunkPos::new(start_chunk.x + 1, start_chunk.z + 1);

        let cold = cache();
        let baseline: Vec<_> = cold
            .starts_near(target, &sets, &NoOtherSets)
            .iter()
            .map(|s| (s.set_id.clone(), s.chunk, s.structure_id.clone()))
            .collect();
        assert!(!baseline.is_empty(), "test seed must produce some starts");

        // Warm the cache by sweeping a spiral of other chunks first, then ask again.
        let warm = cache();
        for x in -6..6 {
            for z in -6..6 {
                warm.starts_near(ChunkPos::new(x, z), &sets, &NoOtherSets);
            }
        }
        let after_warm: Vec<_> = warm
            .starts_near(target, &sets, &NoOtherSets)
            .iter()
            .map(|s| (s.set_id.clone(), s.chunk, s.structure_id.clone()))
            .collect();
        assert_eq!(baseline, after_warm, "generation order changed the starts");

        // Same, with a cache far too small to hold the sweep — eviction must not change output.
        let tiny = StructureStartCache::new(
            1234,
            2,
            1,
            MarkerAssembler {
                half_width: 20,
                height: 4,
            },
        );
        for x in -6..6 {
            for z in -6..6 {
                tiny.starts_near(ChunkPos::new(x, z), &sets, &NoOtherSets);
            }
        }
        let after_eviction: Vec<_> = tiny
            .starts_near(target, &sets, &NoOtherSets)
            .iter()
            .map(|s| (s.set_id.clone(), s.chunk, s.structure_id.clone()))
            .collect();
        assert_eq!(baseline, after_eviction, "eviction changed the starts");
        assert!(tiny.len() <= 1, "capacity was not enforced");
    }

    /// Folia generates on several region threads at once; the cache must not make them disagree.
    #[test]
    fn concurrent_generation_agrees_with_serial_generation() {
        let sets = StdArc::new(registry());
        let seed_area = ChunkArea::around(ChunkPos::new(0, 0), 8);
        let target =
            candidate_starts_in_area(&sets, &NoOtherSets, 1234, seed_area)[0].1;

        let serial: Vec<_> = cache()
            .starts_near(target, &sets, &NoOtherSets)
            .iter()
            .map(|s| (s.set_id.clone(), s.chunk))
            .collect();

        let shared = StdArc::new(cache());
        let mut handles = Vec::new();
        for thread_index in 0..4 {
            let shared = StdArc::clone(&shared);
            let sets = StdArc::clone(&sets);
            handles.push(std::thread::spawn(move || {
                // Each thread walks a different band, so they collide on shared start chunks.
                for z in -8..8 {
                    shared.starts_near(ChunkPos::new(thread_index * 2 - 4, z), &sets, &NoOtherSets);
                }
                shared
                    .starts_near(target, &sets, &NoOtherSets)
                    .iter()
                    .map(|s| (s.set_id.clone(), s.chunk))
                    .collect::<Vec<_>>()
            }));
        }
        for handle in handles {
            assert_eq!(handle.join().unwrap(), serial, "threads disagreed on starts");
        }
    }

    /// A chunk outside every start's reach must be left alone — the search radius is a bound on
    /// what can touch a chunk, not a licence to stamp everything found in it.
    #[test]
    fn chunks_outside_a_start_are_untouched() {
        let sets = registry();
        let narrow = StructureStartCache::new(
            1234,
            2,
            512,
            MarkerAssembler {
                half_width: 2,
                height: 2,
            },
        );
        let area = ChunkArea::around(ChunkPos::new(0, 0), 8);
        let start_chunk = candidate_starts_in_area(&sets, &NoOtherSets, 1234, area)[0].1;
        let far = ChunkPos::new(start_chunk.x + 2, start_chunk.z + 2);
        let mut chunk = empty_chunk(far);
        assert_eq!(narrow.stamp(&mut chunk, &sets, &NoOtherSets), 0);
        assert!(!chunk_contains_marker(&chunk));
    }
}
