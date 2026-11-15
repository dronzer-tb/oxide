//! Surface-rule evaluation: the pass that turns a column of `default_block` into grass, dirt,
//! sand, gravel, snow -- and bedrock, which vanilla spells as a `vertical_gradient` rule rather
//! than a special case.
//!
//! Runs after [`crate::fill_chunk`], over the terrain it produced. The rule tree comes from
//! `noise_settings.surface_rule` and is walked top-down per column, exactly as vanilla's
//! `SurfaceSystem` does: the first `minecraft:block` rule whose enclosing conditions all hold
//! wins for that position.
//!
//! Two known gaps, named rather than approximated:
//!
//! - `minecraft:bandlands` (badlands terracotta banding) places plain terracotta. The real
//!   thing is a 192-entry band table built from a seeded random; reproducing it from memory
//!   would look right and be wrong, which is worse than being visibly incomplete.
//! - `minecraft:temperature` uses the biome's base temperature with a height falloff. Vanilla
//!   also folds in a temperature noise and the `frozen` temperature modifier.
//!
//! Everything else in the 26.2 overworld rule set is implemented. Every reconstructed formula
//! carries a `PARITY-CHECK` marker: RNG and noise underneath are verified against the real
//! 26.2 jar, but this layer's arithmetic is not yet diffed against a vanilla chunk dump.

use std::collections::HashMap;

use oxide_core::{
    BlockState, ChunkData, ChunkPos, Heightmap, HeightmapType, RandomSource, ResourceLocation,
};
use oxide_datapack::{
    NoiseGeneratorSettings, SurfaceCondition, SurfaceRule, SurfaceType, VerticalAnchor,
};
use oxide_noise::{NoiseRouterEvaluator, NormalNoise, RouterSlot, WorldPositionalFactory};

use crate::fill::local_index;

/// Per-biome climate data the `minecraft:temperature` condition needs. Keyed by biome id; the
/// generator carries this instead of the whole biome registry, which it otherwise drops.
pub type BiomeTemperatures = HashMap<ResourceLocation, f32>;

/// Column-invariant state, computed once per (x, z).
struct Column {
    x: i32,
    z: i32,
    surface_depth: i32,
    /// Sampled on first read: only a `stone_depth` with a secondary depth range reads it, and
    /// most columns never reach one. A pure function of (x, z), so when it is sampled does not
    /// change its value.
    surface_secondary: std::cell::Cell<Option<f64>>,
    min_surface_level: i32,
}

/// Per-position state, updated as the scan walks down the column.
struct Cursor {
    x: i32,
    y: i32,
    z: i32,
    stone_depth_above: i32,
    stone_depth_below: i32,
    water_height: i32,
    /// Index into the chunk's biome table (see [`ChunkBiomes`]), not an id: the rule tree asks
    /// "is this biome one of ..." many times per position, and answers it from a precomputed
    /// mask instead of comparing strings.
    biome: usize,
}

const BAND_COUNT: usize = 64;

/// Block positions per section, in [`local_index`] order.
const SECTION_SLOTS: usize = 16 * 16 * 16;

fn generate_clay_bands(random: &mut impl RandomSource) -> [BlockState; BAND_COUNT] {
    let terracotta = BlockState::new(ResourceLocation::minecraft("terracotta"));
    let orange = BlockState::new(ResourceLocation::minecraft("orange_terracotta"));
    let yellow = BlockState::new(ResourceLocation::minecraft("yellow_terracotta"));
    let brown = BlockState::new(ResourceLocation::minecraft("brown_terracotta"));
    let red = BlockState::new(ResourceLocation::minecraft("red_terracotta"));
    let white = BlockState::new(ResourceLocation::minecraft("white_terracotta"));
    let light_gray = BlockState::new(ResourceLocation::minecraft("light_gray_terracotta"));

    let mut bands = std::array::from_fn(|_| terracotta.clone());

    let mut i = 0;
    while i < BAND_COUNT {
        i += random.next_int_bounded(5) as usize + 1;
        if i >= BAND_COUNT {
            break;
        }
        bands[i] = orange.clone();
    }

    make_bands(random, &mut bands, 1, &yellow);
    make_bands(random, &mut bands, 2, &brown);
    make_bands(random, &mut bands, 1, &red);

    let mut i = (random.next_int_bounded(9) + 5) as usize;
    while i < BAND_COUNT {
        make_band(
            &mut bands,
            i,
            (random.next_int_bounded(2) + 1) as usize,
            &white,
        );
        if i >= 1 && random.next_boolean() {
            make_band(&mut bands, i - 1, 1, &light_gray);
        }
        if i + 1 < BAND_COUNT && random.next_boolean() {
            make_band(&mut bands, i + 1, 1, &light_gray);
        }
        i += (random.next_int_bounded(5) + 2) as usize;
    }

    bands
}

fn make_bands(
    random: &mut impl RandomSource,
    bands: &mut [BlockState; BAND_COUNT],
    _count: usize,
    state: &BlockState,
) {
    let j = random.next_int_bounded(4) as usize + 1;
    for _ in 0..j {
        let mut l = random.next_int_bounded(BAND_COUNT as i32) as usize;
        let span = (random.next_int_bounded(3) + 1) as usize;
        for _ in 0..span {
            if l >= BAND_COUNT {
                break;
            }
            bands[l] = state.clone();
            l += 1;
        }
    }
}

