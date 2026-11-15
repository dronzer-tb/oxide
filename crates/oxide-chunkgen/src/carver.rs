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
//! - ~~**Carved substance.**~~ Resolved: carving now asks the same aquifer the fill pass used,
//!   so a cave that breaks into a flooded aquifer fills with water and one that does not stays
//!   dry. Below the carver's `lava_level` it is lava regardless, as vanilla checks first.
//! - **Grass repair.** Vanilla re-runs the surface rule on dirt exposed under a carved-away
//!   grass block. Not done here, so a cave breaching the surface leaves bare dirt.

use oxide_core::{BlockState, ChunkData, ChunkPos, LegacyRandom, RandomSource, ResourceLocation};
use oxide_datapack::{
    CanyonCarverConfig, CarverConfig, CaveCarverConfig, ConfiguredCarver, NoiseGeneratorSettings,
    VerticalAnchor,
};

use crate::aquifer::FluidKind;
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
    mut aquifer: Option<&mut crate::aquifer::Aquifer>,
) {
    let mut mask = CarvingMask::new(world.min_y(), world.height());
    let mut random = LegacyRandom::new(0);
    let chunk = &mut Target::new(chunk);

    for dx in -CARVER_CHUNK_RANGE..=CARVER_CHUNK_RANGE {
        for dz in -CARVER_CHUNK_RANGE..=CARVER_CHUNK_RANGE {
            let source = ChunkPos::new(pos.x + dx, pos.z + dz);
            // Borrowed, not moved out element by element: the configs are large.
            let carvers = carvers_at(source);
            for (index, carver) in carvers.iter().enumerate() {
                random.set_large_feature_seed(
                    world.seed.wrapping_add(index as i64),
                    source.x,
                    source.z,
                );
                match carver {
                    ConfiguredCarver::Cave { config } | ConfiguredCarver::NetherCave { config } => {
                        if random.next_float() <= config.base.probability {
                            carve_cave(
                                chunk,
                                pos,
                                world,
                                config,
                                &mut random,
                                source,
                                &mut mask,
                                aquifer.as_deref_mut(),
                            );
                        }
                    }
                    ConfiguredCarver::Canyon { config } => {
                        if random.next_float() <= config.base.probability {
                            carve_canyon(
                                chunk,
                                pos,
                                world,
                                config,
                                &mut random,
                                source,
                                &mut mask,
                                aquifer.as_deref_mut(),
                            );
                        }
                    }
                }
            }
        }
    }
}

/// `CaveWorldCarver#carve`.
#[allow(clippy::too_many_arguments)]
fn carve_cave(
    chunk: &mut Target,
    pos: ChunkPos,
    world: &CarverWorld,
    config: &CaveCarverConfig,
    random: &mut LegacyRandom,
    source: ChunkPos,
    mask: &mut CarvingMask,
    mut aquifer: Option<&mut crate::aquifer::Aquifer>,
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
                aquifer.as_deref_mut(),
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
                aquifer.as_deref_mut(),
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
    chunk: &mut Target,
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
    aquifer: Option<&mut crate::aquifer::Aquifer>,
) {
    // Vanilla writes `Mth.sin((float)(Math.PI / 2))` -- the table lookup, and of the *float*
    // narrowing of PI/2, not the double. Neither detail is cosmetic: the table returns a
    // quantised value rather than exactly 1, and narrowing first can select a different index.
    let horizontal_radius =
        1.5 + (oxide_core::mth::sin(std::f32::consts::FRAC_PI_2 as f64) as f64 * thickness as f64);
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
        aquifer,
    );
}

