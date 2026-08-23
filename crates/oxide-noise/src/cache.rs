//! Per-chunk caches for the density-function evaluator: what makes `interpolated`,
//! `flat_cache`, `cache_2d` and `cache_once` mean something instead of evaluating straight
//! through.
//!
//! This is not only a speed concern. Vanilla's terrain *is* the trilinearly interpolated value
//! between cell corners -- evaluating the tree exactly at every block computes a different,
//! more "precise" surface than the game has, so a generator that skips interpolation can never
//! match Java no matter what else it gets right. Honouring these nodes is a parity requirement
//! that happens to also remove ~80x of the work.
//!
//! Scoped to one chunk and one thread: the caches are keyed by node identity (the address of
//! the node inside the datapack registry, which is immutable and outlives generation) and held
//! behind `RefCell`, so a `ChunkCaches` must not be shared across threads. Chunks in different
//! Folia regions each build their own.

use std::cell::RefCell;

use oxide_datapack::DensityFunction;

use crate::density::{evaluate, EvalCtx, FunctionContext};

/// A remembered sample: the position it was taken at, and the value there.
type LastSample = ((i32, i32, i32), f64);

/// Columns cached per node: the chunk's 16x16 footprint plus one, because interpolation samples
/// cell corners one step past the chunk's far edge.
const COLUMN_SPAN: usize = 17;
const COLUMNS: usize = COLUMN_SPAN * COLUMN_SPAN;

/// Identity of a node in the density-function tree. The datapack registry owns every node for
/// the generator's lifetime and never mutates it, so its address is a stable key.
fn node_id(df: &DensityFunction) -> usize {
    df as *const DensityFunction as usize
}

pub struct ChunkCaches {
    /// Horizontal cell size in blocks (`size_horizontal * 4` in vanilla's noise settings).
    cell_width: i32,
    /// `log2(cell_width)` when the width is a power of two, which every vanilla noise setting
    /// uses. `x >> k` is exactly `x.div_euclid(1 << k)` for a positive divisor, including for
    /// negative `x`, so this is a substitution and not an approximation; `None` falls back to
    /// the division.
    cell_width_log2: Option<u32>,
    /// Vertical cell size in blocks (`size_vertical * 4`).
    cell_height: i32,
    /// `log2(cell_height)`, as [`Self::cell_width_log2`].
    cell_height_log2: Option<u32>,
    /// `1.0 / cell_width` and `1.0 / cell_height`, so the per-block interpolation fraction is a
    /// multiply rather than a divide.
    inv_cell_width: f64,
    inv_cell_height: f64,
    min_y: i32,
    /// Corner counts, derived from the chunk's 16x16 footprint and the world height.
    cells_x: usize,
    cells_y: usize,
    cells_z: usize,
    origin_x: i32,
    origin_z: i32,

    // All three are association lists keyed by node address, scanned linearly. A chunk's tree
    // holds only a handful of each kind, so a scan of two or three pointer comparisons beats
    // hashing -- and hashing is what these caches were spending their savings on: several
    // HashMap lookups per block, ~98k blocks per chunk, is millions of hashes to avoid
    // arithmetic that interpolation had already made cheap.
    /// node -> cell-corner grid, built on first use within this chunk. Keys live in their own
    /// packed `Vec` so the probe walks a contiguous run of `usize` instead of striding over a
    /// 32-byte tuple per candidate; the grids never move once pushed, so a slot index found in
    /// `interpolated_keys` indexes `interpolated_grids` directly.
    interpolated_keys: RefCell<Vec<usize>>,
    interpolated_grids: RefCell<Vec<Vec<f64>>>,
    /// node -> one value per column of this chunk, for the 2D subtrees (continents, erosion,
    /// factor, offset) that would otherwise be recomputed once per block of height.
    two_d: RefCell<Vec<(usize, Vec<Option<f64>>)>>,
    /// node -> (last position, value).
    once: RefCell<Vec<(usize, LastSample)>>,
}

impl ChunkCaches {
    /// `chunk_min_x`/`chunk_min_z` are the chunk's lowest block coordinates.
    pub fn new(
        chunk_min_x: i32,
        chunk_min_z: i32,
        min_y: i32,
        height: i32,
        size_horizontal: i32,
        size_vertical: i32,
    ) -> Self {
        let cell_width = (size_horizontal * 4).max(1);
        let cell_height = (size_vertical * 4).max(1);
        Self {
            cell_width,
            cell_width_log2: log2_exact(cell_width),
            cell_height,
            cell_height_log2: log2_exact(cell_height),
            inv_cell_width: 1.0 / cell_width as f64,
            inv_cell_height: 1.0 / cell_height as f64,
            min_y,
            cells_x: (16 / cell_width) as usize,
            cells_y: (height / cell_height) as usize,
            cells_z: (16 / cell_width) as usize,
            origin_x: chunk_min_x,
            origin_z: chunk_min_z,
            interpolated_keys: RefCell::new(Vec::new()),
            interpolated_grids: RefCell::new(Vec::new()),
            two_d: RefCell::new(Vec::new()),
            once: RefCell::new(Vec::new()),
        }
    }

