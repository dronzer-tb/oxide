//! Aquifers: the underground water and lava bodies, and the reason a cave floods in one place
//! and stays dry twenty blocks away.
//!
//! Ported from the decompiled 26.2 `Aquifer.NoiseBasedAquifer`. This is what decides, for every
//! non-solid position, whether it is air, water or lava -- replacing the placeholder rule the
//! fill pass used ("below sea level is water"), which put an ocean inside every hill.
//!
//! Mechanically: aquifer centres sit on a jittered 16x12x16 grid, each with its own fluid
//! surface level and type. A position takes the fluid of the nearest centre, and where two
//! centres of different levels compete, a barrier-noise "pressure" term decides whether the
//! boundary is stone instead -- which is what keeps two adjacent aquifers from merging into one
//! lake.
//!
//! PARITY-CHECK notes:
//! - `shouldScheduleFluidUpdate` is not modelled at all: nothing here schedules block updates,
//!   so a fluid vanilla would let flow on the first tick stays put. Its threshold constant
//!   (`similarity(10^2, 12^2)`) is therefore absent rather than dead.
//! - `maxPreliminarySurfaceLevel` is sampled over the grid's corner columns rather than the
//!   whole region vanilla scans, so the `skipSamplingAboveY` shortcut can differ near a steep
//!   ridge. It only gates an optimisation; the fluid answer below the surface is unaffected.

use oxide_core::{BlockState, RandomSource, ResourceLocation};
use oxide_datapack::NoiseGeneratorSettings;
use oxide_noise::{ChunkCaches, NoiseRouterEvaluator, RouterSlot, WorldPositionalFactory};

const X_RANGE: i32 = 10;
const Y_RANGE: i32 = 9;
const Z_RANGE: i32 = 10;
const Y_SPACING: i32 = 12;
const SAMPLE_OFFSET_X: i32 = -5;
const SAMPLE_OFFSET_Y: i32 = 1;
const SAMPLE_OFFSET_Z: i32 = -5;
const WAY_BELOW_MIN_Y: i32 = -2032;
/// Side of the preliminary-surface memo, in quarts: 160 blocks, from five chunks before the
/// chunk's origin to five after.
const SURFACE_MEMO_SPAN: i32 = 40;
/// The same window in chunks.
const SURFACE_MEMO_CHUNKS: i32 = SURFACE_MEMO_SPAN / 4;

/// Side of one page of the per-thread preliminary-surface memo, in quarts (log2): 64 quarts,
/// 256 blocks, 16 chunks.
const SURFACE_PAGE_SHIFT: i32 = 6;
const SURFACE_PAGE_SIDE: i32 = 1 << SURFACE_PAGE_SHIFT;
/// Pages kept per thread before the least recently used one is reused: 16 pages of 4096
/// `i32`s, 256 KiB per thread at most.
const SURFACE_PAGE_COUNT: usize = 16;

/// One dense `SURFACE_PAGE_SIDE`-squared block of quart columns; `i32::MIN` = not sampled yet.
struct SurfacePage {
    page_x: i32,
    page_z: i32,
    last_used: u64,
    levels: Box<[i32]>,
}

/// `preliminary_surface_level` per quart column, shared by every chunk one thread generates
/// against one router, so a neighbour column is scanned once per thread rather than once per
/// chunk whose aquifer reaches it (up to ~20).
///
/// Keyed on [`NoiseRouterEvaluator::id`], which no two evaluators in a process share, even
/// when one is dropped and another is built at the same address. A different id clears every
/// page before use, so a value computed for one router is never returned for another.
#[derive(Default)]
struct SurfaceMemo {
    router_id: u64,
    clock: u64,
    pages: Vec<SurfacePage>,
}

impl SurfaceMemo {
    /// Bind to `router_id`, dropping every page if they were filled for another router.
    fn bind(&mut self, router_id: u64) {
        if self.router_id != router_id {
            self.pages.clear();
            self.router_id = router_id;
        }
    }

    /// The page holding quart column `(quart_x, quart_z)` (block coordinates, multiples of 4)
    /// and the column's index in it, making room for the page if it is not held.
    fn slot(&mut self, quart_x: i32, quart_z: i32) -> (usize, usize) {
        let qx = quart_x >> 2;
        let qz = quart_z >> 2;
        let page_x = qx >> SURFACE_PAGE_SHIFT;
        let page_z = qz >> SURFACE_PAGE_SHIFT;
        let index = ((qz & (SURFACE_PAGE_SIDE - 1)) * SURFACE_PAGE_SIDE
            + (qx & (SURFACE_PAGE_SIDE - 1))) as usize;
        self.clock += 1;
        let clock = self.clock;
        if let Some(found) = self
            .pages
            .iter()
            .position(|p| p.page_x == page_x && p.page_z == page_z)
        {
            self.pages[found].last_used = clock;
            return (found, index);
        }
        let page = if self.pages.len() < SURFACE_PAGE_COUNT {
            self.pages.push(SurfacePage {
                page_x,
                page_z,
                last_used: clock,
                levels: vec![i32::MIN; (SURFACE_PAGE_SIDE * SURFACE_PAGE_SIDE) as usize]
                    .into_boxed_slice(),
            });
            self.pages.len() - 1
        } else {
            let oldest = (0..self.pages.len())
                .min_by_key(|&i| self.pages[i].last_used)
                .expect("SURFACE_PAGE_COUNT > 0");
            let p = &mut self.pages[oldest];
            p.page_x = page_x;
            p.page_z = page_z;
            p.last_used = clock;
            p.levels.fill(i32::MIN);
            oldest
        };
        (page, index)
    }
}

