//! Lowers a density-function tree into a flat instruction list, and runs it.
//!
//! The tree the datapack deserializes into is a good representation for validation and a bad
//! one for evaluation: every node is a `Box`ed enum, every `Reference` is a `HashMap` lookup
//! keyed by a `namespace:path` pair of `String`s, every `noise` node looks its `NormalNoise` up
//! the same way, and reaching a leaf means chasing one pointer per level. Filling a chunk
//! walked fourteen nodes for every one of its 98304 blocks.
//!
//! Compiling happens once, when the router is built. References are inlined, noise ids become
//! indices into a dense slice, and each cache node is given a dense slot so `ChunkCaches` can
//! index straight into an array instead of probing for a node address. What is left per block
//! is a linear sweep over a `Vec<Op>` writing into a register per instruction, with a nested
//! call only where the datapack itself is lazy (`range_choice`, `interval_select`) or where a
//! cache boundary means the subtree is deliberately *not* evaluated here.
//!
//! This is the only evaluator. An earlier tree-walking one was deleted rather than kept beside
//! it: two implementations of the same per-node semantics is how a generator quietly stops
//! matching itself, and the node semantics below are the parity-critical part.

use std::collections::HashMap;

use oxide_datapack::{
    CubicSpline, DensityFunction, DensityFunctionObject, RarityValueMapper, Registry,
    ResourceLocation, SplineValue,
};

use crate::blended_noise::{BlendedNoise, BlendedNoiseParams};
use crate::cache::ChunkCaches;
use crate::density::{clamped_lerp, lerp_f32, map_rarity, FunctionContext};
use crate::normal_noise::NormalNoise;

/// A noise id that the `worldgen/noise` registry did not have. Sampling it yields 0.0, which is
/// what the tree walker did for a missing entry.
const NO_NOISE: u32 = u32::MAX;

/// Which per-chunk cache a `Op::Cache` talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheKind {
    /// `interpolated`: the argument is evaluated at cell corners and blended between them.
    Interpolated,
    /// `cache_2d`: one value per column, sampled at the caller's exact x/z.
    Cache2d,
    /// `flat_cache`: one value per quart cell, sampled at the quart origin.
    FlatCache,
    /// `cache_once` / `cache_all_in_cell`: remembers only the last position.
    Once,
}

/// One instruction. Each writes the register at its own index, and reads registers written by
/// instructions before it.
///
/// The hot arithmetic nodes get their own variants rather than a shared `Binary { op, a, b }`,
/// so dispatch is one jump table rather than two.
#[derive(Debug)]
pub(crate) enum Op {
    Const(f64),

    Noise(Box<NoiseOp>),
    ShiftedNoise(Box<ShiftedNoiseOp>),
    /// `shift` / `shift_a` / `shift_b`: the same offset noise read at three permutations of the
    /// position. See `ShiftKind`.
    Shift {
        noise: u32,
        kind: ShiftKind,
    },
    OldBlendedNoise(Box<BlendedNoiseParams>),
    WeirdScaledSampler(Box<WeirdScaledSamplerOp>),
    YClampedGradient(Box<YClampedGradientOp>),

    Add(u32, u32),
    Mul(u32, u32),
    Min(u32, u32),
    Max(u32, u32),

    Abs(u32),
    Square(u32),
    Cube(u32),
    HalfNegative(u32),
    QuarterNegative(u32),
    Squeeze(u32),
    Invert(u32),
    Clamp(Box<ClampOp>),

    Spline(Box<CompiledSpline>),

    /// Lazy: only the taken branch runs, as in the tree.
    RangeChoice(Box<RangeChoiceOp>),
    IntervalSelect(Box<IntervalSelectOp>),
    FindTopSurface(Box<FindTopSurfaceOp>),

