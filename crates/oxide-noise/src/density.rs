//! Density-function tree evaluator. Deserialization lives in `oxide-datapack`
//! (`density_function.rs`); this is the "do not add `sample`/`compute` here" other half it
//! points at.
//!
//! Scope cuts (return a constant + are flagged below, rather than fabricated): `old_blended_noise`
//! and `end_islands` are each a distinct sub-algorithm outside "noise primitives + interpreter +
//! router" (wave 2 per `docs/ROADMAP.md`); `beardifier` depends on real structure placement
//! output (`oxide-structures` only has placement-chunk selection so far, not piece layout).
//! `old_blended_noise`'s real algorithm (`BlendedNoise`) has been read from decompiled 26.2
//! source since this was written — it's a real, implementable algorithm (three `PerlinNoise`
//! instances built via the *legacy* init path plus `Mth.clampedLerp`), just still out of scope
//! for this wave.

use std::collections::HashMap;

use oxide_datapack::{
    CubicSpline, DensityFunction, DensityFunctionObject, RarityValueMapper, Registry,
    ResourceLocation, SplineValue,
};

use crate::normal_noise::NormalNoise;

/// Vanilla's `DensityFunction.FunctionContext`: the block position a density function is being
/// sampled at.
#[derive(Debug, Clone, Copy)]
pub struct FunctionContext {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Everything an evaluation needs beyond the tree itself: the density-function registry (to
/// resolve named `Reference`s) and a pre-built cache of `NormalNoise` instances keyed by their
/// `worldgen/noise` id (built once per world seed — see [`crate::router::NoiseRouterEvaluator`]).
pub struct EvalCtx<'a> {
    pub df_registry: &'a Registry<DensityFunction>,
    pub noises: &'a HashMap<ResourceLocation, NormalNoise>,
    /// Per-chunk caches. `None` for a one-off sample (a biome lookup, a test); `Some` while
    /// filling a chunk, which is what makes `interpolated`/`flat_cache`/`cache_2d` behave the
    /// way vanilla defines them instead of evaluating straight through. See `cache.rs`.
    pub caches: Option<&'a crate::cache::ChunkCaches>,
}

pub fn evaluate(df: &DensityFunction, ctx: FunctionContext, cx: &EvalCtx) -> f64 {
    match df {
        DensityFunction::Constant(v) => *v,
        DensityFunction::Reference(id) => match cx.df_registry.get(id) {
            Some(inner) => evaluate(inner, ctx, cx),
            // Dangling references are reported by `oxide_datapack::resolve` at load time;
            // degrading to 0.0 here keeps evaluation total instead of panicking mid-chunk.
            None => 0.0,
        },
        DensityFunction::Object(obj) => evaluate_object(obj, ctx, cx),
    }
}

fn sample_noise(id: &ResourceLocation, cx: &EvalCtx, x: f64, y: f64, z: f64) -> f64 {
    match cx.noises.get(id) {
        Some(n) => n.get_value(x, y, z),
        None => 0.0,
    }
}

fn evaluate_object(obj: &DensityFunctionObject, ctx: FunctionContext, cx: &EvalCtx) -> f64 {
    use DensityFunctionObject::*;
    let (bx, by, bz) = (ctx.x as f64, ctx.y as f64, ctx.z as f64);

    match obj {
        Constant { argument } => *argument,

        Noise {
            noise,
            xz_scale,
            y_scale,
        } => sample_noise(noise, cx, bx * xz_scale, by * y_scale, bz * xz_scale),

        ShiftedNoise {
            noise,
            xz_scale,
            y_scale,
            shift_x,
            shift_y,
            shift_z,
        } => {
            let dx = evaluate(shift_x, ctx, cx);
            let dy = evaluate(shift_y, ctx, cx);
            let dz = evaluate(shift_z, ctx, cx);
            sample_noise(
                noise,
                cx,
                bx * xz_scale + dx,
                by * y_scale + dy,
                bz * xz_scale + dz,
            )
        }

        // Scope cut — see module doc.
        OldBlendedNoise { .. } => 0.0,
        EndIslands {} => 0.0,
        Beardifier {} => 0.0,

        WeirdScaledSampler {
            input,
            noise,
            rarity_value_mapper,
        } => {
            let input_value = evaluate(input, ctx, cx);
            // Checked 2026-08-22: `minecraft:weird_scaled_sampler` does not exist in the real
            // 26.2 server's `DensityFunctions` at all (grepped the full decompiled class list —
            // no match), so this node type is never emitted by a real 26.2 datapack and this
            // branch is dead in practice. Left in place (harmless, and `oxide-datapack` still
            // models the JSON shape) rather than deleted, in case a future version reintroduces
            // it; formula/thresholds below remain an unverified reconstruction if that happens.
            let d = map_rarity(*rarity_value_mapper, input_value);
            d * sample_noise(noise, cx, bx / d, by / d, bz / d).abs()
        }

        // These five are what keep vanilla's generator cheap, and `interpolated` is also what
        // *defines* its terrain: the surface is the trilinear blend between cell corners, not
        // the tree evaluated at every block. Without a chunk cache to hang that on (a single
        // point sample, a test) they degrade to evaluating through, which is the right value
        // for every node except `interpolated` -- see cache.rs.
        Interpolated { argument } => match cx.caches {
            Some(caches) => caches.interpolated(argument, ctx, cx),
            None => evaluate(argument, ctx, cx),
        },
        FlatCache { argument } => match cx.caches {
            Some(caches) => caches.flat_cache(argument, ctx, cx),
            None => evaluate(argument, ctx, cx),
        },
        Cache2d { argument } => match cx.caches {
            Some(caches) => caches.cache_2d(argument, ctx, cx),
            None => evaluate(argument, ctx, cx),
        },
        CacheOnce { argument } | CacheAllInCell { argument } => match cx.caches {
            Some(caches) => caches.once(argument, ctx, cx),
            None => evaluate(argument, ctx, cx),
        },

        BlendDensity { argument } => {
            // No legacy-chunk blending is in scope (see `docs/ARCHITECTURE.md`); alpha=1,
            // offset=0 always, which is exactly a passthrough.
            evaluate(argument, ctx, cx)
        }
        BlendAlpha {} => 1.0,
        BlendOffset {} => 0.0,

        Add {
            argument1,
            argument2,
        } => evaluate(argument1, ctx, cx) + evaluate(argument2, ctx, cx),
        Mul {
            argument1,
            argument2,
        } => evaluate(argument1, ctx, cx) * evaluate(argument2, ctx, cx),
        Min {
            argument1,
            argument2,
        } => evaluate(argument1, ctx, cx).min(evaluate(argument2, ctx, cx)),
        Max {
            argument1,
            argument2,
        } => evaluate(argument1, ctx, cx).max(evaluate(argument2, ctx, cx)),

        Abs { argument } => evaluate(argument, ctx, cx).abs(),
        Square { argument } => {
            let v = evaluate(argument, ctx, cx);
            v * v
        }
        Cube { argument } => {
            let v = evaluate(argument, ctx, cx);
            v * v * v
        }
        HalfNegative { argument } => {
            let v = evaluate(argument, ctx, cx);
            if v > 0.0 {
                v
            } else {
                v * 0.5
            }
        }
        QuarterNegative { argument } => {
            let v = evaluate(argument, ctx, cx);
            if v > 0.0 {
                v
            } else {
                v * 0.25
            }
        }
        Squeeze { argument } => {
            let v = evaluate(argument, ctx, cx).clamp(-1.0, 1.0);
            v / 2.0 - v * v * v / 24.0
        }

        YClampedGradient {
            from_y,
            to_y,
            from_value,
            to_value,
        } => clamped_lerp(ctx.y, *from_y, *to_y, *from_value, *to_value),

        RangeChoice {
            input,
            min_inclusive,
            max_exclusive,
            when_in_range,
            when_out_of_range,
        } => {
            let v = evaluate(input, ctx, cx);
            if v >= *min_inclusive && v < *max_exclusive {
                evaluate(when_in_range, ctx, cx)
            } else {
                evaluate(when_out_of_range, ctx, cx)
            }
        }

        IntervalSelect {
            input,
            thresholds,
            functions,
        } => {
            let v = evaluate(input, ctx, cx);
            // The first threshold the input falls short of picks that slot; an input
            // past every threshold picks the last function. That's the `<` chain
            // vanilla hardcoded in the rarity tables this node replaced (see
            // `map_rarity` below, kept for `weird_scaled_sampler`), so `thresholds`
            // is read as ascending -- vanilla's own exports always are.
            let index = thresholds.partition_point(|threshold| v >= *threshold);
            match functions.get(index).or_else(|| functions.last()) {
                Some(function) => evaluate(function, ctx, cx),
                // functions is empty: a malformed node. 0.0 is what an absent
                // density function contributes everywhere else in this file.
                None => 0.0,
            }
        }

        // PARITY-CHECK: reciprocal, inferred from vanilla's own use of it
        // (`mul(0.2734375, invert(factor))` -- the reciprocal of `factor` the
        // pre-26.2 hardcoded formula took), not from a decompiled source. The
        // x == 0 case is likewise unpinned; f64 division gives +/-inf, which
        // propagates rather than silently reading as a plausible density.
        Invert { argument } => 1.0 / evaluate(argument, ctx, cx),

        FindTopSurface {
            cell_height,
            lower_bound,
            upper_bound,
            density,
        } => {
            // Topmost cell-aligned y whose density is positive, scanning down.
            // PARITY-CHECK: vanilla's tie-breaking and whether it scans down from
            // the top or up from the bottom is not pinned to a decompiled source;
            // this slot is not read by fill_chunk, so nothing generated today
            // depends on it (see NoiseRouterEvaluator::sample's callers).
            let top = evaluate(upper_bound, ctx, cx).floor() as i32;
            let step = (*cell_height).max(1);
            let mut y = top - top.rem_euclid(step);
            while y > *lower_bound {
                let at = FunctionContext {
                    x: ctx.x,
                    y,
                    z: ctx.z,
                };
                if evaluate(density, at, cx) > 0.0 {
                    return y as f64;
                }
                y -= step;
            }
            *lower_bound as f64
        }

        Clamp { input, min, max } => evaluate(input, ctx, cx).clamp(*min, *max),

        Spline { spline } => evaluate_spline(spline, ctx, cx) as f64,

        // Verified 2026-08-22 against decompiled `DensityFunctions.ShiftNoise`'s default
        // `compute` method: `offsetNoise.getValue(x*0.25, y*0.25, z*0.25) * 4.0` — the `*4.0`
        // was missing here before. `ShiftA`/`ShiftB` each call it with a permuted (x,y,z):
        // `Shift.compute` passes `(blockX, blockY, blockZ)` unchanged, `ShiftA.compute` passes
        // `(blockX, 0, blockZ)`, `ShiftB.compute` passes `(blockZ, blockX, 0)` — confirming the
        // x/z swap this crate already guessed for `ShiftB`.
        Shift { argument } => sample_noise(argument, cx, bx * 0.25, by * 0.25, bz * 0.25) * 4.0,
        ShiftA { argument } => sample_noise(argument, cx, bx * 0.25, 0.0, bz * 0.25) * 4.0,
        ShiftB { argument } => sample_noise(argument, cx, bz * 0.25, bx * 0.25, 0.0) * 4.0,
    }
}

fn clamped_lerp(y: i32, from_y: i32, to_y: i32, from_value: f64, to_value: f64) -> f64 {
    if y <= from_y {
        from_value
    } else if y >= to_y {
        to_value
    } else {
        let t = (y - from_y) as f64 / (to_y - from_y) as f64;
        from_value + t * (to_value - from_value)
    }
}

fn map_rarity(mapper: RarityValueMapper, value: f64) -> f64 {
    match mapper {
        RarityValueMapper::Type1 => {
            if value < -0.5 {
                0.75
            } else if value < 0.0 {
                1.0
            } else if value < 0.5 {
                1.5
            } else {
                2.0
            }
        }
        RarityValueMapper::Type2 => {
            if value < -0.75 {
                0.5
            } else if value < -0.5 {
                0.75
            } else if value < 0.5 {
                1.0
            } else if value < 0.75 {
                2.0
            } else {
                3.0
            }
        }
    }
}

/// Cubic Hermite spline with explicit per-point derivatives, matching vanilla's
/// `CubicSpline.Multipoint.apply`. Verified 2026-08-22 against decompiled `net.minecraft.util.
/// CubicSpline` — formula, `findIntervalStart`'s "last point with location <= input" semantics,
/// and the two-edge linear-extend cases outside the point range are all exact. `f32` throughout
/// matches the vanilla type (`BoundedFloatFunction`/`float[]` fields in `Multipoint`).
fn evaluate_spline(spline: &CubicSpline, ctx: FunctionContext, cx: &EvalCtx) -> f32 {
    let pos = evaluate(&spline.coordinate, ctx, cx) as f32;
    let points = &spline.points;
    debug_assert!(!points.is_empty(), "spline must have at least one point");

    let idx = points.iter().rposition(|p| p.location <= pos);

    let value_at = |v: &SplineValue| -> f32 {
        match v {
            SplineValue::Constant(c) => *c,
            SplineValue::Spline(s) => evaluate_spline(s, ctx, cx),
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

fn lerp_f32(t: f32, a: f32, b: f32) -> f32 {
    a + t * (b - a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_datapack::{DensityFunctionObject as O, SplinePoint};

    fn ctx() -> FunctionContext {
        FunctionContext { x: 1, y: 64, z: -1 }
    }

    fn empty_cx<'a>(
        df_registry: &'a Registry<DensityFunction>,
        noises: &'a HashMap<ResourceLocation, NormalNoise>,
    ) -> EvalCtx<'a> {
        EvalCtx {
            df_registry,
            noises,
            caches: None,
        }
    }

    /// Thresholds are boundaries between `functions[i]` and `functions[i+1]`: a
    /// value exactly on a threshold belongs to the interval above it, matching the
    /// `<` chain vanilla hardcoded before 26.2 made this a datapack node.
    #[test]
    fn interval_select_picks_by_interval() {
        let df_registry = Registry::default();
        let noises = HashMap::new();
        let cx = empty_cx(&df_registry, &noises);
        for (input, expected) in [
            (-1.0, 10.0),
            (-0.75, 20.0),
            (-0.5, 30.0),
            (0.0, 30.0),
            (0.5, 40.0),
            (0.75, 50.0),
            (1.0, 50.0),
        ] {
            let df = DensityFunction::Object(Box::new(O::IntervalSelect {
                input: DensityFunction::Constant(input),
                thresholds: vec![-0.75, -0.5, 0.5, 0.75],
                functions: vec![
                    DensityFunction::Constant(10.0),
                    DensityFunction::Constant(20.0),
                    DensityFunction::Constant(30.0),
                    DensityFunction::Constant(40.0),
                    DensityFunction::Constant(50.0),
                ],
            }));
            assert_eq!(evaluate(&df, ctx(), &cx), expected, "input {input}");
        }
    }

    #[test]
    fn invert_is_the_reciprocal_not_negation() {
        let df_registry = Registry::default();
        let noises = HashMap::new();
        let cx = empty_cx(&df_registry, &noises);
        let df = DensityFunction::Object(Box::new(O::Invert {
            argument: DensityFunction::Constant(4.0),
        }));
        assert_eq!(evaluate(&df, ctx(), &cx), 0.25);
    }

    /// `y_clamped_gradient` rises with y, so the scan finds the topmost
    /// cell-aligned y where it is still positive rather than the first one it
    /// meets from the bottom.
    #[test]
    fn find_top_surface_scans_down_in_cell_steps() {
        let df_registry = Registry::default();
        let noises = HashMap::new();
        let cx = empty_cx(&df_registry, &noises);
        let df = DensityFunction::Object(Box::new(O::FindTopSurface {
            cell_height: 8,
            lower_bound: -64,
            upper_bound: DensityFunction::Constant(320.0),
            // positive at and below y=64, negative above it
            density: DensityFunction::Object(Box::new(O::YClampedGradient {
                from_y: 64,
                to_y: 72,
                from_value: 1.0,
                to_value: -1.0,
            })),
        }));
        assert_eq!(evaluate(&df, ctx(), &cx), 64.0);
    }

    #[test]
    fn constant_evaluates_to_itself() {
        let df_registry = Registry::default();
        let noises = HashMap::new();
        let cx = empty_cx(&df_registry, &noises);
        let df = DensityFunction::Constant(3.5);
        assert_eq!(evaluate(&df, ctx(), &cx), 3.5);
    }

    #[test]
    fn add_and_mul_are_exact() {
        let df_registry = Registry::default();
        let noises = HashMap::new();
        let cx = empty_cx(&df_registry, &noises);
        let add = DensityFunction::Object(Box::new(O::Add {
            argument1: DensityFunction::Constant(2.0),
            argument2: DensityFunction::Constant(3.0),
        }));
        assert_eq!(evaluate(&add, ctx(), &cx), 5.0);

        let mul = DensityFunction::Object(Box::new(O::Mul {
            argument1: DensityFunction::Constant(2.0),
            argument2: DensityFunction::Constant(3.0),
        }));
        assert_eq!(evaluate(&mul, ctx(), &cx), 6.0);
    }

    #[test]
    fn clamp_bounds_the_input() {
        let df_registry = Registry::default();
        let noises = HashMap::new();
        let cx = empty_cx(&df_registry, &noises);
        let df = DensityFunction::Object(Box::new(O::Clamp {
            input: DensityFunction::Constant(100.0),
            min: -1.0,
            max: 1.0,
        }));
        assert_eq!(evaluate(&df, ctx(), &cx), 1.0);
    }

    #[test]
    fn y_clamped_gradient_interpolates_linearly() {
        let df_registry = Registry::default();
        let noises = HashMap::new();
        let cx = empty_cx(&df_registry, &noises);
        let df = DensityFunction::Object(Box::new(O::YClampedGradient {
            from_y: 0,
            to_y: 100,
            from_value: 0.0,
            to_value: 10.0,
        }));
        assert_eq!(
            evaluate(&df, FunctionContext { x: 0, y: 50, z: 0 }, &cx),
            5.0
        );
        assert_eq!(
            evaluate(&df, FunctionContext { x: 0, y: -10, z: 0 }, &cx),
            0.0
        );
        assert_eq!(
            evaluate(&df, FunctionContext { x: 0, y: 200, z: 0 }, &cx),
            10.0
        );
    }

    #[test]
    fn spline_passes_through_its_points() {
        let df_registry = Registry::default();
        let noises = HashMap::new();
        let cx = empty_cx(&df_registry, &noises);
        let spline = CubicSpline {
            coordinate: DensityFunction::Constant(0.0),
            points: vec![
                SplinePoint {
                    location: -1.0,
                    value: SplineValue::Constant(0.0),
                    derivative: 0.0,
                },
                SplinePoint {
                    location: 0.0,
                    value: SplineValue::Constant(5.0),
                    derivative: 0.0,
                },
                SplinePoint {
                    location: 1.0,
                    value: SplineValue::Constant(10.0),
                    derivative: 0.0,
                },
            ],
        };
        let df = DensityFunction::Object(Box::new(O::Spline { spline }));
        assert!((evaluate(&df, ctx(), &cx) - 5.0).abs() < 1e-6);
    }
}