thread_local! {
    /// The thread's [`SurfaceMemo`]. An [`Aquifer`] takes it out for its whole life and puts it
    /// back on drop, so lookups never touch the thread-local; an aquifer built while another
    /// one holds it just starts with an empty memo.
    static SURFACE_MEMO: std::cell::Cell<Option<Box<SurfaceMemo>>> =
        const { std::cell::Cell::new(None) };
}

/// Chunk-relative offsets, in chunks, at which `computeFluid` samples the preliminary surface.
const SURFACE_SAMPLING_OFFSETS_IN_CHUNKS: [[i32; 2]; 13] = [
    [0, 0],
    [-2, -1],
    [-1, -1],
    [0, -1],
    [1, -1],
    [-3, 0],
    [-2, 0],
    [-1, 0],
    [1, 0],
    [-2, 1],
    [-1, 1],
    [0, 1],
    [1, 1],
];

/// Which of the three block states an aquifer can produce.
///
/// A tag rather than the `BlockState` itself, so [`FluidStatus`] is `Copy`. The old shape held a
/// `BlockState`, which owns a `BTreeMap<String, String>` -- cloning one per block meant a map
/// allocation on every position in the chunk, and `global_fluid` alone built two of them for
/// every block it was asked about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FluidKind {
    Air,
    Sea,
    Lava,
}

/// A fluid surface: everything strictly below `level` is `fluid`, everything at or above is air.
#[derive(Clone, Copy, PartialEq, Eq)]
struct FluidStatus {
    level: i32,
    fluid: FluidKind,
}

impl FluidStatus {
    fn at_kind(&self, y: i32) -> FluidKind {
        if y < self.level {
            self.fluid
        } else {
            FluidKind::Air
        }
    }
}

/// An aquifer for this chunk, or `None` when the dimension's settings disable them.
pub fn for_settings<'a>(
    pos: oxide_core::ChunkPos,
    settings: &NoiseGeneratorSettings,
    router: &'a NoiseRouterEvaluator,
    caches: &'a ChunkCaches,
) -> Option<Aquifer<'a>> {
    settings
        .aquifers_enabled
        .then(|| Aquifer::new(pos.x, pos.z, settings, router, caches))
}

pub struct Aquifer<'a> {
    router: &'a NoiseRouterEvaluator,
    caches: &'a ChunkCaches,
    random: WorldPositionalFactory,

    air: BlockState,
    lava: BlockState,
    sea_fluid: BlockState,
    sea_level: i32,

    min_grid_x: i32,
    min_grid_y: i32,
    min_grid_z: i32,
    grid_size_x: i32,
    grid_size_y: i32,
    grid_size_z: i32,
    skip_sampling_above_y: i32,

    /// Per grid cell: the jittered centre position, and the fluid there once computed.
    locations: Vec<Option<(i32, i32, i32)>>,
    statuses: Vec<Option<FluidStatus>>,
    /// The twelve candidate centres around one anchor cell, per horizontal anchor
    /// `(anchor_x, anchor_z)` of the grid, tagged with the `anchor_y` they were drawn for. The
    /// fill loop walks y, then z, then x, so one row of blocks touches at most two anchors and
    /// one layer four: rebuilding the candidate list per block was most of `compute_kind`.
    anchors: Vec<AnchorCandidates>,
    /// Memoised `preliminary_surface_level` on a dense quart grid of [`SURFACE_MEMO_SPAN`]
    /// squared entries starting at the origin below; `i32::MIN` = not sampled yet.
    surface_levels: Vec<i32>,
    surface_origin_x: i32,
    surface_origin_z: i32,
    /// Private `ChunkCaches` for the other chunks of the surface memo window, built on first
    /// use, one per chunk (`SURFACE_MEMO_CHUNKS` squared slots). See
    /// `preliminary_surface_level` for why sampling through them gives the same values.
    neighbour_caches: Vec<Option<Box<ChunkCaches>>>,
    surface_origin_chunk_x: i32,
    surface_origin_chunk_z: i32,
    own_chunk_x: i32,
    own_chunk_z: i32,
    /// This thread's cross-chunk memo, bound to `router`'s id; returned to the thread on drop.
    shared_surface: Box<SurfaceMemo>,
    /// A run of one block row proven to share one answer: every non-solid position
    /// `(x, run_y, run_z)` with `run_x_lo <= x <= run_x_hi` gets `run_kind`. See `note_run`.
    run_y: i32,
    run_z: i32,
    run_x_lo: i32,
    run_x_hi: i32,
    run_kind: FluidKind,
}

impl Drop for Aquifer<'_> {
    fn drop(&mut self) {
        let memo = std::mem::take(&mut self.shared_surface);
        // `try_with`: during thread teardown the slot may already be gone; the memo is then
        // simply freed.
        let _ = SURFACE_MEMO.try_with(|slot| slot.set(Some(memo)));
    }
}

/// The 12 grid cells `compute_kind` weighs for one anchor, in its `(dx, dy, dz)` loop order:
/// each one's index into `locations`/`statuses` and its jittered centre.
#[derive(Clone, Copy)]
struct AnchorCandidates {
    anchor_y: i32,
    index: [usize; 12],
    centre: [(i32, i32, i32); 12],
}

