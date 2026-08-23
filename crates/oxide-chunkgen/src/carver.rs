//! Cave and ravine carving: the pass that cuts tunnels and canyons out of finished terrain.
//!
//! Ported from the decompiled 26.2 `WorldCarver`, `CaveWorldCarver` and `CanyonWorldCarver`
//! rather than reconstructed -- the step-by-step tunnel walk, the RNG draw order inside it, and
//! `setLargeFeatureSeed` all have to match vanilla exactly or the caves land somewhere else
//! entirely. Divergences that remain are marked `PARITY-CHECK` and listed here:
//!
//! - **Replaceable blocks.** Vanilla gates carving on the `replaceable` *block tag* from the
//!   carver config. Block tags are not modelled, so anything that is not air, fluid or bedrock
//!   is carveable. For the terrain this crate produces -- stone, dirt, grass, sand, gravel --
//!   that is the same set the vanilla tag names.
//! - **Carved substance.** Vanilla asks the aquifer what to put in a carved block; below the
//!   carver's `lava_level` it is lava regardless. Aquifers are not implemented, so a carved
//!   block becomes air, or lava below `lava_level`. Vanilla with aquifers *disabled* would
//!   flood every cave below sea level with water, which is not what the overworld looks like;
//!   air is the closer wrong answer until aquifers land.
//! - **Grass repair.** Vanilla re-runs the surface rule on dirt exposed under a carved-away
//!   grass block. Not done here, so a cave breaching the surface leaves bare dirt.

use oxide_core::{BlockState, ChunkData, ChunkPos, LegacyRandom, RandomSource, ResourceLocation};
use oxide_datapack::{
    CanyonCarverConfig, CarverConfig, CaveCarverConfig, ConfiguredCarver, NoiseGeneratorSettings,
    VerticalAnchor,
};

use crate::fill::local_index;

/// Chunks either side of the one being generated that may carve into it. Vanilla's
/// `applyCarvers` hardcodes 8.
const CARVER_CHUNK_RANGE: i32 = 8;

/// `WorldCarver#getRange`, the carver's own reach in chunks, used to size tunnel length.
const CARVER_RANGE: i32 = 4;

/// Vanilla leaves this many blocks below the world top uncarved.
const PROTECTED_BLOCKS_ON_TOP: i32 = 7;

/// Which positions in this chunk a carver has already visited, so overlapping carvers do not
/// re-carve (and re-count) the same block. Vanilla's `CarvingMask`.
pub struct CarvingMask {
    min_y: i32,
    height: i32,
    bits: Vec<u64>,
}

impl CarvingMask {
    fn new(min_y: i32, height: i32) -> Self {
        let count = 16 * 16 * height as usize;
        Self {
            min_y,
            height,
            bits: vec![0; count.div_ceil(64)],
        }
    }

    fn index(&self, x: usize, y: i32, z: usize) -> Option<usize> {
        let local_y = y - self.min_y;
        if local_y < 0 || local_y >= self.height {
            return None;
        }
        Some((local_y as usize * 16 + z) * 16 + x)
    }

    fn get(&self, x: usize, y: i32, z: usize) -> bool {
        match self.index(x, y, z) {
            Some(index) => self.bits[index / 64] & (1 << (index % 64)) != 0,
            None => true, // out of range reads as "already handled", so nothing is carved there
        }
    }

    fn set(&mut self, x: usize, y: i32, z: usize) {
        if let Some(index) = self.index(x, y, z) {
            self.bits[index / 64] |= 1 << (index % 64);
        }
    }
}

/// Everything carving needs that is not the chunk itself.
pub struct CarverWorld<'a> {
    pub settings: &'a NoiseGeneratorSettings,
    /// The world seed, as passed to `applyCarvers`.
    pub seed: i64,
}

impl CarverWorld<'_> {
    fn min_y(&self) -> i32 {
        self.settings.noise.min_y
    }

    fn height(&self) -> i32 {
        self.settings.noise.height
    }

    fn resolve_anchor(&self, anchor: &VerticalAnchor) -> i32 {
        match anchor {
            VerticalAnchor::Absolute { absolute } => *absolute,
            VerticalAnchor::AboveBottom { above_bottom } => self.min_y() + above_bottom,
            VerticalAnchor::BelowTop { below_top } => self.min_y() + self.height() - 1 - below_top,
        }
    }
}