fn make_band(
    bands: &mut [BlockState; BAND_COUNT],
    start: usize,
    length: usize,
    state: &BlockState,
) {
    for k in 0..length {
        if start + k < BAND_COUNT {
            bands[start + k] = state.clone();
        }
    }
}

/// The rule tree, compiled once per [`SurfaceSystem`]: every name the datapack spells as a
/// string -- noises, randomizers, anchors, result blocks -- is resolved here, so evaluating a
/// position does no hashing, no string comparison and no allocation.
enum CompiledRule<'a> {
    Sequence(Vec<CompiledRule<'a>>),
    Condition(CompiledCondition<'a>, Box<CompiledRule<'a>>),
    /// Index into [`SurfaceSystem::results`].
    Block(usize),
    Badlands,
}

/// The rule tree flattened into a straight-line program, evaluated by
/// [`SurfaceSystem::eval_program`]. A `sequence` is its children laid end to end, and a
/// `condition` is a test that jumps past its `then_run` when it fails: a `then_run` that
/// places nothing falls through to the next sibling, exactly as the tree walk's `find_map` did.
/// Conditions are tested in the tree walk's order, one at a time, so every memo and random
/// draw happens the same way.
enum Op<'a> {
    Test {
        condition: CompiledCondition<'a>,
        /// The condition sat under this many `not`s, an odd number: test its inverse. The
        /// inner condition is still evaluated exactly as the `Not` arm would.
        negate: bool,
        /// Where to continue when the test fails: the first op past this condition's
        /// `then_run`.
        skip: usize,
    },
    Block(usize),
    Badlands,
}

/// Appends `rule`'s program to `ops`.
fn flatten<'a>(rule: CompiledRule<'a>, ops: &mut Vec<Op<'a>>) {
    match rule {
        CompiledRule::Sequence(sequence) => {
            for inner in sequence {
                flatten(inner, ops);
            }
        }
        CompiledRule::Condition(mut condition, then_run) => {
            let mut negate = false;
            while let CompiledCondition::Not(inner) = condition {
                condition = *inner;
                negate = !negate;
            }
            let at = ops.len();
            ops.push(Op::Test {
                condition,
                negate,
                skip: 0,
            });
            flatten(*then_run, ops);
            let end = ops.len();
            if let Op::Test { skip, .. } = &mut ops[at] {
                *skip = end;
            }
        }
        CompiledRule::Block(result) => ops.push(Op::Block(result)),
        CompiledRule::Badlands => ops.push(Op::Badlands),
    }
}

enum CompiledCondition<'a> {
    /// Index into [`SurfaceSystem::biome_sets`], and so into each chunk's biome masks.
    Biome(usize),
    /// A 2D noise threshold depends on the column only, so its answer is remembered per column
    /// in memo slot `memo`.
    Noise2d {
        noise: Option<&'a NormalNoise>,
        min: f64,
        max: f64,
        memo: usize,
    },
    Noise3d {
        noise: Option<&'a NormalNoise>,
        min: f64,
        max: f64,
    },
    VerticalGradient {
        random: WorldPositionalFactory,
        true_y: i32,
        false_y: i32,
    },
    YAbove {
        anchor_y: i32,
        surface_depth_multiplier: i32,
        add_stone_depth: bool,
    },
    Water {
        offset: i32,
        surface_depth_multiplier: i32,
        add_stone_depth: bool,
    },
    Temperature,
    /// Column-only; remembered in memo slot `memo`.
    Steep {
        memo: usize,
    },
    Hole,
    AbovePreliminarySurface,
    StoneDepth {
        offset: i32,
        add_surface_depth: bool,
        secondary_depth_range: i32,
        ceiling: bool,
    },
    Not(Box<CompiledCondition<'a>>),
}

const MEMO_UNKNOWN: u8 = 0;
const MEMO_FALSE: u8 = 1;
const MEMO_TRUE: u8 = 2;

/// What the rule tree needs to know about the biomes of one chunk, as small tables indexed by
/// a per-chunk biome number.
struct ChunkBiomes {
    /// Per section: biome palette index -> chunk biome number.
    section_maps: Vec<Vec<u16>>,
    /// `biome_sets.len()` rows of `count` flags: is chunk biome `b` in biome set `s`?
    masks: Vec<bool>,
    /// Base temperature per chunk biome.
    temperatures: Vec<f32>,
    count: usize,
}

pub struct SurfaceSystem<'a> {
    settings: &'a NoiseGeneratorSettings,
    router: &'a NoiseRouterEvaluator,
    biome_temperatures: &'a BiomeTemperatures,
    min_y: i32,
    height: i32,
    /// Copied out of the chunk before the scan starts, because the scan mutates the chunk and
    /// the `Steep` condition has to read heights the fill pass computed, not rewritten ones.
    ocean_floor: Option<Heightmap>,
    /// The chunk's density-function caches. `find_top_surface` walks a column in cell-height
    /// steps through a deep tree, once per column, so sharing the fill pass's caches is the
    /// difference between cheap and dominating the whole chunk.
    caches: Option<&'a oxide_noise::ChunkCaches>,
    /// Resolved once rather than per column: these are sampled for every column in a chunk.
    surface_noise: Option<&'a NormalNoise>,
    surface_secondary_noise: Option<&'a NormalNoise>,
    clay_bands_offset_noise: Option<&'a NormalNoise>,
    temperature_noise: Option<&'a NormalNoise>,
    /// Built on first use: only badlands columns read it, and building it costs an MD5 and 64
    /// block states.
    clay_bands: std::cell::OnceCell<[BlockState; BAND_COUNT]>,
    program: Vec<Op<'a>>,
    /// The state each `Block` rule places, by the index the compiled rule carries.
    results: Vec<&'a BlockState>,
    /// The id list of each `biome` condition, by the index the compiled condition carries.
    biome_sets: Vec<&'a [ResourceLocation]>,
    /// One slot per column-only condition, cleared at the start of every column.
    memo: Vec<std::cell::Cell<u8>>,
    /// Column-only values the badlands rule and the temperature condition read.
    band_offset: std::cell::Cell<Option<i32>>,
    temperature_offset: std::cell::Cell<Option<f32>>,
}