impl<'a> Aquifer<'a> {
    pub fn new(
        chunk_x: i32,
        chunk_z: i32,
        settings: &NoiseGeneratorSettings,
        router: &'a NoiseRouterEvaluator,
        caches: &'a ChunkCaches,
    ) -> Self {
        let min_block_x = chunk_x * 16;
        let min_block_z = chunk_z * 16;
        let max_block_x = min_block_x + 15;
        let max_block_z = min_block_z + 15;
        let min_y = settings.noise.min_y;
        let height = settings.noise.height;

        let min_grid_x = grid_x(min_block_x + SAMPLE_OFFSET_X);
        let max_grid_x = grid_x(max_block_x + SAMPLE_OFFSET_X) + 1;
        let min_grid_y = grid_y(min_y + SAMPLE_OFFSET_Y) - 1;
        let max_grid_y = grid_y(min_y + height + SAMPLE_OFFSET_Y) + 1;
        let min_grid_z = grid_z(min_block_z + SAMPLE_OFFSET_Z);
        let max_grid_z = grid_z(max_block_z + SAMPLE_OFFSET_Z) + 1;

        let grid_size_x = max_grid_x - min_grid_x + 1;
        let grid_size_y = max_grid_y - min_grid_y + 1;
        let grid_size_z = max_grid_z - min_grid_z + 1;
        let total = (grid_size_x * grid_size_y * grid_size_z) as usize;

        let mut aquifer = Self {
            router,
            caches,
            // Vanilla's aquiferRandom: the RandomState factory hashed with "minecraft:aquifer".
            random: router
                .positional_factory()
                .from_hash_of("minecraft:aquifer")
                .fork_positional(),
            air: BlockState::new(ResourceLocation::minecraft("air")),
            lava: BlockState::new(ResourceLocation::minecraft("lava")),
            sea_fluid: settings.default_fluid.clone(),
            sea_level: settings.sea_level,
            min_grid_x,
            min_grid_y,
            min_grid_z,
            grid_size_x,
            grid_size_y,
            grid_size_z,
            skip_sampling_above_y: i32::MAX,
            locations: vec![None; total],
            statuses: vec![None; total],
            anchors: vec![
                AnchorCandidates {
                    anchor_y: i32::MIN,
                    index: [0; 12],
                    centre: [(0, 0, 0); 12],
                };
                (grid_size_x * grid_size_z) as usize
            ],
            // Thirteen offsets per aquifer centre, heavily overlapping between centres. Centres
            // sit within one grid cell (16 blocks) of the chunk and the offsets reach three
            // chunks back and one forward, so five chunks back covers every lookup.
            surface_levels: vec![i32::MIN; (SURFACE_MEMO_SPAN * SURFACE_MEMO_SPAN) as usize],
            surface_origin_x: min_block_x - 80,
            surface_origin_z: min_block_z - 80,
            neighbour_caches: (0..SURFACE_MEMO_CHUNKS * SURFACE_MEMO_CHUNKS)
                .map(|_| None)
                .collect(),
            surface_origin_chunk_x: chunk_x - 5,
            surface_origin_chunk_z: chunk_z - 5,
            own_chunk_x: chunk_x,
            own_chunk_z: chunk_z,
            shared_surface: {
                let mut memo = SURFACE_MEMO
                    .try_with(|slot| slot.take())
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                memo.bind(router.id());
                memo
            },
            run_y: i32::MIN,
            run_z: 0,
            run_x_lo: 0,
            run_x_hi: -1,
            run_kind: FluidKind::Air,
        };

        // Above the highest surface in reach there is nothing to flood, so vanilla stops
        // sampling. See the PARITY-CHECK note on how this bound is estimated.
        let mut max_surface = i32::MIN;
        for gx in [min_grid_x, max_grid_x] {
            for gz in [min_grid_z, max_grid_z] {
                for offset in [0, X_RANGE - 1] {
                    let x = from_grid_x(gx, offset);
                    let z = from_grid_z(gz, offset);
                    max_surface = max_surface.max(aquifer.preliminary_surface_level(x, z));
                }
            }
        }
        let skip_grid_y = grid_y(adjust_surface_level(max_surface) + Y_SPACING) + 1;
        aquifer.skip_sampling_above_y = from_grid_y(skip_grid_y, Y_SPACING - 1) - 1;
        aquifer
    }

    /// The global rule: lava below both -54 and sea level, otherwise the dimension's fluid up
    /// to sea level. `createFluidPicker` in vanilla.
    /// `Aquifer#globalFluidPicker`. Two fixed values, so this returns them rather than building
    /// one -- the old shape allocated a `BlockState` (and its `BTreeMap`) on every call, and it
    /// is called several times for every block in the chunk.
    fn global_fluid(&self, y: i32) -> FluidStatus {
        if y < (-54).min(self.sea_level) {
            FluidStatus {
                level: -54,
                fluid: FluidKind::Lava,
            }
        } else {
            FluidStatus {
                level: self.sea_level,
                fluid: FluidKind::Sea,
            }
        }
    }

    /// The block state a [`FluidKind`] stands for. The only place a `BlockState` is cloned, and
    /// it happens once per returned block rather than once per intermediate comparison.
    /// Above this y, `compute_substance` answers from `global_fluid` alone: no grid sampling,
    /// no memo writes, no mutation of any kind. A caller that has already proven a whole cell
    /// non-solid can therefore predict the answer without calling in -- see `fill_chunk_culling`.
    pub fn skip_sampling_above_y(&self) -> i32 {
        self.skip_sampling_above_y
    }

