//! Density function IR. Deserialization only — no evaluation (that's
//! `oxide-noise`'s job; do not add `sample`/`compute` here).
//!
//! A density function JSON value is one of:
//! - a bare number → [`DensityFunction::Constant`]
//! - a bare string → [`DensityFunction::Reference`] (a named density function
//!   elsewhere in the registry, resolved in [`crate::resolve`])
//! - an object with a `type` tag → [`DensityFunction::Object`]

use oxide_core::ResourceLocation;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DensityFunction {
    Constant(f64),
    Reference(#[serde(with = "crate::ident_serde::rl")] ResourceLocation),
    Object(Box<DensityFunctionObject>),
}

/// `weird_scaled_sampler`'s rarity value mapper. Vanilla `StringRepresentable`
/// enum with exactly these two values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RarityValueMapper {
    #[serde(rename = "type_1")]
    Type1,
    #[serde(rename = "type_2")]
    Type2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DensityFunctionObject {
    #[serde(rename = "minecraft:constant")]
    Constant { argument: f64 },

    #[serde(rename = "minecraft:noise")]
    Noise {
        #[serde(with = "crate::ident_serde::rl")]
        noise: ResourceLocation,
        xz_scale: f64,
        y_scale: f64,
    },

    #[serde(rename = "minecraft:shifted_noise")]
    ShiftedNoise {
        #[serde(with = "crate::ident_serde::rl")]
        noise: ResourceLocation,
        xz_scale: f64,
        y_scale: f64,
        shift_x: DensityFunction,
        shift_y: DensityFunction,
        shift_z: DensityFunction,
    },

    #[serde(rename = "minecraft:old_blended_noise")]
    OldBlendedNoise {
        xz_scale: f64,
        y_scale: f64,
        xz_factor: f64,
        y_factor: f64,
        smear_scale_multiplier: f64,
    },

    #[serde(rename = "minecraft:end_islands")]
    EndIslands {},

    #[serde(rename = "minecraft:weird_scaled_sampler")]
    WeirdScaledSampler {
        input: DensityFunction,
        #[serde(with = "crate::ident_serde::rl")]
        noise: ResourceLocation,
        rarity_value_mapper: RarityValueMapper,
    },

    #[serde(rename = "minecraft:flat_cache")]
    FlatCache { argument: DensityFunction },

    #[serde(rename = "minecraft:cache_2d")]
    Cache2d { argument: DensityFunction },

    #[serde(rename = "minecraft:cache_once")]
    CacheOnce { argument: DensityFunction },

    #[serde(rename = "minecraft:cache_all_in_cell")]
    CacheAllInCell { argument: DensityFunction },

    #[serde(rename = "minecraft:interpolated")]
    Interpolated { argument: DensityFunction },

    #[serde(rename = "minecraft:blend_density")]
    BlendDensity { argument: DensityFunction },

    #[serde(rename = "minecraft:blend_alpha")]
    BlendAlpha {},

    #[serde(rename = "minecraft:blend_offset")]
    BlendOffset {},

    #[serde(rename = "minecraft:beardifier")]
    Beardifier {},

    #[serde(rename = "minecraft:add")]
    Add {
        argument1: DensityFunction,
        argument2: DensityFunction,
    },

    #[serde(rename = "minecraft:mul")]
    Mul {
        argument1: DensityFunction,
        argument2: DensityFunction,
    },

    #[serde(rename = "minecraft:min")]
    Min {
        argument1: DensityFunction,
        argument2: DensityFunction,
    },

    #[serde(rename = "minecraft:max")]
    Max {
        argument1: DensityFunction,
        argument2: DensityFunction,
    },

    #[serde(rename = "minecraft:abs")]
    Abs { argument: DensityFunction },

    #[serde(rename = "minecraft:square")]
    Square { argument: DensityFunction },

    #[serde(rename = "minecraft:cube")]
    Cube { argument: DensityFunction },

    #[serde(rename = "minecraft:half_negative")]
    HalfNegative { argument: DensityFunction },

    #[serde(rename = "minecraft:quarter_negative")]
    QuarterNegative { argument: DensityFunction },

    #[serde(rename = "minecraft:squeeze")]
    Squeeze { argument: DensityFunction },

    #[serde(rename = "minecraft:y_clamped_gradient")]
    YClampedGradient {
        from_y: i32,
        to_y: i32,
        from_value: f64,
        to_value: f64,
    },

    #[serde(rename = "minecraft:range_choice")]
    RangeChoice {
        input: DensityFunction,
        min_inclusive: f64,
        max_exclusive: f64,
        when_in_range: DensityFunction,
        when_out_of_range: DensityFunction,
    },

    #[serde(rename = "minecraft:clamp")]
    Clamp {
        input: DensityFunction,
        min: f64,
        max: f64,
    },

    #[serde(rename = "minecraft:spline")]
    Spline { spline: CubicSpline },

    // PARITY-CHECK: `shift`/`shift_a`/`shift_b` are modeled as referencing a
    // normal-noise parameter set via a field named `argument`, matching
    // vanilla's `ShiftNoise` codec as best recalled. Unverified against a
    // real Minecraft 26.2 data export — confirm field name/shape before
    // trusting this on real noise_settings/density_function files.
    #[serde(rename = "minecraft:shift")]
    Shift {
        #[serde(with = "crate::ident_serde::rl")]
        argument: ResourceLocation,
    },

    #[serde(rename = "minecraft:shift_a")]
    ShiftA {
        #[serde(with = "crate::ident_serde::rl")]
        argument: ResourceLocation,
    },

    #[serde(rename = "minecraft:shift_b")]
    ShiftB {
        #[serde(with = "crate::ident_serde::rl")]
        argument: ResourceLocation,
    },
}

/// Recursive cubic spline: `{coordinate, points: [{location, value, derivative}]}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CubicSpline {
    pub coordinate: DensityFunction,
    pub points: Vec<SplinePoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplinePoint {
    pub location: f32,
    pub value: SplineValue,
    pub derivative: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SplineValue {
    Constant(f32),
    Spline(Box<CubicSpline>),
}