/// Runs every carver that can reach this chunk.
///
/// `carvers_at` returns the carvers configured for the biome at a source chunk, in the order the
/// biome lists them -- the index is part of the per-chunk seed, so order is load-bearing.
pub fn apply_carvers(
    chunk: &mut ChunkData,
    pos: ChunkPos,
    world: &CarverWorld,
    carvers_at: &dyn Fn(ChunkPos) -> Vec<ConfiguredCarver>,
) {
    let mut mask = CarvingMask::new(world.min_y(), world.height());
    let mut random = LegacyRandom::new(0);

    for dx in -CARVER_CHUNK_RANGE..=CARVER_CHUNK_RANGE {
        for dz in -CARVER_CHUNK_RANGE..=CARVER_CHUNK_RANGE {
            let source = ChunkPos::new(pos.x + dx, pos.z + dz);
            for (index, carver) in carvers_at(source).into_iter().enumerate() {
                random.set_large_feature_seed(
                    world.seed.wrapping_add(index as i64),
                    source.x,
                    source.z,
                );
                match &carver {
                    ConfiguredCarver::Cave { config } | ConfiguredCarver::NetherCave { config } => {
                        if random.next_float() <= config.base.probability {
                            carve_cave(chunk, pos, world, config, &mut random, source, &mut mask);
                        }
                    }
                    ConfiguredCarver::Canyon { config } => {
                        if random.next_float() <= config.base.probability {
                            carve_canyon(chunk, pos, world, config, &mut random, source, &mut mask);
                        }
                    }
                }
            }
        }
    }
}

/// `CaveWorldCarver#carve`.
fn carve_cave(
    chunk: &mut ChunkData,
    pos: ChunkPos,
    world: &CarverWorld,
    config: &CaveCarverConfig,
    random: &mut LegacyRandom,
    source: ChunkPos,
    mask: &mut CarvingMask,
) {
    let max_distance = (CARVER_RANGE * 2 - 1) * 16;
    // Nested nextInt is vanilla's own shape: it biases hard toward few caves. Split into
    // statements because the draws must happen inside-out, in this order.
    let bound_inner = random.next_int_bounded(15) + 1;
    let bound_outer = random.next_int_bounded(bound_inner) + 1;
    let cave_count = random.next_int_bounded(bound_outer);

    for _ in 0..cave_count {
        let x = (source.min_block_x() + random.next_int_bounded(16)) as f64;
        let y = config
            .y
            .sample(random, &|anchor| world.resolve_anchor(anchor)) as f64;
        let z = (source.min_block_z() + random.next_int_bounded(16)) as f64;
        let horizontal_multiplier = config.horizontal_radius_multiplier.sample(random) as f64;
        let vertical_multiplier = config.vertical_radius_multiplier.sample(random) as f64;
        let floor_level = config.floor_level.sample(random) as f64;

        let mut tunnels = 1;
        if random.next_int_bounded(4) == 0 {
            let y_scale = config.y_scale.sample(random) as f64;
            let thickness = 1.0 + random.next_float() * 6.0;
            carve_room(
                chunk,
                pos,
                world,
                &config.base,
                x,
                y,
                z,
                thickness,
                y_scale,
                floor_level,
                mask,
            );
            tunnels += random.next_int_bounded(4);
        }

        for _ in 0..tunnels {
            let horizontal_rotation = random.next_float() * std::f32::consts::TAU;
            let vertical_rotation = (random.next_float() - 0.5) / 4.0;
            let thickness = cave_thickness(random);
            let distance = max_distance - random.next_int_bounded(max_distance / 4);
            let tunnel_seed = random.next_long();
            carve_tunnel(
                chunk,
                pos,
                world,
                &config.base,
                tunnel_seed,
                x,
                y,
                z,
                horizontal_multiplier,
                vertical_multiplier,
                thickness,
                horizontal_rotation,
                vertical_rotation,
                0,
                distance,
                1.0,
                floor_level,
                mask,
            );
        }
    }
}

/// `CaveWorldCarver#getThickness`.
fn cave_thickness(random: &mut LegacyRandom) -> f32 {
    let mut thickness = random.next_float() * 2.0 + random.next_float();
    if random.next_int_bounded(10) == 0 {
        thickness *= random.next_float() * random.next_float() * 3.0 + 1.0;
    }
    thickness
}