/// Scratch state for [`SurfaceSystem::compile_rule`].
#[derive(Default)]
struct Compiler<'a> {
    results: Vec<&'a BlockState>,
    biome_sets: Vec<&'a [ResourceLocation]>,
    memo_slots: usize,
    /// `noise_threshold`-style conditions name their randomizer by string, and deriving a
    /// factory from that name costs an MD5 of it. Each distinct name is derived once.
    named_randoms: Vec<(&'a str, WorldPositionalFactory)>,
}

impl<'a> SurfaceSystem<'a> {
    pub fn new(
        settings: &'a NoiseGeneratorSettings,
        router: &'a NoiseRouterEvaluator,
        biome_temperatures: &'a BiomeTemperatures,
    ) -> Self {
        let min_y = settings.noise.min_y;
        let height = settings.noise.height;
        let mut compiler = Compiler::default();
        let rule = compile_rule(&settings.surface_rule, router, min_y, height, &mut compiler);
        let mut program = Vec::new();
        flatten(rule, &mut program);
        let memo = (0..compiler.memo_slots)
            .map(|_| std::cell::Cell::new(MEMO_UNKNOWN))
            .collect();

        Self {
            settings,
            router,
            biome_temperatures,
            min_y,
            height,
            ocean_floor: None,
            caches: None,
            surface_noise: router.noise(&ResourceLocation::minecraft("surface")),
            surface_secondary_noise: router
                .noise(&ResourceLocation::minecraft("surface_secondary")),
            clay_bands_offset_noise: router
                .noise(&ResourceLocation::minecraft("clay_bands_offset")),
            temperature_noise: router.noise(&ResourceLocation::minecraft("temperature")),
            clay_bands: std::cell::OnceCell::new(),
            program,
            results: compiler.results,
            biome_sets: compiler.biome_sets,
            memo,
            band_offset: std::cell::Cell::new(None),
            temperature_offset: std::cell::Cell::new(None),
        }
    }

    fn clay_bands(&self) -> &[BlockState; BAND_COUNT] {
        self.clay_bands.get_or_init(|| {
            let mut clay_random = self
                .router
                .positional_factory()
                .from_hash_of("minecraft:clay_bands");
            generate_clay_bands(&mut clay_random)
        })
    }

    /// The state a rule result stands for. Results past the rule tree's own are the clay
    /// bands, in band order.
    fn result_state(&self, result: usize) -> &BlockState {
        match self.results.get(result) {
            Some(state) => state,
            None => &self.clay_bands()[result - self.results.len()],
        }
    }

