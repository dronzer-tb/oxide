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
    /// Vertical cell size in blocks (`size_vertical * 4`).
    cell_height: i32,
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
    /// node -> cell-corner grid, built on first use within this chunk.
    interpolated: RefCell<Vec<(usize, Vec<f64>)>>,
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
            cell_height,
            min_y,
            cells_x: (16 / cell_width) as usize,
            cells_y: (height / cell_height) as usize,
            cells_z: (16 / cell_width) as usize,
            origin_x: chunk_min_x,
            origin_z: chunk_min_z,
            interpolated: RefCell::new(Vec::new()),
            two_d: RefCell::new(Vec::new()),
            once: RefCell::new(Vec::new()),
        }
    }

    fn corner_index(&self, cx: usize, cy: usize, cz: usize) -> usize {
        (cy * (self.cells_z + 1) + cz) * (self.cells_x + 1) + cx
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
        if !self.interpolated.borrow().iter().any(|(key, _)| *key == id) {
            // Built into a local first: evaluating a corner recurses back through the
            // evaluator, which may consult this same cache, so no borrow may be held here.
            let grid = self.build_corner_grid(node, eval_cx);
            self.interpolated.borrow_mut().push((id, grid));
        }
        let grids = self.interpolated.borrow();
        let grid = &grids
            .iter()
            .find(|(key, _)| *key == id)
            .expect("just inserted")
            .1;

        let local_x = ctx.x - self.origin_x;
        let local_z = ctx.z - self.origin_z;
        let rel_y = ctx.y - self.min_y;

        let cx_index = (local_x.div_euclid(self.cell_width)).clamp(0, self.cells_x as i32 - 1);
        let cz_index = (local_z.div_euclid(self.cell_width)).clamp(0, self.cells_z as i32 - 1);
        let cy_index = (rel_y.div_euclid(self.cell_height)).clamp(0, self.cells_y as i32 - 1);

        let dx = (local_x - cx_index * self.cell_width) as f64 / self.cell_width as f64;
        let dz = (local_z - cz_index * self.cell_width) as f64 / self.cell_width as f64;
        let dy = (rel_y - cy_index * self.cell_height) as f64 / self.cell_height as f64;

        let (cx_index, cy_index, cz_index) =
            (cx_index as usize, cy_index as usize, cz_index as usize);
        let corner = |ox: usize, oy: usize, oz: usize| {
            grid[self.corner_index(cx_index + ox, cy_index + oy, cz_index + oz)]
        };

        // PARITY-CHECK: interpolation order follows vanilla's `Mth.lerp3` -- blend along x,
        // then y, then z. Any order gives nearly the same number, but not bit-identically.
        lerp3(
            dx,
            dy,
            dz,
            corner(0, 0, 0),
            corner(1, 0, 0),
            corner(0, 1, 0),
            corner(1, 1, 0),
            corner(0, 0, 1),
            corner(1, 0, 1),
            corner(0, 1, 1),
            corner(1, 1, 1),
        )
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
