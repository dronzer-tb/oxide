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
//! Every cache node is given a dense slot when the router's density functions are compiled
//! (see `compiled.rs`), so a lookup here is an array index rather than a search for a node.
//! Scoped to one chunk and one thread: the storage sits behind `RefCell`, so a `ChunkCaches`
//! must not be shared across threads. Chunks in different Folia regions each build their own.

use std::cell::RefCell;

use crate::compiled::{CacheKind, CacheSlotCounts, Program, RunCtx};
use crate::density::FunctionContext;

/// A remembered sample: the position it was taken at, and the value there.
type LastSample = ((i32, i32, i32), f64);

/// Columns cached per node: the chunk's 16x16 footprint plus one, because interpolation samples
/// cell corners one step past the chunk's far edge.
const COLUMN_SPAN: usize = 17;
const COLUMNS: usize = COLUMN_SPAN * COLUMN_SPAN;

/// Widest cell the whole-cell evaluation keeps its per-row scratch on the stack for. Vanilla
/// cells are 4 or 8 blocks wide.
const MAX_CELL_WIDTH: usize = 16;

/// How many programs can each keep a cell memo: one per router slot.
pub(crate) const MEMO_SLOTS: usize = 16;

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

    /// Slot -> cell-corner grid, built on first use within this chunk.
    interpolated: RefCell<Vec<Option<Vec<f64>>>>,
    /// Slot -> one value per column of this chunk, for the 2D subtrees (continents, erosion,
    /// factor, offset) that would otherwise be recomputed once per block of height.
    two_d: RefCell<Vec<Option<Vec<Option<f64>>>>>,
    /// Slot -> (last position, value).
    once: RefCell<Vec<Option<LastSample>>>,

    /// Blocks per cell: `cell_width * cell_height * cell_width`.
    cell_lanes: usize,
    /// Whether this chunk's geometry fits [`CellMemo`]: cells tile the chunk exactly and are
    /// few and small enough for its fixed-size bookkeeping. True for every vanilla setting.
    memo_ok: bool,
    /// One memo per router slot, so two programs sampled alternately (the ore-vein pass reads
    /// `vein_toggle` and `vein_ridged` at the same block) do not evict each other.
    memos: RefCell<[CellMemo; MEMO_SLOTS]>,
}

/// Per-block values of one cell-batchable program for one horizontal layer of cells.
///
/// A chunk fill asks for `final_density` block by block, y outermost, so at any moment it is
/// working through a single layer of cells. Each cell of that layer is evaluated whole on first
/// touch (see `Program::run_cell`) and the rest of its blocks are then plain loads.
struct CellMemo {
    /// The layer the values belong to; `i32::MIN` when empty.
    cell_y: i32,
    /// Bit `cell_z * cells_x + cell_x` set when that cell's values are filled.
    valid: u32,
    /// `cells_x * cells_z` cells of `cell_lanes` values each; allocated on first use.
    values: Vec<f64>,
}

impl ChunkCaches {
    /// Horizontal cell size in blocks, so a caller can iterate this chunk cell by cell.
    pub fn cell_width(&self) -> i32 {
        self.cell_width
    }

    /// Vertical cell size in blocks.
    pub fn cell_height(&self) -> i32 {
        self.cell_height
    }