    /// What `compute_substance` would return for a non-solid position above
    /// [`Self::skip_sampling_above_y`], without touching `self`. `None` means "plain air", which
    /// a chunk section already holds as its palette default.
    pub fn air_above_skip(&self, y: i32) -> bool {
        y > self.skip_sampling_above_y && self.global_fluid(y).at_kind(y) == FluidKind::Air
    }

    /// The block state a [`FluidKind`] stands for.
    pub fn block_for(&self, kind: FluidKind) -> BlockState {
        match kind {
            FluidKind::Air => self.air.clone(),
            FluidKind::Sea => self.sea_fluid.clone(),
            FluidKind::Lava => self.lava.clone(),
        }
    }

    /// `Aquifer#computeSubstance`. `None` means "solid" -- the caller places its default block.
    pub fn compute_substance(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        density: f64,
    ) -> Option<BlockState> {
        self.compute_kind(x, y, z, density)
            .map(|kind| self.block_for(kind))
    }

    /// [`Self::compute_substance`] as a tag instead of a `BlockState`, for a caller that
    /// resolves palette indices itself: the fill loop asks this for every non-solid block, and
    /// building then dropping a `BlockState` each time was two heap allocations per block.
    pub fn compute_kind(&mut self, x: i32, y: i32, z: i32, density: f64) -> Option<FluidKind> {
        if density > 0.0 {
            return None;
        }
        // Inside a proven run (see `note_run`): the answer is known without the search. A run
        // is only recorded for a y below the skip line and above the lava rule, so the two
        // early returns below would not have been taken for this y either.
        if y == self.run_y && z == self.run_z && x >= self.run_x_lo && x <= self.run_x_hi {
            return Some(self.run_kind);
        }
        let global = self.global_fluid(y);
        if y > self.skip_sampling_above_y {
            return Some(global.at_kind(y));
        }
        if global.at_kind(y) == FluidKind::Lava {
            return Some(FluidKind::Lava);
        }

        let anchor_x = grid_x(x + SAMPLE_OFFSET_X);
        let anchor_y = grid_y(y + SAMPLE_OFFSET_Y);
        let anchor_z = grid_z(z + SAMPLE_OFFSET_Z);

        // The three nearest aquifer centres, by squared distance. A candidate that ties an
        // incumbent goes in front of it, as vanilla's cascade of `>=` comparisons does.
        let mut best = [(i32::MAX, 0usize); 3];
        if let Some(slot) = self.anchor_slot(anchor_x, anchor_y, anchor_z) {
            if self.anchors[slot].anchor_y != anchor_y {
                self.fill_anchor(slot, anchor_x, anchor_y, anchor_z);
            }
            let candidates = &self.anchors[slot];
            let mut distances = [0i32; 12];
            for i in 0..12 {
                let (cx, cy, cz) = candidates.centre[i];
                let ddx = cx - x;
                let ddy = cy - y;
                let ddz = cz - z;
                let distance = ddx * ddx + ddy * ddy + ddz * ddz;
                distances[i] = distance;
                let index = candidates.index[i];
                if best[0].0 >= distance {
                    best[2] = best[1];
                    best[1] = best[0];
                    best[0] = (distance, index);
                } else if best[1].0 >= distance {
                    best[2] = best[1];
                    best[1] = (distance, index);
                } else if best[2].0 >= distance {
                    best[2] = (distance, index);
                }
            }
            let kind = self.kind_from_nearest(x, y, z, density, &best);
            self.note_run(slot, x, y, z, anchor_x, &distances, &best);
            kind
        } else {
            self.nearest_slow(x, y, z, anchor_x, anchor_y, anchor_z, &mut best);
            self.kind_from_nearest(x, y, z, density, &best)
        }
    }

    /// After a full fast-path answer at `(x, y, z)`, records how far along +x in this row the
    /// same answer is proven, so the next blocks of the row skip the search.
    ///
    /// Let `p` be the nearest candidate (`best[0]`) and `S` its status. For a later `x' = x + t`
    /// in the same anchor (same y and z, so the same 12 candidates), the gap to candidate `j`
    /// is `f_j(x') = d_j(x') - d_p(x') = f_j(x) - 2 * (cx_j - cx_p) * t`, exact in `i32`. Either
    /// rule below makes `compute_kind(x', y, z, density <= 0)` return `S.at_kind(y)`:
    /// - every `j != p` has `f_j >= 25`: `p` is the unique nearest and the runner-up is at
    ///   least 25 further, so `similarity12 <= 0` and the first return is taken;
    /// - every `j` whose status is not known to equal `S` has `f_j >= 50`: the nearest is then
    ///   some candidate with status `S`, and any candidate `kind_from_nearest` weighs against it
    ///   (best[1] when `similarity12 > 0`, best[2] when `similarity13 > 0` or `similarity23 > 0`)
    ///   is within 50 of the nearest, so also has status `S`. `pressure` of two equal statuses
    ///   returns 0.0 before any noise, each `density + similarity * ... * 0.0` is
    ///   `density + 0.0`, never `> 0` for `density <= 0`, and the result is `S.at_kind(y)`.
    /// The skipped calls only fill pure memos (`location`, `status`), so skipping them cannot
    /// change any other answer.
    #[allow(clippy::too_many_arguments)]
    fn note_run(
        &mut self,
        slot: usize,
        x: i32,
        y: i32,
        z: i32,
        anchor_x: i32,
        distances: &[i32; 12],
        best: &[(i32, usize); 3],
    ) {
        // The last x that maps to this anchor: `grid_x(x' + SAMPLE_OFFSET_X) == anchor_x`.
        let segment_end = from_grid_x(anchor_x, 15) - SAMPLE_OFFSET_X;
        let room = segment_end - x;
        if room <= 0 {
            return;
        }
        let candidates = &self.anchors[slot];
        let Some(p) = candidates.index.iter().position(|&i| i == best[0].1) else {
            return;
        };
        let Some(Some(status_p)) = self.statuses.get(best[0].1).copied() else {
            return;
        };
        let cx_p = candidates.centre[p].0;
        let d_p = distances[p];
        // Largest t each rule allows; `None` = the rule does not hold at t = 0.
        let mut t1 = Some(room);
        let mut t2 = Some(room);
        for j in 0..12 {
            if j == p {
                continue;
            }
            let f = distances[j] - d_p;
            let a = candidates.centre[j].0 - cx_p;
            let limit = |threshold: i32| -> Option<i32> {
                if f < threshold {
                    None
                } else if a <= 0 {
                    Some(room)
                } else {
                    Some((f - threshold) / (2 * a))
                }
            };
            t1 = match (t1, limit(25)) {
                (Some(t), Some(l)) => Some(t.min(l)),
                _ => None,
            };
            let same = self.statuses.get(candidates.index[j]).copied() == Some(Some(status_p));
            if !same {
                t2 = match (t2, limit(50)) {
                    (Some(t), Some(l)) => Some(t.min(l)),
                    _ => None,
                };
            }
            if t1.is_none() && t2.is_none() {
                return;
            }
        }
        let t = t1.unwrap_or(0).max(t2.unwrap_or(0));
        if t > 0 {
            self.run_y = y;
            self.run_z = z;
            self.run_x_lo = x + 1;
            self.run_x_hi = x + t;
            self.run_kind = status_p.at_kind(y);
        }
    }