    /// A cache boundary: `argument` is what the cache fills itself from, and is deliberately
    /// not part of this program's linear sweep.
    Cache {
        kind: CacheKind,
        slot: u32,
        argument: Box<Program>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShiftKind {
    /// `(x, y, z)` unchanged.
    Xyz,
    /// `(x, 0, z)`.
    Xz,
    /// `(z, x, 0)`.
    Zx,
}

pub(crate) struct NoiseOp {
    pub noise: u32,
    pub xz_scale: f64,
    pub y_scale: f64,
}

pub(crate) struct ShiftedNoiseOp {
    pub noise: u32,
    pub xz_scale: f64,
    pub y_scale: f64,
    pub shift_x: u32,
    pub shift_y: u32,
    pub shift_z: u32,
}

pub(crate) struct WeirdScaledSamplerOp {
    pub input: u32,
    pub noise: u32,
    pub mapper: RarityValueMapper,
}

pub(crate) struct YClampedGradientOp {
    pub from_y: i32,
    pub to_y: i32,
    pub from_value: f64,
    pub to_value: f64,
}

pub(crate) struct ClampOp {
    pub input: u32,
    pub min: f64,
    pub max: f64,
}

pub(crate) struct RangeChoiceOp {
    pub input: u32,
    pub min_inclusive: f64,
    pub max_exclusive: f64,
    pub when_in_range: Program,
    pub when_out_of_range: Program,
}

pub(crate) struct IntervalSelectOp {
    pub input: u32,
    pub thresholds: Vec<f64>,
    pub functions: Vec<Program>,
}

pub(crate) struct FindTopSurfaceOp {
    pub cell_height: i32,
    pub lower_bound: i32,
    pub upper_bound: u32,
    pub density: Program,
}

/// `CubicSpline` with its coordinate lowered to a program and its nested splines lowered in
/// place. `f32` throughout matches vanilla's `CubicSpline` field types.
pub(crate) struct CompiledSpline {
    pub coordinate: Program,
    pub points: Vec<CompiledSplinePoint>,
}

pub(crate) struct CompiledSplinePoint {
    pub location: f32,
    pub value: CompiledSplineValue,
    pub derivative: f32,
}

pub(crate) enum CompiledSplineValue {
    Constant(f32),
    Spline(Box<CompiledSpline>),
}

/// A lowered density function. The last instruction's register is the result.
pub(crate) struct Program {
    ops: Vec<Op>,
}

impl Program {
    /// Number of instructions, i.e. how many `f64` registers [`Program::run`] needs. Exposed so
    /// the register budget can be checked against what real datapacks actually compile to.
    pub(crate) fn len(&self) -> usize {
        self.ops.len()
    }
}

/// Everything running a program needs beyond the program itself.
pub(crate) struct RunCtx<'a> {
    /// Indexed by the noise indices the compiler assigned.
    pub noises: &'a [NormalNoise],
    pub blended: &'a BlendedNoise,
    /// `None` for a one-off sample (a biome lookup, a test), where the cache nodes evaluate
    /// straight through -- which is the right value for every node except `interpolated`, whose
    /// value *is* the blend between cell corners. See `cache.rs`.
    pub caches: Option<&'a ChunkCaches>,
}

/// Registers held on the stack up to this many instructions. Every program a real datapack
/// produces is far below it; a larger one spills to a heap allocation rather than failing.
const INLINE_REGISTERS: usize = 16;

impl Program {
    /// The `interpolated` cache slot this program's result comes straight out of, if the whole
    /// program is exactly one such cache node.
    ///
    /// `final_density` in every vanilla-shaped noise router is an `interpolated` wrapper, so
    /// this is how a caller finds the corner grid that already holds that slot's cell corners
    /// -- see [`ChunkCaches::cell_bounds`]. `None` means the program is something else and no
    /// cell-bounds shortcut is valid for it.
    /// One line per instruction, for diagnosing why a shortcut does or does not apply.
    pub(crate) fn dump(&self) -> Vec<String> {
        self.ops.iter().map(|op| format!("{op:?}")).collect()
    }

    pub(crate) fn interpolated_result_slot(&self) -> Option<u32> {
        match self.ops.last()? {
            Op::Cache {
                kind: CacheKind::Interpolated,
                slot,
                ..
            } => Some(*slot),
            _ => None,
        }
    }