    /// Value of an `interpolated` node: the tree is evaluated only at this chunk's cell corners
    /// -- 5x5x49 samples for a standard 384-tall world, against 98304 blocks -- and every block
    /// inside a cell is a trilinear blend of the eight corners around it.
    pub fn interpolated(
        &self,
        node: &DensityFunction,
        ctx: FunctionContext,
        eval_cx: &EvalCtx,
    ) -> f64 {
        let id = node_id(node);
        // Hit path: one borrow, one probe over the packed key run, eight loads out of the grid
        // it names. The previous shape scanned the association list twice (once to test for
        // presence, once to read) and took two `RefCell` borrows to do it, three times per
        // block -- `final_density` reaches five `interpolated` nodes for every block filled.
        if let Some(slot) = self.slot_of(id) {
            return self.blend(slot, ctx);
        }
        // Miss: build outside any borrow, since evaluating a corner recurses back through the
        // evaluator and may consult this same cache. `push` only appends, so a slot handed out
        // by a nested build stays valid.
        let grid = self.build_corner_grid(node, eval_cx);
        let slot = {
            let mut keys = self.interpolated_keys.borrow_mut();
            let mut grids = self.interpolated_grids.borrow_mut();
            keys.push(id);
            grids.push(grid);
            keys.len() - 1
        };
        self.blend(slot, ctx)
    }

    fn slot_of(&self, id: usize) -> Option<usize> {
        self.interpolated_keys
            .borrow()
            .iter()
            .position(|key| *key == id)
    }

    /// The trilinear blend of the eight corners around `ctx` in the grid at `slot`.
    fn blend(&self, slot: usize, ctx: FunctionContext) -> f64 {
        let local_x = ctx.x - self.origin_x;
        let local_z = ctx.z - self.origin_z;
        let rel_y = ctx.y - self.min_y;

        let cx_index = div_cell(local_x, self.cell_width, self.cell_width_log2)
            .clamp(0, self.cells_x as i32 - 1);
        let cz_index = div_cell(local_z, self.cell_width, self.cell_width_log2)
            .clamp(0, self.cells_z as i32 - 1);
        let cy_index = div_cell(rel_y, self.cell_height, self.cell_height_log2)
            .clamp(0, self.cells_y as i32 - 1);

        let dx = (local_x - cx_index * self.cell_width) as f64 * self.inv_cell_width;
        let dz = (local_z - cz_index * self.cell_width) as f64 * self.inv_cell_width;
        let dy = (rel_y - cy_index * self.cell_height) as f64 * self.inv_cell_height;

        let (cx_index, cy_index, cz_index) =
            (cx_index as usize, cy_index as usize, cz_index as usize);

        let grids = self.interpolated_grids.borrow();
        let grid = &grids[slot];
        // The eight corners of one cell are two adjacent x pairs on each of four (y, z) rows,
        // so the row stride is all the indexing this needs.
        let x_stride = 1usize;
        let z_stride = self.cells_x + 1;
        let y_stride = z_stride * (self.cells_z + 1);
        let base = cy_index * y_stride + cz_index * z_stride + cx_index;
        let v000 = grid[base];
        let v100 = grid[base + x_stride];
        let v010 = grid[base + y_stride];
        let v110 = grid[base + y_stride + x_stride];
        let v001 = grid[base + z_stride];
        let v101 = grid[base + z_stride + x_stride];
        let v011 = grid[base + y_stride + z_stride];
        let v111 = grid[base + y_stride + z_stride + x_stride];

        // PARITY-CHECK: interpolation order follows vanilla's `Mth.lerp3` -- blend along x,
        // then y, then z. Any order gives nearly the same number, but not bit-identically.
        lerp3(dx, dy, dz, v000, v100, v010, v110, v001, v101, v011, v111)
    }

    fn build_corner_grid(&self, node: &DensityFunction, eval_cx: &EvalCtx) -> Vec<f64> {
        let mut grid =
            Vec::with_capacity((self.cells_x + 1) * (self.cells_y + 1) * (self.cells_z + 1));
        for cy in 0..=self.cells_y {
            let y = self.min_y + cy as i32 * self.cell_height;
            for cz in 0..=self.cells_z {
                let z = self.origin_z + cz as i32 * self.cell_width;
                for cx in 0..=self.cells_x {
                    let x = self.origin_x + cx as i32 * self.cell_width;
                    grid.push(evaluate(node, FunctionContext { x, y, z }, eval_cx));
                }
            }
        }
        grid
    }