    /// The `anchors` slot for this anchor, when all twelve candidate cells around it lie
    /// inside the grid -- which makes every index in range and distinct, so the per-anchor
    /// list is exactly what the per-block loop in `nearest_slow` would see. `None` otherwise.
    fn anchor_slot(&self, anchor_x: i32, anchor_y: i32, anchor_z: i32) -> Option<usize> {
        let rx = anchor_x - self.min_grid_x;
        let ry = anchor_y - self.min_grid_y;
        let rz = anchor_z - self.min_grid_z;
        let inside = rx >= 0
            && rx + 1 < self.grid_size_x
            && ry >= 1
            && ry + 1 < self.grid_size_y
            && rz >= 0
            && rz + 1 < self.grid_size_z;
        inside.then(|| (rz * self.grid_size_x + rx) as usize)
    }

    /// Draws (or recalls) the twelve candidate centres around an anchor, in the `(dx, dy, dz)`
    /// order `nearest_slow` visits them, calling `location` for each exactly as it does.
    fn fill_anchor(&mut self, slot: usize, anchor_x: i32, anchor_y: i32, anchor_z: i32) {
        let mut entry = AnchorCandidates {
            anchor_y,
            index: [0; 12],
            centre: [(0, 0, 0); 12],
        };
        let mut i = 0;
        for dx in 0..=1 {
            for dy in -1..=1 {
                for dz in 0..=1 {
                    let gx = anchor_x + dx;
                    let gy = anchor_y + dy;
                    let gz = anchor_z + dz;
                    let index = self.index(gx, gy, gz);
                    entry.index[i] = index;
                    entry.centre[i] = self.location(index, gx, gy, gz);
                    i += 1;
                }
            }
        }
        self.anchors[slot] = entry;
    }

    /// The candidate loop for an anchor whose neighbourhood leaves the grid: per block, with
    /// the out-of-range handling of `location` and `status`.
    #[allow(clippy::too_many_arguments)]
    fn nearest_slow(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        anchor_x: i32,
        anchor_y: i32,
        anchor_z: i32,
        best: &mut [(i32, usize); 3],
    ) {
        for dx in 0..=1 {
            for dy in -1..=1 {
                for dz in 0..=1 {
                    let gx = anchor_x + dx;
                    let gy = anchor_y + dy;
                    let gz = anchor_z + dz;
                    let index = self.index(gx, gy, gz);
                    let (cx, cy, cz) = self.location(index, gx, gy, gz);
                    let ddx = cx - x;
                    let ddy = cy - y;
                    let ddz = cz - z;
                    let distance = ddx * ddx + ddy * ddy + ddz * ddz;
                    if best[0].0 >= distance {
                        best[2] = best[1];
                        best[1] = best[0];
                        best[0] = (distance, index);
                    } else if best[1].0 >= distance {
                        best[2] = best[1];
                        best[1] = (distance, index);
                    } else if best[2].0 >= distance {
                        best[2] = (distance, index);
                    }
                }
            }
        }
    }

