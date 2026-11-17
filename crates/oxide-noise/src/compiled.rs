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

use std::cell::RefCell;
use std::collections::HashMap;

use oxide_datapack::{
    CubicSpline, DensityFunction, DensityFunctionObject, RarityValueMapper, Registry,
    ResourceLocation, SplineValue,
};

use crate::blended_noise::{BlendedNoise, BlendedNoiseParams};
use crate::cache::ChunkCaches;
use crate::density::{clamped_lerp, lerp_f32, map_rarity, FunctionContext};
use crate::normal_noise::NormalNoise;
use crate::scratch::Scratch;

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

    /// `c + g * body`, with `body` run only when `g` is not zero. See [`GatedAddOp`].
    GatedAdd(Box<GatedAddOp>),

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

/// `add(c, mul(g, body))` (either operand order in each node) where `c` is a nonzero finite
/// constant, `g` a `y_clamped_gradient` that reaches exactly 0.0, and `body` is proven finite.
///
/// When `g` is +/-0.0 the product is +/-0.0 (finite `body`), and `c + (+/-0.0)` is exactly `c`
/// for any nonzero `c`. So the result is `c` without evaluating `body`, bit for bit what the
/// plain `add`/`mul` pair computes. Otherwise `body` runs and the original expression is
/// evaluated with the original operand order. This is the overworld's top and bottom slides
/// around `final_density`'s interpolated argument: above y=256 and at y=-64, the whole
/// `sloped_cheese` and cave computation cannot change the value.
pub(crate) struct GatedAddOp {
    pub c: f64,
    /// `c` was `add`'s first operand.
    pub c_first: bool,
    /// Register holding the gradient's value.
    pub g: u32,
    /// The gradient was `mul`'s first operand.
    pub g_first: bool,
    pub body: Program,
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
    /// Whether [`Program::run_cell`] can evaluate this program for a whole cell at once: its
    /// only position-dependent leaves are `interpolated` caches and y gradients. True of
    /// `final_density` in every vanilla-shaped router, which is the program a chunk fill runs
    /// once per block.
    cell_batchable: bool,
    /// Whether [`Program::run_lanes`] evaluates this program an instruction at a time across
    /// all its lanes. False only for the two nodes that read a register per lane in a way the
    /// lane sweep does not model (`find_top_surface`, `weird_scaled_sampler`); such a program
    /// runs per lane instead, which is what it did before.
    lane_batchable: bool,
}

/// See [`Program::lane_batchable`].
fn is_lane_batchable(ops: &[Op]) -> bool {
    ops.iter().all(|op| match op {
        Op::FindTopSurface(_) | Op::WeirdScaledSampler(_) => false,
        // Nested programs are checked when they run: each falls back on its own.
        _ => true,
    })
}

/// A set of positions evaluated together by [`Program::run_lanes`], and the batch's own
/// `cache_once` memo.
///
/// `cache_once` is a pure cache -- it returns its argument's value at the position -- so a
/// memo per batch lane gives exactly the values the per-position one does, while letting the
/// argument run once for all lanes that miss.
pub(crate) struct Batch<'a> {
    pub ctxs: &'a [FunctionContext],
    /// `cache_once` slot -> one value per batch lane, [`UNSET`] bits when not taken. The
    /// per-slot buffers come from the thread's scratch pool and go back to it with the batch.
    once: RefCell<Vec<Scratch<u64>>>,
}

/// Marks a `Batch::once` entry as not yet computed. A signalling-NaN payload no density
/// computation produces; compared as bits, so a real NaN value is still stored faithfully.
const UNSET: u64 = 0x7ff4_dead_beef_0001;

impl<'a> Batch<'a> {
    pub(crate) fn new(ctxs: &'a [FunctionContext]) -> Self {
        Self {
            ctxs,
            once: RefCell::new(Vec::new()),
        }
    }
}