    /// Rewrites `chunk`'s `default_block` positions per the rule tree. Positions holding fluid
    /// or air are never rewritten -- vanilla only offers the rule tree a stone position.
    ///
    /// The column scan reads and writes *palette indices*, never `BlockState` values: each
    /// section's palette is classified once up front (see [`SectionClass`]), each biome is a
    /// small number (see [`ChunkBiomes`]), and a rule result is interned into a section's
    /// palette the first time that section receives it.
    pub fn apply(
        &mut self,
        chunk: &mut ChunkData,
        pos: ChunkPos,
        caches: &'a oxide_noise::ChunkCaches,
    ) {
        self.caches = Some(caches);
        self.ocean_floor = chunk.heightmaps.get(&HeightmapType::OceanFloorWg).cloned();
        let top_y = self.min_y + self.height - 1;

        // One pass over each section's palette (a handful of entries), rather than one
        // classification per block position.
        let classes: Vec<SectionClass> = chunk
            .sections
            .iter()
            .map(|section| {
                SectionClass::of(
                    section.block_states.palette(),
                    &self.settings.default_block,
                    &self.settings.default_fluid,
                )
            })
            .collect();
        let biomes = self.chunk_biomes(chunk);
        // Per section, per rule result: the palette index that result has in that section,
        // once it has been written there. `u32::MAX` = not interned yet.
        let result_count = self.results.len() + BAND_COUNT;
        let mut result_index = vec![u32::MAX; chunk.sections.len() * result_count];
        // Every position's kind, read off the chunk once, in slot order per section. The scan
        // of a column only ever reads positions it has not written yet (the current one, and
        // the run below it), and each column reads only itself, so reading the kinds before
        // any write is the same as reading them as the scan goes.
        let mut kinds: Vec<BlockKind> = Vec::with_capacity(chunk.sections.len() * SECTION_SLOTS);
        for (section, class) in chunk.sections.iter().zip(&classes) {
            let start = kinds.len();
            if class.all_air {
                kinds.resize(start + SECTION_SLOTS, BlockKind::Air);
            } else {
                kinds.extend(
                    section
                        .block_states
                        .indices()
                        .iter()
                        .map(|&index| class.kind(index)),
                );
            }
            debug_assert_eq!(kinds.len(), start + SECTION_SLOTS);
        }

        for local_z in 0..16usize {
            let z = pos.min_block_z() + local_z as i32;
            for local_x in 0..16usize {
                let x = pos.min_block_x() + local_x as i32;
                let column = self.column_state(x, z);
                let column_slot = local_index(local_x, 0, local_z);
                for slot in &self.memo {
                    slot.set(MEMO_UNKNOWN);
                }
                self.band_offset.set(None);
                self.temperature_offset.set(None);

                let mut stone_depth_above = 0;
                let mut water_height = i32::MIN;
                // Start of the current run of stone, tracked so stone_depth_below can be
                // derived without rescanning for every y.
                let mut run_bottom = i32::MAX;

                for section_index in (0..chunk.sections.len()).rev() {
                    let section_min_y = self.min_y + section_index as i32 * 16;
                    if section_min_y > top_y {
                        continue;
                    }
                    if classes[section_index].all_air {
                        // Sixteen air positions: exactly what the per-block arm below does.
                        stone_depth_above = 0;
                        water_height = i32::MIN;
                        run_bottom = i32::MAX;
                        continue;
                    }
                    for local_y in (0..16usize).rev() {
                        let y = section_min_y + local_y as i32;
                        let slot = local_index(local_x, local_y, local_z);
                        let kind = kinds[section_index * SECTION_SLOTS + slot];

                        if kind == BlockKind::Air {
                            stone_depth_above = 0;
                            water_height = i32::MIN;
                            run_bottom = i32::MAX;
                            continue;
                        }
                        if kind == BlockKind::DefaultFluid {
                            // PARITY-CHECK: vanilla records the position *above* the fluid
                            // column.
                            water_height = y + 1;
                            stone_depth_above = 0;
                            run_bottom = i32::MAX;
                            continue;
                        }

                        if run_bottom > y {
                            run_bottom = self.run_bottom(&kinds, column_slot, y);
                        }
                        stone_depth_above += 1;
                        let stone_depth_below = y - run_bottom + 1;

                        if kind != BlockKind::DefaultBlock {
                            continue;
                        }

                        let quart = ((local_y / 4) * 4 + local_z / 4) * 4 + local_x / 4;
                        let biome_palette_index =
                            chunk.sections[section_index].biomes.indices()[quart];
                        let cursor = Cursor {
                            x,
                            y,
                            z,
                            stone_depth_above,
                            stone_depth_below,
                            water_height,
                            biome: biomes.section_maps[section_index][biome_palette_index as usize]
                                as usize,
                        };
                        let Some(result) = self.eval_program(&cursor, &column, &biomes) else {
                            continue;
                        };
                        let cached = &mut result_index[section_index * result_count + result];
                        let block_states = &mut chunk.sections[section_index].block_states;
                        if *cached == u32::MAX {
                            // Interning can add a palette entry this section did not have. No
                            // classification needs it: kinds were read up front, and a written
                            // position is never read again.
                            *cached =
                                block_states.index_of_or_insert(self.result_state(result).clone());
                        }
                        block_states.set_index(slot, *cached);
                    }
                }
            }
        }
    }

    /// Numbers the distinct biomes of `chunk` and answers every biome question the rule tree
    /// can ask about them, once.
    fn chunk_biomes(&self, chunk: &ChunkData) -> ChunkBiomes {
        let mut ids: Vec<&ResourceLocation> = Vec::new();
        let section_maps: Vec<Vec<u16>> = chunk
            .sections
            .iter()
            .map(|section| {
                section
                    .biomes
                    .palette()
                    .iter()
                    .map(|id| match ids.iter().position(|known| *known == id) {
                        Some(index) => index as u16,
                        None => {
                            ids.push(id);
                            (ids.len() - 1) as u16
                        }
                    })
                    .collect()
            })
            .collect();
        let count = ids.len();
        let mut masks = Vec::with_capacity(self.biome_sets.len() * count);
        for set in &self.biome_sets {
            for id in &ids {
                masks.push(set.contains(id));
            }
        }
        let temperatures = ids
            .iter()
            .map(|id| self.biome_temperatures.get(*id).copied().unwrap_or(0.5))
            .collect();
        ChunkBiomes {
            section_maps,
            masks,
            temperatures,
            count,
        }
    }

    /// PARITY-CHECK: `surface` noise scaled by 2.75, offset 3.0, plus a quarter of a
    /// per-column random -- vanilla's `SurfaceSystem#getSurfaceDepth`, reconstructed.
    fn column_state(&self, x: i32, z: i32) -> Column {
        let surface = sample_noise(self.surface_noise, x as f64, 0.0, z as f64);
        let mut random = self.router.positional_factory().at(x, 0, z);
        let surface_depth = (surface * 2.75 + 3.0 + random.next_double() * 0.25) as i32;
        Column {
            x,
            z,
            surface_depth,
            surface_secondary: std::cell::Cell::new(None),
            min_surface_level: self.sample(RouterSlot::PreliminarySurfaceLevel, x, 0, z) as i32,
        }
    }

    fn surface_secondary(&self, column: &Column) -> f64 {
        match column.surface_secondary.get() {
            Some(value) => value,
            None => {
                let value = sample_noise(
                    self.surface_secondary_noise,
                    column.x as f64,
                    0.0,
                    column.z as f64,
                );
                column.surface_secondary.set(Some(value));
                value
            }
        }
    }