    /// The min and max of a cell's eight corner values for one `interpolated` slot, or `None`
    /// if that slot's corner grid has not been built yet (nothing has sampled it this chunk).
    ///
    /// Interpolation inside a cell is `lerp3` of exactly these eight corners, and a trilinear
    /// blend never leaves the range of its inputs. So `min > 0` proves every block in the cell
    /// is solid and `max < 0` proves none of them is, without evaluating a single interior
    /// position -- which is what lets the fill skip ~75% of a chunk's density evaluations.
    ///
    /// `(cell_x, cell_y, cell_z)` are cell indices within this chunk, not block coordinates.
    pub fn cell_bounds(
        &self,
        slot: usize,
        cell_x: i32,
        cell_y: i32,
        cell_z: i32,
    ) -> Option<(f64, f64)> {
        let grids = self.interpolated.borrow();
        let grid = grids.get(slot)?.as_ref()?;

        if cell_x < 0
            || cell_y < 0
            || cell_z < 0
            || cell_x >= self.cells_x as i32
            || cell_y >= self.cells_y as i32
            || cell_z >= self.cells_z as i32
        {
            return None;
        }

        let x_stride = 1usize;
        let z_stride = self.cells_x + 1;
        let y_stride = z_stride * (self.cells_z + 1);
        let base = cell_y as usize * y_stride + cell_z as usize * z_stride + cell_x as usize;

        let corners = [
            grid[base],
            grid[base + x_stride],
            grid[base + z_stride],
            grid[base + z_stride + x_stride],
            grid[base + y_stride],
            grid[base + y_stride + x_stride],
            grid[base + y_stride + z_stride],
            grid[base + y_stride + z_stride + x_stride],
        ];

        let mut lo = corners[0];
        let mut hi = corners[0];
        for c in corners.iter().copied().skip(1) {
            if c < lo {
                lo = c;
            }
            if c > hi {
                hi = c;
            }
        }
        Some((lo, hi))
    }

    /// `chunk_min_x`/`chunk_min_z` are the chunk's lowest block coordinates. `slots` comes from
    /// the compiler, and fixes how many entries of each kind this chunk can be asked for.
    pub fn new(
        chunk_min_x: i32,
        chunk_min_z: i32,
        min_y: i32,
        height: i32,
        size_horizontal: i32,
        size_vertical: i32,
        slots: CacheSlotCounts,
    ) -> Self {
        let cell_width = (size_horizontal * 4).max(1);
        let cell_height = (size_vertical * 4).max(1);
        let cells_x = (16 / cell_width) as usize;
        let cells_y = (height / cell_height) as usize;
        let cell_lanes = (cell_width * cell_height * cell_width) as usize;
        let memo_ok = cell_width as usize <= MAX_CELL_WIDTH
            && cells_x as i32 * cell_width == 16
            && cells_y as i32 * cell_height == height
            && cells_x * cells_x <= 32
            && cell_lanes <= 4096;
        Self {
            cell_width,
            cell_width_log2: log2_exact(cell_width),
            cell_height,
            cell_height_log2: log2_exact(cell_height),
            inv_cell_width: 1.0 / cell_width as f64,
            inv_cell_height: 1.0 / cell_height as f64,
            min_y,
            cells_x,
            cells_y,
            cells_z: cells_x,
            origin_x: chunk_min_x,
            origin_z: chunk_min_z,
            interpolated: RefCell::new((0..slots.interpolated).map(|_| None).collect()),
            two_d: RefCell::new((0..slots.two_d).map(|_| None).collect()),
            once: RefCell::new(vec![None; slots.once]),
            cell_lanes,
            memo_ok,
            memos: RefCell::new(std::array::from_fn(|_| CellMemo {
                cell_y: i32::MIN,
                valid: 0,
                values: Vec::new(),
            })),
        }
    }

    /// Blocks per cell, which is the length [`Self::fill_cell_interpolated`] writes.
    pub(crate) fn cell_lanes(&self) -> usize {
        self.cell_lanes
    }

    /// Whether `(cell_x, cell_y, cell_z)` is a cell of this chunk.
    pub(crate) fn has_cell(&self, cell_x: i32, cell_y: i32, cell_z: i32) -> bool {
        (cell_x as u32) < self.cells_x as u32
            && (cell_y as u32) < self.cells_y as u32
            && (cell_z as u32) < self.cells_z as u32
    }

    /// Whether whole-cell evaluation is available for this chunk's geometry.
    pub(crate) fn cells_tile_chunk(&self) -> bool {
        self.memo_ok
    }