    /// The rest of `compute_kind`, once the three nearest centres are known.
    fn kind_from_nearest(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        density: f64,
        best: &[(i32, usize); 3],
    ) -> Option<FluidKind> {
        let status1 = self.status(best[0].1);
        let similarity12 = similarity(best[0].0, best[1].0);
        let fluid = status1.at_kind(y);
        if similarity12 <= 0.0 {
            return Some(fluid);
        }
        if fluid == FluidKind::Sea && self.global_fluid(y - 1).at_kind(y - 1) == FluidKind::Lava {
            return Some(fluid);
        }

        // Where two aquifers of different levels meet, barrier pressure can make the boundary
        // solid instead -- that is what stops them merging into one body.
        let mut barrier_noise: Option<f64> = None;
        let status2 = self.status(best[1].1);
        let pressure12 =
            similarity12 * self.pressure(x, y, z, &mut barrier_noise, &status1, &status2);
        if density + pressure12 > 0.0 {
            return None;
        }

        let status3 = self.status(best[2].1);
        let similarity13 = similarity(best[0].0, best[2].0);
        if similarity13 > 0.0 {
            let pressure13 = similarity12
                * similarity13
                * self.pressure(x, y, z, &mut barrier_noise, &status1, &status3);
            if density + pressure13 > 0.0 {
                return None;
            }
        }
        let similarity23 = similarity(best[1].0, best[2].0);
        if similarity23 > 0.0 {
            let pressure23 = similarity12
                * similarity23
                * self.pressure(x, y, z, &mut barrier_noise, &status2, &status3);
            if density + pressure23 > 0.0 {
                return None;
            }
        }
        Some(fluid)
    }

    fn index(&self, gx: i32, gy: i32, gz: i32) -> usize {
        let x = gx - self.min_grid_x;
        let y = gy - self.min_grid_y;
        let z = gz - self.min_grid_z;
        ((y * self.grid_size_z + z) * self.grid_size_x + x) as usize
    }

    /// The jittered centre of one grid cell, drawn once and remembered.
    fn location(&mut self, index: usize, gx: i32, gy: i32, gz: i32) -> (i32, i32, i32) {
        if let Some(Some(location)) = self.locations.get(index) {
            return *location;
        }
        let mut random = self.random.at(gx, gy, gz);
        let location = (
            from_grid_x(gx, random.next_int_bounded(X_RANGE)),
            from_grid_y(gy, random.next_int_bounded(Y_RANGE)),
            from_grid_z(gz, random.next_int_bounded(Z_RANGE)),
        );
        if let Some(slot) = self.locations.get_mut(index) {
            *slot = Some(location);
        }
        location
    }

    fn status(&mut self, index: usize) -> FluidStatus {
        if let Some(Some(status)) = self.statuses.get(index) {
            return *status;
        }
        let Some(Some(location)) = self.locations.get(index).copied() else {
            return self.global_fluid(0);
        };
        let status = self.compute_fluid(location.0, location.1, location.2);
        if let Some(slot) = self.statuses.get_mut(index) {
            *slot = Some(status);
        }
        status
    }

    /// `NoiseBasedAquifer#computeFluid`: the fluid this aquifer centre holds.
    fn compute_fluid(&mut self, x: i32, y: i32, z: i32) -> FluidStatus {
        let global = self.global_fluid(y);
        let mut lowest_surface = i32::MAX;
        let top_of_cell = y + Y_SPACING;
        let bottom_of_cell = y - Y_SPACING;
        let mut surface_under_global_fluid = false;

        for offset in SURFACE_SAMPLING_OFFSETS_IN_CHUNKS {
            let sample_x = x + offset[0] * 16;
            let sample_z = z + offset[1] * 16;
            let surface = self.preliminary_surface_level(sample_x, sample_z);
            let adjusted = adjust_surface_level(surface);
            let start = offset[0] == 0 && offset[1] == 0;

            if start && bottom_of_cell > adjusted {
                return global;
            }
            let pokes_above = top_of_cell > adjusted;
            if pokes_above || start {
                let at_surface = self.global_fluid(adjusted);
                if at_surface.at_kind(adjusted) != FluidKind::Air {
                    if start {
                        surface_under_global_fluid = true;
                    }
                    if pokes_above {
                        return at_surface;
                    }
                }
            }
            lowest_surface = lowest_surface.min(surface);
        }

        let level =
            self.surface_level(x, y, z, &global, lowest_surface, surface_under_global_fluid);
        let fluid = self.fluid_type(x, y, z, &global, level);
        FluidStatus { level, fluid }
    }

    /// `computeSurfaceLevel`: how high this aquifer's fluid stands, if it holds any.
    fn surface_level(
        &self,
        x: i32,
        y: i32,
        z: i32,
        global: &FluidStatus,
        lowest_surface: i32,
        surface_under_global_fluid: bool,
    ) -> i32 {
        let erosion = self.router.sample(RouterSlot::Erosion, x, y, z);
        let depth = self.router.sample(RouterSlot::Depth, x, y, z);
        // Deep dark stays dry: that is what makes ancient cities generate without water.
        let deep_dark = erosion < -0.225 && depth > 0.9;

        let (partially, fully) = if deep_dark {
            (-1.0, -1.0)
        } else {
            let distance_below_surface = lowest_surface + 8 - y;
            let floodedness_factor = if surface_under_global_fluid {
                clamped_map(distance_below_surface as f64, 0.0, 64.0, 1.0, 0.0)
            } else {
                0.0
            };
            let noise = self
                .router
                .sample(RouterSlot::FluidLevelFloodedness, x, y, z)
                .clamp(-1.0, 1.0);
            let fully_threshold = map(floodedness_factor, 1.0, 0.0, -0.3, 0.8);
            let partially_threshold = map(floodedness_factor, 1.0, 0.0, -0.8, 0.4);
            (noise - partially_threshold, noise - fully_threshold)
        };

        if fully > 0.0 {
            global.level
        } else if partially > 0.0 {
            self.randomized_surface_level(x, y, z, lowest_surface)
        } else {
            WAY_BELOW_MIN_Y
        }
    }

