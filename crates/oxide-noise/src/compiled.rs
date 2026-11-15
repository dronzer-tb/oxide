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
    /// Whether [`Program::run_cell`] can evaluate this program for a whole cell at once: its
    /// only position-dependent leaves are `interpolated` caches and y gradients. True of
    /// `final_density` in every vanilla-shaped router, which is the program a chunk fill runs
    /// once per block.
    cell_batchable: bool,
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
                    fn walk<'a>(s: &'a CompiledSpline, nested: &mut Vec<(&'static str, &'a Program)>) {
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
        let mut inline = ([0.0f64; INLINE_REGISTERS], [0.0f64; INLINE_REGISTERS], [false; INLINE_REGISTERS]);
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
        let mut regs = vec![0.0f64; n * lanes];
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
                        let mut other = vec![0.0f64; lanes];
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
        let cell_batchable = is_cell_batchable(&ops);
        Program {
            ops,
            cell_batchable,
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

            Interpolated { argument } => {
                self.emit_cache(CacheKind::Interpolated, argument, ops)
            }
            FlatCache { argument } => self.emit_cache(CacheKind::FlatCache, argument, ops),
            Cache2d { argument } => self.emit_cache(CacheKind::Cache2d, argument, ops),
            CacheOnce { argument } | CacheAllInCell { argument } => {
                self.emit_cache(CacheKind::Once, argument, ops)
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
