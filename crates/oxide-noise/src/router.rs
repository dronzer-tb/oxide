//! Wires a world seed + `oxide-datapack` registries into something that can sample a
//! `NoiseGeneratorSettings.noise_router` slot at a block position — vanilla's `RandomState` +
//! `NoiseRouter`.

use std::collections::HashMap;

use oxide_datapack::{
    DensityFunction, NoiseGeneratorSettings, NoiseRouter, NormalNoiseParameters, Registry,
    ResourceLocation,
};

use crate::density::{evaluate, EvalCtx, FunctionContext};
use crate::normal_noise::NormalNoise;
use crate::random::WorldRandom;

/// The ~15 named router slots, for callers that want to sample a specific one without matching
/// on `NoiseRouter`'s fields directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterSlot {
    Barrier,
    FluidLevelFloodedness,
    FluidLevelSpread,
    Lava,
    Temperature,
    Vegetation,
    Continents,
    Erosion,
    Depth,
    Ridges,
    InitialDensityWithoutJaggedness,
    PreliminarySurfaceLevel,
    FinalDensity,
    VeinToggle,
    VeinRidged,
    VeinGap,
}

/// Owns its inputs (clones `df_registry` and `settings.noise_router` once at construction)
/// rather than borrowing them, so a caller can keep one of these alive independently of the
/// `Datapack`/`NoiseGeneratorSettings` it was built from — e.g. behind an FFI handle, where
/// tying the evaluator's lifetime to borrowed data would mean a self-referential struct.
pub struct NoiseRouterEvaluator {
    df_registry: Registry<DensityFunction>,
    router: NoiseRouter,
    noises: HashMap<ResourceLocation, NormalNoise>,
}

impl NoiseRouterEvaluator {
    /// Builds every `NormalNoise` in `noise_param_registry` up front, each seeded via
    /// `positional_factory.from_hash_of(id)` off a `RandomSource` seeded directly from `seed`
    /// — matches vanilla's `RandomState` construction. `settings.legacy_random_source` picks
    /// the RNG flavour.
    pub fn new(
        seed: i64,
        settings: &NoiseGeneratorSettings,
        df_registry: &Registry<DensityFunction>,
        noise_param_registry: &Registry<NormalNoiseParameters>,
    ) -> Self {
        let mut base = WorldRandom::new(seed, settings.legacy_random_source);
        let factory = base.fork_positional();

        let mut noises = HashMap::with_capacity(noise_param_registry.len());
        for (id, params) in noise_param_registry.iter() {
            let mut noise_random = factory.from_hash_of(&id.to_string());
            let noise = NormalNoise::create(
                &mut noise_random,
                params.first_octave,
                params.amplitudes.clone(),
            );
            noises.insert(id.clone(), noise);
        }

        Self {
            df_registry: df_registry.clone(),
            router: settings.noise_router.clone(),
            noises,
        }
    }

    fn slot_df(&self, slot: RouterSlot) -> &DensityFunction {
        match slot {
            RouterSlot::Barrier => &self.router.barrier,
            RouterSlot::FluidLevelFloodedness => &self.router.fluid_level_floodedness,
            RouterSlot::FluidLevelSpread => &self.router.fluid_level_spread,
            RouterSlot::Lava => &self.router.lava,
            RouterSlot::Temperature => &self.router.temperature,
            RouterSlot::Vegetation => &self.router.vegetation,
            RouterSlot::Continents => &self.router.continents,
            RouterSlot::Erosion => &self.router.erosion,
            RouterSlot::Depth => &self.router.depth,
            RouterSlot::Ridges => &self.router.ridges,
            RouterSlot::InitialDensityWithoutJaggedness => {
                &self.router.initial_density_without_jaggedness
            }
            RouterSlot::PreliminarySurfaceLevel => &self.router.preliminary_surface_level,
            RouterSlot::FinalDensity => &self.router.final_density,
            RouterSlot::VeinToggle => &self.router.vein_toggle,
            RouterSlot::VeinRidged => &self.router.vein_ridged,
            RouterSlot::VeinGap => &self.router.vein_gap,
        }
    }

    pub fn sample(&self, slot: RouterSlot, x: i32, y: i32, z: i32) -> f64 {
        let cx = EvalCtx {
            df_registry: &self.df_registry,
            noises: &self.noises,
        };
        evaluate(self.slot_df(slot), FunctionContext { x, y, z }, &cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::{BlockState, ResourceLocation as Rl};
    use oxide_datapack::{NoiseDimensionSettings, SurfaceRule};

    fn minimal_settings() -> NoiseGeneratorSettings {
        NoiseGeneratorSettings {
            sea_level: 63,
            disable_mob_generation: false,
            aquifers_enabled: true,
            ore_veins_enabled: true,
            legacy_random_source: false,
            default_block: BlockState::new(Rl::new("minecraft", "stone")),
            default_fluid: BlockState::new(Rl::new("minecraft", "water")),
            noise: NoiseDimensionSettings {
                min_y: -64,
                height: 384,
                size_horizontal: 1,
                size_vertical: 2,
            },
            noise_router: NoiseRouter {
                barrier: DensityFunction::Constant(0.0),
                fluid_level_floodedness: DensityFunction::Constant(0.0),
                fluid_level_spread: DensityFunction::Constant(0.0),
                lava: DensityFunction::Constant(0.0),
                temperature: DensityFunction::Constant(0.0),
                vegetation: DensityFunction::Constant(0.0),
                continents: DensityFunction::Constant(0.0),
                erosion: DensityFunction::Constant(0.0),
                depth: DensityFunction::Constant(0.0),
                ridges: DensityFunction::Constant(0.0),
                initial_density_without_jaggedness: DensityFunction::Constant(1.0),
                preliminary_surface_level: DensityFunction::Constant(1.0),
                final_density: DensityFunction::Constant(1.0),
                vein_toggle: DensityFunction::Constant(0.0),
                vein_ridged: DensityFunction::Constant(0.0),
                vein_gap: DensityFunction::Constant(0.0),
            },
            surface_rule: SurfaceRule::Sequence { sequence: vec![] },
            spawn_target: vec![],
        }
    }

    #[test]
    fn constant_slot_ignores_position() {
        let settings = minimal_settings();
        let df_registry = Registry::default();
        let noise_registry = Registry::default();
        let router = NoiseRouterEvaluator::new(42, &settings, &df_registry, &noise_registry);
        assert_eq!(router.sample(RouterSlot::FinalDensity, 0, 0, 0), 1.0);
        assert_eq!(router.sample(RouterSlot::FinalDensity, 100, 50, -100), 1.0);
    }

    #[test]
    fn different_seeds_dont_panic_on_empty_registries() {
        let settings = minimal_settings();
        let df_registry = Registry::default();
        let noise_registry = Registry::default();
        for seed in [0i64, 1, -1, i64::MAX, i64::MIN] {
            let router = NoiseRouterEvaluator::new(seed, &settings, &df_registry, &noise_registry);
            let _ = router.sample(RouterSlot::Continents, 0, 0, 0);
        }
    }
}
