//! Wires a world seed + `oxide-datapack` registries into something that can sample a
//! `NoiseGeneratorSettings.noise_router` slot at a block position — vanilla's `RandomState` +
//! `NoiseRouter`.

use std::sync::Arc;
use std::collections::HashMap;

use oxide_datapack::{
    DensityFunction, NoiseDimensionSettings, NoiseGeneratorSettings, NoiseRouter,
    NormalNoiseParameters, Registry, ResourceLocation,
};

use crate::blended_noise::BlendedNoise;
use crate::cache::{ChunkCaches, CornerColumns};
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
    const ALL: [RouterSlot; crate::cache::MEMO_SLOTS] = [
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

    /// Stable display name, for diagnostics that report per-slot figures.
    pub fn name(self) -> &'static str {
        match self {
            RouterSlot::Barrier => "barrier",
            RouterSlot::FluidLevelFloodedness => "fluid_level_floodedness",
            RouterSlot::FluidLevelSpread => "fluid_level_spread",
            RouterSlot::Lava => "lava",
            RouterSlot::Temperature => "temperature",
            RouterSlot::Vegetation => "vegetation",
            RouterSlot::Continents => "continents",
            RouterSlot::Erosion => "erosion",
            RouterSlot::Depth => "depth",
            RouterSlot::Ridges => "ridges",
            RouterSlot::InitialDensityWithoutJaggedness => "initial_density_without_jaggedness",
            RouterSlot::PreliminarySurfaceLevel => "preliminary_surface_level",
            RouterSlot::FinalDensity => "final_density",
            RouterSlot::VeinToggle => "vein_toggle",
            RouterSlot::VeinRidged => "vein_ridged",
            RouterSlot::VeinGap => "vein_gap",
        }
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
    /// Unique per evaluator for the life of the process; see [`Self::id`].
    id: u64,
    /// Interpolation corner columns shared by every chunk this router builds caches for, on
    /// every thread: neighbouring chunks share their edge columns.
    corner_columns: Arc<CornerColumns>,
}

