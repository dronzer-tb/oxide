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
use oxide_noise::{NoiseRouterEvaluator, RouterSlot, WorldPositionalFactory};

use crate::fill::local_index;

/// Per-biome climate data the `minecraft:temperature` condition needs. Keyed by biome id; the
/// generator carries this instead of the whole biome registry, which it otherwise drops.
pub type BiomeTemperatures = HashMap<ResourceLocation, f32>;

/// Column-invariant state, computed once per (x, z).
struct Column {
    surface_depth: i32,
    surface_secondary: f64,
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
    biome: ResourceLocation,
}

const BAND_COUNT: usize = 64;

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
        make_band(&mut bands, i, (random.next_int_bounded(2) + 1) as usize, &white);
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
    /// Built once rather than per column: these are looked up for every column in a chunk.
    surface_noise_id: ResourceLocation,
    surface_secondary_noise_id: ResourceLocation,
    clay_bands_offset_id: ResourceLocation,
    temperature_noise_id: ResourceLocation,
    clay_bands: [BlockState; BAND_COUNT],
    /// `noise_threshold`-style conditions name their randomizer by string, and deriving a
    /// factory from that name costs an MD5 of it. The names are a handful of constants out of
    /// the rule tree and the derivation is pure, so each is derived once per chunk instead of
    /// once per block considered -- the profile had 2.5% of generation sitting in MD5.
    named_randoms: std::cell::RefCell<Vec<(String, WorldPositionalFactory)>>,
}

