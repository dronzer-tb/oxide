//! Reference resolution and validation, run once after all registries load.
//!
//! Cycle detection uses standard DFS with a recursion-stack (white/gray/black
//! colouring), which is what correctly distinguishes a genuine reference
//! cycle (a back-edge to a node still on the current path) from a diamond —
//! the same named density function referenced from two different places,
//! which is the normal, legitimate shape produced by wrapping a shared
//! subexpression in a `cache_2d`/`flat_cache`/`interpolated`/... node so it's
//! evaluated once and reused. A naive "have I seen this id before anywhere"
//! set would misflag every diamond as a cycle; recursion-stack membership
//! does not.

use crate::biome::Biome;
use crate::density_function::{DensityFunction, DensityFunctionObject};
use crate::error::{DatapackError, Result};
use crate::multi_noise::MultiNoiseSource;
use crate::noise_param::NormalNoiseParameters;
use crate::noise_settings::NoiseGeneratorSettings;
use crate::registry::Registry;
use crate::surface_rule::{SurfaceCondition, SurfaceRule};
use oxide_core::ResourceLocation;
use std::collections::HashSet;

const DF_REGISTRY: &str = "worldgen/density_function";

/// Every dangling id and cycle found is reported (not just the first), so a
/// caller can fix a pack in one pass rather than one error at a time.
#[derive(Debug, Default)]
pub struct ResolutionReport {
    pub errors: Vec<DatapackError>,
}

