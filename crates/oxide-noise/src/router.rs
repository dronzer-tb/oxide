//! Wires a world seed + `oxide-datapack` registries into something that can sample a
//! `NoiseGeneratorSettings.noise_router` slot at a block position — vanilla's `RandomState` +
//! `NoiseRouter`.

use std::collections::HashMap;

use oxide_datapack::{
    DensityFunction, NoiseDimensionSettings, NoiseGeneratorSettings, NoiseRouter,
    NormalNoiseParameters, Registry, ResourceLocation,
};

use crate::blended_noise::BlendedNoise;
use crate::cache::ChunkCaches;
use crate::compiled::{CacheSlotCounts, Compiler, Program, RunCtx};
use crate::density::FunctionContext;
use crate::normal_noise::NormalNoise;
use crate::random::{WorldPositionalFactory, WorldRandom};

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

impl RouterSlot {
    /// The slots in a fixed order, which is the order their compiled programs are stored in.
    const ALL: [RouterSlot; 16] = [
        RouterSlot::Barrier,
        RouterSlot::FluidLevelFloodedness,
        RouterSlot::FluidLevelSpread,
        RouterSlot::Lava,
        RouterSlot::Temperature,
        RouterSlot::Vegetation,
        RouterSlot::Continents,
        RouterSlot::Erosion,
        RouterSlot::Depth,
        RouterSlot::Ridges,
        RouterSlot::InitialDensityWithoutJaggedness,
        RouterSlot::PreliminarySurfaceLevel,
        RouterSlot::FinalDensity,
        RouterSlot::VeinToggle,
        RouterSlot::VeinRidged,
        RouterSlot::VeinGap,
    ];

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|slot| *slot == self)
            .expect("RouterSlot::ALL lists every variant")
    }
}

/// Owns its inputs (clones `df_registry` and `settings.noise_router` once at construction)
/// rather than borrowing them, so a caller can keep one of these alive independently of the
/// `Datapack`/`NoiseGeneratorSettings` it was built from — e.g. behind an FFI handle, where
/// tying the evaluator's lifetime to borrowed data would mean a self-referential struct.
pub struct NoiseRouterEvaluator {
    /// One compiled program per [`RouterSlot`], in `RouterSlot::ALL` order. Compilation copies
    /// out every constant it needs, so neither the density-function registry nor the
    /// `NoiseRouter` is kept alive past construction.
    programs: Vec<Program>,
    /// How many cache slots of each kind the programs use, which sizes a [`ChunkCaches`].
    cache_slots: CacheSlotCounts,
    /// The `worldgen/noise` registry, seeded, dense so the compiled programs can index it.
    noises: Vec<NormalNoise>,
    noise_index: HashMap<ResourceLocation, u32>,
    /// Kept for consumers outside the density-function tree -- the surface system needs the
    /// same `RandomState` positional factory vanilla's `SurfaceSystem` uses, and deriving a
    /// second one from the seed would not match it.
    positional_factory: WorldPositionalFactory,
    /// Cell dimensions and vertical bounds, needed to size a chunk's interpolation grid.
    noise: NoiseDimensionSettings,
    /// The three Perlin stacks behind every `minecraft:old_blended_noise` node in this router.
    /// Vanilla builds one `BlendedNoise` per node but seeds them all from the same
    /// `"minecraft:terrain"` random, so the stacks are identical and only the per-node scaling
    /// constants differ -- those travel with the node and are applied at evaluation.
    blended: BlendedNoise,
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

        // `RandomState.NoiseWiringHelper.wrapNew`: a legacy-random world seeds the terrain
        // noise straight off the world seed, everything else off `"minecraft:terrain"`.
        let mut terrain_random = if settings.legacy_random_source {
            WorldRandom::new(seed, true)
        } else {
            factory.from_hash_of("minecraft:terrain")
        };
        let blended = BlendedNoise::new(&mut terrain_random);

        let mut noises = Vec::with_capacity(noise_param_registry.len());
        let mut noise_index = HashMap::with_capacity(noise_param_registry.len());
        for (id, params) in noise_param_registry.iter() {
            let mut noise_random = factory.from_hash_of(&id.to_string());
            let noise = NormalNoise::create(
                &mut noise_random,
                params.first_octave,
                params.amplitudes.clone(),
            );
            noise_index.insert(id.clone(), noises.len() as u32);
            noises.push(noise);
        }

