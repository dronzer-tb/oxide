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

use std::cell::{Cell, OnceCell, RefCell};
use std::sync::{Arc, Mutex};

use crate::compiled::{Batch, CacheKind, CacheSlotCounts, Program, RunCtx};
use crate::density::FunctionContext;
use crate::scratch::Scratch;

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
    two_d: RefCell<Vec<Option<Scratch<Option<f64>>>>>,
    /// Slot -> (last position, value).
    once: RefCell<Vec<Option<LastSample>>>,

    /// Blocks per cell: `cell_width * cell_height * cell_width`.
    cell_lanes: usize,
    /// Whether this chunk's geometry fits [`CellMemo`]: cells tile the chunk exactly and are
    /// few and small enough for its fixed-size bookkeeping. True for every vanilla setting.
    memo_ok: bool,
    /// One memo per router slot, so two programs sampled alternately (the ore-vein pass reads
    /// `vein_toggle` and `vein_ridged` at the same block) do not evict each other. Plain
    /// `Cell`s rather than a `RefCell`, so a hit is loads only: no borrow flag to check and
    /// write back on every one of the ~15k calls per chunk the ore-vein pass makes.
    memos: [CellMemo; MEMO_SLOTS],

    /// Corner columns shared with the other chunks of the same router, when this chunk's
    /// geometry matches the one the shared cache was built for. See [`CornerColumns`].
    shared_columns: Option<Arc<CornerColumns>>,
}