    /// Router sample through the chunk's caches when the surface pass is running inside one.
    fn sample(&self, slot: RouterSlot, x: i32, y: i32, z: i32) -> f64 {
        match self.caches {
            Some(caches) => self.router.sample_in_chunk(caches, slot, x, y, z),
            None => self.router.sample(slot, x, y, z),
        }
    }

    /// Lowest y of the contiguous non-air, non-fluid run containing `y`, read off the chunk's
    /// up-front `kinds` for the column whose slot at local y 0 is `column_slot`.
    fn run_bottom(&self, kinds: &[BlockKind], column_slot: usize, y: i32) -> i32 {
        let mut bottom = y;
        while bottom > self.min_y {
            let below = (bottom - 1 - self.min_y) as usize;
            let section_index = below / 16;
            let local_y = below % 16;
            match kinds.get(section_index * SECTION_SLOTS + local_y * 256 + column_slot) {
                Some(&kind) if kind != BlockKind::Air && kind != BlockKind::DefaultFluid => {
                    bottom -= 1;
                }
                _ => break,
            }
        }
        bottom
    }

    /// The result index of the first `block` rule whose enclosing conditions all hold.
    fn eval_program(
        &self,
        cursor: &Cursor,
        column: &Column,
        biomes: &ChunkBiomes,
    ) -> Option<usize> {
        let program = &self.program[..];
        let mut pc = 0;
        while let Some(op) = program.get(pc) {
            match op {
                Op::Test {
                    condition,
                    negate,
                    skip,
                } => {
                    if self.eval_condition(condition, cursor, column, biomes) != *negate {
                        pc += 1;
                    } else {
                        pc = *skip;
                    }
                }
                Op::Block(result) => return Some(*result),
                Op::Badlands => return Some(self.badlands(cursor)),
            }
        }
        None
    }

    /// The `bandlands` rule's result: a clay band picked by y and a per-column offset.
    fn badlands(&self, cursor: &Cursor) -> usize {
        let offset = match self.band_offset.get() {
            Some(offset) => offset,
            None => {
                let offset = match self.clay_bands_offset_noise {
                    Some(noise) => (noise.get_value(cursor.x as f64, 0.0, cursor.z as f64) * 4.0)
                        .round() as i32,
                    None => 0,
                };
                self.band_offset.set(Some(offset));
                offset
            }
        };
        let band = (cursor.y + offset).rem_euclid(BAND_COUNT as i32) as usize;
        self.results.len() + band
    }

    fn eval_condition(
        &self,
        condition: &CompiledCondition<'a>,
        cursor: &Cursor,
        column: &Column,
        biomes: &ChunkBiomes,
    ) -> bool {
        match condition {
            CompiledCondition::Biome(set) => biomes.masks[set * biomes.count + cursor.biome],

            CompiledCondition::Noise2d {
                noise,
                min,
                max,
                memo,
            } => self.memoized(*memo, || match noise {
                Some(n) => {
                    let value = n.get_value(cursor.x as f64, 0.0, cursor.z as f64);
                    value >= *min && value <= *max
                }
                None => false,
            }),

            CompiledCondition::Noise3d { noise, min, max } => match noise {
                Some(n) => {
                    let value = n.get_value(cursor.x as f64, cursor.y as f64, cursor.z as f64);
                    value >= *min && value <= *max
                }
                None => false,
            },

            // PARITY-CHECK: linear probability between the two anchors, sampled from the
            // shared positional factory keyed by `random_name` -- vanilla's
            // `SurfaceRules.VerticalGradientConditionSource`, reconstructed.
            CompiledCondition::VerticalGradient {
                random,
                true_y,
                false_y,
            } => {
                if cursor.y <= *true_y {
                    return true;
                }
                if cursor.y >= *false_y {
                    return false;
                }
                let span = (false_y - true_y) as f64;
                let probability = (false_y - cursor.y) as f64 / span;
                let mut random = random.at(cursor.x, cursor.y, cursor.z);
                // nextFloat, not nextDouble: vanilla compares a float draw here, and the two
                // consume the generator differently, so the choice changes every bedrock
                // position -- not just precision.
                (random.next_float() as f64) < probability
            }

            CompiledCondition::YAbove {
                anchor_y,
                surface_depth_multiplier,
                add_stone_depth,
            } => {
                let stone = if *add_stone_depth {
                    cursor.stone_depth_above
                } else {
                    0
                };
                cursor.y + stone >= anchor_y + column.surface_depth * surface_depth_multiplier
            }

            CompiledCondition::Water {
                offset,
                surface_depth_multiplier,
                add_stone_depth,
            } => {
                if cursor.water_height == i32::MIN {
                    return true;
                }
                let stone = if *add_stone_depth {
                    cursor.stone_depth_above
                } else {
                    0
                };
                cursor.y + stone
                    >= cursor.water_height
                        + offset
                        + column.surface_depth * surface_depth_multiplier
            }

            CompiledCondition::Temperature => {
                let base = biomes.temperatures[cursor.biome];
                let adjusted = if cursor.y > 80 {
                    let noise_val = match self.temperature_offset.get() {
                        Some(value) => value,
                        None => {
                            let value = match self.temperature_noise {
                                Some(noise) => {
                                    noise.get_value(
                                        cursor.x as f64 / 8.0,
                                        0.0,
                                        cursor.z as f64 / 8.0,
                                    ) as f32
                                        * 8.0
                                }
                                None => 0.0,
                            };
                            self.temperature_offset.set(Some(value));
                            value
                        }
                    };
                    base - (noise_val + (cursor.y - 80) as f32) * 0.05 / 40.0
                } else {
                    base
                };
                adjusted < 0.15
            }

            // PARITY-CHECK: "steep" when the ocean-floor height differs by 4+ blocks between
            // the columns one step away on Z -- vanilla's `Steep` condition, reconstructed.
            // Vanilla clamps the neighbour lookup into this chunk rather than reaching into
            // the next one, so edge columns compare against themselves; that clamp is kept.
            CompiledCondition::Steep { memo } => self.memoized(*memo, || {
                let Some(heightmap) = self.ocean_floor.as_ref() else {
                    return false;
                };
                let local_x = (cursor.x & 15) as usize;
                let local_z = (cursor.z & 15) as usize;
                let center = heightmap.get(local_x, local_z);
                let x_below = if local_x > 0 {
                    heightmap.get(local_x - 1, local_z)
                } else {
                    center
                };
                let x_above = if local_x < 15 {
                    heightmap.get(local_x + 1, local_z)
                } else {
                    center
                };
                let z_below = if local_z > 0 {
                    heightmap.get(local_x, local_z - 1)
                } else {
                    center
                };
                let z_above = if local_z < 15 {
                    heightmap.get(local_x, local_z + 1)
                } else {
                    center
                };
                (x_above - x_below).abs() >= 4 || (z_above - z_below).abs() >= 4
            }),

            CompiledCondition::Hole => column.surface_depth <= 0,

            CompiledCondition::AbovePreliminarySurface => cursor.y >= column.min_surface_level,

            CompiledCondition::StoneDepth {
                offset,
                add_surface_depth,
                secondary_depth_range,
                ceiling,
            } => {
                let depth = if *ceiling {
                    cursor.stone_depth_below
                } else {
                    cursor.stone_depth_above
                };
                let surface = if *add_surface_depth {
                    column.surface_depth
                } else {
                    0
                };
                let secondary = if *secondary_depth_range == 0 {
                    0
                } else {
                    // PARITY-CHECK: maps the secondary noise from [-1, 1] onto
                    // [0, secondary_depth_range].
                    map_range(
                        self.surface_secondary(column),
                        -1.0,
                        1.0,
                        0.0,
                        *secondary_depth_range as f64,
                    ) as i32
                };
                depth <= 1 + offset + surface + secondary
            }

            CompiledCondition::Not(invert) => !self.eval_condition(invert, cursor, column, biomes),
        }
    }