    /// `cache_2d`: the wrapped subtree does not depend on y, so one value per column serves
    /// every block above and below it.
    pub fn cache_2d(&self, node: &DensityFunction, ctx: FunctionContext, eval_cx: &EvalCtx) -> f64 {
        self.column_cached(node, ctx.x, ctx.z, ctx, eval_cx)
    }

    /// `flat_cache`: like `cache_2d`, but vanilla samples once per quart cell and reuses that
    /// value across the 4x4 block area, so the sample position is snapped down to the quart
    /// origin rather than taken at the caller's exact x/z.
    pub fn flat_cache(
        &self,
        node: &DensityFunction,
        ctx: FunctionContext,
        eval_cx: &EvalCtx,
    ) -> f64 {
        let quart_x = ctx.x >> 2 << 2;
        let quart_z = ctx.z >> 2 << 2;
        self.column_cached(node, quart_x, quart_z, ctx, eval_cx)
    }

    /// One value per (x, z) within this chunk's span, stored densely. A position outside the
    /// span -- which the corner grid never asks for, but a caller could -- is evaluated
    /// uncached rather than growing the storage.
    fn column_cached(
        &self,
        node: &DensityFunction,
        sample_x: i32,
        sample_z: i32,
        ctx: FunctionContext,
        eval_cx: &EvalCtx,
    ) -> f64 {
        let sample_ctx = FunctionContext {
            x: sample_x,
            y: ctx.y,
            z: sample_z,
        };
        let Some(slot) = self.column_slot(sample_x, sample_z) else {
            return evaluate(node, sample_ctx, eval_cx);
        };
        let id = node_id(node);

        if let Some((_, columns)) = self.two_d.borrow().iter().find(|(key, _)| *key == id) {
            if let Some(value) = columns[slot] {
                return value;
            }
        }
        // Not held across the recursive evaluate below, which may touch this same cache.
        let value = evaluate(node, sample_ctx, eval_cx);

        let mut cache = self.two_d.borrow_mut();
        match cache.iter_mut().find(|(key, _)| *key == id) {
            Some((_, columns)) => columns[slot] = Some(value),
            None => {
                let mut columns = vec![None; COLUMNS];
                columns[slot] = Some(value);
                cache.push((id, columns));
            }
        }
        value
    }

    fn column_slot(&self, x: i32, z: i32) -> Option<usize> {
        let local_x = x - self.origin_x;
        let local_z = z - self.origin_z;
        if !(0..COLUMN_SPAN as i32).contains(&local_x)
            || !(0..COLUMN_SPAN as i32).contains(&local_z)
        {
            return None;
        }
        Some(local_x as usize * COLUMN_SPAN + local_z as usize)
    }

    /// `cache_once` / `cache_all_in_cell`: remember only the last position, which is all the
    /// repeated-lookup pattern inside one column needs.
    pub fn once(&self, node: &DensityFunction, ctx: FunctionContext, eval_cx: &EvalCtx) -> f64 {
        let id = node_id(node);
        let position = (ctx.x, ctx.y, ctx.z);
        if let Some((_, (cached_position, value))) =
            self.once.borrow().iter().find(|(key, _)| *key == id)
        {
            if *cached_position == position {
                return *value;
            }
        }
        let value = evaluate(node, ctx, eval_cx);

        let mut cache = self.once.borrow_mut();
        match cache.iter_mut().find(|(key, _)| *key == id) {
            Some((_, slot)) => *slot = (position, value),
            None => cache.push((id, (position, value))),
        }
        value
    }
}

/// `log2(n)` when `n` is a positive power of two.
fn log2_exact(n: i32) -> Option<u32> {
    (n > 0 && (n & (n - 1)) == 0).then(|| n.trailing_zeros())
}

/// `value.div_euclid(cell)`, as a shift when `cell` is a power of two. Arithmetic right shift
/// is floor division, which is what `div_euclid` is for a positive divisor.
#[inline]
fn div_cell(value: i32, cell: i32, log2: Option<u32>) -> i32 {
    match log2 {
        Some(k) => value >> k,
        None => value.div_euclid(cell),
    }
}

/// Vanilla `Mth.lerp3`.
#[allow(clippy::too_many_arguments)]
fn lerp3(
    dx: f64,
    dy: f64,
    dz: f64,
    v000: f64,
    v100: f64,
    v010: f64,
    v110: f64,
    v001: f64,
    v101: f64,
    v011: f64,
    v111: f64,
) -> f64 {
    let z0 = lerp2(dx, dy, v000, v100, v010, v110);
    let z1 = lerp2(dx, dy, v001, v101, v011, v111);
    lerp(dz, z0, z1)
}

fn lerp2(dx: f64, dy: f64, v00: f64, v10: f64, v01: f64, v11: f64) -> f64 {
    lerp(dy, lerp(dx, v00, v10), lerp(dx, v01, v11))
}

fn lerp(delta: f64, from: f64, to: f64) -> f64 {
    from + delta * (to - from)
}