    pub(crate) fn run(&self, ctx: FunctionContext, cx: &RunCtx) -> f64 {
        let n = self.ops.len();
        debug_assert!(n > 0, "a compiled program always ends in its result");
        let mut inline = [0.0f64; INLINE_REGISTERS];
        let mut spilled;
        let regs: &mut [f64] = if n <= INLINE_REGISTERS {
            &mut inline[..n]
        } else {
            spilled = vec![0.0; n];
            &mut spilled
        };

        for i in 0..n {
            let value = eval_op(&self.ops[i], regs, ctx, cx);
            regs[i] = value;
        }
        regs[n - 1]
    }
}

fn sample_noise(noise: u32, cx: &RunCtx, x: f64, y: f64, z: f64) -> f64 {
    match cx.noises.get(noise as usize) {
        Some(n) => n.get_value(x, y, z),
        None => 0.0,
    }
}

fn eval_op(op: &Op, regs: &[f64], ctx: FunctionContext, cx: &RunCtx) -> f64 {
    let (bx, by, bz) = (ctx.x as f64, ctx.y as f64, ctx.z as f64);
    match op {
        Op::Const(v) => *v,

        Op::Noise(n) => sample_noise(
            n.noise,
            cx,
            bx * n.xz_scale,
            by * n.y_scale,
            bz * n.xz_scale,
        ),
        Op::ShiftedNoise(n) => sample_noise(
            n.noise,
            cx,
            bx * n.xz_scale + regs[n.shift_x as usize],
            by * n.y_scale + regs[n.shift_y as usize],
            bz * n.xz_scale + regs[n.shift_z as usize],
        ),
        // Verified 2026-08-22 against decompiled `DensityFunctions.ShiftNoise`'s default
        // `compute` method: `offsetNoise.getValue(x*0.25, y*0.25, z*0.25) * 4.0`. `ShiftA` /
        // `ShiftB` each call it with a permuted (x, y, z): `Shift.compute` passes
        // `(blockX, blockY, blockZ)` unchanged, `ShiftA.compute` passes `(blockX, 0, blockZ)`,
        // `ShiftB.compute` passes `(blockZ, blockX, 0)`.
        Op::Shift { noise, kind } => {
            let (sx, sy, sz) = match kind {
                ShiftKind::Xyz => (bx * 0.25, by * 0.25, bz * 0.25),
                ShiftKind::Xz => (bx * 0.25, 0.0, bz * 0.25),
                ShiftKind::Zx => (bz * 0.25, bx * 0.25, 0.0),
            };
            sample_noise(*noise, cx, sx, sy, sz) * 4.0
        }
        Op::OldBlendedNoise(params) => cx.blended.compute(params, bx, by, bz),

        Op::WeirdScaledSampler(w) => {
            // Checked 2026-08-22: `minecraft:weird_scaled_sampler` does not exist in the real
            // 26.2 server's `DensityFunctions` at all, so this branch is dead in practice.
            // Kept because `oxide-datapack` still models the JSON shape; the formula and
            // thresholds remain an unverified reconstruction if a future version brings it back.
            let d = map_rarity(w.mapper, regs[w.input as usize]);
            d * sample_noise(w.noise, cx, bx / d, by / d, bz / d).abs()
        }
        Op::YClampedGradient(g) => clamped_lerp(ctx.y, g.from_y, g.to_y, g.from_value, g.to_value),

        Op::Add(a, b) => regs[*a as usize] + regs[*b as usize],
        Op::Mul(a, b) => regs[*a as usize] * regs[*b as usize],
        Op::Min(a, b) => regs[*a as usize].min(regs[*b as usize]),
        Op::Max(a, b) => regs[*a as usize].max(regs[*b as usize]),

        Op::Abs(a) => regs[*a as usize].abs(),
        Op::Square(a) => {
            let v = regs[*a as usize];
            v * v
        }
        Op::Cube(a) => {
            let v = regs[*a as usize];
            v * v * v
        }
        Op::HalfNegative(a) => {
            let v = regs[*a as usize];
            if v > 0.0 {
                v
            } else {
                v * 0.5
            }
        }
        Op::QuarterNegative(a) => {
            let v = regs[*a as usize];
            if v > 0.0 {
                v
            } else {
                v * 0.25
            }
        }
        Op::Squeeze(a) => {
            let v = regs[*a as usize].clamp(-1.0, 1.0);
            v / 2.0 - v * v * v / 24.0
        }
        // PARITY-CHECK: reciprocal, inferred from vanilla's own use of it
        // (`mul(0.2734375, invert(factor))`), not from a decompiled source. The x == 0 case is
        // likewise unpinned; f64 division gives +/-inf, which propagates rather than silently
        // reading as a plausible density.
        Op::Invert(a) => 1.0 / regs[*a as usize],
        Op::Clamp(c) => regs[c.input as usize].clamp(c.min, c.max),

        Op::Spline(s) => eval_spline(s, ctx, cx) as f64,

        Op::RangeChoice(r) => {
            let v = regs[r.input as usize];
            if v >= r.min_inclusive && v < r.max_exclusive {
                r.when_in_range.run(ctx, cx)
            } else {
                r.when_out_of_range.run(ctx, cx)
            }
        }
        Op::IntervalSelect(s) => {
            let v = regs[s.input as usize];
            // The first threshold the input falls short of picks that slot; an input past every
            // threshold picks the last function. `thresholds` is read as ascending -- vanilla's
            // own exports always are.
            let index = s.thresholds.partition_point(|threshold| v >= *threshold);
            match s.functions.get(index).or_else(|| s.functions.last()) {
                Some(program) => program.run(ctx, cx),
                // functions is empty: a malformed node. 0.0 is what an absent density function
                // contributes everywhere else here.
                None => 0.0,
            }
        }
        Op::FindTopSurface(f) => {
            // Topmost cell-aligned y whose density is positive, scanning down.
            // PARITY-CHECK: vanilla's tie-breaking, and whether it scans down from the top or up
            // from the bottom, is not pinned to a decompiled source.
            let top = regs[f.upper_bound as usize].floor() as i32;
            let step = f.cell_height.max(1);
            let mut y = top - top.rem_euclid(step);
            while y > f.lower_bound {
                let at = FunctionContext {
                    x: ctx.x,
                    y,
                    z: ctx.z,
                };
                if f.density.run(at, cx) > 0.0 {
                    return y as f64;
                }
                y -= step;
            }
            f.lower_bound as f64
        }

        Op::Cache {
            kind,
            slot,
            argument,
        } => match cx.caches {
            Some(caches) => caches.cached(*kind, *slot as usize, argument, ctx, cx),
            None => argument.run(ctx, cx),
        },
    }
}

/// Cubic Hermite spline with explicit per-point derivatives, matching vanilla's
/// `CubicSpline.Multipoint.apply`. Verified 2026-08-22 against decompiled
/// `net.minecraft.util.CubicSpline` -- formula, `findIntervalStart`'s "last point with location
/// <= input" semantics, and the two-edge linear-extend cases outside the point range.
fn eval_spline(spline: &CompiledSpline, ctx: FunctionContext, cx: &RunCtx) -> f32 {
    let pos = spline.coordinate.run(ctx, cx) as f32;
    let points = &spline.points;
    debug_assert!(!points.is_empty(), "spline must have at least one point");

    let idx = points.iter().rposition(|p| p.location <= pos);

    let value_at = |v: &CompiledSplineValue| -> f32 {
        match v {
            CompiledSplineValue::Constant(c) => *c,
            CompiledSplineValue::Spline(s) => eval_spline(s, ctx, cx),
        }
    };

    match idx {
        None => {
            let p0 = &points[0];
            value_at(&p0.value) + p0.derivative * (pos - p0.location)
        }
        Some(i) if i == points.len() - 1 => {
            let p = &points[i];
            value_at(&p.value) + p.derivative * (pos - p.location)
        }
        Some(i) => {
            let p0 = &points[i];
            let p1 = &points[i + 1];
            let span = p1.location - p0.location;
            let t = (pos - p0.location) / span;
            let v0 = value_at(&p0.value);
            let v1 = value_at(&p1.value);
            let f = p0.derivative * span - (v1 - v0);
            let g = -p1.derivative * span + (v1 - v0);
            lerp_f32(t, v0, v1) + t * (1.0 - t) * lerp_f32(t, f, g)
        }
    }
}

/// How many cache slots of each kind a compiled router uses, which is what sizes a
/// [`ChunkCaches`].
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheSlotCounts {
    pub interpolated: usize,
    pub two_d: usize,
    pub once: usize,
}

/// Lowers density-function trees, sharing cache slots and noise indices across every tree it
/// compiles -- the router's slots reference the same registry nodes, and a node that appears in
/// two of them must share one cache entry the way vanilla's shared instances do.
pub(crate) struct Compiler<'a> {
    registry: &'a Registry<DensityFunction>,
    noise_index: &'a HashMap<ResourceLocation, u32>,
    /// Cache node address -> slot within its kind. Addresses are stable: the registry owns
    /// every node for the router's lifetime and never mutates it.
    slots: HashMap<usize, u32>,
    counts: CacheSlotCounts,
    /// `Reference` nodes currently being inlined, so a cyclic pack cannot recurse forever.
    resolving: Vec<usize>,
}