    /// A column-only condition's answer, computed on first ask within the column.
    fn memoized(&self, slot: usize, compute: impl FnOnce() -> bool) -> bool {
        match self.memo[slot].get() {
            MEMO_TRUE => true,
            MEMO_FALSE => false,
            _ => {
                let value = compute();
                self.memo[slot].set(if value { MEMO_TRUE } else { MEMO_FALSE });
                value
            }
        }
    }
}

fn sample_noise(noise: Option<&NormalNoise>, x: f64, y: f64, z: f64) -> f64 {
    match noise {
        Some(noise) => noise.get_value(x, y, z),
        // A datapack without the noise the rules ask for: 0.0 keeps the pass running
        // rather than aborting a chunk, and reads as "no contribution".
        None => 0.0,
    }
}

fn resolve_anchor(anchor: &VerticalAnchor, min_y: i32, height: i32) -> i32 {
    match anchor {
        VerticalAnchor::Absolute { absolute } => *absolute,
        VerticalAnchor::AboveBottom { above_bottom } => min_y + above_bottom,
        VerticalAnchor::BelowTop { below_top } => min_y + height - 1 - below_top,
    }
}

fn compile_rule<'a>(
    rule: &'a SurfaceRule,
    router: &'a NoiseRouterEvaluator,
    min_y: i32,
    height: i32,
    compiler: &mut Compiler<'a>,
) -> CompiledRule<'a> {
    match rule {
        SurfaceRule::Sequence { sequence } => CompiledRule::Sequence(
            sequence
                .iter()
                .map(|inner| compile_rule(inner, router, min_y, height, compiler))
                .collect(),
        ),
        SurfaceRule::Condition { if_true, then_run } => CompiledRule::Condition(
            compile_condition(if_true, router, min_y, height, compiler),
            Box::new(compile_rule(then_run, router, min_y, height, compiler)),
        ),
        SurfaceRule::Block { result_state } => {
            compiler.results.push(result_state);
            CompiledRule::Block(compiler.results.len() - 1)
        }
        SurfaceRule::Badlands {} => CompiledRule::Badlands,
    }
}