    /// `program`'s value at `ctx`, through the cell memo: the whole cell containing `ctx` is
    /// evaluated on first touch and remembered while the caller stays in that layer of cells.
    /// `None` when the position is outside this chunk or the geometry does not fit, in which
    /// case the caller evaluates the position on its own.
    ///
    /// `program` must be cell-batchable, and `memo_slot` (below [`MEMO_SLOTS`]) must always be
    /// passed with the same program. Values are bit-identical to `program.run(ctx, ..)`.
    pub(crate) fn sample_via_cell(
        &self,
        memo_slot: usize,
        program: &Program,
        ctx: FunctionContext,
        cx: &RunCtx,
    ) -> Option<f64> {
        if !self.memo_ok {
            return None;
        }
        let local_x = ctx.x - self.origin_x;
        let local_z = ctx.z - self.origin_z;
        let rel_y = ctx.y - self.min_y;
        if local_x as u32 >= 16
            || local_z as u32 >= 16
            || rel_y as u32 >= (self.cells_y as i32 * self.cell_height) as u32
        {
            return None;
        }
        let cell_x = div_cell(local_x, self.cell_width, self.cell_width_log2);
        let cell_z = div_cell(local_z, self.cell_width, self.cell_width_log2);
        let cell_y = div_cell(rel_y, self.cell_height, self.cell_height_log2);
        let lanes = self.cell_lanes;
        let cell_index = cell_z as usize * self.cells_x + cell_x as usize;
        let lane = (((rel_y - cell_y * self.cell_height) * self.cell_width
            + (local_z - cell_z * self.cell_width))
            * self.cell_width
            + (local_x - cell_x * self.cell_width)) as usize;

        {
            let memo = &self.memos.borrow()[memo_slot];
            if memo.cell_y == cell_y && memo.valid & (1 << cell_index) != 0 {
                return Some(memo.values[cell_index * lanes + lane]);
            }
        }

        // Taken out for the evaluation, which reads the corner grids but never the memos.
        let mut values = std::mem::take(&mut self.memos.borrow_mut()[memo_slot].values);
        if values.is_empty() {
            values = vec![0.0; self.cells_x * self.cells_z * lanes];
        }
        program.run_cell(
            self,
            (cell_x, cell_y, cell_z),
            cx,
            &mut values[cell_index * lanes..(cell_index + 1) * lanes],
        );
        let value = values[cell_index * lanes + lane];

        let memo = &mut self.memos.borrow_mut()[memo_slot];
        if memo.cell_y != cell_y {
            memo.cell_y = cell_y;
            memo.valid = 0;
        }
        memo.valid |= 1 << cell_index;
        memo.values = values;
        Some(value)
    }