impl<'a> Compiler<'a> {
    pub(crate) fn new(
        registry: &'a Registry<DensityFunction>,
        noise_index: &'a HashMap<ResourceLocation, u32>,
    ) -> Self {
        Self {
            registry,
            noise_index,
            slots: HashMap::new(),
            counts: CacheSlotCounts::default(),
            resolving: Vec::new(),
        }
    }

    pub(crate) fn counts(&self) -> CacheSlotCounts {
        self.counts
    }

    pub(crate) fn compile(&mut self, df: &DensityFunction) -> Program {
        let mut ops = Vec::new();
        self.emit(df, &mut ops);
        if ops.is_empty() {
            ops.push(Op::Const(0.0));
        }
        Program { ops }
    }

    fn noise(&self, id: &ResourceLocation) -> u32 {
        self.noise_index.get(id).copied().unwrap_or(NO_NOISE)
    }

    /// Emits `df` into `ops` and returns the register holding its value.
    fn emit(&mut self, df: &DensityFunction, ops: &mut Vec<Op>) -> u32 {
        match df {
            DensityFunction::Constant(v) => push(ops, Op::Const(*v)),
            DensityFunction::Reference(id) => match self.registry.get(id) {
                Some(inner) => {
                    let address = inner as *const DensityFunction as usize;
                    if self.resolving.contains(&address) {
                        // A cycle. `oxide_datapack::resolve` reports these at load time; a
                        // constant here keeps compilation total instead of recursing forever.
                        return push(ops, Op::Const(0.0));
                    }
                    self.resolving.push(address);
                    let reg = self.emit(inner, ops);
                    self.resolving.pop();
                    reg
                }
                // Dangling references are reported by `oxide_datapack::resolve` at load time;
                // degrading to 0.0 keeps compilation total.
                None => push(ops, Op::Const(0.0)),
            },
            DensityFunction::Object(obj) => self.emit_object(obj, ops),
        }
    }