fn compile_condition<'a>(
    condition: &'a SurfaceCondition,
    router: &'a NoiseRouterEvaluator,
    min_y: i32,
    height: i32,
    compiler: &mut Compiler<'a>,
) -> CompiledCondition<'a> {
    let memo_slot = |compiler: &mut Compiler<'a>| {
        compiler.memo_slots += 1;
        compiler.memo_slots - 1
    };
    match condition {
        SurfaceCondition::Biome { biome_is } => {
            compiler.biome_sets.push(biome_is.as_slice());
            CompiledCondition::Biome(compiler.biome_sets.len() - 1)
        }
        SurfaceCondition::NoiseThreshold {
            noise,
            min_threshold,
            max_threshold,
            is_3d,
        } => {
            let noise = router.noise(noise);
            if *is_3d {
                CompiledCondition::Noise3d {
                    noise,
                    min: *min_threshold,
                    max: *max_threshold,
                }
            } else {
                CompiledCondition::Noise2d {
                    noise,
                    min: *min_threshold,
                    max: *max_threshold,
                    memo: memo_slot(compiler),
                }
            }
        }
        SurfaceCondition::VerticalGradient {
            random_name,
            true_at_and_below,
            false_at_and_above,
        } => {
            let known = compiler
                .named_randoms
                .iter()
                .find(|(name, _)| *name == random_name.as_str())
                .map(|(_, factory)| factory.clone());
            let random = match known {
                Some(factory) => factory,
                None => {
                    let factory = router
                        .positional_factory()
                        .from_hash_of(random_name)
                        .fork_positional();
                    compiler
                        .named_randoms
                        .push((random_name.as_str(), factory.clone()));
                    factory
                }
            };
            CompiledCondition::VerticalGradient {
                random,
                true_y: resolve_anchor(true_at_and_below, min_y, height),
                false_y: resolve_anchor(false_at_and_above, min_y, height),
            }
        }
        SurfaceCondition::YAbove {
            anchor,
            surface_depth_multiplier,
            add_stone_depth,
        } => CompiledCondition::YAbove {
            anchor_y: resolve_anchor(anchor, min_y, height),
            surface_depth_multiplier: *surface_depth_multiplier,
            add_stone_depth: *add_stone_depth,
        },
        SurfaceCondition::Water {
            offset,
            surface_depth_multiplier,
            add_stone_depth,
        } => CompiledCondition::Water {
            offset: *offset,
            surface_depth_multiplier: *surface_depth_multiplier,
            add_stone_depth: *add_stone_depth,
        },
        SurfaceCondition::Temperature {} => CompiledCondition::Temperature,
        SurfaceCondition::Steep {} => CompiledCondition::Steep {
            memo: memo_slot(compiler),
        },
        SurfaceCondition::Hole {} => CompiledCondition::Hole,
        SurfaceCondition::AbovePreliminarySurface {} => CompiledCondition::AbovePreliminarySurface,
        SurfaceCondition::StoneDepth {
            offset,
            add_surface_depth,
            secondary_depth_range,
            surface_type,
        } => CompiledCondition::StoneDepth {
            offset: *offset,
            add_surface_depth: *add_surface_depth,
            secondary_depth_range: *secondary_depth_range,
            ceiling: matches!(surface_type, SurfaceType::Ceiling),
        },
        SurfaceCondition::Not { invert } => CompiledCondition::Not(Box::new(compile_condition(
            invert, router, min_y, height, compiler,
        ))),
    }
}

/// Compares by name without building a `ResourceLocation`: this runs once per *palette entry*
/// (a handful per section), not once per block position.
fn is_air(state: &BlockState) -> bool {
    state.name.path() == "air" && state.name.namespace() == "minecraft"
}

/// The only three block distinctions the surface column scan makes. Everything the rule tree
/// needs to know about a position reduces to one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Air,
    DefaultFluid,
    DefaultBlock,
    /// Anything else -- already-surfaced ground, ore, deepslate. Counts toward stone depth but
    /// is never offered to the rule tree, matching the old `&state != default_block` guard.
    Other,
}

/// A section's palette, pre-classified into [`BlockKind`] per palette index, so the ~98k
/// "is this air / the default fluid / the default block?" questions a chunk's scan asks are
/// each a table read.
///
/// Behaviour is identical to comparing states by construction: the classification uses the
/// same [`is_air`] helper and the same `==` on `BlockState`, evaluated per palette entry
/// instead of per position.
struct SectionClass {
    kinds: Vec<BlockKind>,
    /// Every palette entry is air, so every position is: the scan can step over the section.
    all_air: bool,
}

impl SectionClass {
    fn of(palette: &[BlockState], default_block: &BlockState, default_fluid: &BlockState) -> Self {
        let mut class = Self {
            kinds: Vec::with_capacity(palette.len()),
            all_air: true,
        };
        class.extend(palette, default_block, default_fluid);
        class
    }

    /// Classifies the palette entries added since this was built. A palette only ever grows at
    /// its end, so the entries already classified stay valid.
    fn extend(
        &mut self,
        palette: &[BlockState],
        default_block: &BlockState,
        default_fluid: &BlockState,
    ) {
        for state in &palette[self.kinds.len().min(palette.len())..] {
            let kind = if is_air(state) {
                BlockKind::Air
            } else if state == default_fluid {
                BlockKind::DefaultFluid
            } else if state == default_block {
                BlockKind::DefaultBlock
            } else {
                BlockKind::Other
            };
            self.all_air &= kind == BlockKind::Air;
            self.kinds.push(kind);
        }
    }

    /// A palette index this table has no entry for can only come from a write made after it was
    /// built; `Other` is the conservative answer (counts as ground, never re-offered to the rule
    /// tree), which is what a freshly written surface block should be anyway.
    fn kind(&self, palette_index: u32) -> BlockKind {
        self.kinds
            .get(palette_index as usize)
            .copied()
            .unwrap_or(BlockKind::Other)
    }
}

