//! `worldgen/noise_settings/*.json` — noise generator settings.

use crate::climate::ClimateParam;
use crate::density_function::DensityFunction;
use crate::surface_rule::SurfaceRule;
use oxide_core::BlockState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseGeneratorSettings {
    pub sea_level: i32,
    #[serde(default)]
    pub disable_mob_generation: bool,
    #[serde(default = "default_true")]
    pub aquifers_enabled: bool,
    #[serde(default = "default_true")]
    pub ore_veins_enabled: bool,
    #[serde(default)]
    pub legacy_random_source: bool,
    #[serde(with = "crate::ident_serde::block_state")]
    pub default_block: BlockState,
    #[serde(with = "crate::ident_serde::block_state")]
    pub default_fluid: BlockState,
    pub noise: NoiseDimensionSettings,
    pub noise_router: NoiseRouter,
    pub surface_rule: SurfaceRule,
    #[serde(default)]
    pub spawn_target: Vec<SpawnTarget>,
}

fn default_true() -> bool {
    true
}

/// The `noise` block of a noise_settings file: vertical bounds and cell size.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseDimensionSettings {
    pub min_y: i32,
    pub height: i32,
    pub size_horizontal: i32,
    pub size_vertical: i32,
}

/// The ~15 named density-function slots vanilla's `NoiseRouter` wires up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseRouter {
    #[serde(default = "zero")]
    pub barrier: DensityFunction,
    #[serde(default = "zero")]
    pub fluid_level_floodedness: DensityFunction,
    #[serde(default = "zero")]
    pub fluid_level_spread: DensityFunction,
    #[serde(default = "zero")]
    pub lava: DensityFunction,
    pub temperature: DensityFunction,
    pub vegetation: DensityFunction,
    pub continents: DensityFunction,
    pub erosion: DensityFunction,
    pub depth: DensityFunction,
    pub ridges: DensityFunction,
    /// 26.2 dropped the router's `initial_density_without_jaggedness` slot and
    /// exports this instead -- vanilla no longer binary-searches a density for the
    /// preliminary surface level, the datapack hands it over directly (see the new
    /// `minecraft:find_top_surface` density function in the same export). Both are
    /// defaulted so a pre-26.2 pack and a 26.2 pack each load; whichever the pack
    /// omits stays a constant zero.
    #[serde(default = "zero")]
    pub preliminary_surface_level: DensityFunction,
    #[serde(default = "zero")]
    pub initial_density_without_jaggedness: DensityFunction,
    pub final_density: DensityFunction,
    #[serde(default = "zero")]
    pub vein_toggle: DensityFunction,
    #[serde(default = "zero")]
    pub vein_ridged: DensityFunction,
    #[serde(default = "zero")]
    pub vein_gap: DensityFunction,
}

fn zero() -> DensityFunction {
    DensityFunction::Constant(0.0)
}

/// One point in the `spawn_target` climate-point list used to pick a world
/// spawn position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnTarget {
    pub temperature: ClimateParam,
    pub humidity: ClimateParam,
    pub continentalness: ClimateParam,
    pub erosion: ClimateParam,
    pub depth: ClimateParam,
    pub weirdness: ClimateParam,
    #[serde(default)]
    pub offset: f32,
}