/// `CaveWorldCarver#createTunnel`.
#[allow(clippy::too_many_arguments)]
fn carve_tunnel(
    chunk: &mut Target,
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
    mut aquifer: Option<&mut crate::aquifer::Aquifer>,
) {
    let mut random = WalkRandom::new(tunnel_seed);
    let split_point = random.next_int_bounded(distance / 2) + distance / 4;
    let steep = random.next_int_bounded(6) == 0;
    let mut y_rota = 0.0f32;
    let mut x_rota = 0.0f32;

    // An upper bound on `|horizontal_radius * horizontal_multiplier|` over every step: the table
    // sine is at most 1 in magnitude, so `|sin * thickness| <= |thickness|` after rounding, and
    // every later operation rounds monotonically. With it, `carve_ellipsoid`'s own first test
    // (`|x - center| > 16 + 2r`) can be decided without computing the step's radius; it only
    // ever skips a call that would have returned at that test.
    let reach_limit = 16.0 + ((1.5 + thickness.abs() as f64) * horizontal_multiplier.abs()) * 2.0;
    let center_x = pos.min_block_x() as f64 + 8.0;
    let center_z = pos.min_block_z() as f64 + 8.0;

    for current_step in step..distance {
        let cos_pitch = oxide_core::mth::cos(vertical_rotation as f64);
        x += (oxide_core::mth::cos(horizontal_rotation as f64) * cos_pitch) as f64;
        y += oxide_core::mth::sin(vertical_rotation as f64) as f64;
        z += (oxide_core::mth::sin(horizontal_rotation as f64) * cos_pitch) as f64;
        vertical_rotation *= if steep { 0.92 } else { 0.7 };
        vertical_rotation += x_rota * 0.1;
        horizontal_rotation += y_rota * 0.1;
        x_rota *= 0.9;
        y_rota *= 0.75;
        // The step's six floats, then (below) its nextInt(4), all from one jump of the LCG.
        let (draws, int_seed) = random.step_floats();
        x_rota += (draws[0] - draws[1]) * draws[2] * 2.0;
        y_rota += (draws[3] - draws[4]) * draws[5] * 4.0;

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
                aquifer.as_deref_mut(),
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
                aquifer.as_deref_mut(),
            );
            return;
        }
        if random.commit_bound_4(int_seed) == 0 {
            continue;
        }
        if !can_reach(pos, x, z, current_step, distance, thickness) {
            return;
        }
        if (x - center_x).abs() > reach_limit || (z - center_z).abs() > reach_limit {
            continue;
        }
        let horizontal_radius = 1.5
            + (oxide_core::mth::sin(
                (std::f32::consts::PI * current_step as f32 / distance as f32) as f64,
            ) * thickness) as f64;
        let vertical_radius = horizontal_radius * y_scale;
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
            aquifer.as_deref_mut(),
        );
    }
}

