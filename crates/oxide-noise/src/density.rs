//! Density-function tree evaluator. Deserialization lives in `oxide-datapack`
//! (`density_function.rs`); this is the "do not add `sample`/`compute` here" other half it
//! points at.
//!
//! Scope cuts (return a constant + are flagged below, rather than fabricated): `old_blended_noise`
//! and `end_islands` are each a distinct sub-algorithm outside "noise primitives + interpreter +
//! router" (wave 2 per `docs/ROADMAP.md`); `beardifier` depends on structure placement, which is
//! `oxide-structures` (wave 4, not built yet).

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
            // PARITY-CHECK: rarity thresholds/outputs and the `d * |noise(pos / d)|` shape are
            // reconstructed from memory of `WeirdScaledSampler` (used by the Amplified preset's
            // caves-of-chaos "clay bands" style terrain). Not verified against 26.2.
            let d = map_rarity(*rarity_value_mapper, input_value);
            d * sample_noise(noise, cx, bx / d, by / d, bz / d).abs()
        }

        FlatCache { argument }
        | Cache2d { argument }
        | CacheOnce { argument }
        | CacheAllInCell { argument }
        | Interpolated { argument } => {
            // Memoization/cell-interpolation is a performance concern for the real chunk fill
            // loop (`oxide-chunkgen`, wave 3), not a value-changing one for a single point
            // sample — passthrough preserves the correct value here.
            evaluate(argument, ctx, cx)
        }

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

        Clamp { input, min, max } => evaluate(input, ctx, cx).clamp(*min, *max),

        Spline { spline } => evaluate_spline(spline, ctx, cx) as f64,

        // PARITY-CHECK: `0.25` xz/y scale and `ShiftB`'s x/z argument swap are reconstructed
        // from memory of `ShiftNoise`/`ShiftA`/`ShiftB` (used for terrain-warping the
        // continentalness/erosion/... climate samples). Not verified against 26.2.
        Shift { argument } => sample_noise(argument, cx, bx * 0.25, by * 0.25, bz * 0.25),
        ShiftA { argument } => sample_noise(argument, cx, bx * 0.25, 0.0, bz * 0.25),
        ShiftB { argument } => sample_noise(argument, cx, bz * 0.25, bx * 0.25, 0.0),
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
/// `CubicSpline.Multipoint.apply`. PARITY-CHECK: reconstructed from memory; not verified
/// against 26.2. `f32` throughout matches the vanilla type (`ToFloatFunction`/`float[]` fields
/// in `SplinePoint`).
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
        }
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