/// Vanilla `Mth.map`: linear remap of `value` from one range onto another, unclamped.
fn map_range(value: f64, from_lo: f64, from_hi: f64, to_lo: f64, to_hi: f64) -> f64 {
    to_lo + (value - from_lo) * (to_hi - to_lo) / (from_hi - from_lo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::{ChunkStatus, HeightmapType};
    use oxide_datapack::{
        DensityFunction, DensityFunctionObject, NoiseDimensionSettings, NoiseRouter, Registry,
    };

    /// Solid at and below y=0, air above -- a step function, so every column has exactly one
    /// surface position and the rule under test has an unambiguous target.
    fn settings(surface_rule: SurfaceRule) -> NoiseGeneratorSettings {
        let step = DensityFunction::Object(Box::new(DensityFunctionObject::YClampedGradient {
            from_y: 0,
            to_y: 1,
            from_value: 1.0,
            to_value: -1.0,
        }));
        NoiseGeneratorSettings {
            sea_level: -64,
            disable_mob_generation: false,
            aquifers_enabled: false,
            ore_veins_enabled: false,
            legacy_random_source: false,
            default_block: BlockState::new(ResourceLocation::minecraft("stone")),
            default_fluid: BlockState::new(ResourceLocation::minecraft("water")),
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
                preliminary_surface_level: DensityFunction::Constant(0.0),
                initial_density_without_jaggedness: DensityFunction::Constant(0.0),
                final_density: step,
                vein_toggle: DensityFunction::Constant(0.0),
                vein_ridged: DensityFunction::Constant(0.0),
                vein_gap: DensityFunction::Constant(0.0),
            },
            surface_rule,
            spawn_target: Vec::new(),
        }
    }

    fn run(rule: SurfaceRule) -> (oxide_core::ChunkData, NoiseGeneratorSettings) {
        let settings = settings(rule);
        let df_registry = Registry::default();
        let noise_registry = Registry::default();
        let router = NoiseRouterEvaluator::new(42, &settings, &df_registry, &noise_registry);
        let temperatures = BiomeTemperatures::new();
        let chunk = crate::generate_chunk(
            ChunkPos::new(0, 0),
            &settings,
            &router,
            None,
            &temperatures,
            None,
        );
        (chunk, settings)
    }

    fn block_at(chunk: &oxide_core::ChunkData, x: usize, y: i32, z: usize) -> BlockState {
        let index = ((y - chunk.min_y) / 16) as usize;
        let local_y = (y - chunk.min_y).rem_euclid(16) as usize;
        chunk.sections[index]
            .block_states
            .get(local_index(x, local_y, z))
            .clone()
    }

    /// A bare `block` rule matches every stone position, so the whole solid column is rewritten
    /// -- the check is that the pass reaches stone at all and leaves air alone.
    #[test]
    fn block_rule_rewrites_stone_and_leaves_air_alone() {
        let (chunk, _) = run(SurfaceRule::Block {
            result_state: BlockState::new(ResourceLocation::minecraft("grass_block")),
        });
        assert_eq!(chunk.status, ChunkStatus::Surface);
        assert_eq!(
            block_at(&chunk, 0, 0, 0).name,
            ResourceLocation::minecraft("grass_block")
        );
        assert_eq!(
            block_at(&chunk, 0, 100, 0).name,
            ResourceLocation::minecraft("air")
        );
    }

    /// `stone_depth` with offset 0 and no extras matches only the topmost stone position, which
    /// is what every vanilla "put grass on the surface" rule is built from.
    #[test]
    fn stone_depth_floor_matches_only_the_top_block() {
        let (chunk, settings) = run(SurfaceRule::Condition {
            if_true: SurfaceCondition::StoneDepth {
                offset: 0,
                add_surface_depth: false,
                secondary_depth_range: 0,
                surface_type: SurfaceType::Floor,
            },
            then_run: Box::new(SurfaceRule::Block {
                result_state: BlockState::new(ResourceLocation::minecraft("grass_block")),
            }),
        });

        let surface_y = chunk.heightmaps[&HeightmapType::OceanFloorWg].get(0, 0) + chunk.min_y - 1;
        assert_eq!(
            block_at(&chunk, 0, surface_y, 0).name,
            ResourceLocation::minecraft("grass_block"),
            "top solid block at y={surface_y} should be grass"
        );
        assert_eq!(
            block_at(&chunk, 0, surface_y - 1, 0),
            settings.default_block,
            "the block under the surface must stay stone"
        );
    }

    /// Bedrock is a vertical_gradient rule in vanilla, not a special case: below the lower
    /// anchor it always matches, above the upper one it never does.
    #[test]
    fn vertical_gradient_is_certain_outside_its_anchors() {
        let (chunk, _) = run(SurfaceRule::Condition {
            if_true: SurfaceCondition::VerticalGradient {
                random_name: "minecraft:bedrock_floor".to_string(),
                true_at_and_below: VerticalAnchor::AboveBottom { above_bottom: 0 },
                false_at_and_above: VerticalAnchor::AboveBottom { above_bottom: 5 },
            },
            then_run: Box::new(SurfaceRule::Block {
                result_state: BlockState::new(ResourceLocation::minecraft("bedrock")),
            }),
        });

        assert_eq!(
            block_at(&chunk, 0, -64, 0).name,
            ResourceLocation::minecraft("bedrock"),
            "the world's bottom layer is always bedrock"
        );
        assert_eq!(
            block_at(&chunk, 0, -59, 0).name,
            ResourceLocation::minecraft("stone"),
            "at and above the upper anchor bedrock never appears"
        );
    }
}