    /// One `interpolated` slot at every block of the cell `(cell_x, cell_y, cell_z)`, written
    /// to `dst` in lane order `(local_y * cell_width + local_z) * cell_width + local_x`.
    ///
    /// Bit-identical to [`Self::interpolated`] at each block: `lerp3` blends along x, then y,
    /// then z, and this performs those same lerps on the same operands. The x stage depends
    /// only on `local_x` and the y stage only on `(local_x, local_y)`, so they are computed
    /// once and reused -- 1.6 lerps per block instead of 7.
    pub(crate) fn fill_cell_interpolated(
        &self,
        slot: usize,
        argument: &Program,
        cell: (i32, i32, i32),
        cx: &RunCtx,
        dst: &mut [f64],
    ) {
        self.ensure_corner_grid(slot, argument, cx);
        let grids = self.interpolated.borrow();
        let grid = grids[slot].as_ref().expect("just built");

        let cw = self.cell_width as usize;
        let ch = self.cell_height as usize;
        debug_assert!(cw <= MAX_CELL_WIDTH && dst.len() == cw * ch * cw);
        let x_stride = 1usize;
        let z_stride = self.cells_x + 1;
        let y_stride = z_stride * (self.cells_z + 1);
        let base = cell.1 as usize * y_stride + cell.2 as usize * z_stride + cell.0 as usize;
        let v000 = grid[base];
        let v100 = grid[base + x_stride];
        let v010 = grid[base + y_stride];
        let v110 = grid[base + y_stride + x_stride];
        let v001 = grid[base + z_stride];
        let v101 = grid[base + z_stride + x_stride];
        let v011 = grid[base + y_stride + z_stride];
        let v111 = grid[base + y_stride + z_stride + x_stride];

        let mut x00 = [0.0f64; MAX_CELL_WIDTH];
        let mut x10 = [0.0f64; MAX_CELL_WIDTH];
        let mut x01 = [0.0f64; MAX_CELL_WIDTH];
        let mut x11 = [0.0f64; MAX_CELL_WIDTH];
        for lx in 0..cw {
            let dx = lx as f64 * self.inv_cell_width;
            x00[lx] = lerp(dx, v000, v100);
            x10[lx] = lerp(dx, v010, v110);
            x01[lx] = lerp(dx, v001, v101);
            x11[lx] = lerp(dx, v011, v111);
        }
        for ly in 0..ch {
            let dy = ly as f64 * self.inv_cell_height;
            let mut y0 = [0.0f64; MAX_CELL_WIDTH];
            let mut y1 = [0.0f64; MAX_CELL_WIDTH];
            for lx in 0..cw {
                y0[lx] = lerp(dy, x00[lx], x10[lx]);
                y1[lx] = lerp(dy, x01[lx], x11[lx]);
            }
            for lz in 0..cw {
                let dz = lz as f64 * self.inv_cell_width;
                let row = &mut dst[(ly * cw + lz) * cw..(ly * cw + lz + 1) * cw];
                for lx in 0..cw {
                    row[lx] = lerp(dz, y0[lx], y1[lx]);
                }
            }
        }
    }

    /// `f(block_y)` for every block of a cell in layer `cell_y`, in the same lane order.
    pub(crate) fn fill_cell_by_y(&self, cell_y: i32, dst: &mut [f64], f: impl Fn(i32) -> f64) {
        let per_y = (self.cell_width * self.cell_width) as usize;
        let base_y = self.min_y + cell_y * self.cell_height;
        for (ly, row) in dst.chunks_mut(per_y).enumerate() {
            row.fill(f(base_y + ly as i32));
        }
    }

    fn ensure_corner_grid(&self, slot: usize, argument: &Program, cx: &RunCtx) {
        if self.interpolated.borrow()[slot].is_none() {
            // Built outside the borrow: evaluating a corner recurses back through the evaluator,
            // which may consult this same cache.
            let grid = self.build_corner_grid(argument, cx);
            self.interpolated.borrow_mut()[slot] = Some(grid);
        }
    }

    /// Dispatches one `Op::Cache` to the storage its kind uses.
    pub(crate) fn cached(
        &self,
        kind: CacheKind,
        slot: usize,
        argument: &Program,
        ctx: FunctionContext,
        cx: &RunCtx,
    ) -> f64 {
        match kind {
            CacheKind::Interpolated => self.interpolated(slot, argument, ctx, cx),
            CacheKind::Cache2d => self.column_cached(slot, argument, ctx.x, ctx.z, ctx, cx),
            // `flat_cache`: like `cache_2d`, but vanilla samples once per quart cell and reuses
            // that value across the 4x4 block area, so the sample position is snapped down to
            // the quart origin rather than taken at the caller's exact x/z.
            CacheKind::FlatCache => {
                self.column_cached(slot, argument, ctx.x >> 2 << 2, ctx.z >> 2 << 2, ctx, cx)
            }
            CacheKind::Once => self.once(slot, argument, ctx, cx),
        }
    }