/// Per-block values of one cell-batchable program for one horizontal layer of cells.
///
/// A chunk fill asks for `final_density` block by block, y outermost, so at any moment it is
/// working through a single layer of cells. Each cell of that layer is evaluated whole on first
/// touch (see `Program::run_cell`) and the rest of its blocks are then plain loads.
struct CellMemo {
    /// The layer the values belong to; `i32::MIN` when empty.
    cell_y: Cell<i32>,
    /// Bit `cell_z * cells_x + cell_x` set when that cell's values are filled.
    valid: Cell<u32>,
    /// `cells_x * cells_z` cells of `cell_lanes` values each; allocated on first use.
    values: OnceCell<Box<[Cell<f64>]>>,
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
            memos: std::array::from_fn(|_| CellMemo {
                cell_y: Cell::new(i32::MIN),
                valid: Cell::new(0),
                values: OnceCell::new(),
            }),
            shared_columns: None,
        }
    }

    /// Lets this chunk read and fill `shared`'s corner columns. Ignored -- the chunk keeps
    /// computing every corner itself -- unless the geometry is the one `shared` was sized for
    /// and the chunk's origin sits on the cell lattice, so that a shared column always means
    /// the same 49 (`cells_y + 1`) positions.
    pub(crate) fn with_shared_columns(mut self, shared: &Arc<CornerColumns>) -> Self {
        if shared.min_y == self.min_y
            && shared.cells_y == self.cells_y
            && shared.cell_height == self.cell_height
            && shared.cell_width == self.cell_width
            && self.origin_x.rem_euclid(self.cell_width) == 0
            && self.origin_z.rem_euclid(self.cell_width) == 0
        {
            self.shared_columns = Some(Arc::clone(shared));
        }
        self
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
    #[inline]
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

        let memo = &self.memos[memo_slot];
        if memo.cell_y.get() == cell_y && memo.valid.get() & (1 << cell_index) != 0 {
            if let Some(values) = memo.values.get() {
                return Some(values[cell_index * lanes + lane].get());
            }
        }
        Some(self.fill_memo(memo, program, (cell_x, cell_y, cell_z), cell_index, lane, cx))
    }

    /// The miss half of [`Self::sample_via_cell`]: evaluates the whole cell, stores it in
    /// `memo` and returns lane `lane` of it. Kept out of line so the hit path stays small
    /// enough to inline into every caller.
    #[inline(never)]
    fn fill_memo(
        &self,
        memo: &CellMemo,
        program: &Program,
        cell: (i32, i32, i32),
        cell_index: usize,
        lane: usize,
        cx: &RunCtx,
    ) -> f64 {
        let lanes = self.cell_lanes;
        // The evaluation reads the corner grids but never the memos; were it to, the memo's
        // tag is only updated below, so it would see this cell as not yet filled.
        let mut cell_values = Scratch::filled(lanes, 0.0f64);
        program.run_cell(self, cell, cx, &mut cell_values);
        let values = memo.values.get_or_init(|| {
            (0..self.cells_x * self.cells_z * lanes)
                .map(|_| Cell::new(0.0))
                .collect()
        });
        for (dst, value) in values[cell_index * lanes..(cell_index + 1) * lanes]
            .iter()
            .zip(cell_values.iter())
        {
            dst.set(*value);
        }
        if memo.cell_y.get() != cell.1 {
            memo.cell_y.set(cell.1);
            memo.valid.set(0);
        }
        memo.valid.set(memo.valid.get() | 1 << cell_index);
        cell_values[lane]
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

        let corners = [v000, v100, v010, v110, v001, v101, v011, v111];
        let (iw, ih) = (self.inv_cell_width, self.inv_cell_height);
        match cw {
            4 => lerp_cell_fixed::<4>(corners, iw, ih, dst),
            8 => lerp_cell_fixed::<8>(corners, iw, ih, dst),
            _ => lerp_cell_any(corners, cw, ch, iw, ih, dst),
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
            let grid = self.build_corner_grid(slot, argument, cx);
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

    fn build_corner_grid(&self, slot: usize, argument: &Program, cx: &RunCtx) -> Vec<f64> {
        if let Some(shared) = self.shared_columns.as_deref() {
            return self.build_corner_grid_shared(shared, slot, argument, cx);
        }
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

    /// [`Self::build_corner_grid`], one corner column at a time, taking the columns on the
    /// chunk's edge from `shared` when a neighbour already computed them.
    ///
    /// Same values: a corner's value is a pure function of its block position (the caches the
    /// argument reads are keyed by position and their arguments do not depend on evaluation
    /// order), so neither the column-major order nor a column computed by another chunk can
    /// change a bit. Interior columns are never shared by two chunks, so they bypass `shared`.
    fn build_corner_grid_shared(
        &self,
        shared: &CornerColumns,
        slot: usize,
        argument: &Program,
        cx: &RunCtx,
    ) -> Vec<f64> {
        let nx = self.cells_x + 1;
        let nz = self.cells_z + 1;
        let ny = self.cells_y + 1;
        let y_stride = nx * nz;
        let mut grid = vec![0.0; y_stride * ny];
        let mut column = vec![0.0; ny];
        let lanes: Vec<u32> = (0..ny as u32).collect();
        let mut ctxs = Vec::with_capacity(ny);
        for cz in 0..nz {
            let z = self.origin_z + cz as i32 * self.cell_width;
            for cx_index in 0..nx {
                let x = self.origin_x + cx_index as i32 * self.cell_width;
                let base = cz * nx + cx_index;
                let edge = cx_index == 0 || cx_index == nx - 1 || cz == 0 || cz == nz - 1;
                if edge && shared.get(slot, x, z, &mut column) {
                    for (cy, value) in column.iter().enumerate() {
                        grid[cy * y_stride + base] = *value;
                    }
                    continue;
                }
                // The whole column in one lane batch (see `Program::run_lanes`): the same value
                // per corner as one `run` each, since every cache it reads is pure.
                ctxs.clear();
                ctxs.extend((0..ny).map(|cy| FunctionContext {
                    x,
                    y: self.min_y + cy as i32 * self.cell_height,
                    z,
                }));
                argument.run_lanes(&lanes, &Batch::new(&ctxs), cx, &mut column);
                for (cy, value) in column.iter().enumerate() {
                    grid[cy * y_stride + base] = *value;
                }
                if edge {
                    shared.insert(slot, x, z, &column);
                }
            }
        }
        grid
    }

    #[cfg(test)]
    pub(crate) fn has_shared_columns(&self) -> bool {
        self.shared_columns.is_some()
    }

    /// The corner grid of one `interpolated` slot, if built, for tests that compare grids.
    #[cfg(test)]
    pub(crate) fn corner_grid(&self, slot: usize) -> Option<Vec<f64>> {
        self.interpolated.borrow().get(slot)?.clone()
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
        // Pooled: a chunk's caches (and the aquifer's per-neighbour ones) are built and dropped
        // many times per chunk; reusing the column vectors keeps that off the allocator.
        cache[slot].get_or_insert_with(|| Scratch::filled(COLUMNS, None))[column] = Some(value);
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

/// Ways per set in [`CornerColumns`].
const COLUMN_WAYS: usize = 4;
/// Independently locked shards of [`CornerColumns`].
const COLUMN_SHARDS: usize = 64;
/// Sets per shard: 64 shards x 128 sets x 4 ways = 32768 columns, ~13 MB for a 384-tall world
/// (49 values each). A radius-16 pregen touches ~8k distinct edge columns per slot, and only a
/// neighbour generated soon after needs one, so this keeps the useful ones resident.
const COLUMN_SETS: usize = 128;

/// Interpolation corner columns shared by every chunk of one router, across threads.
///
/// A chunk's corner columns at local x = 16 / z = 16 are the x = 0 / z = 0 columns of its +x
/// / +z neighbours, so 9 of a chunk's 25 columns are computed by two or four chunks. A column
/// is a pure function of `(slot, x, z)` for fixed geometry, so whichever chunk computes it
/// first can hand it to the others.
///
/// Bounded and lossy: set-associative with FIFO replacement inside a set, fixed capacity, one
/// lock per shard. Every entry keeps its full key, so a hit is always the exact column and an
/// eviction only costs a recomputation.
pub(crate) struct CornerColumns {
    min_y: i32,
    cells_y: usize,
    cell_height: i32,
    cell_width: i32,
    /// `cells_y + 1` values per column.
    column_len: usize,
    shards: Box<[Shard]>,
    /// Print [`ColumnStats`] to stderr when dropped: `OXIDE_CORNER_STATS` set at construction.
    report: bool,
}

struct Shard {
    entries: Mutex<ColumnShard>,
    /// Lock acquisitions that found the shard already held by another thread. Counted outside
    /// the lock (that is the point); an uncontended acquire never touches it.
    contended: std::sync::atomic::AtomicU64,
}

/// Totals over every shard, for ruling lock contention in or out rather than assuming it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ColumnStats {
    /// Edge-column lookups answered from the cache.
    pub hits: u64,
    /// Edge-column lookups that had to compute the column.
    pub misses: u64,
    /// Inserts that displaced another column.
    pub evictions: u64,
    /// Lock acquisitions (lookups and inserts) that had to wait for another thread.
    pub contended: u64,
    /// All lock acquisitions.
    pub acquisitions: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ColumnKey {
    slot: u32,
    x: i32,
    z: i32,
}

impl ColumnKey {
    const EMPTY: ColumnKey = ColumnKey {
        slot: u32::MAX,
        x: 0,
        z: 0,
    };
}

struct ColumnShard {
    /// `COLUMN_SETS * COLUMN_WAYS` keys, way-minor.
    keys: Box<[ColumnKey]>,
    /// `column_len` values per key, in the same order; allocated on the first insert.
    values: Vec<f64>,
    /// Per set, the way the next insert replaces.
    next_way: Box<[u8]>,
    /// Counted under the lock, which is already held, so they cost nothing extra.
    hits: u64,
    misses: u64,
    evictions: u64,
    acquisitions: u64,
}

impl CornerColumns {
    /// Sized for the geometry [`ChunkCaches::new`] derives from the same arguments.
    pub(crate) fn new(min_y: i32, height: i32, size_horizontal: i32, size_vertical: i32) -> Self {
        let cell_width = (size_horizontal * 4).max(1);
        let cell_height = (size_vertical * 4).max(1);
        let cells_y = (height / cell_height) as usize;
        let shards = (0..COLUMN_SHARDS)
            .map(|_| {
                Shard {
                    entries: Mutex::new(ColumnShard {
                        keys: vec![ColumnKey::EMPTY; COLUMN_SETS * COLUMN_WAYS]
                            .into_boxed_slice(),
                        values: Vec::new(),
                        next_way: vec![0; COLUMN_SETS].into_boxed_slice(),
                        hits: 0,
                        misses: 0,
                        evictions: 0,
                        acquisitions: 0,
                    }),
                    contended: Default::default(),
                }
            })
            .collect();
        Self {
            min_y,
            cells_y,
            cell_height,
            cell_width,
            column_len: cells_y + 1,
            shards,
            report: std::env::var_os("OXIDE_CORNER_STATS").is_some(),
        }
    }

    /// Shard index and set index within it.
    #[inline]
    fn locate(&self, slot: usize, x: i32, z: i32) -> (usize, usize) {
        // splitmix64 finaliser over the packed key.
        let packed = ((x as u32 as u64) << 32) | (z as u32 as u64);
        let mut h = packed ^ (slot as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        h ^= h >> 31;
        let shard = (h as usize) % COLUMN_SHARDS;
        let set = ((h >> 32) as usize) % COLUMN_SETS;
        (shard, set)
    }

    fn lock(&self, shard: usize) -> std::sync::MutexGuard<'_, ColumnShard> {
        // A panic elsewhere while holding the lock leaves only whole entries behind: keys are
        // written after their values.
        let shard = &self.shards[shard];
        let mut guard = match shard.entries.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                shard
                    .contended
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                shard
                    .entries
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
            }
        };
        guard.acquisitions += 1;
        guard
    }

    /// Totals so far over every shard.
    pub(crate) fn stats(&self) -> ColumnStats {
        let mut total = ColumnStats::default();
        for shard in self.shards.iter() {
            let guard = shard
                .entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            total.hits += guard.hits;
            total.misses += guard.misses;
            total.evictions += guard.evictions;
            total.acquisitions += guard.acquisitions;
            total.contended += shard.contended.load(std::sync::atomic::Ordering::Relaxed);
        }
        total
    }

    /// Copies the column into `out` (`column_len` long) and returns `true` when present.
    fn get(&self, slot: usize, x: i32, z: i32, out: &mut [f64]) -> bool {
        debug_assert_eq!(out.len(), self.column_len);
        let key = ColumnKey {
            slot: slot as u32,
            x,
            z,
        };
        let (shard, set) = self.locate(slot, x, z);
        let mut guard = self.lock(shard);
        let ways = &guard.keys[set * COLUMN_WAYS..(set + 1) * COLUMN_WAYS];
        match ways.iter().position(|k| *k == key) {
            Some(way) => {
                let start = (set * COLUMN_WAYS + way) * self.column_len;
                out.copy_from_slice(&guard.values[start..start + self.column_len]);
                guard.hits += 1;
                true
            }
            None => {
                guard.misses += 1;
                false
            }
        }
    }

    /// Stores `column`, unless another chunk stored the same one in the meantime.
    fn insert(&self, slot: usize, x: i32, z: i32, column: &[f64]) {
        debug_assert_eq!(column.len(), self.column_len);
        let key = ColumnKey {
            slot: slot as u32,
            x,
            z,
        };
        let (shard, set) = self.locate(slot, x, z);
        let mut guard = self.lock(shard);
        let shard = &mut *guard;
        let ways = &shard.keys[set * COLUMN_WAYS..(set + 1) * COLUMN_WAYS];
        if ways.contains(&key) {
            return;
        }
        let way = match ways.iter().position(|k| *k == ColumnKey::EMPTY) {
            Some(way) => way,
            None => {
                let way = shard.next_way[set] as usize;
                shard.next_way[set] = ((way + 1) % COLUMN_WAYS) as u8;
                shard.evictions += 1;
                way
            }
        };
        if shard.values.is_empty() {
            shard.values = vec![0.0; COLUMN_SETS * COLUMN_WAYS * self.column_len];
        }
        let index = set * COLUMN_WAYS + way;
        let start = index * self.column_len;
        shard.values[start..start + self.column_len].copy_from_slice(column);
        shard.keys[index] = key;
    }

    /// `(hits, misses)` of edge-column lookups so far.
    #[cfg(test)]
    pub(crate) fn counts(&self) -> (usize, usize) {
        let stats = self.stats();
        (stats.hits as usize, stats.misses as usize)
    }
}

impl Drop for CornerColumns {
    fn drop(&mut self) {
        if self.report {
            let s = self.stats();
            let lookups = (s.hits + s.misses).max(1);
            let acquisitions = s.acquisitions.max(1);
            eprintln!(
                "CORNER hits={} misses={} hit_rate={:.3} evictions={} contended={} acquisitions={} contended_rate={:.5}",
                s.hits,
                s.misses,
                s.hits as f64 / lookups as f64,
                s.evictions,
                s.contended,
                s.acquisitions,
                s.contended as f64 / acquisitions as f64,
            );
            #[cfg(feature = "hash-stats")]
            {
                let [pr, pc, br, bc] = crate::improved_noise::hash_stats::global();
                let rate = |r: u64, c: u64| r as f64 / (r + c).max(1) as f64;
                eprintln!(
                    "HASH perlin_reused={pr} perlin_computed={pc} perlin_rate={:.3} blended_reused={br} blended_computed={bc} blended_rate={:.3} all_rate={:.3}",
                    rate(pr, pc),
                    rate(br, bc),
                    rate(pr + br, pc + bc),
                );
            }
        }
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

/// The trilinear fill of [`ChunkCaches::fill_cell_interpolated`] for any cell width up to
/// [`MAX_CELL_WIDTH`]. `c` is `[v000, v100, v010, v110, v001, v101, v011, v111]`.
fn lerp_cell_any(c: [f64; 8], cw: usize, ch: usize, iw: f64, ih: f64, dst: &mut [f64]) {
    let [v000, v100, v010, v110, v001, v101, v011, v111] = c;
    let mut x00 = [0.0f64; MAX_CELL_WIDTH];
    let mut x10 = [0.0f64; MAX_CELL_WIDTH];
    let mut x01 = [0.0f64; MAX_CELL_WIDTH];
    let mut x11 = [0.0f64; MAX_CELL_WIDTH];
    for lx in 0..cw {
        let dx = lx as f64 * iw;
        x00[lx] = lerp(dx, v000, v100);
        x10[lx] = lerp(dx, v010, v110);
        x01[lx] = lerp(dx, v001, v101);
        x11[lx] = lerp(dx, v011, v111);
    }
    for ly in 0..ch {
        let dy = ly as f64 * ih;
        let mut y0 = [0.0f64; MAX_CELL_WIDTH];
        let mut y1 = [0.0f64; MAX_CELL_WIDTH];
        for lx in 0..cw {
            y0[lx] = lerp(dy, x00[lx], x10[lx]);
            y1[lx] = lerp(dy, x01[lx], x11[lx]);
        }
        for lz in 0..cw {
            let dz = lz as f64 * iw;
            let row = &mut dst[(ly * cw + lz) * cw..(ly * cw + lz + 1) * cw];
            for lx in 0..cw {
                row[lx] = lerp(dz, y0[lx], y1[lx]);
            }
        }
    }
}

/// [`lerp_cell_any`] with the cell width known at compile time, so the per-lane loops have
/// constant trip counts and no bounds checks and unroll / vectorize. Every lane performs the
/// same lerps on the same operands in the same order (x, then y, then z), so the results are
/// bit-identical; only the loop control changes. The layer count is `dst.len() / (CW * CW)`.
#[inline(always)]
fn lerp_cell_fixed<const CW: usize>(c: [f64; 8], iw: f64, ih: f64, dst: &mut [f64]) {
    let [v000, v100, v010, v110, v001, v101, v011, v111] = c;
    let mut d = [0.0f64; CW];
    for (lx, d) in d.iter_mut().enumerate() {
        *d = lx as f64 * iw;
    }
    let x00: [f64; CW] = std::array::from_fn(|lx| lerp(d[lx], v000, v100));
    let x10: [f64; CW] = std::array::from_fn(|lx| lerp(d[lx], v010, v110));
    let x01: [f64; CW] = std::array::from_fn(|lx| lerp(d[lx], v001, v101));
    let x11: [f64; CW] = std::array::from_fn(|lx| lerp(d[lx], v011, v111));
    for (ly, layer) in dst.chunks_exact_mut(CW * CW).enumerate() {
        let dy = ly as f64 * ih;
        let y0: [f64; CW] = std::array::from_fn(|lx| lerp(dy, x00[lx], x10[lx]));
        let y1: [f64; CW] = std::array::from_fn(|lx| lerp(dy, x01[lx], x11[lx]));
        for (lz, row) in layer.chunks_exact_mut(CW).enumerate() {
            // `dz` for row `lz` is the same product `dx` is for lane `lz`.
            let dz = d[lz];
            for lx in 0..CW {
                row[lx] = lerp(dz, y0[lx], y1[lx]);
            }
        }
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

#[inline(always)]
fn lerp(delta: f64, from: f64, to: f64) -> f64 {
    from + delta * (to - from)
}

#[cfg(test)]
mod lerp_cell_tests {
    use super::*;

    /// The fixed-width fill must equal the general one bit for bit, signed zeros, infinities
    /// and NaN payloads included, for every supported width and several layer counts.
    #[test]
    fn fixed_width_fill_is_bit_identical() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let specials = [0.0, -0.0, f64::INFINITY, f64::NEG_INFINITY, f64::from_bits(0x7ff8_0000_0000_1234), 1e-310];
        for round in 0..2000 {
            let mut c = [0.0f64; 8];
            for v in &mut c {
                let r = next();
                *v = if r % 7 == 0 {
                    specials[(r >> 8) as usize % specials.len()]
                } else {
                    (r >> 11) as f64 / (1u64 << 53) as f64 * 4.0 - 2.0
                };
            }
            for (cw, ch) in [(4usize, 8usize), (4, 4), (8, 4), (8, 8)] {
                let (iw, ih) = (1.0 / cw as f64, 1.0 / ch as f64);
                let mut want = vec![0.0; cw * ch * cw];
                let mut got = vec![f64::from_bits(round); cw * ch * cw];
                lerp_cell_any(c, cw, ch, iw, ih, &mut want);
                match cw {
                    4 => lerp_cell_fixed::<4>(c, iw, ih, &mut got),
                    _ => lerp_cell_fixed::<8>(c, iw, ih, &mut got),
                }
                // When two *different* NaNs meet in one `fadd` (a payload NaN corner and a NaN
                // generated by `inf - inf`), which payload survives depends on how LLVM orders
                // the commutative operands -- unspecified by Rust, in the old code too. Only
                // there is "both NaN" accepted; everywhere else the bits must match.
                let two_nan_sources = c.iter().any(|v| v.is_nan())
                    && c.iter().any(|v| v.is_infinite());
                for (w, g) in want.iter().zip(&got) {
                    if two_nan_sources && w.is_nan() && g.is_nan() {
                        continue;
                    }
                    assert_eq!(w.to_bits(), g.to_bits(), "cw {cw} ch {ch} corners {c:?}");
                }
            }
        }
    }
}