/// `CaveWorldCarver#createRoom`.
#[allow(clippy::too_many_arguments)]
fn carve_room(
    chunk: &mut ChunkData,
    pos: ChunkPos,
    world: &CarverWorld,
    config: &CarverConfig,
    x: f64,
    y: f64,
    z: f64,
    thickness: f32,
    y_scale: f64,
    floor_level: f64,
    mask: &mut CarvingMask,
) {
    // sin(PI/2) == 1, written as vanilla writes it.
    let horizontal_radius = 1.5 + (std::f64::consts::FRAC_PI_2.sin() * thickness as f64);
    let vertical_radius = horizontal_radius * y_scale;
    carve_ellipsoid(
        chunk,
        pos,
        world,
        config,
        x + 1.0,
        y,
        z,
        horizontal_radius,
        vertical_radius,
        floor_level,
        mask,
    );
}

/// `CaveWorldCarver#createTunnel`.
#[allow(clippy::too_many_arguments)]
fn carve_tunnel(
    chunk: &mut ChunkData,
    pos: ChunkPos,
    world: &CarverWorld,
    config: &CarverConfig,
    tunnel_seed: i64,
    mut x: f64,
    mut y: f64,
    mut z: f64,
    horizontal_multiplier: f64,
    vertical_multiplier: f64,
    thickness: f32,
    mut horizontal_rotation: f32,
    mut vertical_rotation: f32,
    step: i32,
    distance: i32,
    y_scale: f64,
    floor_level: f64,
    mask: &mut CarvingMask,
) {
    let mut random = LegacyRandom::new(tunnel_seed);
    let split_point = random.next_int_bounded(distance / 2) + distance / 4;
    let steep = random.next_int_bounded(6) == 0;
    let mut y_rota = 0.0f32;
    let mut x_rota = 0.0f32;

    for current_step in step..distance {
        let horizontal_radius = 1.5
            + ((std::f32::consts::PI * current_step as f32 / distance as f32).sin() * thickness)
                as f64;
        let vertical_radius = horizontal_radius * y_scale;

        let cos_pitch = vertical_rotation.cos();
        x += (horizontal_rotation.cos() * cos_pitch) as f64;
        y += vertical_rotation.sin() as f64;
        z += (horizontal_rotation.sin() * cos_pitch) as f64;
        vertical_rotation *= if steep { 0.92 } else { 0.7 };
        vertical_rotation += x_rota * 0.1;
        horizontal_rotation += y_rota * 0.1;
        x_rota *= 0.9;
        y_rota *= 0.75;
        x_rota += (random.next_float() - random.next_float()) * random.next_float() * 2.0;
        y_rota += (random.next_float() - random.next_float()) * random.next_float() * 4.0;

        if current_step == split_point && thickness > 1.0 {
            // Splits into two tunnels at right angles and stops walking this one.
            let seed_a = random.next_long();
            let thickness_a = random.next_float() * 0.5 + 0.5;
            let seed_b = random.next_long();
            let thickness_b = random.next_float() * 0.5 + 0.5;
            carve_tunnel(
                chunk,
                pos,
                world,
                config,
                seed_a,
                x,
                y,
                z,
                horizontal_multiplier,
                vertical_multiplier,
                thickness_a,
                horizontal_rotation - std::f32::consts::FRAC_PI_2,
                vertical_rotation / 3.0,
                current_step,
                distance,
                1.0,
                floor_level,
                mask,
            );
            carve_tunnel(
                chunk,
                pos,
                world,
                config,
                seed_b,
                x,
                y,
                z,
                horizontal_multiplier,
                vertical_multiplier,
                thickness_b,
                horizontal_rotation + std::f32::consts::FRAC_PI_2,
                vertical_rotation / 3.0,
                current_step,
                distance,
                1.0,
                floor_level,
                mask,
            );
            return;
        }
        if random.next_int_bounded(4) == 0 {
            continue;
        }
        if !can_reach(pos, x, z, current_step, distance, thickness) {
            return;
        }
        carve_ellipsoid(
            chunk,
            pos,
            world,
            config,
            x,
            y,
            z,
            horizontal_radius * horizontal_multiplier,
            vertical_radius * vertical_multiplier,
            floor_level,
            mask,
        );
    }
}