/// Source of [`NoiseRouterEvaluator::id`]. 64 bits never wrap in practice, so no two
/// evaluators built in one process share an id, even after one is dropped and another is
/// built at the same address.
static NEXT_ROUTER_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl NoiseRouterEvaluator {
    /// An identity no other evaluator in this process has had or will have. A cache that
    /// outlives one chunk (for example, per thread, across chunks) keys on this so it can never
    /// hand one router's values to another. An address cannot do that, since it is reused
    /// after a drop, and a seed cannot either, since two routers can share a seed with
    /// different datapacks.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Instruction count of each router slot's compiled program, in `RouterSlot::ALL` order.
    /// Used to size the register budget in `Program::run` against real datapacks rather than
    /// against a guess.
    pub fn program_sizes(&self) -> Vec<(&'static str, usize)> {
        RouterSlot::ALL
            .iter()
            .zip(self.programs.iter())
            .map(|(slot, program)| (slot.name(), program.len()))
            .collect()
    }

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
        let mut compiler = Compiler::new(df_registry, &noise_index)
            .with_noise_bounds(noises.iter().map(NormalNoise::max_abs).collect());
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
            id: NEXT_ROUTER_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            corner_columns: Arc::new(CornerColumns::new(
                settings.noise.min_y,
                settings.noise.height,
                settings.noise.size_horizontal,
                settings.noise.size_vertical,
            )),
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
        let program = &self.programs[slot.index()];
        let ctx = FunctionContext { x, y, z };
        // A program that is only a blend of `interpolated` nodes (`final_density`) is evaluated
        // a whole cell at a time and then read back per block; see `Program::run_cell`.
        if program.cell_batchable() {
            if let Some(value) = caches.sample_via_cell(slot.index(), program, ctx, &cx) {
                return value;
            }
        }
        program.run(ctx, &cx)
    }

    /// `slot` at every block of one cell of `caches`, for a caller that fills cell by cell and
    /// wants no per-block call at all. `out` must hold [`Self::cell_len`] values and is written
    /// in lane order `(local_y * cell_width + local_z) * cell_width + local_x`, each value
    /// bit-identical to [`Self::sample_in_chunk`] at that block.
    ///
    /// Returns `false`, leaving `out` untouched, when `slot`'s program cannot be evaluated this
    /// way (it reads something other than `interpolated` nodes) or the cell is outside the
    /// chunk; the caller then falls back to per-block sampling.
    pub fn sample_cell(
        &self,
        caches: &ChunkCaches,
        slot: RouterSlot,
        cell_x: i32,
        cell_y: i32,
        cell_z: i32,
        out: &mut [f64],
    ) -> bool {
        let program = &self.programs[slot.index()];
        if !program.cell_batchable()
            || !caches.cells_tile_chunk()
            || !caches.has_cell(cell_x, cell_y, cell_z)
            || out.len() != caches.cell_lanes()
        {
            return false;
        }
        let cx = RunCtx {
            noises: &self.noises,
            blended: &self.blended,
            caches: Some(caches),
        };
        program.run_cell(caches, (cell_x, cell_y, cell_z), &cx, out);
        true
    }

    /// Blocks per cell for `caches`: the length [`Self::sample_cell`] writes.
    pub fn cell_len(&self, caches: &ChunkCaches) -> usize {
        caches.cell_lanes()
    }

    /// The `interpolated` cache slot that `slot`'s program resolves through, when its result is
    /// exactly one such cache node -- which `final_density` is in every vanilla-shaped router.
    /// Lets a caller read that slot's cell-corner bounds via [`ChunkCaches::cell_bounds`] and
    /// skip per-block evaluation for cells that are uniformly solid or uniformly empty.
    /// Whether `slot`'s value is a function of x and z alone: for any fixed (x, z),
    /// [`Self::sample_in_chunk`] returns bit-identical values at every y, through any chunk's
    /// caches, and so does [`Self::sample`]. A caller may then sample the slot once per column
    /// and reuse the value for the whole column.
    ///
    /// The two entry points are each y-independent, but they need not agree with each other:
    /// with caches a `flat_cache` node samples at the quart origin, without them at the exact
    /// position. Reuse values from the one you sample with.
    ///
    /// Proven on the compiled program, not assumed. A 2D `shifted_noise` (`y_scale` 0) only
    /// qualifies when its `shift_y` is a constant that is neither -0.0 nor NaN, since its y
    /// input is `y * 0.0 + shift_y` and `y * 0.0` keeps the sign of y. A plain `noise` with
    /// `y_scale` 0 does not qualify, for the same reason. Any `interpolated` node disqualifies
    /// the program, because it reads this chunk's corner grid.
    /// Whether, through chunk caches, `slot`'s value is the same at every block of a quart
    /// column: [`Self::sample_in_chunk`] at `(x, y, z)` returns exactly the bits it returns at
    /// `(x & !3, y, z & !3)`, so a caller may sample once per quart and reuse the value for the
    /// other 15 columns at the same y. Combine with [`Self::is_y_independent`] for one sample
    /// per quart column.
    ///
    /// It holds through `flat_cache`, which samples at the quart origin, and not through
    /// [`Self::sample`], where no cache snaps.
    pub fn is_quart_invariant(&self, slot: RouterSlot) -> bool {
        self.programs[slot.index()].quart_invariant()
    }

    pub fn is_y_independent(&self, slot: RouterSlot) -> bool {
        self.programs[slot.index()].y_independent()
    }

    /// Instruction listing for one slot's program, for diagnostics.
    pub fn dump_program(&self, slot: RouterSlot) -> Vec<String> {
        self.programs[slot.index()].dump()
    }

    /// Conservative bounds on `slot`'s value across one whole cell of `caches`, or `None` when
    /// none can be proven. See `Program::cell_bounds`.
    pub fn cell_bounds(
        &self,
        caches: &ChunkCaches,
        slot: RouterSlot,
        cell_x: i32,
        cell_y: i32,
        cell_z: i32,
    ) -> Option<(f64, f64)> {
        self.programs[slot.index()].cell_bounds(caches, cell_x, cell_y, cell_z)
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
        .with_shared_columns(&self.corner_columns)
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

    /// A `final_density`-shaped program: `add(squeeze(interpolated(a)), range_choice(
    /// interpolated(b), .., const, interpolated(c) + gradient))`, over blended noise so the
    /// corner grids vary along every axis.
    fn interpolated_settings() -> NoiseGeneratorSettings {
        use oxide_datapack::DensityFunctionObject as O;
        fn obj(o: O) -> DensityFunction {
            DensityFunction::Object(Box::new(o))
        }
        fn blended(xz_scale: f64) -> DensityFunction {
            obj(O::Interpolated {
                argument: obj(O::OldBlendedNoise {
                    xz_scale,
                    y_scale: 0.125,
                    xz_factor: 80.0,
                    y_factor: 160.0,
                    smear_scale_multiplier: 8.0,
                }),
            })
        }
        let mut settings = minimal_settings();
        // `add`, not vanilla's `min`, at the top so a test can tell from the result which
        // `range_choice` branch a block took (the in-range constant dominates the sum).
        settings.noise_router.final_density = obj(O::Add {
            argument1: obj(O::Squeeze {
                argument: blended(0.25),
            }),
            argument2: obj(O::RangeChoice {
                input: blended(0.5),
                min_inclusive: -1000000.0,
                max_exclusive: 0.0,
                when_in_range: DensityFunction::Constant(64.0),
                when_out_of_range: obj(O::Add {
                    argument1: blended(1.0),
                    argument2: obj(O::YClampedGradient {
                        from_y: -64,
                        to_y: 320,
                        from_value: 0.2,
                        to_value: -0.2,
                    }),
                }),
            }),
        });
        settings
    }

    /// The whole-cell path must not change a single bit: every block of a chunk, sampled
    /// through the cell memo and through `sample_cell`, against plain per-block evaluation.
    #[test]
    fn whole_cell_evaluation_is_bit_identical_to_per_block() {
        let settings = interpolated_settings();
        let df_registry = Registry::default();
        let noise_registry = Registry::default();
        let router = NoiseRouterEvaluator::new(7, &settings, &df_registry, &noise_registry);
        let program = &router.programs[RouterSlot::FinalDensity.index()];
        assert!(program.cell_batchable());

        for (chunk_x, chunk_z) in [(0, 0), (-3, 5)] {
            let memo_caches = router.chunk_caches(chunk_x, chunk_z);
            let plain_caches = router.chunk_caches(chunk_x, chunk_z);
            let cell_caches = router.chunk_caches(chunk_x, chunk_z);
            let cx = RunCtx {
                noises: &router.noises,
                blended: &router.blended,
                caches: Some(&plain_caches),
            };
            let lanes = router.cell_len(&cell_caches);
            assert_eq!(lanes, 4 * 8 * 4);
            // Every cell of the chunk up front, indexed [cell_y][cell_z][cell_x].
            let mut cells = vec![0.0; 48 * 4 * 4 * lanes];
            for (index, cell) in cells.chunks_mut(lanes).enumerate() {
                let (cell_y, cell_z, cell_x) = (index / 16, index / 4 % 4, index % 4);
                assert!(router.sample_cell(
                    &cell_caches,
                    RouterSlot::FinalDensity,
                    cell_x as i32,
                    cell_y as i32,
                    cell_z as i32,
                    cell
                ));
            }
            let (mut in_range, mut out_of_range) = (0, 0);
            for y in -64..320 {
                for local_z in 0..16 {
                    for local_x in 0..16 {
                        let (x, z) = (chunk_x * 16 + local_x, chunk_z * 16 + local_z);
                        let expected = program.run(FunctionContext { x, y, z }, &cx);
                        let via_memo =
                            router.sample_in_chunk(&memo_caches, RouterSlot::FinalDensity, x, y, z);
                        assert_eq!(expected.to_bits(), via_memo.to_bits(), "memo at {x} {y} {z}");

                        let (cell_x, cell_y, cell_z) = (local_x / 4, (y + 64) / 8, local_z / 4);
                        let cell = ((cell_y * 4 + cell_z) * 4 + cell_x) as usize * lanes;
                        let lane = (((y + 64) % 8 * 4 + local_z % 4) * 4 + local_x % 4) as usize;
                        assert_eq!(
                            expected.to_bits(),
                            cells[cell + lane].to_bits(),
                            "cell at {x} {y} {z}"
                        );

                        if expected > 32.0 {
                            in_range += 1;
                        } else {
                            out_of_range += 1;
                        }
                    }
                }
            }
            // Both `range_choice` branches, and cells that mix them, must have been exercised.
            assert!(in_range > 1000 && out_of_range > 1000, "{in_range} / {out_of_range}");
        }
    }

    /// Outside the chunk the memo does not apply and sampling falls back to the per-block path.
    #[test]
    fn sampling_outside_the_chunk_matches_per_block() {
        let settings = interpolated_settings();
        let df_registry = Registry::default();
        let noise_registry = Registry::default();
        let router = NoiseRouterEvaluator::new(7, &settings, &df_registry, &noise_registry);
        let program = &router.programs[RouterSlot::FinalDensity.index()];
        let caches = router.chunk_caches(0, 0);
        let plain = router.chunk_caches(0, 0);
        let cx = RunCtx {
            noises: &router.noises,
            blended: &router.blended,
            caches: Some(&plain),
        };
        for (x, y, z) in [(16, 0, 3), (-1, 10, 0), (4, 320, 4), (4, -65, 4), (5, 9, 16)] {
            assert_eq!(
                program.run(FunctionContext { x, y, z }, &cx).to_bits(),
                router
                    .sample_in_chunk(&caches, RouterSlot::FinalDensity, x, y, z)
                    .to_bits()
            );
        }
    }

    /// Corner grids built through the router's shared column cache equal, bit for bit, grids
    /// each chunk computes entirely on its own, and the columns two chunks share are hits.
    #[test]
    fn shared_corner_columns_are_bit_identical_and_hit_on_shared_edges() {
        let settings = interpolated_settings();
        let df_registry = Registry::default();
        let noise_registry = Registry::default();
        let router = NoiseRouterEvaluator::new(7, &settings, &df_registry, &noise_registry);
        let noise = &settings.noise;
        let slots = router.cache_slots.interpolated;
        assert!(slots >= 3);

        // (0,0) first, then its +z, +x and diagonal neighbours: 5, 5 and 9 shared columns.
        let order = [((0, 0), 0), ((0, 1), 5), ((1, 0), 5), ((1, 1), 9)];
        let (mut expected_hits, mut expected_lookups) = (0, 0);
        for ((chunk_x, chunk_z), shared_columns) in order {
            let shared = router.chunk_caches(chunk_x, chunk_z);
            assert!(shared.has_shared_columns());
            let plain = ChunkCaches::new(
                chunk_x * 16,
                chunk_z * 16,
                noise.min_y,
                noise.height,
                noise.size_horizontal,
                noise.size_vertical,
                router.cache_slots,
            );
            assert!(!plain.has_shared_columns());
            // A few samples build whichever grids `final_density` reads, in both caches alike.
            for (y, local_z, local_x) in [(-64, 0, 0), (-64, 15, 15), (100, 7, 9), (319, 3, 12)] {
                let (x, z) = (chunk_x * 16 + local_x, chunk_z * 16 + local_z);
                let a = router.sample_in_chunk(&shared, RouterSlot::FinalDensity, x, y, z);
                let b = router.sample_in_chunk(&plain, RouterSlot::FinalDensity, x, y, z);
                assert_eq!(a.to_bits(), b.to_bits(), "sample at {x} {y} {z}");
            }
            let mut grids = 0;
            for slot in 0..slots {
                let (a, b) = (shared.corner_grid(slot), plain.corner_grid(slot));
                assert_eq!(a.is_some(), b.is_some(), "slot {slot} built in only one cache");
                if let (Some(a), Some(b)) = (a, b) {
                    assert_eq!(a.len(), 5 * 49 * 5);
                    let a: Vec<u64> = a.iter().map(|v| v.to_bits()).collect();
                    let b: Vec<u64> = b.iter().map(|v| v.to_bits()).collect();
                    assert_eq!(a, b, "slot {slot} of chunk {chunk_x} {chunk_z}");
                    grids += 1;
                }
            }
            assert_eq!(grids, slots, "every interpolated slot is built");
            expected_hits += shared_columns * grids;
            // Every chunk looks up its 16 edge columns per grid, and nothing else.
            expected_lookups += 16 * grids;
            let (hits, misses) = router.corner_columns.counts();
            assert_eq!(hits, expected_hits, "hits after chunk {chunk_x} {chunk_z}");
            assert_eq!(hits + misses, expected_lookups, "lookups after {chunk_x} {chunk_z}");
        }
    }

    /// `add(c, mul(gradient, body))` is lowered to a gated add that skips `body` where the
    /// gradient is exactly zero; every y must still give the plain expression's exact bits, in
    /// every operand order.
    #[test]
    fn gated_add_is_bit_identical_and_skips_where_the_gradient_is_zero() {
        use oxide_datapack::DensityFunctionObject as O;
        fn obj(o: O) -> DensityFunction {
            DensityFunction::Object(Box::new(o))
        }
        let params = crate::blended_noise::BlendedNoiseParams {
            xz_scale: 0.25,
            y_scale: 0.125,
            xz_factor: 80.0,
            y_factor: 160.0,
            smear_scale_multiplier: 8.0,
        };
        let blended = || {
            obj(O::OldBlendedNoise {
                xz_scale: params.xz_scale,
                y_scale: params.y_scale,
                xz_factor: params.xz_factor,
                y_factor: params.y_factor,
                smear_scale_multiplier: params.smear_scale_multiplier,
            })
        };
        let gradient = || {
            obj(O::YClampedGradient {
                from_y: 240,
                to_y: 256,
                from_value: 1.0,
                to_value: 0.0,
            })
        };
        let body = || {
            obj(O::Add {
                argument1: DensityFunction::Constant(0.078125),
                argument2: blended(),
            })
        };
        for (c_first, g_first) in [(true, true), (true, false), (false, true), (false, false)] {
            let mul = if g_first {
                obj(O::Mul { argument1: gradient(), argument2: body() })
            } else {
                obj(O::Mul { argument1: body(), argument2: gradient() })
            };
            let c = DensityFunction::Constant(-0.078125);
            let add = if c_first {
                obj(O::Add { argument1: c, argument2: mul })
            } else {
                obj(O::Add { argument1: mul, argument2: c })
            };
            let mut settings = minimal_settings();
            settings.noise_router.final_density = add;
            let router = NoiseRouterEvaluator::new(
                9,
                &settings,
                &Registry::default(),
                &Registry::default(),
            );
            assert!(
                router.dump_program(RouterSlot::FinalDensity).iter().any(|l| l.contains("GatedAdd")),
                "not gated: {:?}",
                router.dump_program(RouterSlot::FinalDensity)
            );
            let mut skipped = 0;
            for y in -64..=320 {
                for (x, z) in [(0, 0), (-37, 211), (1234, -999)] {
                    let g = crate::density::clamped_lerp(y, 240, 256, 1.0, 0.0);
                    let b = 0.078125 + router.blended.compute(&params, x as f64, y as f64, z as f64);
                    let product = if g_first { g * b } else { b * g };
                    let expected = if c_first { -0.078125 + product } else { product + -0.078125 };
                    let got = router.sample(RouterSlot::FinalDensity, x, y, z);
                    assert_eq!(got.to_bits(), expected.to_bits(), "{c_first} {g_first} at {x} {y} {z}");
                    if g == 0.0 {
                        skipped += 1;
                    }
                }
            }
            assert!(skipped > 100, "the gate must have been exercised: {skipped}");
        }
    }

    /// No finiteness proof, no gate: `invert` can produce an infinity, and `0 * inf` is NaN.
    #[test]
    fn gated_add_needs_a_finite_body() {
        use oxide_datapack::DensityFunctionObject as O;
        fn obj(o: O) -> DensityFunction {
            DensityFunction::Object(Box::new(o))
        }
        let mut settings = minimal_settings();
        settings.noise_router.final_density = obj(O::Add {
            argument1: DensityFunction::Constant(-0.5),
            argument2: obj(O::Mul {
                argument1: obj(O::YClampedGradient {
                    from_y: 0,
                    to_y: 10,
                    from_value: 1.0,
                    to_value: 0.0,
                }),
                argument2: obj(O::Invert {
                    argument: obj(O::YClampedGradient {
                        from_y: 0,
                        to_y: 10,
                        from_value: 1.0,
                        to_value: 0.0,
                    }),
                }),
            }),
        });
        let router =
            NoiseRouterEvaluator::new(9, &settings, &Registry::default(), &Registry::default());
        let dump = router.dump_program(RouterSlot::FinalDensity);
        assert!(!dump.iter().any(|l| l.contains("GatedAdd")), "{dump:?}");
        // y = 20: gradient 0, invert(0) = inf, 0 * inf = NaN; the plain lowering keeps it.
        assert!(router.sample(RouterSlot::FinalDensity, 0, 20, 0).is_nan());
    }

    /// The y-independence predicate accepts the vanilla climate shape (a 2D `shifted_noise`
    /// over `shift_a`/`shift_b` through 2D caches) and rejects every shape where y can leak in.
    /// Where it says yes, the values are bit-identical at every y.
    #[test]
    fn y_independence_predicate() {
        use oxide_core::ResourceLocation as Rl;
        use oxide_datapack::{DensityFunctionObject as O, NormalNoiseParameters};
        fn obj(o: O) -> DensityFunction {
            DensityFunction::Object(Box::new(o))
        }
        let noises: Registry<NormalNoiseParameters> = [
            (Rl::new("minecraft", "t"), NormalNoiseParameters { first_octave: -10, amplitudes: vec![1.5, 0.0, 1.0, 0.0, 0.0, 0.0] }),
            (Rl::new("minecraft", "offset"), NormalNoiseParameters { first_octave: -3, amplitudes: vec![1.0, 1.0, 1.0, 0.0] }),
        ]
        .into_iter()
        .collect();
        let shift = |o: O| obj(O::Cache2d { argument: obj(O::FlatCache { argument: obj(o) }) });
        let climate = |shift_y: f64, y_scale: f64| {
            obj(O::ShiftedNoise {
                noise: Rl::new("minecraft", "t"),
                xz_scale: 0.25,
                y_scale,
                shift_x: shift(O::ShiftA { argument: Rl::new("minecraft", "offset") }),
                shift_y: DensityFunction::Constant(shift_y),
                shift_z: shift(O::ShiftB { argument: Rl::new("minecraft", "offset") }),
            })
        };
        let cases: Vec<(&str, DensityFunction, bool)> = vec![
            ("2d shifted noise", climate(0.0, 0.0), true),
            ("nonzero shift_y", climate(3.5, 0.0), true),
            ("negative zero shift_y", climate(-0.0, 0.0), false),
            ("3d shifted noise", climate(0.0, 1.0), false),
            ("plain noise with y_scale 0", obj(O::Noise { noise: Rl::new("minecraft", "t"), xz_scale: 1.0, y_scale: 0.0 }), false),
            ("interpolated", obj(O::Interpolated { argument: climate(0.0, 0.0) }), false),
            ("y gradient", obj(O::YClampedGradient { from_y: -64, to_y: 320, from_value: 1.0, to_value: -1.0 }), false),
            ("sum of 2d terms", obj(O::Add { argument1: climate(0.0, 0.0), argument2: obj(O::Abs { argument: climate(0.0, 0.0) }) }), true),
        ];
        for (name, df, expected) in cases {
            let mut settings = minimal_settings();
            settings.noise_router.temperature = df;
            let router = NoiseRouterEvaluator::new(5, &settings, &Registry::default(), &noises);
            assert_eq!(router.is_y_independent(RouterSlot::Temperature), expected, "{name}");
            if expected {
                for (cx, cz) in [(0, 0), (-2, 7)] {
                    let caches = router.chunk_caches(cx, cz);
                    for (x, z) in [(cx * 16, cz * 16 + 3), (cx * 16 + 15, cz * 16 + 16), (cx * 16 - 40, cz * 16 + 99)] {
                        let first = router.sample_in_chunk(&caches, RouterSlot::Temperature, x, -64, z);
                        // Another chunk's caches give the same column value.
                        let other = router.chunk_caches(cx + 1, cz - 1);
                        assert_eq!(first.to_bits(), router.sample_in_chunk(&other, RouterSlot::Temperature, x, 77, z).to_bits(), "{name}, other chunk");
                        // Without caches the value differs (no flat_cache snapping) but is
                        // itself y-independent.
                        let bare = router.sample(RouterSlot::Temperature, x, -64, z);
                        assert_eq!(bare.to_bits(), router.sample(RouterSlot::Temperature, x, 200, z).to_bits(), "{name}, no caches");
                        for y in [-64, -1, 0, 1, 63, 64, 319] {
                            let v = router.sample_in_chunk(&caches, RouterSlot::Temperature, x, y, z);
                            assert_eq!(v.to_bits(), first.to_bits(), "{name} at {x} {y} {z}");
                        }
                    }
                }
            }
        }
    }

    /// `Program::run_lanes` must give every lane exactly the bits `Program::run` gives that
    /// position, with and without chunk caches, over a program that reaches blended noise, a
    /// normal noise with a zero-amplitude octave (plain, shifted and shift_a/shift_b), both
    /// `range_choice` branches, a gated add both open and shut, a `cache_once` hit (the same
    /// subtree behind two markers shares one slot) and a 2D cache.
    #[test]
    fn run_lanes_is_bit_identical_to_run() {
        use crate::compiled::Batch;
        use oxide_datapack::DensityFunctionObject as O;
        fn obj(o: O) -> DensityFunction {
            DensityFunction::Object(Box::new(o))
        }
        let noise_id = Rl::new("minecraft", "lanes_test");
        let noise = |xz_scale: f64, y_scale: f64| {
            obj(O::Noise {
                noise: noise_id.clone(),
                xz_scale,
                y_scale,
            })
        };
        let blended = || {
            obj(O::OldBlendedNoise {
                xz_scale: 0.25,
                y_scale: 0.125,
                xz_factor: 80.0,
                y_factor: 160.0,
                smear_scale_multiplier: 8.0,
            })
        };
        let once_blended = || obj(O::CacheOnce { argument: blended() });
        // `c + gradient * body`: shut (gradient exactly 0) above y = 200.
        let gated = obj(O::Add {
            argument1: DensityFunction::Constant(-0.078125),
            argument2: obj(O::Mul {
                argument1: obj(O::YClampedGradient {
                    from_y: 0,
                    to_y: 200,
                    from_value: 1.0,
                    to_value: 0.0,
                }),
                argument2: obj(O::Add {
                    argument1: DensityFunction::Constant(0.078125),
                    argument2: blended(),
                }),
            }),
        });
        let mut settings = minimal_settings();
        settings.noise_router.final_density = obj(O::Add {
            argument1: once_blended(),
            argument2: obj(O::RangeChoice {
                input: noise(1.0, 1.0),
                min_inclusive: 0.0,
                max_exclusive: 1000.0,
                when_in_range: obj(O::Add {
                    argument1: DensityFunction::Constant(64.0),
                    argument2: obj(O::ShiftedNoise {
                        noise: noise_id.clone(),
                        xz_scale: 0.5,
                        y_scale: 0.25,
                        shift_x: obj(O::ShiftA {
                            argument: noise_id.clone(),
                        }),
                        shift_y: DensityFunction::Constant(0.0),
                        shift_z: obj(O::ShiftB {
                            argument: noise_id.clone(),
                        }),
                    }),
                }),
                when_out_of_range: obj(O::Add {
                    argument1: once_blended(),
                    argument2: obj(O::Add {
                        argument1: obj(O::Cache2d {
                            argument: noise(0.75, 0.0),
                        }),
                        argument2: gated,
                    }),
                }),
            }),
        });
        let df_registry = Registry::default();
        let noise_registry: Registry<NormalNoiseParameters> = [(
            noise_id.clone(),
            NormalNoiseParameters {
                first_octave: -5,
                amplitudes: vec![1.0, 1.0, 0.5, 0.0, 0.25],
            },
        )]
        .into_iter()
        .collect();
        let router = NoiseRouterEvaluator::new(11, &settings, &df_registry, &noise_registry);
        let dump = router.dump_program(RouterSlot::FinalDensity);
        assert!(dump.iter().any(|l| l.contains("GatedAdd")), "not gated: {dump:?}");
        let program = &router.programs[RouterSlot::FinalDensity.index()];

        let (mut in_range, mut out_of_range) = (0, 0);
        for with_caches in [false, true] {
            for (chunk_x, chunk_z) in [(0, 0), (-3, 5)] {
                let lane_caches = router.chunk_caches(chunk_x, chunk_z);
                let plain_caches = router.chunk_caches(chunk_x, chunk_z);
                let lane_cx = RunCtx {
                    noises: &router.noises,
                    blended: &router.blended,
                    caches: with_caches.then_some(&lane_caches),
                };
                let plain_cx = RunCtx {
                    noises: &router.noises,
                    blended: &router.blended,
                    caches: with_caches.then_some(&plain_caches),
                };
                for (local_x, local_z) in [(0, 0), (4, 12), (16, 16), (7, 3)] {
                    let (x, z) = (chunk_x * 16 + local_x, chunk_z * 16 + local_z);
                    let ctxs: Vec<FunctionContext> =
                        (0..49).map(|cy| FunctionContext { x, y: -64 + cy * 8, z }).collect();
                    let lanes: Vec<u32> = (0..49).collect();
                    let mut out = vec![0.0; 49];
                    program.run_lanes(&lanes, &Batch::new(&ctxs), &lane_cx, &mut out);
                    for (ctx, value) in ctxs.iter().zip(&out) {
                        let expected = program.run(*ctx, &plain_cx);
                        assert_eq!(expected.to_bits(), value.to_bits(), "at {ctx:?}");
                        if expected > 32.0 {
                            in_range += 1;
                        } else {
                            out_of_range += 1;
                        }
                    }
                    // A strided subset of the column, so the lanes are not 0..k.
                    let sub: Vec<u32> = (0..49).step_by(3).collect();
                    let mut out = vec![0.0; sub.len()];
                    program.run_lanes(&sub, &Batch::new(&ctxs), &lane_cx, &mut out);
                    for (&l, value) in sub.iter().zip(&out) {
                        let expected = program.run(ctxs[l as usize], &plain_cx);
                        assert_eq!(expected.to_bits(), value.to_bits(), "subset lane {l}");
                    }
                }
            }
        }
        assert!(in_range > 20 && out_of_range > 20, "{in_range} / {out_of_range}");
    }

    /// The quart-invariance predicate accepts a `flat_cache` over noise and constants or
    /// gradients, and rejects exact-position noise, `cache_2d` over noise and `interpolated`.
    /// Where it says yes, every block of a quart column gives the quart origin's bits.
    #[test]
    fn quart_invariance_predicate() {
        use oxide_core::ResourceLocation as Rl;
        use oxide_datapack::{DensityFunctionObject as O, NormalNoiseParameters};
        fn obj(o: O) -> DensityFunction {
            DensityFunction::Object(Box::new(o))
        }
        let noises: Registry<NormalNoiseParameters> = [(
            Rl::new("minecraft", "n"),
            NormalNoiseParameters { first_octave: -4, amplitudes: vec![1.0, 1.0] },
        )]
        .into_iter()
        .collect();
        let noise = || obj(O::Noise { noise: Rl::new("minecraft", "n"), xz_scale: 1.0, y_scale: 1.0 });
        let gradient = || obj(O::YClampedGradient { from_y: -64, to_y: 320, from_value: 1.0, to_value: -1.0 });
        let cases: Vec<(&str, DensityFunction, bool)> = vec![
            ("flat_cache over 3d noise", obj(O::FlatCache { argument: noise() }), true),
            ("flat_cache plus a gradient", obj(O::Add { argument1: obj(O::FlatCache { argument: noise() }), argument2: gradient() }), true),
            ("gradient alone", gradient(), true),
            ("noise", noise(), false),
            ("cache_2d over noise", obj(O::Cache2d { argument: noise() }), false),
            ("cache_2d over flat_cache", obj(O::Cache2d { argument: obj(O::FlatCache { argument: noise() }) }), true),
            ("interpolated", obj(O::Interpolated { argument: obj(O::FlatCache { argument: noise() }) }), false),
            ("flat_cache times noise", obj(O::Mul { argument1: obj(O::FlatCache { argument: noise() }), argument2: noise() }), false),
        ];
        for (name, df, expected) in cases {
            let mut settings = minimal_settings();
            settings.noise_router.vegetation = df;
            let router = NoiseRouterEvaluator::new(3, &settings, &Registry::default(), &noises);
            assert_eq!(router.is_quart_invariant(RouterSlot::Vegetation), expected, "{name}");
            if expected {
                for (cx, cz) in [(0, 0), (-3, 2)] {
                    let caches = router.chunk_caches(cx, cz);
                    for y in [-64, 0, 77, 319] {
                        for (qx, qz) in [(0, 0), (4, 12), (12, 8)] {
                            let (ox, oz) = (cx * 16 + qx, cz * 16 + qz);
                            let origin = router.sample_in_chunk(&caches, RouterSlot::Vegetation, ox, y, oz);
                            for (dx, dz) in [(1, 0), (3, 3), (0, 2), (2, 1)] {
                                let v = router.sample_in_chunk(&caches, RouterSlot::Vegetation, ox + dx, y, oz + dz);
                                assert_eq!(v.to_bits(), origin.to_bits(), "{name} at {} {y} {}", ox + dx, oz + dz);
                            }
                        }
                    }
                }
            }
        }
    }
}