/// See [`Program::cell_batchable`]. Requires at least one `interpolated` leaf: a program
/// without one gains nothing from being evaluated a cell at a time.
fn is_cell_batchable(ops: &[Op]) -> bool {
    fn all_supported(ops: &[Op]) -> bool {
        ops.iter().all(|op| match op {
            Op::Const(_)
            | Op::YClampedGradient(_)
            | Op::Add(..)
            | Op::Mul(..)
            | Op::Min(..)
            | Op::Max(..)
            | Op::Abs(_)
            | Op::Square(_)
            | Op::Cube(_)
            | Op::HalfNegative(_)
            | Op::QuarterNegative(_)
            | Op::Squeeze(_)
            | Op::Invert(_)
            | Op::Clamp(_) => true,
            Op::Cache { kind, .. } => *kind == CacheKind::Interpolated,
            Op::RangeChoice(r) => {
                all_supported(&r.when_in_range.ops) && all_supported(&r.when_out_of_range.ops)
            }
            _ => false,
        })
    }
    fn has_interpolated(ops: &[Op]) -> bool {
        ops.iter().any(|op| match op {
            Op::Cache { kind, .. } => *kind == CacheKind::Interpolated,
            Op::RangeChoice(r) => {
                has_interpolated(&r.when_in_range.ops) || has_interpolated(&r.when_out_of_range.ops)
            }
            _ => false,
        })
    }
    all_supported(ops) && has_interpolated(ops)
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
    /// Appends a byte string that is equal for two programs exactly when they are structurally
    /// identical: same instructions, same constants bit for bit, same noises, same nested
    /// programs. Nested cache nodes contribute their `(kind, slot)`, which is already canonical
    /// because slots are themselves assigned by fingerprint, bottom up.
    fn fingerprint(&self, out: &mut Vec<u8>) {
        fn u(out: &mut Vec<u8>, v: u32) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        fn f(out: &mut Vec<u8>, v: f64) {
            out.extend_from_slice(&v.to_bits().to_le_bytes());
        }
        fn spline(out: &mut Vec<u8>, s: &CompiledSpline) {
            s.coordinate.fingerprint(out);
            u(out, s.points.len() as u32);
            for p in &s.points {
                u(out, p.location.to_bits());
                u(out, p.derivative.to_bits());
                match &p.value {
                    CompiledSplineValue::Constant(c) => {
                        out.push(0);
                        u(out, c.to_bits());
                    }
                    CompiledSplineValue::Spline(inner) => {
                        out.push(1);
                        spline(out, inner);
                    }
                }
            }
        }

        u(out, self.ops.len() as u32);
        for op in &self.ops {
            match op {
                Op::Const(v) => {
                    out.push(0);
                    f(out, *v);
                }
                Op::Noise(n) => {
                    out.push(1);
                    u(out, n.noise);
                    f(out, n.xz_scale);
                    f(out, n.y_scale);
                }
                Op::ShiftedNoise(n) => {
                    out.push(2);
                    u(out, n.noise);
                    f(out, n.xz_scale);
                    f(out, n.y_scale);
                    u(out, n.shift_x);
                    u(out, n.shift_y);
                    u(out, n.shift_z);
                }
                Op::Shift { noise, kind } => {
                    out.push(3);
                    u(out, *noise);
                    out.push(*kind as u8);
                }
                Op::OldBlendedNoise(p) => {
                    out.push(4);
                    f(out, p.xz_scale);
                    f(out, p.y_scale);
                    f(out, p.xz_factor);
                    f(out, p.y_factor);
                    f(out, p.smear_scale_multiplier);
                }
                Op::WeirdScaledSampler(w) => {
                    out.push(5);
                    u(out, w.input);
                    u(out, w.noise);
                    out.extend_from_slice(format!("{:?};", w.mapper).as_bytes());
                }
                Op::YClampedGradient(g) => {
                    out.push(6);
                    u(out, g.from_y as u32);
                    u(out, g.to_y as u32);
                    f(out, g.from_value);
                    f(out, g.to_value);
                }
                Op::Add(a, b) | Op::Mul(a, b) | Op::Min(a, b) | Op::Max(a, b) => {
                    out.push(match op {
                        Op::Add(..) => 7,
                        Op::Mul(..) => 8,
                        Op::Min(..) => 9,
                        _ => 10,
                    });
                    u(out, *a);
                    u(out, *b);
                }
                Op::Abs(a)
                | Op::Square(a)
                | Op::Cube(a)
                | Op::HalfNegative(a)
                | Op::QuarterNegative(a)
                | Op::Squeeze(a)
                | Op::Invert(a) => {
                    out.push(match op {
                        Op::Abs(_) => 11,
                        Op::Square(_) => 12,
                        Op::Cube(_) => 13,
                        Op::HalfNegative(_) => 14,
                        Op::QuarterNegative(_) => 15,
                        Op::Squeeze(_) => 16,
                        _ => 17,
                    });
                    u(out, *a);
                }
                Op::Clamp(c) => {
                    out.push(18);
                    u(out, c.input);
                    f(out, c.min);
                    f(out, c.max);
                }
                Op::Spline(s) => {
                    out.push(19);
                    spline(out, s);
                }
                Op::RangeChoice(r) => {
                    out.push(20);
                    u(out, r.input);
                    f(out, r.min_inclusive);
                    f(out, r.max_exclusive);
                    r.when_in_range.fingerprint(out);
                    r.when_out_of_range.fingerprint(out);
                }
                Op::IntervalSelect(s) => {
                    out.push(21);
                    u(out, s.input);
                    u(out, s.thresholds.len() as u32);
                    for t in &s.thresholds {
                        f(out, *t);
                    }
                    u(out, s.functions.len() as u32);
                    for program in &s.functions {
                        program.fingerprint(out);
                    }
                }
                Op::FindTopSurface(t) => {
                    out.push(22);
                    u(out, t.cell_height as u32);
                    u(out, t.lower_bound as u32);
                    u(out, t.upper_bound);
                    t.density.fingerprint(out);
                }
                Op::GatedAdd(g) => {
                    out.push(24);
                    f(out, g.c);
                    out.push(g.c_first as u8);
                    u(out, g.g);
                    out.push(g.g_first as u8);
                    g.body.fingerprint(out);
                }
                Op::Cache { kind, slot, .. } => {
                    out.push(23);
                    out.push(*kind as u8);
                    u(out, *slot);
                }
            }
        }
    }

    /// One line per instruction, for diagnosing why a shortcut does or does not apply. Nested
    /// programs (lazy branches, cache arguments, spline coordinates) are listed indented under
    /// the instruction that owns them.
    pub(crate) fn dump(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.dump_into(0, &mut out);
        out
    }

    fn dump_into(&self, depth: usize, out: &mut Vec<String>) {
        let pad = "  ".repeat(depth);
        for (i, op) in self.ops.iter().enumerate() {
            let mut nested: Vec<(&str, &Program)> = Vec::new();
            let text = match op {
                Op::Const(v) => format!("Const({v})"),
                Op::Noise(n) => format!("Noise(#{} xz={} y={})", n.noise, n.xz_scale, n.y_scale),
                Op::ShiftedNoise(n) => format!(
                    "ShiftedNoise(#{} xz={} y={} shift=r{},r{},r{})",
                    n.noise, n.xz_scale, n.y_scale, n.shift_x, n.shift_y, n.shift_z
                ),
                Op::Add(a, b) => format!("Add(r{a}, r{b})"),
                Op::Mul(a, b) => format!("Mul(r{a}, r{b})"),
                Op::Min(a, b) => format!("Min(r{a}, r{b})"),
                Op::Max(a, b) => format!("Max(r{a}, r{b})"),
                Op::Squeeze(a) => format!("Squeeze(r{a})"),
                Op::Clamp(c) => format!("Clamp(r{}, {}, {})", c.input, c.min, c.max),
                Op::Spline(s) => {
                    fn walk<'a>(
                        s: &'a CompiledSpline,
                        nested: &mut Vec<(&'static str, &'a Program)>,
                    ) {
                        nested.push(("coordinate", &s.coordinate));
                        for p in &s.points {
                            if let CompiledSplineValue::Spline(inner) = &p.value {
                                walk(inner, nested);
                            }
                        }
                    }
                    let mut all = Vec::new();
                    walk(s, &mut all);
                    let text = format!("Spline({} coordinate programs)", all.len());
                    // Listing every nested coordinate drowns the output; the first is enough to
                    // see what kind of program a spline coordinate is.
                    nested.extend(all.into_iter().take(1));
                    text
                }
                Op::RangeChoice(r) => {
                    nested.push(("in_range", &r.when_in_range));
                    nested.push(("out_of_range", &r.when_out_of_range));
                    format!(
                        "RangeChoice(r{} in [{}, {}))",
                        r.input, r.min_inclusive, r.max_exclusive
                    )
                }
                Op::Shift { noise, kind } => format!("Shift(#{noise} {kind:?})"),
                Op::OldBlendedNoise(_) => "OldBlendedNoise".to_string(),
                Op::WeirdScaledSampler(_) => "WeirdScaledSampler".to_string(),
                Op::YClampedGradient(g) => format!(
                    "YClampedGradient({}..{} -> {}..{})",
                    g.from_y, g.to_y, g.from_value, g.to_value
                ),
                Op::Abs(a) => format!("Abs(r{a})"),
                Op::Square(a) => format!("Square(r{a})"),
                Op::Cube(a) => format!("Cube(r{a})"),
                Op::HalfNegative(a) => format!("HalfNegative(r{a})"),
                Op::QuarterNegative(a) => format!("QuarterNegative(r{a})"),
                Op::Invert(a) => format!("Invert(r{a})"),
                Op::IntervalSelect(s) => {
                    for f in &s.functions {
                        nested.push(("function", f));
                    }
                    format!("IntervalSelect(r{} {:?})", s.input, s.thresholds)
                }
                Op::FindTopSurface(f) => {
                    nested.push(("density", &f.density));
                    format!(
                        "FindTopSurface(upper=r{} lower={} step={})",
                        f.upper_bound, f.lower_bound, f.cell_height
                    )
                }
                Op::GatedAdd(g) => {
                    nested.push(("body", &g.body));
                    format!("GatedAdd({} + r{} * body)", g.c, g.g)
                }
                Op::Cache {
                    kind,
                    slot,
                    argument,
                } => {
                    nested.push(("argument", argument));
                    format!("Cache(kind={kind:?}, slot={slot})")
                }
            };
            out.push(format!("{pad}{i:3}: {text}"));
            for (label, program) in nested {
                out.push(format!("{pad}     {label}:"));
                program.dump_into(depth + 3, out);
            }
        }
    }

    /// A conservative bound on this program's value across one whole cell, computed from the
    /// `interpolated` corner grids it reads, or `None` when no such bound can be proven.
    ///
    /// Why this is not simply "is the result an interpolated node": in a real overworld router
    /// `final_density` is `min(squeeze(interpolated(...)), noodle_caves)`, so the interpolated
    /// node is two monotonic wrappers deep and then combined with a second term. Interval
    /// arithmetic handles that: each op maps an input range to an output range, and anything
    /// this cannot bound returns `None` and simply gives up the shortcut.
    ///
    /// The bound is only ever *widened*, never narrowed, so a caller may trust `hi <= 0` to mean
    /// "no position in this cell is solid". It deliberately does not try to prove the opposite:
    /// `min` with an unbounded term (noodle caves) can pull any position down, so `lo > 0` for
    /// the interpolated term alone would NOT prove the cell is solid.
    pub(crate) fn cell_bounds(
        &self,
        caches: &ChunkCaches,
        cell_x: i32,
        cell_y: i32,
        cell_z: i32,
    ) -> Option<(f64, f64)> {
        let n = self.ops.len();
        // On the stack for every real program: this runs once per cell row of a chunk fill,
        // and three heap allocations per call were most of its cost.
        let mut inline = (
            [0.0f64; INLINE_REGISTERS],
            [0.0f64; INLINE_REGISTERS],
            [false; INLINE_REGISTERS],
        );
        let mut spilled;
        let (lo, hi, known): (&mut [f64], &mut [f64], &mut [bool]) = if n <= INLINE_REGISTERS {
            (&mut inline.0[..n], &mut inline.1[..n], &mut inline.2[..n])
        } else {
            spilled = (vec![0.0f64; n], vec![0.0f64; n], vec![false; n]);
            (&mut spilled.0, &mut spilled.1, &mut spilled.2)
        };

        for i in 0..n {
            let bounds: Option<(f64, f64)> = match &self.ops[i] {
                Op::Const(v) => Some((*v, *v)),

                // The whole point: a cell's corner grid bounds this node exactly.
                Op::Cache {
                    kind: CacheKind::Interpolated,
                    slot,
                    ..
                } => caches.cell_bounds(*slot as usize, cell_x, cell_y, cell_z),

                // Monotonic non-decreasing unary ops: bounds map straight through.
                Op::Squeeze(a) => {
                    let (l, h) = get(lo, hi, known, *a)?;
                    Some((squeeze(l), squeeze(h)))
                }
                Op::HalfNegative(a) => {
                    let (l, h) = get(lo, hi, known, *a)?;
                    Some((half_negative(l), half_negative(h)))
                }
                Op::QuarterNegative(a) => {
                    let (l, h) = get(lo, hi, known, *a)?;
                    Some((quarter_negative(l), quarter_negative(h)))
                }
                Op::Cube(a) => {
                    let (l, h) = get(lo, hi, known, *a)?;
                    Some((l * l * l, h * h * h))
                }
                Op::Clamp(c) => {
                    let (l, h) = get(lo, hi, known, c.input)?;
                    Some((l.clamp(c.min, c.max), h.clamp(c.min, c.max)))
                }

                Op::Add(a, b) => {
                    let (al, ah) = get(lo, hi, known, *a)?;
                    let (bl, bh) = get(lo, hi, known, *b)?;
                    Some((al + bl, ah + bh))
                }
                // `min` only ever lowers its result, so an upper bound on EITHER side is an
                // upper bound on the whole -- even if the other side cannot be bounded at all.
                // This is the case that matters: a real `final_density` is
                // `min(squeeze(interpolated(..)), noodle_caves)`, and the cave term is a lazy
                // `RangeChoice` this cannot bound. Without this asymmetry the whole shortcut
                // would never fire on any real overworld router.
                //
                // The lower bound is only known when both sides are, and is unused by the
                // caller (which claims "all air", never "all solid"); f64::NEG_INFINITY is the
                // honest value when one side is unknown.
                Op::Min(a, b) => {
                    let ba = get(lo, hi, known, *a);
                    let bb = get(lo, hi, known, *b);
                    match (ba, bb) {
                        (Some((al, ah)), Some((bl, bh))) => Some((al.min(bl), ah.min(bh))),
                        (Some((_, ah)), None) => Some((f64::NEG_INFINITY, ah)),
                        (None, Some((_, bh))) => Some((f64::NEG_INFINITY, bh)),
                        (None, None) => None,
                    }
                }
                // Mirror of `min`: `max` only ever raises, so a lower bound on either side
                // bounds the whole from below. The upper bound needs both.
                Op::Max(a, b) => {
                    let ba = get(lo, hi, known, *a);
                    let bb = get(lo, hi, known, *b);
                    match (ba, bb) {
                        (Some((al, ah)), Some((bl, bh))) => Some((al.max(bl), ah.max(bh))),
                        (Some((al, _)), None) => Some((al, f64::INFINITY)),
                        (None, Some((bl, _))) => Some((bl, f64::INFINITY)),
                        (None, None) => None,
                    }
                }
                Op::Mul(a, b) => {
                    let (al, ah) = get(lo, hi, known, *a)?;
                    let (bl, bh) = get(lo, hi, known, *b)?;
                    // Sign-agnostic: the extremes of a product are among the corner products.
                    let c = [al * bl, al * bh, ah * bl, ah * bh];
                    let mut mn = c[0];
                    let mut mx = c[0];
                    for v in c.iter().copied().skip(1) {
                        if v < mn {
                            mn = v;
                        }
                        if v > mx {
                            mx = v;
                        }
                    }
                    Some((mn, mx))
                }

                // Anything else -- raw noise, splines, lazy branches, other cache kinds --
                // is not bounded here. Giving up costs only the shortcut.
                _ => None,
            };

            match bounds {
                Some((l, h)) => {
                    lo[i] = l;
                    hi[i] = h;
                    known[i] = true;
                }
                None => known[i] = false,
            }
        }

        if known[n - 1] {
            Some((lo[n - 1], hi[n - 1]))
        } else {
            None
        }
    }

    pub(crate) fn cell_batchable(&self) -> bool {
        self.cell_batchable
    }

    /// Whether this program's value is a function of x and z alone: bit-identical at every
    /// `ctx.y`, through any chunk's caches in any state, or through none.
    ///
    /// [`cache_pure`] makes every cache node return exactly its argument's value (no
    /// `interpolated` node, which would read this chunk's corner grid; 2D caches only over
    /// y-independent arguments, so the y they were first filled at cannot matter). Under that,
    /// [`y_independent_ops`] holds for the result: `ctx.y` only enters through the leaves it
    /// rejects. Floating point is deterministic for identical inputs, so identical inputs at
    /// every instruction give identical bits, NaN payloads included.
    /// See [`quart_invariant_ops`]: through chunk caches, this program's value is the same at
    /// every block of a quart column.
    pub(crate) fn quart_invariant(&self) -> bool {
        quart_invariant_ops(&self.ops)
            .last()
            .copied()
            .unwrap_or(false)
    }

    pub(crate) fn y_independent(&self) -> bool {
        cache_pure(&self.ops)
            && y_independent_ops(&self.ops)
                .last()
                .copied()
                .unwrap_or(false)
    }

    /// Evaluates this program at every block of one cell of `caches`, writing `out` in the
    /// lane order [`ChunkCaches::fill_cell_interpolated`] defines. Only valid when
    /// [`Program::cell_batchable`] is true.
    ///
    /// Each lane sees exactly the arithmetic [`Program::run`] would do at that block -- the
    /// same operations on the same operands in the same order, no FMA, no reassociation -- so
    /// the values are bit-identical to per-block evaluation. What changes is that dispatch is
    /// paid once per instruction per cell, and the trilinear blend shares its x and y stages
    /// across the cell instead of redoing all seven lerps for every block.
    ///
    /// `range_choice` stays lazy at cell granularity: a branch no lane selects is never run, so
    /// the corner grids only that branch reads are still built only when needed.
    pub(crate) fn run_cell(
        &self,
        caches: &ChunkCaches,
        cell: (i32, i32, i32),
        cx: &RunCtx,
        out: &mut [f64],
    ) {
        fn unary(dst: &mut [f64], a: &[f64], f: impl Fn(f64) -> f64) {
            for (d, a) in dst.iter_mut().zip(a) {
                *d = f(*a);
            }
        }
        fn binary(dst: &mut [f64], a: &[f64], b: &[f64], f: impl Fn(f64, f64) -> f64) {
            for ((d, a), b) in dst.iter_mut().zip(a).zip(b) {
                *d = f(*a, *b);
            }
        }

        let lanes = out.len();
        let n = self.ops.len();
        // Every instruction below writes all `lanes` of its register before any later one
        // reads it (each arm fills `dst` completely; `range_choice` runs at least one branch,
        // whose own `run_cell` writes all of `dst`), so the stale contents are never seen.
        let mut regs = Scratch::overwritten(n * lanes, 0.0f64);
        for i in 0..n {
            let (done, rest) = regs.split_at_mut(i * lanes);
            let dst = &mut rest[..lanes];
            let reg = |r: u32| &done[r as usize * lanes..(r as usize + 1) * lanes];
            match &self.ops[i] {
                Op::Const(v) => dst.fill(*v),
                Op::Cache {
                    kind: CacheKind::Interpolated,
                    slot,
                    argument,
                } => caches.fill_cell_interpolated(*slot as usize, argument, cell, cx, dst),
                Op::YClampedGradient(g) => caches.fill_cell_by_y(cell.1, dst, |y| {
                    clamped_lerp(y, g.from_y, g.to_y, g.from_value, g.to_value)
                }),

                Op::Add(a, b) => binary(dst, reg(*a), reg(*b), |a, b| a + b),
                Op::Mul(a, b) => binary(dst, reg(*a), reg(*b), |a, b| a * b),
                Op::Min(a, b) => binary(dst, reg(*a), reg(*b), |a, b| a.min(b)),
                Op::Max(a, b) => binary(dst, reg(*a), reg(*b), |a, b| a.max(b)),

                Op::Abs(a) => unary(dst, reg(*a), |v| v.abs()),
                Op::Square(a) => unary(dst, reg(*a), |v| v * v),
                Op::Cube(a) => unary(dst, reg(*a), |v| v * v * v),
                Op::HalfNegative(a) => unary(dst, reg(*a), half_negative),
                Op::QuarterNegative(a) => unary(dst, reg(*a), quarter_negative),
                Op::Squeeze(a) => unary(dst, reg(*a), squeeze),
                Op::Invert(a) => unary(dst, reg(*a), |v| 1.0 / v),
                Op::Clamp(c) => unary(dst, reg(c.input), |v| v.clamp(c.min, c.max)),

                Op::RangeChoice(r) => {
                    let input = reg(r.input);
                    let inside = |v: f64| v >= r.min_inclusive && v < r.max_exclusive;
                    let any_in = input.iter().any(|v| inside(*v));
                    let any_out = input.iter().any(|v| !inside(*v));
                    if any_in {
                        r.when_in_range.run_cell(caches, cell, cx, dst);
                    }
                    if any_out && !any_in {
                        r.when_out_of_range.run_cell(caches, cell, cx, dst);
                    } else if any_out {
                        let mut other = Scratch::overwritten(lanes, 0.0f64);
                        r.when_out_of_range.run_cell(caches, cell, cx, &mut other);
                        for ((d, o), v) in dst.iter_mut().zip(&other).zip(input) {
                            if !inside(*v) {
                                *d = *o;
                            }
                        }
                    }
                }

                _ => unreachable!("run_cell on a program that is not cell_batchable"),
            }
        }
        out.copy_from_slice(&regs[(n - 1) * lanes..]);
    }

    /// Evaluates this program at `batch.ctxs[l]` for every `l` in `lanes`, writing `out[i]` for
    /// `lanes[i]`. Bit-identical to calling [`Program::run`] at each position: every lane sees
    /// the same operations on the same operands in the same order. What changes is that
    /// dispatch is paid per instruction rather than per instruction per position, and that the
    /// noise leaves loop over lanes inside their octave loops (reusing lattice hashes between
    /// lanes that share a cell).
    ///
    /// Lazy nodes stay lazy per lane: a `range_choice`, `interval_select` or gated-add body
    /// runs only for the lanes that select it.
    pub(crate) fn run_lanes(&self, lanes: &[u32], batch: &Batch, cx: &RunCtx, out: &mut [f64]) {
        debug_assert_eq!(lanes.len(), out.len());
        if !self.lane_batchable {
            for (o, &l) in out.iter_mut().zip(lanes) {
                *o = self.run(batch.ctxs[l as usize], cx);
            }
            return;
        }
        fn unary(dst: &mut [f64], a: &[f64], f: impl Fn(f64) -> f64) {
            for (d, a) in dst.iter_mut().zip(a) {
                *d = f(*a);
            }
        }
        fn binary(dst: &mut [f64], a: &[f64], b: &[f64], f: impl Fn(f64, f64) -> f64) {
            for ((d, a), b) in dst.iter_mut().zip(a).zip(b) {
                *d = f(*a, *b);
            }
        }
        fn noise_lanes(
            cx: &RunCtx,
            noise: u32,
            xs: &[f64],
            ys: &[f64],
            zs: &[f64],
            dst: &mut [f64],
        ) {
            match cx.noises.get(noise as usize) {
                Some(n) => n.get_values(xs, ys, zs, dst),
                None => dst.fill(0.0),
            }
        }

        let k = lanes.len();
        let n = self.ops.len();
        let pos = |i: usize| batch.ctxs[lanes[i] as usize];
        // Same values as collecting `pos(i).x as f64` and friends; only the storage is reused.
        let mut bx = Scratch::filled(k, 0.0f64);
        let mut by = Scratch::filled(k, 0.0f64);
        let mut bz = Scratch::filled(k, 0.0f64);
        for i in 0..k {
            let p = pos(i);
            bx[i] = p.x as f64;
            by[i] = p.y as f64;
            bz[i] = p.z as f64;
        }
        let mut sx = Scratch::filled(k, 0.0f64);
        let mut sy = Scratch::filled(k, 0.0f64);
        let mut sz = Scratch::filled(k, 0.0f64);
        let mut regs = Scratch::filled(n * k, 0.0f64);

        for i in 0..n {
            let (done, rest) = regs.split_at_mut(i * k);
            let dst = &mut rest[..k];
            let reg = |r: u32| &done[r as usize * k..(r as usize + 1) * k];
            match &self.ops[i] {
                Op::Const(v) => dst.fill(*v),

                Op::Noise(o) => {
                    for j in 0..k {
                        sx[j] = bx[j] * o.xz_scale;
                        sy[j] = by[j] * o.y_scale;
                        sz[j] = bz[j] * o.xz_scale;
                    }
                    noise_lanes(cx, o.noise, &sx, &sy, &sz, dst);
                }
                Op::ShiftedNoise(o) => {
                    let (rx, ry, rz) = (reg(o.shift_x), reg(o.shift_y), reg(o.shift_z));
                    for j in 0..k {
                        sx[j] = bx[j] * o.xz_scale + rx[j];
                        sy[j] = by[j] * o.y_scale + ry[j];
                        sz[j] = bz[j] * o.xz_scale + rz[j];
                    }
                    noise_lanes(cx, o.noise, &sx, &sy, &sz, dst);
                }
                Op::Shift { noise, kind } => {
                    for j in 0..k {
                        (sx[j], sy[j], sz[j]) = match kind {
                            ShiftKind::Xyz => (bx[j] * 0.25, by[j] * 0.25, bz[j] * 0.25),
                            ShiftKind::Xz => (bx[j] * 0.25, 0.0, bz[j] * 0.25),
                            ShiftKind::Zx => (bz[j] * 0.25, bx[j] * 0.25, 0.0),
                        };
                    }
                    noise_lanes(cx, *noise, &sx, &sy, &sz, dst);
                    for d in dst.iter_mut() {
                        *d *= 4.0;
                    }
                }
                Op::OldBlendedNoise(params) => cx.blended.compute_lanes(params, &bx, &by, &bz, dst),
                Op::YClampedGradient(g) => {
                    for (j, d) in dst.iter_mut().enumerate() {
                        *d = clamped_lerp(pos(j).y, g.from_y, g.to_y, g.from_value, g.to_value);
                    }
                }

                Op::Add(a, b) => binary(dst, reg(*a), reg(*b), |a, b| a + b),
                Op::Mul(a, b) => binary(dst, reg(*a), reg(*b), |a, b| a * b),
                Op::Min(a, b) => binary(dst, reg(*a), reg(*b), |a, b| a.min(b)),
                Op::Max(a, b) => binary(dst, reg(*a), reg(*b), |a, b| a.max(b)),
                Op::Abs(a) => unary(dst, reg(*a), |v| v.abs()),
                Op::Square(a) => unary(dst, reg(*a), |v| v * v),
                Op::Cube(a) => unary(dst, reg(*a), |v| v * v * v),
                Op::HalfNegative(a) => unary(dst, reg(*a), half_negative),
                Op::QuarterNegative(a) => unary(dst, reg(*a), quarter_negative),
                Op::Squeeze(a) => unary(dst, reg(*a), squeeze),
                Op::Invert(a) => unary(dst, reg(*a), |v| 1.0 / v),
                Op::Clamp(c) => unary(dst, reg(c.input), |v| v.clamp(c.min, c.max)),

                // Spline coordinates are themselves cached 2D lookups in every vanilla router;
                // the per-lane walk is the scalar one.
                Op::Spline(s) => {
                    for (j, d) in dst.iter_mut().enumerate() {
                        *d = eval_spline(s, pos(j), cx) as f64;
                    }
                }

                Op::RangeChoice(r) => {
                    let input = reg(r.input);
                    let (mut inside, mut outside) = (Scratch::empty(), Scratch::empty());
                    for (j, &v) in input.iter().enumerate() {
                        if v >= r.min_inclusive && v < r.max_exclusive {
                            inside.push(j);
                        } else {
                            outside.push(j);
                        }
                    }
                    run_subset(&r.when_in_range, &inside, lanes, batch, cx, dst);
                    run_subset(&r.when_out_of_range, &outside, lanes, batch, cx, dst);
                }
                Op::IntervalSelect(s) => {
                    let input = reg(s.input);
                    if s.functions.is_empty() {
                        dst.fill(0.0);
                    } else {
                        // Each lane's function index once; then each function, in function order,
                        // over its lanes in ascending order -- the same runs as one group per
                        // function, without a nested allocation per call.
                        let last = s.functions.len() - 1;
                        let mut index = Scratch::filled(k, 0usize);
                        for (j, &v) in input.iter().enumerate() {
                            index[j] = s
                                .thresholds
                                .partition_point(|threshold| v >= *threshold)
                                .min(last);
                        }
                        let mut group = Scratch::empty();
                        for (f, function) in s.functions.iter().enumerate() {
                            group.clear();
                            group.extend((0..k).filter(|&j| index[j] == f));
                            run_subset(function, &group, lanes, batch, cx, dst);
                        }
                    }
                }

                Op::GatedAdd(g) => {
                    let gate = reg(g.g);
                    let mut open = Scratch::empty();
                    open.extend((0..k).filter(|&j| gate[j] != 0.0));
                    let mut body = Scratch::filled(k, 0.0f64);
                    run_subset(&g.body, &open, lanes, batch, cx, &mut body);
                    for j in 0..k {
                        let gv = gate[j];
                        dst[j] = if gv == 0.0 {
                            g.c
                        } else {
                            let bv = body[j];
                            let product = if g.g_first { gv * bv } else { bv * gv };
                            if g.c_first {
                                g.c + product
                            } else {
                                product + g.c
                            }
                        };
                    }
                }

                Op::Cache {
                    kind: CacheKind::Once,
                    slot,
                    argument,
                } if cx.caches.is_some() => {
                    let slot = *slot as usize;
                    let mut missing = Scratch::empty();
                    {
                        let mut memo = batch.once.borrow_mut();
                        while memo.len() <= slot {
                            memo.push(Scratch::empty());
                        }
                        if memo[slot].is_empty() {
                            memo[slot] = Scratch::filled(batch.ctxs.len(), UNSET);
                        }
                        for (j, &l) in lanes.iter().enumerate() {
                            match memo[slot][l as usize] {
                                UNSET => missing.push(j),
                                bits => dst[j] = f64::from_bits(bits),
                            }
                        }
                    }
                    if !missing.is_empty() {
                        // Not borrowed across the run: the argument may reach other once slots.
                        run_subset(argument, &missing, lanes, batch, cx, dst);
                        let mut memo = batch.once.borrow_mut();
                        for &j in &missing {
                            memo[slot][lanes[j] as usize] = dst[j].to_bits();
                        }
                    }
                }
                Op::Cache {
                    kind,
                    slot,
                    argument,
                } => match cx.caches {
                    Some(caches) => {
                        for (j, d) in dst.iter_mut().enumerate() {
                            *d = caches.cached(*kind, *slot as usize, argument, pos(j), cx);
                        }
                    }
                    // Uncached, each node is its argument at the position.
                    None => argument.run_lanes(lanes, batch, cx, dst),
                },

                Op::FindTopSurface(_) | Op::WeirdScaledSampler(_) => {
                    unreachable!("run_lanes on a program that is not lane_batchable")
                }
            }
        }
        out.copy_from_slice(&regs[(n - 1) * k..]);
    }

    pub(crate) fn run(&self, ctx: FunctionContext, cx: &RunCtx) -> f64 {
        let n = self.ops.len();
        debug_assert!(n > 0, "a compiled program always ends in its result");
        let mut inline = [0.0f64; INLINE_REGISTERS];
        let mut spilled;
        let regs: &mut [f64] = if n <= INLINE_REGISTERS {
            &mut inline[..n]
        } else {
            spilled = Scratch::filled(n, 0.0f64);
            &mut spilled
        };

        for i in 0..n {
            let value = eval_op(&self.ops[i], regs, ctx, cx);
            regs[i] = value;
        }
        regs[n - 1]
    }
}