/// `CanyonWorldCarver#carve` plus `doCarve`, minus the per-height width factors, which shape a
/// ravine's walls. PARITY-CHECK: `initWidthFactors` is not ported yet, so ravines come out with
/// straight walls instead of vanilla's rippled ones.
#[allow(clippy::too_many_arguments)]
fn carve_canyon(
    chunk: &mut Target,
    pos: ChunkPos,
    world: &CarverWorld,
    config: &CanyonCarverConfig,
    random: &mut LegacyRandom,
    source: ChunkPos,
    mask: &mut CarvingMask,
    mut aquifer: Option<&mut crate::aquifer::Aquifer>,
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
            + (oxide_core::mth::sin(
                (current_step as f32 * std::f32::consts::PI / distance as f32) as f64,
            ) * thickness) as f64;
        let mut vertical_radius = horizontal_radius * y_scale;
        horizontal_radius *= config.shape.horizontal_radius_factor.sample(&mut walk) as f64;
        vertical_radius =
            update_vertical_radius(config, &mut walk, vertical_radius, distance, current_step);

        let cos_pitch = oxide_core::mth::cos(vertical_rotation as f64);
        let sin_pitch = oxide_core::mth::sin(vertical_rotation as f64);
        x += (oxide_core::mth::cos(horizontal_rotation as f64) * cos_pitch) as f64;
        y += sin_pitch as f64;
        z += (oxide_core::mth::sin(horizontal_rotation as f64) * cos_pitch) as f64;
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
            aquifer.as_deref_mut(),
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
    chunk: &mut Target,
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
    mut aquifer: Option<&mut crate::aquifer::Aquifer>,
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

    // `zd` depends only on the column's z and `yd` only on the height, so each is computed once
    // per ellipsoid instead of once per column or block: the same expressions on the same
    // inputs, and the sums below keep their order, `(xd*xd + yd*yd) + zd*zd`.
    let mut zd_table = [0.0f64; 16];
    if min_z <= max_z {
        for local_z in min_z..=max_z {
            let world_z = chunk_min_z + local_z;
            zd_table[local_z as usize] = (world_z as f64 + 0.5 - z) / horizontal_radius;
        }
    }
    let y_low = min_y + 1;
    let y_span = (max_y - y_low + 1).max(0) as usize;
    let mut yd_table = [0.0f64; YD_TABLE_LEN];
    let use_yd_table = y_span <= YD_TABLE_LEN;
    let mut yd_filled = false;

    for local_x in min_x..=max_x {
        let world_x = chunk_min_x + local_x;
        let xd = (world_x as f64 + 0.5 - x) / horizontal_radius;
        for local_z in min_z..=max_z {
            let world_z = chunk_min_z + local_z;
            let zd = zd_table[local_z as usize];
            if xd * xd + zd * zd >= 1.0 {
                continue;
            }
            if use_yd_table && !yd_filled {
                for (i, slot) in yd_table[..y_span].iter_mut().enumerate() {
                    *slot = ((y_low + i as i32) as f64 - 0.5 - y) / vertical_radius;
                }
                yd_filled = true;
            }
            for world_y in (y_low..=max_y).rev() {
                let yd = if use_yd_table {
                    yd_table[(world_y - y_low) as usize]
                } else {
                    (world_y as f64 - 0.5 - y) / vertical_radius
                };
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
                // Vanilla asks the aquifer what a carved position becomes: air above the water
                // table, water inside a flooded one, and nothing at all -- left solid -- where
                // the aquifer says so. Below the carver's lava_level it is lava regardless,
                // checked first exactly as getCarveState does.
                let carved = if world_y <= lava_level {
                    Carved::CarverLava
                } else {
                    match aquifer.as_deref_mut() {
                        Some(aquifer) => match aquifer.compute_kind(world_x, world_y, world_z, 0.0)
                        {
                            Some(FluidKind::Air) => Carved::AquiferAir,
                            Some(FluidKind::Sea) => Carved::AquiferSea,
                            Some(FluidKind::Lava) => Carved::AquiferLava,
                            None => continue,
                        },
                        None => Carved::Air,
                    }
                };
                set_block(
                    chunk,
                    world,
                    local_x as usize,
                    world_y,
                    local_z as usize,
                    carved,
                    aquifer.as_deref(),
                );
            }
        }
    }
}

/// Heights an ellipsoid's per-height `yd` table covers; a taller ellipsoid computes `yd` inline.
const YD_TABLE_LEN: usize = 64;

const LCG_MULTIPLIER: u64 = 0x5DEECE66D;
const LCG_ADDEND: u64 = 0xB;
const LCG_MASK: u64 = (1 << 48) - 1;

/// `(a, c)` with `state_k = a * state_0 + c (mod 2^48)` after `k` steps of the LCG.
const fn lcg_jump(k: u32) -> (u64, u64) {
    let mut a = 1u64;
    let mut c = 0u64;
    let mut i = 0;
    while i < k {
        a = a.wrapping_mul(LCG_MULTIPLIER) & LCG_MASK;
        c = (c.wrapping_mul(LCG_MULTIPLIER).wrapping_add(LCG_ADDEND)) & LCG_MASK;
        i += 1;
    }
    (a, c)
}

const LCG_JUMPS: [(u64, u64); 8] = [
    lcg_jump(0),
    lcg_jump(1),
    lcg_jump(2),
    lcg_jump(3),
    lcg_jump(4),
    lcg_jump(5),
    lcg_jump(6),
    lcg_jump(7),
];

/// The cave tunnel walk's `java.util.Random`, bit-for-bit the same sequence as
/// [`LegacyRandom`] (same `setSeed` scramble, same `next(bits)`, same `nextInt(bound)`), but
/// with the state visible so a step's seven draws can be taken from one jump of the LCG instead
/// of a chain of seven dependent multiplies. Only the walk uses it; nothing else sees its state.
struct WalkRandom {
    seed: u64,
}