/// `CanyonWorldCarver#carve` plus `doCarve`, minus the per-height width factors, which shape a
/// ravine's walls. PARITY-CHECK: `initWidthFactors` is not ported yet, so ravines come out with
/// straight walls instead of vanilla's rippled ones.
#[allow(clippy::too_many_arguments)]
fn carve_canyon(
    chunk: &mut ChunkData,
    pos: ChunkPos,
    world: &CarverWorld,
    config: &CanyonCarverConfig,
    random: &mut LegacyRandom,
    source: ChunkPos,
    mask: &mut CarvingMask,
) {
    let max_distance = (CARVER_RANGE * 2 - 1) * 16;
    let mut x = (source.min_block_x() + random.next_int_bounded(16)) as f64;
    let mut y = config
        .y
        .sample(random, &|anchor| world.resolve_anchor(anchor)) as f64;
    let mut z = (source.min_block_z() + random.next_int_bounded(16)) as f64;
    let mut horizontal_rotation = random.next_float() * std::f32::consts::TAU;
    let mut vertical_rotation = config.vertical_rotation.sample(random);
    let y_scale = config.y_scale.sample(random) as f64;
    let thickness = config.shape.thickness.sample(random);
    let distance = (max_distance as f32 * config.shape.distance_factor.sample(random)) as i32;

    let tunnel_seed = random.next_long();
    let mut walk = LegacyRandom::new(tunnel_seed);
    let mut y_rota = 0.0f32;
    let mut x_rota = 0.0f32;

    for current_step in 0..distance {
        let mut horizontal_radius = 1.5
            + ((current_step as f32 * std::f32::consts::PI / distance as f32).sin() * thickness)
                as f64;
        let mut vertical_radius = horizontal_radius * y_scale;
        horizontal_radius *= config.shape.horizontal_radius_factor.sample(&mut walk) as f64;
        vertical_radius =
            update_vertical_radius(config, &mut walk, vertical_radius, distance, current_step);

        let cos_pitch = vertical_rotation.cos();
        let sin_pitch = vertical_rotation.sin();
        x += (horizontal_rotation.cos() * cos_pitch) as f64;
        y += sin_pitch as f64;
        z += (horizontal_rotation.sin() * cos_pitch) as f64;
        vertical_rotation *= 0.7;
        vertical_rotation += x_rota * 0.05;
        horizontal_rotation += y_rota * 0.05;
        x_rota *= 0.8;
        y_rota *= 0.5;
        x_rota += (walk.next_float() - walk.next_float()) * walk.next_float() * 2.0;
        y_rota += (walk.next_float() - walk.next_float()) * walk.next_float() * 4.0;

        if walk.next_int_bounded(4) == 0 {
            continue;
        }
        if !can_reach(pos, x, z, current_step, distance, thickness) {
            return;
        }
        carve_ellipsoid(
            chunk,
            pos,
            world,
            &config.base,
            x,
            y,
            z,
            horizontal_radius,
            vertical_radius,
            // A canyon has no floor cut-off; -1.0 lets the whole ellipsoid carve.
            -1.0,
            mask,
        );
    }
}

/// `CanyonWorldCarver#updateVerticalRadius`.
fn update_vertical_radius(
    config: &CanyonCarverConfig,
    random: &mut LegacyRandom,
    vertical_radius: f64,
    distance: i32,
    current_step: i32,
) -> f64 {
    let progress = 1.0 - (2 * current_step - distance).abs() as f32 / distance as f32;
    let factor = config.shape.vertical_radius_default_factor
        + config.shape.vertical_radius_center_factor * progress;
    vertical_radius * factor as f64 * (random.next_float() * 0.25 + 0.75) as f64
}

/// `WorldCarver#canReach`: gives up on a tunnel once it can no longer reach this chunk.
fn can_reach(
    pos: ChunkPos,
    x: f64,
    z: f64,
    current_step: i32,
    total_steps: i32,
    thickness: f32,
) -> bool {
    let middle_x = pos.min_block_x() as f64 + 8.0;
    let middle_z = pos.min_block_z() as f64 + 8.0;
    let dx = x - middle_x;
    let dz = z - middle_z;
    let remaining = (total_steps - current_step) as f64;
    let reach = (thickness + 2.0 + 16.0) as f64;
    dx * dx + dz * dz - remaining * remaining <= reach * reach
}