    /// `computeRandomizedFluidSurfaceLevel`: a partially flooded aquifer's level, quantised so
    /// neighbouring cells share levels and produce flat surfaces rather than staircases.
    fn randomized_surface_level(&self, x: i32, y: i32, z: i32, lowest_surface: i32) -> i32 {
        let cell_x = x.div_euclid(16);
        let cell_y = y.div_euclid(40);
        let cell_z = z.div_euclid(16);
        let middle_y = cell_y * 40 + 20;
        let spread = self
            .router
            .sample(RouterSlot::FluidLevelSpread, cell_x, cell_y, cell_z)
            * 10.0;
        let quantized = quantize(spread, 3);
        lowest_surface.min(middle_y + quantized)
    }

    /// `computeFluidType`: deep aquifers can be lava instead of water.
    fn fluid_type(
        &self,
        x: i32,
        y: i32,
        z: i32,
        global: &FluidStatus,
        surface_level: i32,
    ) -> FluidKind {
        if surface_level <= -10
            && surface_level != WAY_BELOW_MIN_Y
            && global.fluid != FluidKind::Lava
        {
            let cell_x = x.div_euclid(64);
            let cell_y = y.div_euclid(40);
            let cell_z = z.div_euclid(64);
            let lava_noise = self.router.sample(RouterSlot::Lava, cell_x, cell_y, cell_z);
            if lava_noise.abs() > 0.3 {
                return FluidKind::Lava;
            }
        }
        global.fluid
    }

    /// `calculatePressure`: how strongly the boundary between two aquifers resists being fluid.
    fn pressure(
        &self,
        x: i32,
        y: i32,
        z: i32,
        barrier_noise: &mut Option<f64>,
        first: &FluidStatus,
        second: &FluidStatus,
    ) -> f64 {
        let type1 = first.at_kind(y);
        let type2 = second.at_kind(y);
        // Water meeting lava is always a barrier -- that is why they never touch underground.
        if (type1 == FluidKind::Lava && type2 == FluidKind::Sea)
            || (type1 == FluidKind::Sea && type2 == FluidKind::Lava)
        {
            return 2.0;
        }
        let level_difference = (first.level - second.level).abs();
        if level_difference == 0 {
            return 0.0;
        }

        let average_level = 0.5 * (first.level + second.level) as f64;
        let above_average = y as f64 + 0.5 - average_level;
        let half_difference = level_difference as f64 / 2.0;
        let toward_middle = half_difference - above_average.abs();
        let gradient = if above_average > 0.0 {
            let center = toward_middle;
            if center > 0.0 {
                center / 1.5
            } else {
                center / 2.5
            }
        } else {
            let center = 3.0 + toward_middle;
            if center > 0.0 {
                center / 3.0
            } else {
                center / 10.0
            }
        };

        // Outside +/-2 the gradient decides on its own, so the noise is never sampled -- which
        // is why vanilla threads the sampled value through as a lazily-filled holder.
        let noise = if !(-2.0..=2.0).contains(&gradient) {
            0.0
        } else {
            match barrier_noise {
                Some(value) => *value,
                None => {
                    let value = self.router.sample(RouterSlot::Barrier, x, y, z);
                    *barrier_noise = Some(value);
                    value
                }
            }
        };
        2.0 * (noise + gradient)
    }

    /// `NoiseChunk#preliminarySurfaceLevel`.
    ///
    /// Quantised to the quart column before both the lookup *and* the evaluation, because that
    /// is what vanilla does: `QuartPos.toBlock(QuartPos.fromBlock(x))` is `x & !3`. Sampling the
    /// exact column instead gives a different surface level for three columns in every four, and
    /// therefore different aquifer levels and different terrain -- this is a parity requirement
    /// that also happens to cut the distinct keys by sixteen.
    ///
    /// Memoised for the same reason vanilla keeps a `Long2IntMap`: `compute_fluid` samples
    /// thirteen chunk offsets per aquifer centre and adjacent centres overlap heavily.
    fn preliminary_surface_level(&mut self, x: i32, z: i32) -> i32 {
        let quart_x = x & !3;
        let quart_z = z & !3;
        // Every position this chunk's aquifer centres ask about falls in a fixed window around
        // the chunk, so the memo is a dense quart grid. Anything outside it (not reachable from
        // `compute_fluid`) is simply computed.
        let slot_x = (quart_x - self.surface_origin_x) >> 2;
        let slot_z = (quart_z - self.surface_origin_z) >> 2;
        let slot = ((0..SURFACE_MEMO_SPAN).contains(&slot_x)
            && (0..SURFACE_MEMO_SPAN).contains(&slot_z))
        .then(|| (slot_z * SURFACE_MEMO_SPAN + slot_x) as usize);
        if let Some(slot) = slot {
            let level = self.surface_levels[slot];
            if level != i32::MIN {
                return level;
            }
        }
        // Then this thread's memo across chunks, for the same router (see `SurfaceMemo`). Its
        // values came from this same function on an earlier chunk, through whichever caches
        // that chunk used; by the argument below they are the same bits as computing here.
        let (page, index) = self.shared_surface.slot(quart_x, quart_z);
        let shared = self.shared_surface.pages[page].levels[index];
        if shared != i32::MIN {
            if let Some(slot) = slot {
                self.surface_levels[slot] = shared;
            }
            return shared;
        }
        // A column of this chunk goes through the chunk's own caches, as it always has. A
        // column of another chunk in the window goes through a private `ChunkCaches` for that
        // chunk, so its `cache_2d` nodes hold `offset` and `factor` across the whole
        // `find_top_surface` scan; through this chunk's caches they miss and are re-evaluated,
        // noise and splines included, at every 8-block step of the scan.
        //
        // Same value either way, for the vanilla `preliminary_surface_level`: its program has
        // no `interpolated` node (the only kind that clamps to the chunk's cell grid); a
        // `cache_2d`/`flat_cache` node, hit or miss, yields its argument at the same (x, z) --
        // the miss path through this chunk's caches runs it at the scan's y, a hit returns it
        // as run at the first y that column saw; and every cached argument (`offset`, `factor`,
        // and the continents/erosion/ridges/shift chains under them) is a bit-exact function
        // of (x, z) alone: `shifted_noise` there has `y_scale` 0 and a constant 0 `shift_y`
        // (`y * 0.0 + 0.0` is +0.0 for every y), `shift_a`/`shift_b` never read y, and
        // `blend_alpha`/`blend_offset` compile to constants. `cache_once` remembers exact
        // positions only. The y-dependent `y_clamped_gradient` terms sit outside every cache.
        let chunk_x = quart_x >> 4;
        let chunk_z = quart_z >> 4;
        let caches: &ChunkCaches = match slot {
            Some(_) if chunk_x != self.own_chunk_x || chunk_z != self.own_chunk_z => {
                let neighbour = ((chunk_z - self.surface_origin_chunk_z) * SURFACE_MEMO_CHUNKS
                    + (chunk_x - self.surface_origin_chunk_x))
                    as usize;
                let router = self.router;
                self.neighbour_caches[neighbour]
                    .get_or_insert_with(|| Box::new(router.chunk_caches(chunk_x, chunk_z)))
            }
            _ => self.caches,
        };
        let level = self
            .router
            .sample_in_chunk(
                caches,
                RouterSlot::PreliminarySurfaceLevel,
                quart_x,
                0,
                quart_z,
            )
            .floor() as i32;
        if let Some(slot) = slot {
            self.surface_levels[slot] = level;
        }
        // `slot` above cannot have been evicted since: nothing between touches the memo.
        self.shared_surface.pages[page].levels[index] = level;
        level
    }
}