impl WalkRandom {
    fn new(seed: i64) -> Self {
        Self {
            seed: (seed as u64 ^ LCG_MULTIPLIER) & LCG_MASK,
        }
    }

    #[inline]
    fn advance(seed: u64, k: usize) -> u64 {
        let (a, c) = LCG_JUMPS[k];
        a.wrapping_mul(seed).wrapping_add(c) & LCG_MASK
    }

    #[inline]
    fn bits_of(seed: u64, bits: u32) -> i32 {
        (seed >> (48 - bits)) as i32
    }

    #[inline]
    fn float_of(seed: u64) -> f32 {
        Self::bits_of(seed, 24) as f32 / 16_777_216.0
    }

    fn next_bits(&mut self, bits: u32) -> i32 {
        self.seed = Self::advance(self.seed, 1);
        Self::bits_of(self.seed, bits)
    }

    fn next_float(&mut self) -> f32 {
        self.seed = Self::advance(self.seed, 1);
        Self::float_of(self.seed)
    }

    fn next_long(&mut self) -> i64 {
        let hi = self.next_bits(32) as i64;
        let lo = self.next_bits(32) as i64;
        (hi << 32).wrapping_add(lo)
    }

    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "bound must be positive");
        if bound.isolate_lowest_one() == bound {
            return ((bound as i64).wrapping_mul(self.next_bits(31) as i64) >> 31) as i32;
        }
        loop {
            let bits = self.next_bits(31);
            let val = bits % bound;
            if bits.wrapping_sub(val).wrapping_add(bound - 1) >= 0 {
                return val;
            }
        }
    }

    /// Six `nextFloat`s in draw order, leaving the generator after the sixth, plus the state a
    /// seventh draw would produce (for [`Self::commit_bound_4`]).
    #[inline]
    fn step_floats(&mut self) -> ([f32; 6], u64) {
        let s = self.seed;
        let draws = [
            Self::float_of(Self::advance(s, 1)),
            Self::float_of(Self::advance(s, 2)),
            Self::float_of(Self::advance(s, 3)),
            Self::float_of(Self::advance(s, 4)),
            Self::float_of(Self::advance(s, 5)),
            Self::float_of(Self::advance(s, 6)),
        ];
        self.seed = Self::advance(s, 6);
        (draws, Self::advance(s, 7))
    }

    /// `nextInt(4)` right after [`Self::step_floats`], given the state it returned.
    #[inline]
    fn commit_bound_4(&mut self, next_seed: u64) -> i32 {
        self.seed = next_seed;
        ((4i64).wrapping_mul(Self::bits_of(next_seed, 31) as i64) >> 31) as i32
    }
}

/// What a carved position becomes. The carver's own lava (below `lava_level`) carries a
/// `level=0` property and the aquifer's does not, so the two are distinct states and distinct
/// slots here; `Air` is the plain air written when there is no aquifer.
#[derive(Clone, Copy)]
enum Carved {
    CarverLava = 0,
    Air = 1,
    AquiferAir = 2,
    AquiferSea = 3,
    AquiferLava = 4,
}

const NOT_INTERNED: u32 = u32::MAX;

/// Per-section palette bookkeeping, valid for one `apply_carvers` call. Palettes only ever
/// append, so a palette index, once classified or resolved, keeps its meaning.
struct SectionCache {
    /// `can_replace` per palette index, for the prefix of the palette classified so far.
    replaceable: Vec<bool>,
    /// Palette index of each [`Carved`] state in this section, once it has been written here.
    written: [u32; 5],
}

/// The chunk being carved, plus per-section palette caches so the per-block path compares and
/// writes palette indices instead of hashing and cloning `BlockState`s.
struct Target<'c> {
    chunk: &'c mut ChunkData,
    sections: Vec<SectionCache>,
    lava: BlockState,
    air: BlockState,
}

impl<'c> Target<'c> {
    fn new(chunk: &'c mut ChunkData) -> Self {
        let sections = (0..chunk.sections.len())
            .map(|_| SectionCache {
                replaceable: Vec::new(),
                written: [NOT_INTERNED; 5],
            })
            .collect();
        Self {
            chunk,
            sections,
            lava: BlockState::new(ResourceLocation::minecraft("lava")).with_property("level", "0"),
            air: BlockState::new(ResourceLocation::minecraft("air")),
        }
    }
}