/// `WorldCarver#carveEllipsoid`.
#[allow(clippy::too_many_arguments)]
fn carve_ellipsoid(
    chunk: &mut ChunkData,
    pos: ChunkPos,
    world: &CarverWorld,
    config: &CarverConfig,
    x: f64,
    y: f64,
    z: f64,
    horizontal_radius: f64,
    vertical_radius: f64,
    floor_level: f64,
    mask: &mut CarvingMask,
) {
    let center_x = pos.min_block_x() as f64 + 8.0;
    let center_z = pos.min_block_z() as f64 + 8.0;
    let max_delta = 16.0 + horizontal_radius * 2.0;
    if (x - center_x).abs() > max_delta || (z - center_z).abs() > max_delta {
        return;
    }

    let chunk_min_x = pos.min_block_x();
    let chunk_min_z = pos.min_block_z();
    let min_x = ((x - horizontal_radius).floor() as i32 - chunk_min_x - 1).max(0);
    let max_x = ((x + horizontal_radius).floor() as i32 - chunk_min_x).min(15);
    let min_y = ((y - vertical_radius).floor() as i32 - 1).max(world.min_y() + 1);
    let max_y = ((y + vertical_radius).floor() as i32 + 1)
        .min(world.min_y() + world.height() - 1 - PROTECTED_BLOCKS_ON_TOP);
    let min_z = ((z - horizontal_radius).floor() as i32 - chunk_min_z - 1).max(0);
    let max_z = ((z + horizontal_radius).floor() as i32 - chunk_min_z).min(15);

    let lava_level = world.resolve_anchor(&config.lava_level);
    let lava = BlockState::new(ResourceLocation::minecraft("lava"));
    let air = BlockState::new(ResourceLocation::minecraft("air"));

    for local_x in min_x..=max_x {
        let world_x = chunk_min_x + local_x;
        let xd = (world_x as f64 + 0.5 - x) / horizontal_radius;
        for local_z in min_z..=max_z {
            let world_z = chunk_min_z + local_z;
            let zd = (world_z as f64 + 0.5 - z) / horizontal_radius;
            if xd * xd + zd * zd >= 1.0 {
                continue;
            }
            for world_y in (min_y + 1..=max_y).rev() {
                let yd = (world_y as f64 - 0.5 - y) / vertical_radius;
                // CaveWorldCarver::shouldSkip
                if yd <= floor_level || xd * xd + yd * yd + zd * zd >= 1.0 {
                    continue;
                }
                if mask.get(local_x as usize, world_y, local_z as usize) {
                    continue;
                }
                mask.set(local_x as usize, world_y, local_z as usize);

                if !can_replace(chunk, world, local_x as usize, world_y, local_z as usize) {
                    continue;
                }
                let state = if world_y <= lava_level { &lava } else { &air };
                set_block(
                    chunk,
                    world,
                    local_x as usize,
                    world_y,
                    local_z as usize,
                    state.clone(),
                );
            }
        }
    }
}

/// PARITY-CHECK: stands in for vanilla's `replaceable` block tag -- see the module doc.
fn can_replace(chunk: &ChunkData, world: &CarverWorld, x: usize, y: i32, z: usize) -> bool {
    let Some(state) = get_block(chunk, world, x, y, z) else {
        return false;
    };
    let name = state.name.path();
    name != "air" && name != "bedrock" && name != "water" && name != "lava"
}

fn section_of(world: &CarverWorld, y: i32) -> Option<usize> {
    if y < world.min_y() || y >= world.min_y() + world.height() {
        return None;
    }
    Some(((y - world.min_y()) / 16) as usize)
}

fn get_block(
    chunk: &ChunkData,
    world: &CarverWorld,
    x: usize,
    y: i32,
    z: usize,
) -> Option<BlockState> {
    let section = chunk.sections.get(section_of(world, y)?)?;
    let local_y = (y - world.min_y()).rem_euclid(16) as usize;
    Some(section.block_states.get(local_index(x, local_y, z)).clone())
}

fn set_block(
    chunk: &mut ChunkData,
    world: &CarverWorld,
    x: usize,
    y: i32,
    z: usize,
    state: BlockState,
) {
    let Some(index) = section_of(world, y) else {
        return;
    };
    let Some(section) = chunk.sections.get_mut(index) else {
        return;
    };
    let local_y = (y - world.min_y()).rem_euclid(16) as usize;
    section.block_states.set(local_index(x, local_y, z), state);
}