fn adjust_surface_level(preliminary: i32) -> i32 {
    preliminary + 8
}

fn similarity(distance1: i32, distance2: i32) -> f64 {
    1.0 - (distance2 - distance1) as f64 / 25.0
}

fn grid_x(block: i32) -> i32 {
    block >> 4
}

fn from_grid_x(grid: i32, offset: i32) -> i32 {
    (grid << 4) + offset
}

fn grid_y(block: i32) -> i32 {
    block.div_euclid(Y_SPACING)
}

fn from_grid_y(grid: i32, offset: i32) -> i32 {
    grid * Y_SPACING + offset
}

fn grid_z(block: i32) -> i32 {
    block >> 4
}

fn from_grid_z(grid: i32, offset: i32) -> i32 {
    (grid << 4) + offset
}

/// Vanilla `Mth.quantize`.
fn quantize(value: f64, resolution: i32) -> i32 {
    (value / resolution as f64).floor() as i32 * resolution
}

/// Vanilla `Mth.clampedMap`.
fn clamped_map(value: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    let factor = (value - from_min) / (from_max - from_min);
    if factor < 0.0 {
        to_min
    } else if factor > 1.0 {
        to_max
    } else {
        to_min + factor * (to_max - to_min)
    }
}

/// Vanilla `Mth.map`, unclamped.
fn map(value: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    to_min + (value - from_min) * (to_max - to_min) / (from_max - from_min)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_datapack::Registry;

    fn settings(surface: f64) -> NoiseGeneratorSettings {
        let json = serde_json::json!({
            "default_block": {"Name": "minecraft:stone"},
            "default_fluid": {"Name": "minecraft:water"},
            "sea_level": 63,
            "disable_mob_generation": false,
            "aquifers_enabled": true,
            "ore_veins_enabled": false,
            "legacy_random_source": false,
            "noise": {"min_y": -64, "height": 384, "size_horizontal": 1, "size_vertical": 2},
            "noise_router": {
                "temperature": 0.0,
                "vegetation": 0.0,
                "continents": 0.0,
                "erosion": 0.0,
                "depth": 0.0,
                "ridges": 0.0,
                "preliminary_surface_level": surface,
                "final_density": 0.0
            },
            "surface_rule": {"type": "minecraft:block", "result_state": {"Name": "minecraft:stone"}},
            "spawn_target": []
        });
        serde_json::from_value(json).expect("valid fixture")
    }

    /// Two routers used one after the other on one thread, the second very likely built where
    /// the first was dropped: the second must never see the first's memoised surface levels.
    #[test]
    fn surface_memo_never_crosses_routers() {
        let df = Registry::default();
        let noises = Registry::default();
        for (round, surface) in [10.0, 50.0, 10.0, -20.0].into_iter().enumerate() {
            let settings = settings(surface);
            let router = NoiseRouterEvaluator::new(7, &settings, &df, &noises);
            let caches = router.chunk_caches(0, 0);
            for chunk in [(0, 0), (1, 0), (0, 1)] {
                let mut aquifer = Aquifer::new(chunk.0, chunk.1, &settings, &router, &caches);
                for (x, z) in [(0, 0), (-40, 8), (20, -60), (300, 300)] {
                    assert_eq!(
                        aquifer.preliminary_surface_level(x, z),
                        surface as i32,
                        "round {round}, chunk {chunk:?}, column ({x}, {z})"
                    );
                }
            }
        }
    }
}