    /// A cache node's slot, assigned once per distinct node so that a node reached through two
    /// references shares one cache entry.
    fn slot_for(&mut self, obj: &DensityFunctionObject, kind: CacheKind) -> u32 {
        let address = obj as *const DensityFunctionObject as usize;
        if let Some(slot) = self.slots.get(&address) {
            return *slot;
        }
        let counter = match kind {
            CacheKind::Interpolated => &mut self.counts.interpolated,
            CacheKind::Cache2d | CacheKind::FlatCache => &mut self.counts.two_d,
            CacheKind::Once => &mut self.counts.once,
        };
        let slot = *counter as u32;
        *counter += 1;
        self.slots.insert(address, slot);
        slot
    }

    fn emit_cache(
        &mut self,
        obj: &DensityFunctionObject,
        kind: CacheKind,
        argument: &DensityFunction,
        ops: &mut Vec<Op>,
    ) -> u32 {
        let slot = self.slot_for(obj, kind);
        let argument = Box::new(self.compile(argument));
        push(
            ops,
            Op::Cache {
                kind,
                slot,
                argument,
            },
        )
    }

    fn emit_object(&mut self, obj: &DensityFunctionObject, ops: &mut Vec<Op>) -> u32 {
        use DensityFunctionObject::*;

        // A helper per arity keeps the emit order (arguments first, then the operator) uniform.
        macro_rules! unary {
            ($variant:expr, $argument:expr) => {{
                let a = self.emit($argument, ops);
                push(ops, $variant(a))
            }};
        }
        macro_rules! binary {
            ($variant:expr, $a:expr, $b:expr) => {{
                let a = self.emit($a, ops);
                let b = self.emit($b, ops);
                push(ops, $variant(a, b))
            }};
        }

        match obj {
            Constant { argument } => push(ops, Op::Const(*argument)),

            Noise {
                noise,
                xz_scale,
                y_scale,
            } => push(
                ops,
                Op::Noise(Box::new(NoiseOp {
                    noise: self.noise(noise),
                    xz_scale: *xz_scale,
                    y_scale: *y_scale,
                })),
            ),
            ShiftedNoise {
                noise,
                xz_scale,
                y_scale,
                shift_x,
                shift_y,
                shift_z,
            } => {
                let sx = self.emit(shift_x, ops);
                let sy = self.emit(shift_y, ops);
                let sz = self.emit(shift_z, ops);
                push(
                    ops,
                    Op::ShiftedNoise(Box::new(ShiftedNoiseOp {
                        noise: self.noise(noise),
                        xz_scale: *xz_scale,
                        y_scale: *y_scale,
                        shift_x: sx,
                        shift_y: sy,
                        shift_z: sz,
                    })),
                )
            }
            Shift { argument } => push(
                ops,
                Op::Shift {
                    noise: self.noise(argument),
                    kind: ShiftKind::Xyz,
                },
            ),
            ShiftA { argument } => push(
                ops,
                Op::Shift {
                    noise: self.noise(argument),
                    kind: ShiftKind::Xz,
                },
            ),
            ShiftB { argument } => push(
                ops,
                Op::Shift {
                    noise: self.noise(argument),
                    kind: ShiftKind::Zx,
                },
            ),
            OldBlendedNoise {
                xz_scale,
                y_scale,
                xz_factor,
                y_factor,
                smear_scale_multiplier,
            } => push(
                ops,
                Op::OldBlendedNoise(Box::new(BlendedNoiseParams {
                    xz_scale: *xz_scale,
                    y_scale: *y_scale,
                    xz_factor: *xz_factor,
                    y_factor: *y_factor,
                    smear_scale_multiplier: *smear_scale_multiplier,
                })),
            ),

            // Scope cuts, returning a constant rather than a fabricated value: `end_islands` is
            // a distinct sub-algorithm (wave 2 per `docs/ROADMAP.md`), and `beardifier` needs
            // structure piece layout, which `oxide-structures` does not produce yet.
            EndIslands {} => push(ops, Op::Const(0.0)),
            Beardifier {} => push(ops, Op::Const(0.0)),
            // No legacy-chunk blending is in scope (see `docs/ARCHITECTURE.md`): alpha is 1 and
            // offset 0 everywhere, which makes `blend_density` exactly a passthrough.
            BlendAlpha {} => push(ops, Op::Const(1.0)),
            BlendOffset {} => push(ops, Op::Const(0.0)),
            BlendDensity { argument } => self.emit(argument, ops),

            WeirdScaledSampler {
                input,
                noise,
                rarity_value_mapper,
            } => {
                let input = self.emit(input, ops);
                push(
                    ops,
                    Op::WeirdScaledSampler(Box::new(WeirdScaledSamplerOp {
                        input,
                        noise: self.noise(noise),
                        mapper: *rarity_value_mapper,
                    })),
                )
            }

            Interpolated { argument } => {
                self.emit_cache(obj, CacheKind::Interpolated, argument, ops)
            }
            FlatCache { argument } => self.emit_cache(obj, CacheKind::FlatCache, argument, ops),
            Cache2d { argument } => self.emit_cache(obj, CacheKind::Cache2d, argument, ops),
            CacheOnce { argument } | CacheAllInCell { argument } => {
                self.emit_cache(obj, CacheKind::Once, argument, ops)
            }

            Add {
                argument1,
                argument2,
            } => binary!(Op::Add, argument1, argument2),
            Mul {
                argument1,
                argument2,
            } => binary!(Op::Mul, argument1, argument2),
            Min {
                argument1,
                argument2,
            } => binary!(Op::Min, argument1, argument2),
            Max {
                argument1,
                argument2,
            } => binary!(Op::Max, argument1, argument2),

            Abs { argument } => unary!(Op::Abs, argument),
            Square { argument } => unary!(Op::Square, argument),
            Cube { argument } => unary!(Op::Cube, argument),
            HalfNegative { argument } => unary!(Op::HalfNegative, argument),
            QuarterNegative { argument } => unary!(Op::QuarterNegative, argument),
            Squeeze { argument } => unary!(Op::Squeeze, argument),
            Invert { argument } => unary!(Op::Invert, argument),

            Clamp { input, min, max } => {
                let input = self.emit(input, ops);
                push(
                    ops,
                    Op::Clamp(Box::new(ClampOp {
                        input,
                        min: *min,
                        max: *max,
                    })),
                )
            }

            YClampedGradient {
                from_y,
                to_y,
                from_value,
                to_value,
            } => push(
                ops,
                Op::YClampedGradient(Box::new(YClampedGradientOp {
                    from_y: *from_y,
                    to_y: *to_y,
                    from_value: *from_value,
                    to_value: *to_value,
                })),
            ),

            RangeChoice {
                input,
                min_inclusive,
                max_exclusive,
                when_in_range,
                when_out_of_range,
            } => {
                let input = self.emit(input, ops);
                let when_in_range = self.compile(when_in_range);
                let when_out_of_range = self.compile(when_out_of_range);
                push(
                    ops,
                    Op::RangeChoice(Box::new(RangeChoiceOp {
                        input,
                        min_inclusive: *min_inclusive,
                        max_exclusive: *max_exclusive,
                        when_in_range,
                        when_out_of_range,
                    })),
                )
            }
            IntervalSelect {
                input,
                thresholds,
                functions,
            } => {
                let input = self.emit(input, ops);
                let functions = functions.iter().map(|f| self.compile(f)).collect();
                push(
                    ops,
                    Op::IntervalSelect(Box::new(IntervalSelectOp {
                        input,
                        thresholds: thresholds.clone(),
                        functions,
                    })),
                )
            }
            FindTopSurface {
                cell_height,
                lower_bound,
                upper_bound,
                density,
            } => {
                let upper_bound = self.emit(upper_bound, ops);
                let density = self.compile(density);
                push(
                    ops,
                    Op::FindTopSurface(Box::new(FindTopSurfaceOp {
                        cell_height: *cell_height,
                        lower_bound: *lower_bound,
                        upper_bound,
                        density,
                    })),
                )
            }

            Spline { spline } => {
                let spline = self.compile_spline(spline);
                push(ops, Op::Spline(Box::new(spline)))
            }
        }
    }

    fn compile_spline(&mut self, spline: &CubicSpline) -> CompiledSpline {
        CompiledSpline {
            coordinate: self.compile(&spline.coordinate),
            points: spline
                .points
                .iter()
                .map(|p| CompiledSplinePoint {
                    location: p.location,
                    value: match &p.value {
                        SplineValue::Constant(c) => CompiledSplineValue::Constant(*c),
                        SplineValue::Spline(s) => {
                            CompiledSplineValue::Spline(Box::new(self.compile_spline(s)))
                        }
                    },
                    derivative: p.derivative,
                })
                .collect(),
        }
    }
}

fn push(ops: &mut Vec<Op>, op: Op) -> u32 {
    ops.push(op);
    (ops.len() - 1) as u32
}