/// `squeeze`, exactly as `eval_op` computes it. Monotonic non-decreasing on [-1, 1]:
/// d/dv (v/2 - v^3/24) = 1/2 - v^2/8, which is positive for |v| <= 1, so bounds map end to end.
fn squeeze(v: f64) -> f64 {
    let v = v.clamp(-1.0, 1.0);
    v / 2.0 - v * v * v / 24.0
}

/// `half_negative`, exactly as `eval_op` computes it. Monotonic non-decreasing.
fn half_negative(v: f64) -> f64 {
    if v > 0.0 {
        v
    } else {
        v * 0.5
    }
}

/// `quarter_negative`, exactly as `eval_op` computes it. Monotonic non-decreasing.
fn quarter_negative(v: f64) -> f64 {
    if v > 0.0 {
        v
    } else {
        v * 0.25
    }
}

/// A register's proven bounds, or `None` if that register was not bounded.
fn get(lo: &[f64], hi: &[f64], known: &[bool], i: u32) -> Option<(f64, f64)> {
    let i = i as usize;
    if *known.get(i)? {
        Some((lo[i], hi[i]))
    } else {
        None
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

        Op::GatedAdd(g) => {
            let gv = regs[g.g as usize];
            if gv == 0.0 {
                g.c
            } else {
                let bv = g.body.run(ctx, cx);
                let product = if g.g_first { gv * bv } else { bv * gv };
                if g.c_first {
                    g.c + product
                } else {
                    product + g.c
                }
            }
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
    /// `(kind, argument fingerprint)` -> slot within its kind. Keyed by structure rather than
    /// by node address: a datapack reaches the same subtree (`sloped_cheese`, most expensively)
    /// through several distinct `cache_once` nodes, and vanilla's `NoiseChunk.wrap` shares one
    /// cache between structurally equal markers. Sharing never changes a value -- equal
    /// arguments are equal functions of position -- it only stops the subtree, blended noise
    /// and all, being evaluated once per marker at every cell corner.
    slots: HashMap<Vec<u8>, u32>,
    counts: CacheSlotCounts,
    /// `Reference` nodes currently being inlined, so a cyclic pack cannot recurse forever.
    resolving: Vec<usize>,
    /// Indexed like the noises: an upper bound on each noise's `|value|`. Empty means unknown,
    /// which makes every noise leaf unbounded and so disables [`GatedAddOp`].
    noise_bounds: Vec<f64>,
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
            noise_bounds: Vec::new(),
        }
    }

    /// Supplies per-noise magnitude bounds (see `NormalNoise::max_abs`), indexed like the
    /// noise indices this compiler assigns.
    pub(crate) fn with_noise_bounds(mut self, bounds: Vec<f64>) -> Self {
        self.noise_bounds = bounds;
        self
    }

    /// `df` with `Reference`s followed, as long as they resolve and do not cycle.
    fn resolved<'d>(&self, mut df: &'d DensityFunction) -> &'d DensityFunction
    where
        'a: 'd,
    {
        for _ in 0..64 {
            match df {
                DensityFunction::Reference(id) => match self.registry.get(id) {
                    Some(inner) => df = inner,
                    None => return df,
                },
                _ => return df,
            }
        }
        df
    }

    fn constant_of(&self, df: &DensityFunction) -> Option<f64> {
        match self.resolved(df) {
            DensityFunction::Constant(v) => Some(*v),
            DensityFunction::Object(obj) => match obj.as_ref() {
                DensityFunctionObject::Constant { argument } => Some(*argument),
                _ => None,
            },
            _ => None,
        }
    }

    /// Emits `add(a1, a2)` as a [`GatedAddOp`] when its shape and a finiteness proof allow;
    /// `None` leaves it to the plain lowering.
    fn try_emit_gated_add(
        &mut self,
        a1: &DensityFunction,
        a2: &DensityFunction,
        ops: &mut Vec<Op>,
    ) -> Option<u32> {
        let (c, c_first, mul) = match (self.constant_of(a1), self.constant_of(a2)) {
            (Some(c), _) => (c, true, a2),
            (_, Some(c)) => (c, false, a1),
            _ => return None,
        };
        if c == 0.0 || !c.is_finite() {
            return None;
        }
        let DensityFunction::Object(mul_obj) = self.resolved(mul) else {
            return None;
        };
        let DensityFunctionObject::Mul {
            argument1,
            argument2,
        } = mul_obj.as_ref()
        else {
            return None;
        };
        let reaches_zero = |df: &DensityFunction| match self.resolved(df) {
            DensityFunction::Object(o) => matches!(
                o.as_ref(),
                DensityFunctionObject::YClampedGradient { from_value, to_value, .. }
                    if *from_value == 0.0 || *to_value == 0.0
            ),
            _ => false,
        };
        let (gradient, body, g_first) = if reaches_zero(argument1) {
            (argument1, argument2, true)
        } else if reaches_zero(argument2) {
            (argument2, argument1, false)
        } else {
            return None;
        };

        // Emitted in the plain lowering's order (the constant has no cache slots, so where it
        // goes does not matter; the gradient and the body keep their relative order unless the
        // body came first, and the gradient has no slots either).
        let body = self.compile(body);
        if !self.max_abs(&body).is_some_and(|b| b < 1e290) {
            // Unprovable: lower plainly. The body was compiled for nothing, but its cache slots
            // are keyed by structure, so re-emitting it reuses exactly the same ones.
            return None;
        }
        let g = self.emit(gradient, ops);
        Some(push(
            ops,
            Op::GatedAdd(Box::new(GatedAddOp {
                c,
                c_first,
                g,
                g_first,
                body,
            })),
        ))
    }

    /// A conservative bound on `|value|` of `program` at any block position, or `None` when it
    /// may not be finite. Exact arithmetic is not needed: only finiteness is used, and every
    /// bound here is far above what it bounds.
    fn max_abs(&self, program: &Program) -> Option<f64> {
        // Block coordinates are i32.
        const COORD: f64 = 2.2e9;
        let noise = |index: u32| self.noise_bounds.get(index as usize).copied();
        let scaled_ok = |scale: f64| scale.is_finite() && scale.abs() < 1e100;
        let mut b: Vec<Option<f64>> = Vec::with_capacity(program.ops.len());
        for op in &program.ops {
            let r = |i: &u32| b.get(*i as usize).copied().flatten();
            let v: Option<f64> = match op {
                Op::Const(v) => v.is_finite().then(|| v.abs()),
                Op::Noise(n) => {
                    if scaled_ok(n.xz_scale) && scaled_ok(n.y_scale) {
                        noise(n.noise)
                    } else {
                        None
                    }
                }
                Op::ShiftedNoise(n) => {
                    let shifts = r(&n.shift_x)
                        .zip(r(&n.shift_y))
                        .zip(r(&n.shift_z))
                        .map(|((x, y), z)| x.max(y).max(z));
                    match shifts {
                        Some(s) if scaled_ok(n.xz_scale) && scaled_ok(n.y_scale) && s < 1e200 => {
                            noise(n.noise)
                        }
                        _ => None,
                    }
                }
                Op::Shift { noise: n, .. } => noise(*n).map(|v| v * 4.0),
                Op::OldBlendedNoise(p) => crate::blended_noise::blended_max_abs(p),
                Op::WeirdScaledSampler(_) | Op::Invert(_) => None,
                Op::YClampedGradient(g) => (g.from_value.is_finite() && g.to_value.is_finite())
                    .then(|| g.from_value.abs().max(g.to_value.abs())),
                Op::Add(x, y) => r(x).zip(r(y)).map(|(x, y)| x + y),
                Op::Mul(x, y) => r(x).zip(r(y)).map(|(x, y)| x * y),
                Op::Min(x, y) | Op::Max(x, y) => r(x).zip(r(y)).map(|(x, y)| x.max(y)),
                Op::Abs(x) | Op::HalfNegative(x) | Op::QuarterNegative(x) => r(x),
                Op::Square(x) => r(x).map(|x| x * x),
                Op::Cube(x) => r(x).map(|x| x * x * x),
                // Clamped to [-1, 1] first (a finite input is never NaN), then
                // |v/2 - v^3/24| <= 1/2 + 1/24.
                Op::Squeeze(x) => r(x).map(|_| 0.6),
                Op::Clamp(c) => r(&c.input).and_then(|_| {
                    (c.min.is_finite() && c.max.is_finite()).then(|| c.min.abs().max(c.max.abs()))
                }),
                Op::Spline(s) => self.spline_max_abs(s),
                Op::RangeChoice(c) => self
                    .max_abs(&c.when_in_range)
                    .zip(self.max_abs(&c.when_out_of_range))
                    .map(|(x, y)| x.max(y)),
                Op::IntervalSelect(s) => s
                    .functions
                    .iter()
                    .try_fold(0.0f64, |acc, f| self.max_abs(f).map(|v| acc.max(v))),
                // A height: an i32 (saturating `floor() as i32` of the bound) or the lower bound.
                Op::FindTopSurface(_) => Some(COORD),
                Op::GatedAdd(g) => self
                    .max_abs(&g.body)
                    .zip(r(&g.g))
                    .map(|(body, gv)| g.c.abs() + gv * body),
                // Every cache returns its argument's value at some position (`interpolated`: a
                // convex blend of such values).
                Op::Cache { argument, .. } => self.max_abs(argument).map(|v| v * 1.001),
            };
            b.push(v.filter(|v| v.is_finite() && *v < 1e290));
        }
        b.last().copied().flatten()
    }

    /// Bound on a spline's `f32` value, from its coordinate's bound. Outside the points it
    /// extends linearly (`v + d * (pos - loc)`); inside, `t` is in `[0, 1)`, so
    /// `lerp(t, v0, v1)` stays within the values and `t * (1 - t) <= 1/4`.
    fn spline_max_abs(&self, s: &CompiledSpline) -> Option<f64> {
        let coordinate = self.max_abs(&s.coordinate)?;
        if coordinate > 1e30 {
            return None; // `as f32` could overflow to infinity.
        }
        let mut values = 0.0f64;
        let mut derivatives = 0.0f64;
        let mut locations = 0.0f64;
        for p in &s.points {
            if !p.location.is_finite() || !p.derivative.is_finite() {
                return None;
            }
            let v = match &p.value {
                CompiledSplineValue::Constant(c) => {
                    if !c.is_finite() {
                        return None;
                    }
                    c.abs() as f64
                }
                CompiledSplineValue::Spline(inner) => self.spline_max_abs(inner)?,
            };
            values = values.max(v);
            derivatives = derivatives.max(p.derivative.abs() as f64);
            locations = locations.max(p.location.abs() as f64);
        }
        let reach = coordinate + locations;
        let outside = values + derivatives * reach;
        let inside = values + 0.25 * (derivatives * 2.0 * locations + 2.0 * values);
        let bound = outside.max(inside) * 1.01;
        // Kept well inside f32 so no intermediate (`v1 - v0`, `d * span`) can overflow.
        (bound < 1e30).then_some(bound)
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
        let cell_batchable = is_cell_batchable(&ops);
        let lane_batchable = is_lane_batchable(&ops);
        Program {
            ops,
            cell_batchable,
            lane_batchable,
        }
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

    /// A cache node's slot, assigned once per distinct `(kind, argument)` so that equal cache
    /// nodes share one entry however the datapack reached them.
    fn slot_for(&mut self, kind: CacheKind, argument: &Program) -> u32 {
        let mut key = vec![kind as u8];
        argument.fingerprint(&mut key);
        if let Some(slot) = self.slots.get(&key) {
            return *slot;
        }
        let counter = match kind {
            CacheKind::Interpolated => &mut self.counts.interpolated,
            CacheKind::Cache2d | CacheKind::FlatCache => &mut self.counts.two_d,
            CacheKind::Once => &mut self.counts.once,
        };
        let slot = *counter as u32;
        *counter += 1;
        self.slots.insert(key, slot);
        slot
    }

    fn emit_cache(
        &mut self,
        kind: CacheKind,
        argument: &DensityFunction,
        ops: &mut Vec<Op>,
    ) -> u32 {
        let argument = Box::new(self.compile(argument));
        let slot = self.slot_for(kind, &argument);
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

            Interpolated { argument } => self.emit_cache(CacheKind::Interpolated, argument, ops),
            FlatCache { argument } => self.emit_cache(CacheKind::FlatCache, argument, ops),
            Cache2d { argument } => self.emit_cache(CacheKind::Cache2d, argument, ops),
            CacheOnce { argument } | CacheAllInCell { argument } => {
                self.emit_cache(CacheKind::Once, argument, ops)
            }

            Add {
                argument1,
                argument2,
            } => match self.try_emit_gated_add(argument1, argument2, ops) {
                Some(reg) => reg,
                None => binary!(Op::Add, argument1, argument2),
            },
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

/// For each instruction of `ops`, whether its value is provably the same at every `ctx.y` for
/// a fixed x and z -- bit for bit, not merely mathematically. Stated for evaluation through a
/// chunk's caches of a program that [`cache_pure`] accepts (see [`Program::column_invariant`]
/// for the whole argument).
///
/// Conservative: anything not listed reads y (gradients, 3D noise, blended noise,
/// `interpolated`) or is not worth proving (`weird_scaled_sampler`).
///
/// The one subtle leaf is `shifted_noise` with `y_scale == 0`: its y input is
/// `block_y * 0.0 + shift_y`, and `block_y * 0.0` is `+0.0` or `-0.0` depending on the sign of
/// `block_y`. Adding `shift_y` erases that sign exactly when `shift_y` is not `-0.0` and not NaN
/// (`-0.0 + 0.0 == +0.0` in round-to-nearest; any nonzero or infinite value absorbs a zero), so
/// that is only accepted with a constant `shift_y` known to be neither. Vanilla's 2D noises
/// (`continents`, `erosion`, `ridges`) are all of this shape with `shift_y` = 0. A plain `noise`
/// with `y_scale == 0` would pass the signed zero straight through and is not accepted.
///
/// `find_top_surface` reads `ctx.x`, `ctx.z` and its upper-bound register, never `ctx.y`: its
/// density program runs at the scan's own heights. So it is y-independent when its upper bound
/// is, whatever its density program reads.
///
/// Cache nodes are accepted when their whole argument is: `cache_2d`/`flat_cache` return the
/// argument at the caller's y and the (possibly snapped) column, or a value it gave at that
/// column at some other y; `cache_once` returns the argument at exactly the caller's position.
pub(crate) fn y_independent_ops(ops: &[Op]) -> Vec<bool> {
    fn whole(p: &Program) -> bool {
        y_independent_ops(&p.ops).iter().all(|v| *v)
    }
    fn spline(s: &CompiledSpline) -> bool {
        whole(&s.coordinate)
            && s.points.iter().all(|p| match &p.value {
                CompiledSplineValue::Constant(_) => true,
                CompiledSplineValue::Spline(inner) => spline(inner),
            })
    }
    let mut indep = vec![false; ops.len()];
    for (i, op) in ops.iter().enumerate() {
        let r = |a: &u32| indep.get(*a as usize).copied().unwrap_or(false);
        indep[i] = match op {
            Op::Const(_) => true,
            Op::Shift { kind, .. } => *kind != ShiftKind::Xyz,
            Op::ShiftedNoise(n) => {
                let sign_safe_y = match ops.get(n.shift_y as usize) {
                    Some(Op::Const(c)) => !c.is_nan() && c.to_bits() != (-0.0f64).to_bits(),
                    _ => false,
                };
                n.y_scale == 0.0 && sign_safe_y && r(&n.shift_x) && r(&n.shift_z)
            }
            Op::Add(a, b) | Op::Mul(a, b) | Op::Min(a, b) | Op::Max(a, b) => r(a) && r(b),
            Op::Abs(a)
            | Op::Square(a)
            | Op::Cube(a)
            | Op::HalfNegative(a)
            | Op::QuarterNegative(a)
            | Op::Squeeze(a)
            | Op::Invert(a) => r(a),
            Op::Clamp(c) => r(&c.input),
            Op::Spline(s) => spline(s),
            Op::RangeChoice(c) => {
                r(&c.input) && whole(&c.when_in_range) && whole(&c.when_out_of_range)
            }
            Op::IntervalSelect(s) => r(&s.input) && s.functions.iter().all(whole),
            Op::FindTopSurface(f) => r(&f.upper_bound),
            // The gradient never qualifies, so in practice this is false; stated in full so a
            // future y-independent gate would still be judged correctly.
            Op::GatedAdd(g) => r(&g.g) && whole(&g.body),
            Op::Cache { kind, argument, .. } => *kind != CacheKind::Interpolated && whole(argument),
            Op::Noise(_)
            | Op::OldBlendedNoise(_)
            | Op::WeirdScaledSampler(_)
            | Op::YClampedGradient(_) => false,
        };
    }
    indep
}

/// Whether every value `ops` computes through a chunk's caches is independent of which chunk's
/// caches they are and of what those caches already hold: the program gives the same bits
/// through any `ChunkCaches`, fresh or used, as it would through a fresh one.
///
/// Requires, at every depth (lazy branches, spline coordinates, `find_top_surface` densities,
/// cache arguments):
/// - no `interpolated` node: its value is a blend of *this chunk's* cell corners, and a
///   position outside the chunk is clamped into the chunk's corner grid;
/// - every `cache_2d`/`flat_cache` argument y-independent and itself pure: a column slot is
///   filled at whichever y first asks for it, and read back at every other y;
/// - every `cache_once` argument pure: its slot is keyed by the full position, so a hit
///   returns the argument at exactly that position.
///
/// Slot sharing between programs does not break this: two cache nodes share a slot only when
/// their arguments' fingerprints are equal, which is a full structural serialisation, so
/// whichever program filled a slot computed it with an identical argument.
/// For each instruction of `ops`, whether its value is provably the same at every block of a
/// quart column: through a chunk's caches, the value at `(x, y, z)` equals the value at
/// `(x & !3, y, z & !3)`, bit for bit.
///
/// `flat_cache` is what makes this hold: through chunk caches it samples its argument at the
/// quart origin, so it qualifies whatever that argument reads. Without chunk caches it
/// evaluates at the exact position and does not, so this is stated for `sample_in_chunk` only.
/// The other caches (`cache_2d`, `cache_once`) evaluate at the exact position and qualify only
/// when their argument does. `interpolated` blends within a cell and never qualifies. Noise
/// leaves read the exact x and z. Constants and y gradients do not read x or z at all.
pub(crate) fn quart_invariant_ops(ops: &[Op]) -> Vec<bool> {
    fn whole(p: &Program) -> bool {
        quart_invariant_ops(&p.ops).iter().all(|v| *v)
    }
    fn spline(s: &CompiledSpline) -> bool {
        whole(&s.coordinate)
            && s.points.iter().all(|p| match &p.value {
                CompiledSplineValue::Constant(_) => true,
                CompiledSplineValue::Spline(inner) => spline(inner),
            })
    }
    let mut inv = vec![false; ops.len()];
    for (i, op) in ops.iter().enumerate() {
        let r = |a: &u32| inv.get(*a as usize).copied().unwrap_or(false);
        inv[i] = match op {
            Op::Const(_) | Op::YClampedGradient(_) => true,
            Op::Noise(_)
            | Op::ShiftedNoise(_)
            | Op::Shift { .. }
            | Op::OldBlendedNoise(_)
            | Op::WeirdScaledSampler(_) => false,
            Op::Add(a, b) | Op::Mul(a, b) | Op::Min(a, b) | Op::Max(a, b) => r(a) && r(b),
            Op::Abs(a)
            | Op::Square(a)
            | Op::Cube(a)
            | Op::HalfNegative(a)
            | Op::QuarterNegative(a)
            | Op::Squeeze(a)
            | Op::Invert(a) => r(a),
            Op::Clamp(c) => r(&c.input),
            Op::Spline(s) => spline(s),
            Op::RangeChoice(c) => {
                r(&c.input) && whole(&c.when_in_range) && whole(&c.when_out_of_range)
            }
            Op::IntervalSelect(s) => r(&s.input) && s.functions.iter().all(whole),
            // Its density program runs at the caller's x and z.
            Op::FindTopSurface(f) => r(&f.upper_bound) && whole(&f.density),
            Op::GatedAdd(g) => r(&g.g) && whole(&g.body),
            Op::Cache { kind, argument, .. } => match kind {
                CacheKind::FlatCache => true,
                CacheKind::Interpolated => false,
                CacheKind::Cache2d | CacheKind::Once => whole(argument),
            },
        };
    }
    inv
}

pub(crate) fn cache_pure(ops: &[Op]) -> bool {
    fn spline(s: &CompiledSpline) -> bool {
        cache_pure(&s.coordinate.ops)
            && s.points.iter().all(|p| match &p.value {
                CompiledSplineValue::Constant(_) => true,
                CompiledSplineValue::Spline(inner) => spline(inner),
            })
    }
    ops.iter().all(|op| match op {
        Op::Cache { kind, argument, .. } => match kind {
            CacheKind::Interpolated => false,
            CacheKind::Cache2d | CacheKind::FlatCache => {
                cache_pure(&argument.ops) && y_independent_ops(&argument.ops).iter().all(|v| *v)
            }
            CacheKind::Once => cache_pure(&argument.ops),
        },
        Op::Spline(s) => spline(s),
        Op::RangeChoice(c) => {
            cache_pure(&c.when_in_range.ops) && cache_pure(&c.when_out_of_range.ops)
        }
        Op::IntervalSelect(s) => s.functions.iter().all(|p| cache_pure(&p.ops)),
        Op::FindTopSurface(f) => cache_pure(&f.density.ops),
        Op::GatedAdd(g) => cache_pure(&g.body.ops),
        _ => true,
    })
}

/// Runs `program` for the lanes `subset` (indices into `lanes`) and scatters the results into
/// `dst` at those indices.
fn run_subset(
    program: &Program,
    subset: &[usize],
    lanes: &[u32],
    batch: &Batch,
    cx: &RunCtx,
    dst: &mut [f64],
) {
    if subset.is_empty() {
        return;
    }
    if subset.len() == lanes.len() {
        program.run_lanes(lanes, batch, cx, dst);
        return;
    }
    let mut sub_lanes = Scratch::empty();
    sub_lanes.extend(subset.iter().map(|&j| lanes[j]));
    let mut values = Scratch::filled(subset.len(), 0.0f64);
    program.run_lanes(&sub_lanes, batch, cx, &mut values);
    for (&j, v) in subset.iter().zip(values.iter()) {
        dst[j] = *v;
    }
}

fn push(ops: &mut Vec<Op>, op: Op) -> u32 {
    ops.push(op);
    (ops.len() - 1) as u32
}