    /// Value of an `interpolated` node: the tree is evaluated only at this chunk's cell corners
    /// -- 5x5x49 samples for a standard 384-tall world, against 98304 blocks -- and every block
    /// inside a cell is a trilinear blend of the eight corners around it.
    fn interpolated(
        &self,
        slot: usize,
        argument: &Program,
        ctx: FunctionContext,
        cx: &RunCtx,
    ) -> f64 {
        self.ensure_corner_grid(slot, argument, cx);

        let local_x = ctx.x - self.origin_x;
        let local_z = ctx.z - self.origin_z;
        let rel_y = ctx.y - self.min_y;

        let cell_x = div_cell(local_x, self.cell_width, self.cell_width_log2)
            .clamp(0, self.cells_x as i32 - 1);
        let cell_z = div_cell(local_z, self.cell_width, self.cell_width_log2)
            .clamp(0, self.cells_z as i32 - 1);
        let cell_y = div_cell(rel_y, self.cell_height, self.cell_height_log2)
            .clamp(0, self.cells_y as i32 - 1);

        let dx = (local_x - cell_x * self.cell_width) as f64 * self.inv_cell_width;
        let dz = (local_z - cell_z * self.cell_width) as f64 * self.inv_cell_width;
        let dy = (rel_y - cell_y * self.cell_height) as f64 * self.inv_cell_height;

        let grids = self.interpolated.borrow();
        let grid = grids[slot].as_ref().expect("just built");
        // The eight corners of one cell are two adjacent x pairs on each of four (y, z) rows,
        // so the row strides are all the indexing this needs.
        let x_stride = 1usize;
        let z_stride = self.cells_x + 1;
        let y_stride = z_stride * (self.cells_z + 1);
        let base = cell_y as usize * y_stride + cell_z as usize * z_stride + cell_x as usize;

        // PARITY-CHECK: interpolation order follows vanilla's `Mth.lerp3` -- blend along x,
        // then y, then z. Any order gives nearly the same number, but not bit-identically.
        lerp3(
            dx,
            dy,
            dz,
            grid[base],
            grid[base + x_stride],
            grid[base + y_stride],
            grid[base + y_stride + x_stride],
            grid[base + z_stride],
            grid[base + z_stride + x_stride],
            grid[base + y_stride + z_stride],
            grid[base + y_stride + z_stride + x_stride],
        )
    }

    fn build_corner_grid(&self, argument: &Program, cx: &RunCtx) -> Vec<f64> {
        let mut grid =
            Vec::with_capacity((self.cells_x + 1) * (self.cells_y + 1) * (self.cells_z + 1));
        for cy in 0..=self.cells_y {
            let y = self.min_y + cy as i32 * self.cell_height;
            for cz in 0..=self.cells_z {
                let z = self.origin_z + cz as i32 * self.cell_width;
                for cx_index in 0..=self.cells_x {
                    let x = self.origin_x + cx_index as i32 * self.cell_width;
                    grid.push(argument.run(FunctionContext { x, y, z }, cx));
                }
            }
        }
        grid
    }

    /// One value per (x, z) within this chunk's span, stored densely. A position outside the
    /// span -- which the corner grid never asks for, but a caller could -- is evaluated
    /// uncached rather than growing the storage.
    fn column_cached(
        &self,
        slot: usize,
        argument: &Program,
        sample_x: i32,
        sample_z: i32,
        ctx: FunctionContext,
        cx: &RunCtx,
    ) -> f64 {
        let sample_ctx = FunctionContext {
            x: sample_x,
            y: ctx.y,
            z: sample_z,
        };
        let Some(column) = self.column_slot(sample_x, sample_z) else {
            return argument.run(sample_ctx, cx);
        };

        if let Some(columns) = self.two_d.borrow()[slot].as_ref() {
            if let Some(value) = columns[column] {
                return value;
            }
        }
        // Not held across the run below, which may touch this same cache.
        let value = argument.run(sample_ctx, cx);

        let mut cache = self.two_d.borrow_mut();
        cache[slot].get_or_insert_with(|| vec![None; COLUMNS])[column] = Some(value);
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
    fn once(&self, slot: usize, argument: &Program, ctx: FunctionContext, cx: &RunCtx) -> f64 {
        let position = (ctx.x, ctx.y, ctx.z);
        if let Some((cached_position, value)) = self.once.borrow()[slot] {
            if cached_position == position {
                return value;
            }
        }
        let value = argument.run(ctx, cx);
        self.once.borrow_mut()[slot] = Some((position, value));
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