impl<'a> SurfaceSystem<'a> {
    pub fn new(
        settings: &'a NoiseGeneratorSettings,
        router: &'a NoiseRouterEvaluator,
        biome_temperatures: &'a BiomeTemperatures,
    ) -> Self {
        let mut clay_random = router
            .positional_factory()
            .from_hash_of("minecraft:clay_bands");
        let clay_bands = generate_clay_bands(&mut clay_random);

        Self {
            settings,
            router,
            biome_temperatures,
            min_y: settings.noise.min_y,
            height: settings.noise.height,
            ocean_floor: None,
            caches: None,
            surface_noise_id: ResourceLocation::minecraft("surface"),
            surface_secondary_noise_id: ResourceLocation::minecraft("surface_secondary"),
            clay_bands_offset_id: ResourceLocation::minecraft("clay_bands_offset"),
            temperature_noise_id: ResourceLocation::minecraft("temperature"),
            clay_bands,
            named_randoms: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Rewrites `chunk`'s `default_block` positions per the rule tree. Positions holding fluid
    /// or air are never rewritten -- vanilla only offers the rule tree a stone position.
    ///
    /// The column scan reads *palette indices*, never `BlockState` values. A chunk is ~98k
    /// positions and the old scan cloned a `BlockState` -- a `ResourceLocation` plus a
    /// `BTreeMap<String, String>` -- at every one of them, then answered "is this air?" by
    /// comparing strings. `perf` put ~15% of total generation time in `malloc`/`free`,
    /// `BTreeMap::clone_subtree`, `BTreeMap::eq` and `drop_glue<BlockState>` because of it.
    /// Classifying each section's palette once up front (see [`SectionClass`]) turns the inner
    /// loop into `u32` compares against three precomputed indices, with zero allocation.
    pub fn apply(
        &mut self,
        chunk: &mut ChunkData,
        pos: ChunkPos,
        caches: &'a oxide_noise::ChunkCaches,
    ) {
        self.caches = Some(caches);
        self.ocean_floor = chunk.heightmaps.get(&HeightmapType::OceanFloorWg).cloned();
        let rule = &self.settings.surface_rule;
        let top_y = self.min_y + self.height - 1;

        // One pass over each section's palette (a handful of entries), rather than one
        // classification per block position.
        let mut classes: Vec<SectionClass> = chunk
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

        for local_z in 0..16usize {
            let z = pos.min_block_z() + local_z as i32;
            for local_x in 0..16usize {
                let x = pos.min_block_x() + local_x as i32;
                let column = self.column_state(x, z);

                let mut stone_depth_above = 0;
                let mut water_height = i32::MIN;
                // Start of the current run of stone, tracked so stone_depth_below can be
                // derived without rescanning for every y.
                let mut run_bottom = i32::MAX;

                for y in (self.min_y..=top_y).rev() {
                    let Some(kind) = self.classify(chunk, &classes, local_x, y, local_z) else {
                        continue;
                    };

                    if kind == BlockKind::Air {
                        stone_depth_above = 0;
                        water_height = i32::MIN;
                        run_bottom = i32::MAX;
                        continue;
                    }
                    if kind == BlockKind::DefaultFluid {
                        // PARITY-CHECK: vanilla records the position *above* the fluid column.
                        water_height = y + 1;
                        stone_depth_above = 0;
                        run_bottom = i32::MAX;
                        continue;
                    }

                    if run_bottom > y {
                        run_bottom = self.run_bottom(chunk, &classes, local_x, y, local_z);
                    }
                    stone_depth_above += 1;
                    let stone_depth_below = y - run_bottom + 1;

                    if kind != BlockKind::DefaultBlock {
                        continue;
                    }

                    let cursor = Cursor {
                        x,
                        y,
                        z,
                        stone_depth_above,
                        stone_depth_below,
                        water_height,
                        biome: self.biome_at(chunk, local_x, y, local_z),
                    };
                    if let Some(result) = self.eval_rule(rule, &cursor, &column) {
                        self.set_block(chunk, local_x, y, local_z, result);
                        // A write can intern a palette entry this section did not have, which
                        // would leave its cached classification short an index. Re-derive just
                        // that section's -- once per actual write, not per position scanned.
                        if let Some(si) = self.section_of(y) {
                            if let (Some(section), Some(slot)) =
                                (chunk.sections.get(si), classes.get_mut(si))
                            {
                                *slot = SectionClass::of(
                                    section.block_states.palette(),
                                    &self.settings.default_block,
                                    &self.settings.default_fluid,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// PARITY-CHECK: `surface` noise scaled by 2.75, offset 3.0, plus a quarter of a
    /// per-column random -- vanilla's `SurfaceSystem#getSurfaceDepth`, reconstructed.
    fn column_state(&self, x: i32, z: i32) -> Column {
        let surface = self.sample_noise(&self.surface_noise_id, x as f64, 0.0, z as f64);
        let mut random = self.router.positional_factory().at(x, 0, z);
        let surface_depth = (surface * 2.75 + 3.0 + random.next_double() * 0.25) as i32;
        Column {
            surface_depth,
            surface_secondary: self.sample_noise(
                &self.surface_secondary_noise_id,
                x as f64,
                0.0,
                z as f64,
            ),
            min_surface_level: self.sample(RouterSlot::PreliminarySurfaceLevel, x, 0, z) as i32,
        }
    }

    /// Router sample through the chunk's caches when the surface pass is running inside one.
    fn sample(&self, slot: RouterSlot, x: i32, y: i32, z: i32) -> f64 {
        match self.caches {
            Some(caches) => self.router.sample_in_chunk(caches, slot, x, y, z),
            None => self.router.sample(slot, x, y, z),
        }
    }

    fn sample_noise(&self, id: &ResourceLocation, x: f64, y: f64, z: f64) -> f64 {
        match self.router.noise(id) {
            Some(noise) => noise.get_value(x, y, z),
            // A datapack without the noise the rules ask for: 0.0 keeps the pass running
            // rather than aborting a chunk, and reads as "no contribution".
            None => 0.0,
        }
    }

    /// Lowest y of the contiguous non-air, non-fluid run containing `y`.
    fn run_bottom(
        &self,
        chunk: &ChunkData,
        classes: &[SectionClass],
        local_x: usize,
        y: i32,
        local_z: usize,
    ) -> i32 {
        let mut bottom = y;
        while bottom > self.min_y {
            match self.classify(chunk, classes, local_x, bottom - 1, local_z) {
                Some(kind) if kind != BlockKind::Air && kind != BlockKind::DefaultFluid => {
                    bottom -= 1;
                }
                _ => break,
            }
        }
        bottom
    }

    fn eval_rule(
        &self,
        rule: &SurfaceRule,
        cursor: &Cursor,
        column: &Column,
    ) -> Option<BlockState> {
        match rule {
            SurfaceRule::Sequence { sequence } => sequence
                .iter()
                .find_map(|inner| self.eval_rule(inner, cursor, column)),
            SurfaceRule::Condition { if_true, then_run } => {
                if self.eval_condition(if_true, cursor, column) {
                    self.eval_rule(then_run, cursor, column)
                } else {
                    None
                }
            }
            SurfaceRule::Block { result_state } => Some(result_state.clone()),
            SurfaceRule::Badlands {} => {
                let offset_noise = self.router.noise(&self.clay_bands_offset_id);
                let offset = match offset_noise {
                    Some(noise) => {
                        (noise.get_value(cursor.x as f64, 0.0, cursor.z as f64) * 4.0).round()
                            as i32
                    }
                    None => 0,
                };
                let band = (cursor.y + offset).rem_euclid(BAND_COUNT as i32) as usize;
                Some(self.clay_bands[band].clone())
            }
        }
    }

    /// The positional factory a rule's named randomizer forks to, derived once per name.
    fn named_random(&self, name: &str) -> WorldPositionalFactory {
        if let Some((_, factory)) = self
            .named_randoms
            .borrow()
            .iter()
            .find(|(known, _)| known == name)
        {
            return factory.clone();
        }
        let factory = self
            .router
            .positional_factory()
            .from_hash_of(name)
            .fork_positional();
        self.named_randoms
            .borrow_mut()
            .push((name.to_string(), factory.clone()));
        factory
    }

    fn eval_condition(
        &self,
        condition: &SurfaceCondition,
        cursor: &Cursor,
        column: &Column,
    ) -> bool {
        match condition {
            SurfaceCondition::Biome { biome_is } => biome_is.contains(&cursor.biome),

            SurfaceCondition::NoiseThreshold {
                noise,
                min_threshold,
                max_threshold,
                is_3d,
            } => {
                let y = if *is_3d { cursor.y as f64 } else { 0.0 };
                let value = match self.router.noise(noise) {
                    Some(n) => n.get_value(cursor.x as f64, y, cursor.z as f64),
                    None => return false,
                };
                value >= *min_threshold && value <= *max_threshold
            }

            // PARITY-CHECK: linear probability between the two anchors, sampled from the
            // shared positional factory keyed by `random_name` -- vanilla's
            // `SurfaceRules.VerticalGradientConditionSource`, reconstructed.
            SurfaceCondition::VerticalGradient {
                random_name,
                true_at_and_below,
                false_at_and_above,
            } => {
                let true_y = self.resolve_anchor(true_at_and_below);
                let false_y = self.resolve_anchor(false_at_and_above);
                if cursor.y <= true_y {
                    return true;
                }
                if cursor.y >= false_y {
                    return false;
                }
                let span = (false_y - true_y) as f64;
                let probability = (false_y - cursor.y) as f64 / span;
                let mut random = self
                    .named_random(random_name)
                    .at(cursor.x, cursor.y, cursor.z);
                // nextFloat, not nextDouble: vanilla compares a float draw here, and the two
                // consume the generator differently, so the choice changes every bedrock
                // position -- not just precision.
                (random.next_float() as f64) < probability
            }

            SurfaceCondition::YAbove {
                anchor,
                surface_depth_multiplier,
                add_stone_depth,
            } => {
                let stone = if *add_stone_depth {
                    cursor.stone_depth_above
                } else {
                    0
                };
                cursor.y + stone
                    >= self.resolve_anchor(anchor) + column.surface_depth * surface_depth_multiplier
            }

            SurfaceCondition::Water {
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

            SurfaceCondition::Temperature {} => {
                let base = self
                    .biome_temperatures
                    .get(&cursor.biome)
                    .copied()
                    .unwrap_or(0.5);
                let adjusted = if cursor.y > 80 {
                    let temp_noise = self.router.noise(&self.temperature_noise_id);
                    let noise_val = match temp_noise {
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
            SurfaceCondition::Steep {} => {
                let Some(heightmap) = self.ocean_floor.as_ref() else {
                    return false;
                };
                let local_x = (cursor.x & 15) as usize;
                let local_z = (cursor.z & 15) as usize;
                let center = heightmap.get(local_x, local_z);
                let x_below = if local_x > 0 { heightmap.get(local_x - 1, local_z) } else { center };
                let x_above = if local_x < 15 { heightmap.get(local_x + 1, local_z) } else { center };
                let z_below = if local_z > 0 { heightmap.get(local_x, local_z - 1) } else { center };
                let z_above = if local_z < 15 { heightmap.get(local_x, local_z + 1) } else { center };
                (x_above - x_below).abs() >= 4 || (z_above - z_below).abs() >= 4
            }

            SurfaceCondition::Hole {} => column.surface_depth <= 0,

            SurfaceCondition::AbovePreliminarySurface {} => cursor.y >= column.min_surface_level,

            SurfaceCondition::StoneDepth {
                offset,
                add_surface_depth,
                secondary_depth_range,
                surface_type,
            } => {
                let depth = match surface_type {
                    SurfaceType::Floor => cursor.stone_depth_above,
                    SurfaceType::Ceiling => cursor.stone_depth_below,
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
                        column.surface_secondary,
                        -1.0,
                        1.0,
                        0.0,
                        *secondary_depth_range as f64,
                    ) as i32
                };
                depth <= 1 + offset + surface + secondary
            }

            SurfaceCondition::Not { invert } => !self.eval_condition(invert, cursor, column),
        }
    }

    fn resolve_anchor(&self, anchor: &VerticalAnchor) -> i32 {
        match anchor {
            VerticalAnchor::Absolute { absolute } => *absolute,
            VerticalAnchor::AboveBottom { above_bottom } => self.min_y + above_bottom,
            VerticalAnchor::BelowTop { below_top } => self.min_y + self.height - 1 - below_top,
        }
    }

    fn section_of(&self, y: i32) -> Option<usize> {
        if y < self.min_y || y >= self.min_y + self.height {
            return None;
        }
        Some(((y - self.min_y) / 16) as usize)
    }

    fn get_block(
        &self,
        chunk: &ChunkData,
        local_x: usize,
        y: i32,
        local_z: usize,
    ) -> Option<BlockState> {
        let index = self.section_of(y)?;
        let section = chunk.sections.get(index)?;
        let local_y = (y - self.min_y).rem_euclid(16) as usize;
        Some(
            section
                .block_states
                .get(local_index(local_x, local_y, local_z))
                .clone(),
        )
    }

    /// What the block at `(local_x, y, local_z)` is, as a three-way tag read off the section's
    /// precomputed palette classification. This is the allocation-free replacement for
    /// `get_block` in the column scan: one `u32` array read plus index compares, against a
    /// `BlockState` clone (heap `BTreeMap`) plus string comparison before.
    fn classify(
        &self,
        chunk: &ChunkData,
        classes: &[SectionClass],
        local_x: usize,
        y: i32,
        local_z: usize,
    ) -> Option<BlockKind> {
        let index = self.section_of(y)?;
        let section = chunk.sections.get(index)?;
        let class = classes.get(index)?;
        let local_y = (y - self.min_y).rem_euclid(16) as usize;
        let palette_index =
            section.block_states.indices()[local_index(local_x, local_y, local_z)];
        Some(class.kind(palette_index))
    }

    fn set_block(
        &self,
        chunk: &mut ChunkData,
        local_x: usize,
        y: i32,
        local_z: usize,
        state: BlockState,
    ) {
        let Some(index) = self.section_of(y) else {
            return;
        };
        let Some(section) = chunk.sections.get_mut(index) else {
            return;
        };
        let local_y = (y - self.min_y).rem_euclid(16) as usize;
        section
            .block_states
            .set(local_index(local_x, local_y, local_z), state);
    }

    fn biome_at(
        &self,
        chunk: &ChunkData,
        local_x: usize,
        y: i32,
        local_z: usize,
    ) -> ResourceLocation {
        let fallback = || ResourceLocation::minecraft("plains");
        let Some(index) = self.section_of(y) else {
            return fallback();
        };
        let Some(section) = chunk.sections.get(index) else {
            return fallback();
        };
        let local_y = (y - self.min_y).rem_euclid(16) as usize;
        let quart = ((local_y / 4) * 4 + local_z / 4) * 4 + local_x / 4;
        section.biomes.get(quart).clone()
    }
}

/// Compares by name without building a `ResourceLocation`: this runs once per *palette entry*
/// now (a handful per section), rather than once per block position.
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

/// A section's palette, pre-classified into [`BlockKind`] per palette index.
///
/// The scan asks "is this position air / the default fluid / the default block?" ~98k times per
/// chunk. Answering that from the `BlockState` itself meant cloning a heap `BTreeMap` and
/// comparing strings every time. A section's palette holds only a handful of distinct states,
/// so classifying it once and then indexing this table makes each of those 98k questions a
/// bounds-checked byte read.
///
/// Behaviour is identical by construction: the classification uses the same [`is_air`] helper
/// and the same `==` on `BlockState` the inline checks used, just evaluated per palette entry
/// instead of per position.
struct SectionClass {
    kinds: Vec<BlockKind>,
}

impl SectionClass {
    fn of(palette: &[BlockState], default_block: &BlockState, default_fluid: &BlockState) -> Self {
        Self {
            kinds: palette
                .iter()
                .map(|state| {
                    if is_air(state) {
                        BlockKind::Air
                    } else if state == default_fluid {
                        BlockKind::DefaultFluid
                    } else if state == default_block {
                        BlockKind::DefaultBlock
                    } else {
                        BlockKind::Other
                    }
                })
                .collect(),
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