impl ResolutionReport {
    pub fn into_result(self) -> Result<()> {
        match self.errors.into_iter().next() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RefKind {
    DensityFunction,
    Noise,
}

/// Walk `df`, calling `on_ref(kind, id)` for every embedded reference: a
/// named density function (`Reference` nodes) or a noise-parameter id (the
/// `noise`/`argument` fields on `noise`, `shifted_noise`,
/// `weird_scaled_sampler`, `shift`, `shift_a`, `shift_b`).
fn walk_density_function(
    df: &DensityFunction,
    on_ref: &mut impl FnMut(RefKind, &ResourceLocation),
) {
    match df {
        DensityFunction::Constant(_) => {}
        DensityFunction::Reference(id) => on_ref(RefKind::DensityFunction, id),
        DensityFunction::Object(obj) => walk_object(obj, on_ref),
    }
}

fn walk_object(obj: &DensityFunctionObject, on_ref: &mut impl FnMut(RefKind, &ResourceLocation)) {
    use DensityFunctionObject::*;
    match obj {
        Constant { .. } => {}
        Noise { noise, .. } => on_ref(RefKind::Noise, noise),
        ShiftedNoise {
            noise,
            shift_x,
            shift_y,
            shift_z,
            ..
        } => {
            on_ref(RefKind::Noise, noise);
            walk_density_function(shift_x, on_ref);
            walk_density_function(shift_y, on_ref);
            walk_density_function(shift_z, on_ref);
        }
        OldBlendedNoise { .. } => {}
        EndIslands {} => {}
        WeirdScaledSampler { input, noise, .. } => {
            on_ref(RefKind::Noise, noise);
            walk_density_function(input, on_ref);
        }
        FlatCache { argument }
        | Cache2d { argument }
        | CacheOnce { argument }
        | CacheAllInCell { argument }
        | Interpolated { argument }
        | BlendDensity { argument }
        | Abs { argument }
        | Square { argument }
        | Cube { argument }
        | HalfNegative { argument }
        | QuarterNegative { argument }
        | Squeeze { argument } => walk_density_function(argument, on_ref),
        BlendAlpha {} | BlendOffset {} | Beardifier {} => {}
        Add {
            argument1,
            argument2,
        }
        | Mul {
            argument1,
            argument2,
        }
        | Min {
            argument1,
            argument2,
        }
        | Max {
            argument1,
            argument2,
        } => {
            walk_density_function(argument1, on_ref);
            walk_density_function(argument2, on_ref);
        }
        YClampedGradient { .. } => {}
        RangeChoice {
            input,
            when_in_range,
            when_out_of_range,
            ..
        } => {
            walk_density_function(input, on_ref);
            walk_density_function(when_in_range, on_ref);
            walk_density_function(when_out_of_range, on_ref);
        }
        Clamp { input, .. } => walk_density_function(input, on_ref),
        Spline { spline } => walk_spline(spline, on_ref),
        Shift { argument } | ShiftA { argument } | ShiftB { argument } => {
            on_ref(RefKind::Noise, argument)
        }
    }
}

fn walk_spline(
    spline: &crate::density_function::CubicSpline,
    on_ref: &mut impl FnMut(RefKind, &ResourceLocation),
) {
    walk_density_function(&spline.coordinate, on_ref);
    for point in &spline.points {
        if let crate::density_function::SplineValue::Spline(inner) = &point.value {
            walk_spline(inner, on_ref);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Color {
    White,
    Gray,
    Black,
}

/// Validate every `Reference` in the density_function registry resolves, and
/// that the reference graph among registry entries has no cycles.
pub fn resolve_density_functions(
    registry: &Registry<DensityFunction>,
    report: &mut ResolutionReport,
) {
    // Dangling reference check (independent of cycle detection).
    for (id, df) in registry.iter() {
        walk_density_function(df, &mut |kind, target| {
            if kind == RefKind::DensityFunction && !registry.contains(target) {
                report.errors.push(DatapackError::DanglingReference {
                    registry: DF_REGISTRY,
                    referrer: id.to_string(),
                    kind: "density_function",
                    target: target.to_string(),
                });
            }
        });
    }

    // Cycle detection via DFS with recursion-stack colouring.
    let mut colors: std::collections::HashMap<ResourceLocation, Color> = registry
        .iter()
        .map(|(id, _)| (id.clone(), Color::White))
        .collect();
    let mut path: Vec<ResourceLocation> = Vec::new();

    let ids: Vec<ResourceLocation> = registry.iter().map(|(id, _)| id.clone()).collect();
    for id in &ids {
        if colors.get(id) == Some(&Color::White) {
            visit(id, registry, &mut colors, &mut path, report);
        }
    }
}

fn visit(
    id: &ResourceLocation,
    registry: &Registry<DensityFunction>,
    colors: &mut std::collections::HashMap<ResourceLocation, Color>,
    path: &mut Vec<ResourceLocation>,
    report: &mut ResolutionReport,
) {
    colors.insert(id.clone(), Color::Gray);
    path.push(id.clone());

    if let Some(df) = registry.get(id) {
        let mut children: Vec<ResourceLocation> = Vec::new();
        walk_density_function(df, &mut |kind, target| {
            if kind == RefKind::DensityFunction {
                children.push(target.clone());
            }
        });

        for child in children {
            if !registry.contains(&child) {
                // Dangling reference already reported above; skip to avoid
                // treating a missing node as either a cycle or a no-op.
                continue;
            }
            match colors.get(&child).copied().unwrap_or(Color::White) {
                Color::White => visit(&child, registry, colors, path, report),
                Color::Gray => {
                    // Back-edge: genuine cycle. Report the path from the
                    // cycle's start back to itself.
                    let start = path.iter().position(|n| *n == child).unwrap_or(0);
                    let mut cycle: Vec<String> =
                        path[start..].iter().map(|n| n.to_string()).collect();
                    cycle.push(child.to_string());
                    report.errors.push(DatapackError::Cycle {
                        registry: DF_REGISTRY,
                        cycle: cycle.join(" -> "),
                    });
                }
                Color::Black => {}
            }
        }
    }

    path.pop();
    colors.insert(id.clone(), Color::Black);
}

/// Validate density-function-embedded noise references against the noise
/// registry.
pub fn resolve_noise_refs_in_density_functions(
    df_registry: &Registry<DensityFunction>,
    noise_registry: &Registry<NormalNoiseParameters>,
    report: &mut ResolutionReport,
) {
    for (id, df) in df_registry.iter() {
        walk_density_function(df, &mut |kind, target| {
            if kind == RefKind::Noise && !noise_registry.contains(target) {
                report.errors.push(DatapackError::DanglingReference {
                    registry: DF_REGISTRY,
                    referrer: id.to_string(),
                    kind: "noise",
                    target: target.to_string(),
                });
            }
        });
    }
}

/// Validate a `noise_settings` entry's embedded density-function trees
/// (the noise router slots) against both registries, and its surface rule's
/// noise-threshold references against the noise registry.
pub fn resolve_noise_settings(
    id: &ResourceLocation,
    settings: &NoiseGeneratorSettings,
    df_registry: &Registry<DensityFunction>,
    noise_registry: &Registry<NormalNoiseParameters>,
    report: &mut ResolutionReport,
) {
    let router = &settings.noise_router;
    let slots = [
        &router.barrier,
        &router.fluid_level_floodedness,
        &router.fluid_level_spread,
        &router.lava,
        &router.temperature,
        &router.vegetation,
        &router.continents,
        &router.erosion,
        &router.depth,
        &router.ridges,
        &router.initial_density_without_jaggedness,
        &router.final_density,
        &router.vein_toggle,
        &router.vein_ridged,
        &router.vein_gap,
    ];
    for slot in slots {
        walk_density_function(slot, &mut |kind, target| {
            let (registry_ok, kind_name) = match kind {
                RefKind::DensityFunction => (df_registry.contains(target), "density_function"),
                RefKind::Noise => (noise_registry.contains(target), "noise"),
            };
            if !registry_ok {
                report.errors.push(DatapackError::DanglingReference {
                    registry: "worldgen/noise_settings",
                    referrer: id.to_string(),
                    kind: kind_name,
                    target: target.to_string(),
                });
            }
        });
    }

    walk_surface_rule(&settings.surface_rule, &mut |noise_id| {
        if !noise_registry.contains(noise_id) {
            report.errors.push(DatapackError::DanglingReference {
                registry: "worldgen/noise_settings",
                referrer: id.to_string(),
                kind: "noise",
                target: noise_id.to_string(),
            });
        }
    });
}

fn walk_surface_rule(rule: &SurfaceRule, on_noise_ref: &mut impl FnMut(&ResourceLocation)) {
    match rule {
        SurfaceRule::Sequence { sequence } => {
            for r in sequence {
                walk_surface_rule(r, on_noise_ref);
            }
        }
        SurfaceRule::Condition { if_true, then_run } => {
            walk_surface_condition(if_true, on_noise_ref);
            walk_surface_rule(then_run, on_noise_ref);
        }
        SurfaceRule::Block { .. } | SurfaceRule::Badlands {} => {}
    }
}

fn walk_surface_condition(
    cond: &SurfaceCondition,
    on_noise_ref: &mut impl FnMut(&ResourceLocation),
) {
    match cond {
        SurfaceCondition::NoiseThreshold { noise, .. } => on_noise_ref(noise),
        SurfaceCondition::Not { invert } => walk_surface_condition(invert, on_noise_ref),
        _ => {}
    }
}

/// Validate biome ids referenced from multi-noise parameter lists and biomes
/// referenced from dimension biome sources resolve against the biome registry.
pub fn resolve_biome_refs(
    registry_name: &'static str,
    referrer: &ResourceLocation,
    ids: impl IntoIterator<Item = ResourceLocation>,
    biome_registry: &Registry<Biome>,
    report: &mut ResolutionReport,
) {
    for id in ids {
        if !biome_registry.contains(&id) {
            report.errors.push(DatapackError::DanglingReference {
                registry: registry_name,
                referrer: referrer.to_string(),
                kind: "biome",
                target: id.to_string(),
            });
        }
    }
}

pub fn multi_noise_biome_ids(source: &MultiNoiseSource) -> Vec<ResourceLocation> {
    match source {
        MultiNoiseSource::Preset { .. } => Vec::new(),
        MultiNoiseSource::Explicit { biomes } => {
            let seen: HashSet<ResourceLocation> = biomes.iter().map(|b| b.biome.clone()).collect();
            seen.into_iter().collect()
        }
    }
}