/// PARITY-CHECK: stands in for vanilla's `replaceable` block tag -- see the module doc.
fn is_replaceable(state: &BlockState) -> bool {
    let name = state.name.path();
    name != "air" && name != "bedrock" && name != "water" && name != "lava"
}

fn can_replace(target: &mut Target, world: &CarverWorld, x: usize, y: i32, z: usize) -> bool {
    let Some(index) = section_of(world, y) else {
        return false;
    };
    let Some(section) = target.chunk.sections.get(index) else {
        return false;
    };
    let local_y = (y - world.min_y()).rem_euclid(16) as usize;
    let palette_index = section.block_states.indices()[local_index(x, local_y, z)] as usize;
    let cache = &mut target.sections[index];
    if palette_index >= cache.replaceable.len() {
        let palette = section.block_states.palette();
        let known = cache.replaceable.len();
        cache
            .replaceable
            .extend(palette[known..].iter().map(is_replaceable));
    }
    cache.replaceable[palette_index]
}

fn section_of(world: &CarverWorld, y: i32) -> Option<usize> {
    if y < world.min_y() || y >= world.min_y() + world.height() {
        return None;
    }
    Some(((y - world.min_y()) / 16) as usize)
}

fn set_block(
    target: &mut Target,
    world: &CarverWorld,
    x: usize,
    y: i32,
    z: usize,
    carved: Carved,
    aquifer: Option<&crate::aquifer::Aquifer>,
) {
    let Some(index) = section_of(world, y) else {
        return;
    };
    let Some(section) = target.chunk.sections.get_mut(index) else {
        return;
    };
    let local_y = (y - world.min_y()).rem_euclid(16) as usize;
    let slot = &mut target.sections[index].written[carved as usize];
    if *slot == NOT_INTERNED {
        // First write of this state into this section: intern it now, at the same point the
        // per-block `set` used to, so palette order is unchanged.
        let state = match (carved, aquifer) {
            (Carved::CarverLava, _) => target.lava.clone(),
            (Carved::Air, _) => target.air.clone(),
            (Carved::AquiferAir, Some(aquifer)) => aquifer.block_for(FluidKind::Air),
            (Carved::AquiferSea, Some(aquifer)) => aquifer.block_for(FluidKind::Sea),
            (Carved::AquiferLava, Some(aquifer)) => aquifer.block_for(FluidKind::Lava),
            _ => unreachable!("aquifer states are only produced with an aquifer"),
        };
        *slot = section.block_states.index_of_or_insert(state);
    }
    section
        .block_states
        .set_index(local_index(x, local_y, z), *slot);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The walk's generator must replay `LegacyRandom` exactly, through the stepped draws.
    #[test]
    fn walk_random_matches_legacy_random() {
        let seeds = [
            0i64,
            1,
            -1,
            42,
            i64::MIN,
            i64::MAX,
            0x5DEECE66D,
            -123_456_789_012_345,
        ];
        for (n, &seed) in seeds.iter().enumerate() {
            let mut walk = WalkRandom::new(seed);
            let mut legacy = LegacyRandom::new(seed);
            for bound in [1, 2, 3, 4, 6, 7, 42, 56, 1 << 30, i32::MAX] {
                assert_eq!(walk.next_int_bounded(bound), legacy.next_int_bounded(bound));
            }
            for round in 0..2000 {
                let (draws, int_seed) = walk.step_floats();
                for draw in draws {
                    assert_eq!(draw.to_bits(), legacy.next_float().to_bits());
                }
                if (round + n) % 97 == 0 {
                    assert_eq!(walk.next_long(), legacy.next_long());
                    assert_eq!(walk.next_float().to_bits(), legacy.next_float().to_bits());
                } else {
                    assert_eq!(walk.commit_bound_4(int_seed), legacy.next_int_bounded(4));
                }
            }
            assert_eq!(walk.next_long(), legacy.next_long());
        }
    }
}
