//! The position a density function is sampled at, and the scalar helpers the compiled
//! evaluator's node semantics are written in terms of.
//!
//! Deserialization lives in `oxide-datapack` (`density_function.rs`); lowering and evaluation
//! live in `compiled.rs`. This module holds only what both sides share, so there is exactly one
//! definition of each formula.

/// Vanilla's `DensityFunction.FunctionContext`: the block position a density function is being
/// sampled at.
#[derive(Debug, Clone, Copy)]
pub struct FunctionContext {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// `y_clamped_gradient`: linear in y between two heights, flat outside them.
pub(crate) fn clamped_lerp(y: i32, from_y: i32, to_y: i32, from_value: f64, to_value: f64) -> f64 {
    if y <= from_y {
        from_value
    } else if y >= to_y {
        to_value
    } else {
        let t = (y - from_y) as f64 / (to_y - from_y) as f64;
        from_value + t * (to_value - from_value)
    }
}

/// The rarity tables `weird_scaled_sampler` selects between. See the `PARITY-CHECK` note on
/// `Op::WeirdScaledSampler`: this node does not exist in a real 26.2 datapack.
pub(crate) fn map_rarity(mapper: oxide_datapack::RarityValueMapper, value: f64) -> f64 {
    use oxide_datapack::RarityValueMapper;
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

/// `f32` throughout matches vanilla's `CubicSpline` field types.
pub(crate) fn lerp_f32(t: f32, a: f32, b: f32) -> f32 {
    a + t * (b - a)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use oxide_datapack::{
        CubicSpline, DensityFunction, DensityFunctionObject as O, Registry, SplinePoint,
        SplineValue,
    };

    use super::FunctionContext;
    use crate::compiled::{Compiler, RunCtx};

    fn ctx() -> FunctionContext {
        FunctionContext { x: 1, y: 64, z: -1 }
    }

    /// One shared seed-0 instance: building it costs 40 `ImprovedNoise` constructions, and no
    /// test here depends on its values.
    fn test_blended() -> &'static crate::blended_noise::BlendedNoise {
        static BLENDED: std::sync::OnceLock<crate::blended_noise::BlendedNoise> =
            std::sync::OnceLock::new();
        BLENDED.get_or_init(|| {
            crate::blended_noise::BlendedNoise::new(&mut crate::random::WorldRandom::new(0, false))
        })
    }

    /// Compiles `df` against empty registries and runs it at `at`, with no chunk caches -- which
    /// is what a one-off sample does.
    fn eval(df: &DensityFunction, at: FunctionContext) -> f64 {
        let registry = Registry::default();
        let noise_index = HashMap::new();
        let program = Compiler::new(&registry, &noise_index).compile(df);
        program.run(
            at,
            &RunCtx {
                noises: &[],
                blended: test_blended(),
                caches: None,
            },
        )
    }

    /// Thresholds are boundaries between `functions[i]` and `functions[i+1]`: a value exactly on
    /// a threshold belongs to the interval above it, matching the `<` chain vanilla hardcoded
    /// before 26.2 made this a datapack node.
    #[test]
    fn interval_select_picks_by_interval() {
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
            assert_eq!(eval(&df, ctx()), expected, "input {input}");
        }
    }

    #[test]
    fn invert_is_the_reciprocal_not_negation() {
        let df = DensityFunction::Object(Box::new(O::Invert {
            argument: DensityFunction::Constant(4.0),
        }));
        assert_eq!(eval(&df, ctx()), 0.25);
    }

    /// `y_clamped_gradient` rises with y, so the scan finds the topmost cell-aligned y where it
    /// is still positive rather than the first one it meets from the bottom.
    #[test]
    fn find_top_surface_scans_down_in_cell_steps() {
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
        assert_eq!(eval(&df, ctx()), 64.0);
    }

    #[test]
    fn constant_evaluates_to_itself() {
        assert_eq!(eval(&DensityFunction::Constant(3.5), ctx()), 3.5);
    }

    #[test]
    fn add_and_mul_are_exact() {
        let add = DensityFunction::Object(Box::new(O::Add {
            argument1: DensityFunction::Constant(2.0),
            argument2: DensityFunction::Constant(3.0),
        }));
        assert_eq!(eval(&add, ctx()), 5.0);

        let mul = DensityFunction::Object(Box::new(O::Mul {
            argument1: DensityFunction::Constant(2.0),
            argument2: DensityFunction::Constant(3.0),
        }));
        assert_eq!(eval(&mul, ctx()), 6.0);
    }

    #[test]
    fn clamp_bounds_the_input() {
        let df = DensityFunction::Object(Box::new(O::Clamp {
            input: DensityFunction::Constant(100.0),
            min: -1.0,
            max: 1.0,
        }));
        assert_eq!(eval(&df, ctx()), 1.0);
    }

    #[test]
    fn y_clamped_gradient_interpolates_linearly() {
        let df = DensityFunction::Object(Box::new(O::YClampedGradient {
            from_y: 0,
            to_y: 100,
            from_value: 0.0,
            to_value: 10.0,
        }));
        assert_eq!(eval(&df, FunctionContext { x: 0, y: 50, z: 0 }), 5.0);
        assert_eq!(eval(&df, FunctionContext { x: 0, y: -10, z: 0 }), 0.0);
        assert_eq!(eval(&df, FunctionContext { x: 0, y: 200, z: 0 }), 10.0);
    }

    #[test]
    fn spline_passes_through_its_points() {
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
        assert!((eval(&df, ctx()) - 5.0).abs() < 1e-6);
    }

    /// Each cache node gets its own slot, and the count is what sizes a chunk's cache arrays.
    /// Nodes reached through two different references are one node in the registry, so they
    /// share a slot -- that path is covered by the generator's chunk-hash fingerprint rather
    /// than here, since a `Registry` cannot be populated outside `oxide-datapack`'s loader.
    #[test]
    fn each_cache_node_gets_its_own_slot() {
        let df = DensityFunction::Object(Box::new(O::Add {
            argument1: DensityFunction::Object(Box::new(O::Interpolated {
                argument: DensityFunction::Constant(1.0),
            })),
            argument2: DensityFunction::Object(Box::new(O::FlatCache {
                argument: DensityFunction::Object(Box::new(O::CacheOnce {
                    argument: DensityFunction::Constant(2.0),
                })),
            })),
        }));

        let registry = Registry::default();
        let noise_index = HashMap::new();
        let mut compiler = Compiler::new(&registry, &noise_index);
        let program = compiler.compile(&df);
        assert_eq!(compiler.counts().interpolated, 1);
        assert_eq!(compiler.counts().two_d, 1);
        assert_eq!(compiler.counts().once, 1);
        // With no chunk caches the cache nodes evaluate straight through.
        assert_eq!(
            program.run(
                ctx(),
                &RunCtx {
                    noises: &[],
                    blended: test_blended(),
                    caches: None,
                }
            ),
            3.0
        );
    }
}