        // One compiler across all sixteen slots: they reach the same registry nodes, and a node
        // shared between two of them has to share one cache slot the way vanilla's shared
        // instances do.
        let mut compiler = Compiler::new(df_registry, &noise_index);
        let programs = RouterSlot::ALL
            .iter()
            .map(|slot| compiler.compile(slot_df(&settings.noise_router, *slot)))
            .collect();
        let cache_slots = compiler.counts();

        Self {
            programs,
            cache_slots,
            noises,
            noise_index,
            positional_factory: factory,
            noise: settings.noise.clone(),
            blended,
        }
    }

    /// The `RandomState` positional factory every seeded-per-position consumer must share.
    pub fn positional_factory(&self) -> &WorldPositionalFactory {
        &self.positional_factory
    }

    /// One named noise from the `worldgen/noise` registry, already seeded. `None` when the
    /// datapack has no such entry -- callers decide whether that is fatal.
    pub fn noise(&self, id: &ResourceLocation) -> Option<&NormalNoise> {
        self.noise_index
            .get(id)
            .and_then(|index| self.noises.get(*index as usize))
    }

    /// One-off sample with no chunk context: `interpolated` degrades to evaluating the tree
    /// exactly, which is both slow and slightly off vanilla. Fine for a biome lookup or a
    /// test; use [`Self::sample_in_chunk`] for anything filling a chunk.
    pub fn sample(&self, slot: RouterSlot, x: i32, y: i32, z: i32) -> f64 {
        let cx = RunCtx {
            noises: &self.noises,
            blended: &self.blended,
            caches: None,
        };
        self.programs[slot.index()].run(FunctionContext { x, y, z }, &cx)
    }

    /// Sample within a chunk, honouring the datapack's caching and interpolation nodes. This
    /// is the path chunk generation must use: it is ~80x less work than [`Self::sample`] and,
    /// because vanilla's terrain is the interpolated value rather than the exact one, it is
    /// also the one that can match Java.
    pub fn sample_in_chunk(
        &self,
        caches: &ChunkCaches,
        slot: RouterSlot,
        x: i32,
        y: i32,
        z: i32,
    ) -> f64 {
        let cx = RunCtx {
            noises: &self.noises,
            blended: &self.blended,
            caches: Some(caches),
        };
        self.programs[slot.index()].run(FunctionContext { x, y, z }, &cx)
    }

    /// A `ChunkCaches` sized for this router's settings, for the chunk at `(chunk_x, chunk_z)`.
    pub fn chunk_caches(&self, chunk_x: i32, chunk_z: i32) -> ChunkCaches {
        ChunkCaches::new(
            chunk_x * 16,
            chunk_z * 16,
            self.noise.min_y,
            self.noise.height,
            self.noise.size_horizontal,
            self.noise.size_vertical,
            self.cache_slots,
        )
    }
}

/// The density function a named router slot holds.
fn slot_df(router: &NoiseRouter, slot: RouterSlot) -> &DensityFunction {
    match slot {
        RouterSlot::Barrier => &router.barrier,
        RouterSlot::FluidLevelFloodedness => &router.fluid_level_floodedness,
        RouterSlot::FluidLevelSpread => &router.fluid_level_spread,
        RouterSlot::Lava => &router.lava,
        RouterSlot::Temperature => &router.temperature,
        RouterSlot::Vegetation => &router.vegetation,
        RouterSlot::Continents => &router.continents,
        RouterSlot::Erosion => &router.erosion,
        RouterSlot::Depth => &router.depth,
        RouterSlot::Ridges => &router.ridges,
        RouterSlot::InitialDensityWithoutJaggedness => &router.initial_density_without_jaggedness,
        RouterSlot::PreliminarySurfaceLevel => &router.preliminary_surface_level,
        RouterSlot::FinalDensity => &router.final_density,
        RouterSlot::VeinToggle => &router.vein_toggle,
        RouterSlot::VeinRidged => &router.vein_ridged,
        RouterSlot::VeinGap => &router.vein_gap,
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
