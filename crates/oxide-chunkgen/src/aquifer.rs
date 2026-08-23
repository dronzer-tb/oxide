//! Aquifers: the underground water and lava bodies, and the reason a cave floods in one place
//! and stays dry twenty blocks away.
//!
//! Ported from the decompiled 26.2 `Aquifer.NoiseBasedAquifer`. This is what decides, for every
//! non-solid position, whether it is air, water or lava -- replacing the placeholder rule the
//! fill pass used ("below sea level is water"), which put an ocean inside every hill.
//!
//! Mechanically: aquifer centres sit on a jittered 16x12x16 grid, each with its own fluid
//! surface level and type. A position takes the fluid of the nearest centre, and where two
//! centres of different levels compete, a barrier-noise "pressure" term decides whether the
//! boundary is stone instead -- which is what keeps two adjacent aquifers from merging into one
//! lake.
//!
//! PARITY-CHECK notes:
//! - `shouldScheduleFluidUpdate` is not modelled at all: nothing here schedules block updates,
//!   so a fluid vanilla would let flow on the first tick stays put. Its threshold constant
//!   (`similarity(10^2, 12^2)`) is therefore absent rather than dead.
//! - `maxPreliminarySurfaceLevel` is sampled over the grid's corner columns rather than the
//!   whole region vanilla scans, so the `skipSamplingAboveY` shortcut can differ near a steep
//!   ridge. It only gates an optimisation; the fluid answer below the surface is unaffected.

use oxide_core::{BlockState, RandomSource, ResourceLocation};
use oxide_datapack::NoiseGeneratorSettings;
use oxide_noise::{ChunkCaches, NoiseRouterEvaluator, RouterSlot, WorldPositionalFactory};

const X_RANGE: i32 = 10;
const Y_RANGE: i32 = 9;
const Z_RANGE: i32 = 10;
const Y_SPACING: i32 = 12;
const SAMPLE_OFFSET_X: i32 = -5;
const SAMPLE_OFFSET_Y: i32 = 1;
const SAMPLE_OFFSET_Z: i32 = -5;
const WAY_BELOW_MIN_Y: i32 = -2032;

/// Chunk-relative offsets, in chunks, at which `computeFluid` samples the preliminary surface.
const SURFACE_SAMPLING_OFFSETS_IN_CHUNKS: [[i32; 2]; 13] = [
    [0, 0],
    [-2, -1],
    [-1, -1],
    [0, -1],
    [1, -1],
    [-3, 0],
    [-2, 0],
    [-1, 0],
    [1, 0],
    [-2, 1],
    [-1, 1],
    [0, 1],
    [1, 1],
];

/// A fluid surface: everything strictly below `level` is `fluid`, everything at or above is air.
#[derive(Clone, PartialEq)]
struct FluidStatus {
    level: i32,
    fluid: BlockState,
}

impl FluidStatus {
    fn at(&self, y: i32, air: &BlockState) -> BlockState {
        if y < self.level {
            self.fluid.clone()
        } else {
            air.clone()
        }
    }
}

/// An aquifer for this chunk, or `None` when the dimension's settings disable them.
pub fn for_settings<'a>(
    pos: oxide_core::ChunkPos,
    settings: &NoiseGeneratorSettings,
    router: &'a NoiseRouterEvaluator,
    caches: &'a ChunkCaches,
) -> Option<Aquifer<'a>> {
    settings
        .aquifers_enabled
        .then(|| Aquifer::new(pos.x, pos.z, settings, router, caches))
}

pub struct Aquifer<'a> {
    router: &'a NoiseRouterEvaluator,
    caches: &'a ChunkCaches,
    random: WorldPositionalFactory,

    air: BlockState,
    lava: BlockState,
    sea_fluid: BlockState,
    sea_level: i32,

    min_grid_x: i32,
    min_grid_y: i32,
    min_grid_z: i32,
    grid_size_x: i32,
    grid_size_z: i32,
    skip_sampling_above_y: i32,

    /// Per grid cell: the jittered centre position, and the fluid there once computed.
    locations: Vec<Option<(i32, i32, i32)>>,
    statuses: Vec<Option<FluidStatus>>,
}

impl<'a> Aquifer<'a> {
    pub fn new(
        chunk_x: i32,
        chunk_z: i32,
        settings: &NoiseGeneratorSettings,
        router: &'a NoiseRouterEvaluator,
        caches: &'a ChunkCaches,
    ) -> Self {
        let min_block_x = chunk_x * 16;
        let min_block_z = chunk_z * 16;
        let max_block_x = min_block_x + 15;
        let max_block_z = min_block_z + 15;
        let min_y = settings.noise.min_y;
        let height = settings.noise.height;

        let min_grid_x = grid_x(min_block_x + SAMPLE_OFFSET_X);
        let max_grid_x = grid_x(max_block_x + SAMPLE_OFFSET_X) + 1;
        let min_grid_y = grid_y(min_y + SAMPLE_OFFSET_Y) - 1;
        let max_grid_y = grid_y(min_y + height + SAMPLE_OFFSET_Y) + 1;
        let min_grid_z = grid_z(min_block_z + SAMPLE_OFFSET_Z);
        let max_grid_z = grid_z(max_block_z + SAMPLE_OFFSET_Z) + 1;

        let grid_size_x = max_grid_x - min_grid_x + 1;
        let grid_size_y = max_grid_y - min_grid_y + 1;
        let grid_size_z = max_grid_z - min_grid_z + 1;
        let total = (grid_size_x * grid_size_y * grid_size_z) as usize;

        let mut aquifer = Self {
            router,
            caches,
            // Vanilla's aquiferRandom: the RandomState factory hashed with "minecraft:aquifer".
            random: router
                .positional_factory()
                .from_hash_of("minecraft:aquifer")
                .fork_positional(),
            air: BlockState::new(ResourceLocation::minecraft("air")),
            lava: BlockState::new(ResourceLocation::minecraft("lava")),
            sea_fluid: settings.default_fluid.clone(),
            sea_level: settings.sea_level,
            min_grid_x,
            min_grid_y,
            min_grid_z,
            grid_size_x,
            grid_size_z,
            skip_sampling_above_y: i32::MAX,
            locations: vec![None; total],
            statuses: vec![None; total],
        };

        // Above the highest surface in reach there is nothing to flood, so vanilla stops
        // sampling. See the PARITY-CHECK note on how this bound is estimated.
        let mut max_surface = i32::MIN;
        for gx in [min_grid_x, max_grid_x] {
            for gz in [min_grid_z, max_grid_z] {
                for offset in [0, X_RANGE - 1] {
                    let x = from_grid_x(gx, offset);
                    let z = from_grid_z(gz, offset);
                    max_surface = max_surface.max(aquifer.preliminary_surface_level(x, z));
                }
            }
        }
        let skip_grid_y = grid_y(adjust_surface_level(max_surface) + Y_SPACING) + 1;
        aquifer.skip_sampling_above_y = from_grid_y(skip_grid_y, Y_SPACING - 1) - 1;
        aquifer
    }

    /// The global rule: lava below both -54 and sea level, otherwise the dimension's fluid up
    /// to sea level. `createFluidPicker` in vanilla.
    fn global_fluid(&self, y: i32) -> FluidStatus {
        if y < (-54).min(self.sea_level) {
            FluidStatus {
                level: -54,
                fluid: self.lava.clone(),
            }
        } else {
            FluidStatus {
                level: self.sea_level,
                fluid: self.sea_fluid.clone(),
            }
        }
    }

    /// `Aquifer#computeSubstance`. `None` means "solid" -- the caller places its default block.
    pub fn compute_substance(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        density: f64,
    ) -> Option<BlockState> {
        if density > 0.0 {
            return None;
        }
        let global = self.global_fluid(y);
        if y > self.skip_sampling_above_y {
            return Some(global.at(y, &self.air));
        }
        if global.at(y, &self.air) == self.lava {
            return Some(self.lava.clone());
        }

        let anchor_x = grid_x(x + SAMPLE_OFFSET_X);
        let anchor_y = grid_y(y + SAMPLE_OFFSET_Y);
        let anchor_z = grid_z(z + SAMPLE_OFFSET_Z);

        // The four nearest aquifer centres, by squared distance.
        let mut best = [(i32::MAX, 0usize); 4];
        for dx in 0..=1 {
            for dy in -1..=1 {
                for dz in 0..=1 {
                    let gx = anchor_x + dx;
                    let gy = anchor_y + dy;
                    let gz = anchor_z + dz;
                    let index = self.index(gx, gy, gz);
                    let (cx, cy, cz) = self.location(index, gx, gy, gz);
                    let ddx = cx - x;
                    let ddy = cy - y;
                    let ddz = cz - z;
                    let distance = ddx * ddx + ddy * ddy + ddz * ddz;
                    for slot in 0..4 {
                        if best[slot].0 >= distance {
                            best[slot..].rotate_right(1);
                            best[slot] = (distance, index);
                            break;
                        }
                    }
                }
            }
        }

        let status1 = self.status(best[0].1);
        let similarity12 = similarity(best[0].0, best[1].0);
        let fluid = status1.at(y, &self.air);
        if similarity12 <= 0.0 {
            return Some(fluid);
        }
        if fluid == self.sea_fluid && self.global_fluid(y - 1).at(y - 1, &self.air) == self.lava {
            return Some(fluid);
        }

        // Where two aquifers of different levels meet, barrier pressure can make the boundary
        // solid instead -- that is what stops them merging into one body.
        let mut barrier_noise: Option<f64> = None;
        let status2 = self.status(best[1].1);
        let pressure12 =
            similarity12 * self.pressure(x, y, z, &mut barrier_noise, &status1, &status2);
        if density + pressure12 > 0.0 {
            return None;
        }

        let status3 = self.status(best[2].1);
        let similarity13 = similarity(best[0].0, best[2].0);
        if similarity13 > 0.0 {
            let pressure13 = similarity12
                * similarity13
                * self.pressure(x, y, z, &mut barrier_noise, &status1, &status3);
            if density + pressure13 > 0.0 {
                return None;
            }
        }
        let similarity23 = similarity(best[1].0, best[2].0);
        if similarity23 > 0.0 {
            let pressure23 = similarity12
                * similarity23
                * self.pressure(x, y, z, &mut barrier_noise, &status2, &status3);
            if density + pressure23 > 0.0 {
                return None;
            }
        }
        Some(fluid)
    }

    fn index(&self, gx: i32, gy: i32, gz: i32) -> usize {
        let x = gx - self.min_grid_x;
        let y = gy - self.min_grid_y;
        let z = gz - self.min_grid_z;
        ((y * self.grid_size_z + z) * self.grid_size_x + x) as usize
    }

    /// The jittered centre of one grid cell, drawn once and remembered.
    fn location(&mut self, index: usize, gx: i32, gy: i32, gz: i32) -> (i32, i32, i32) {
        if let Some(Some(location)) = self.locations.get(index) {
            return *location;
        }
        let mut random = self.random.at(gx, gy, gz);
        let location = (
            from_grid_x(gx, random.next_int_bounded(X_RANGE)),
            from_grid_y(gy, random.next_int_bounded(Y_RANGE)),
            from_grid_z(gz, random.next_int_bounded(Z_RANGE)),
        );
        if let Some(slot) = self.locations.get_mut(index) {
            *slot = Some(location);
        }
        location
    }

    fn status(&mut self, index: usize) -> FluidStatus {
        if let Some(Some(status)) = self.statuses.get(index) {
            return status.clone();
        }
        let Some(Some(location)) = self.locations.get(index).copied() else {
            return self.global_fluid(0);
        };
        let status = self.compute_fluid(location.0, location.1, location.2);
        if let Some(slot) = self.statuses.get_mut(index) {
            *slot = Some(status.clone());
        }
        status
    }

    /// `NoiseBasedAquifer#computeFluid`: the fluid this aquifer centre holds.
    fn compute_fluid(&mut self, x: i32, y: i32, z: i32) -> FluidStatus {
        let global = self.global_fluid(y);
        let mut lowest_surface = i32::MAX;
        let top_of_cell = y + Y_SPACING;
        let bottom_of_cell = y - Y_SPACING;
        let mut surface_under_global_fluid = false;

        for offset in SURFACE_SAMPLING_OFFSETS_IN_CHUNKS {
            let sample_x = x + offset[0] * 16;
            let sample_z = z + offset[1] * 16;
            let surface = self.preliminary_surface_level(sample_x, sample_z);
            let adjusted = adjust_surface_level(surface);
            let start = offset[0] == 0 && offset[1] == 0;

            if start && bottom_of_cell > adjusted {
                return global;
            }
            let pokes_above = top_of_cell > adjusted;
            if pokes_above || start {
                let at_surface = self.global_fluid(adjusted);
                if at_surface.at(adjusted, &self.air) != self.air {
                    if start {
                        surface_under_global_fluid = true;
                    }
                    if pokes_above {
                        return at_surface;
                    }
                }
            }
            lowest_surface = lowest_surface.min(surface);
        }

        let level =
            self.surface_level(x, y, z, &global, lowest_surface, surface_under_global_fluid);
        let fluid = self.fluid_type(x, y, z, &global, level);
        FluidStatus { level, fluid }
    }

    /// `computeSurfaceLevel`: how high this aquifer's fluid stands, if it holds any.
    fn surface_level(
        &self,
        x: i32,
        y: i32,
        z: i32,
        global: &FluidStatus,
        lowest_surface: i32,
        surface_under_global_fluid: bool,
    ) -> i32 {
        let erosion = self.router.sample(RouterSlot::Erosion, x, y, z);
        let depth = self.router.sample(RouterSlot::Depth, x, y, z);
        // Deep dark stays dry: that is what makes ancient cities generate without water.
        let deep_dark = erosion < -0.225 && depth > 0.9;

        let (partially, fully) = if deep_dark {
            (-1.0, -1.0)
        } else {
            let distance_below_surface = lowest_surface + 8 - y;
            let floodedness_factor = if surface_under_global_fluid {
                clamped_map(distance_below_surface as f64, 0.0, 64.0, 1.0, 0.0)
            } else {
                0.0
            };
            let noise = self
                .router
                .sample(RouterSlot::FluidLevelFloodedness, x, y, z)
                .clamp(-1.0, 1.0);
            let fully_threshold = map(floodedness_factor, 1.0, 0.0, -0.3, 0.8);
            let partially_threshold = map(floodedness_factor, 1.0, 0.0, -0.8, 0.4);
            (noise - partially_threshold, noise - fully_threshold)
        };

        if fully > 0.0 {
            global.level
        } else if partially > 0.0 {
            self.randomized_surface_level(x, y, z, lowest_surface)
        } else {
            WAY_BELOW_MIN_Y
        }
    }

    /// `computeRandomizedFluidSurfaceLevel`: a partially flooded aquifer's level, quantised so
    /// neighbouring cells share levels and produce flat surfaces rather than staircases.
    fn randomized_surface_level(&self, x: i32, y: i32, z: i32, lowest_surface: i32) -> i32 {
        let cell_x = x.div_euclid(16);
        let cell_y = y.div_euclid(40);
        let cell_z = z.div_euclid(16);
        let middle_y = cell_y * 40 + 20;
        let spread = self
            .router
            .sample(RouterSlot::FluidLevelSpread, cell_x, cell_y, cell_z)
            * 10.0;
        let quantized = quantize(spread, 3);
        lowest_surface.min(middle_y + quantized)
    }

    /// `computeFluidType`: deep aquifers can be lava instead of water.
    fn fluid_type(
        &self,
        x: i32,
        y: i32,
        z: i32,
        global: &FluidStatus,
        surface_level: i32,
    ) -> BlockState {
        if surface_level <= -10 && surface_level != WAY_BELOW_MIN_Y && global.fluid != self.lava {
            let cell_x = x.div_euclid(64);
            let cell_y = y.div_euclid(40);
            let cell_z = z.div_euclid(64);
            let lava_noise = self.router.sample(RouterSlot::Lava, cell_x, cell_y, cell_z);
            if lava_noise.abs() > 0.3 {
                return self.lava.clone();
            }
        }
        global.fluid.clone()
    }

    /// `calculatePressure`: how strongly the boundary between two aquifers resists being fluid.
    fn pressure(
        &self,
        x: i32,
        y: i32,
        z: i32,
        barrier_noise: &mut Option<f64>,
        first: &FluidStatus,
        second: &FluidStatus,
    ) -> f64 {
        let type1 = first.at(y, &self.air);
        let type2 = second.at(y, &self.air);
        // Water meeting lava is always a barrier -- that is why they never touch underground.
        if (type1 == self.lava && type2 == self.sea_fluid)
            || (type1 == self.sea_fluid && type2 == self.lava)
        {
            return 2.0;
        }
        let level_difference = (first.level - second.level).abs();
        if level_difference == 0 {
            return 0.0;
        }

        let average_level = 0.5 * (first.level + second.level) as f64;
        let above_average = y as f64 + 0.5 - average_level;
        let half_difference = level_difference as f64 / 2.0;
        let toward_middle = half_difference - above_average.abs();
        let gradient = if above_average > 0.0 {
            let center = toward_middle;
            if center > 0.0 {
                center / 1.5
            } else {
                center / 2.5
            }
        } else {
            let center = 3.0 + toward_middle;
            if center > 0.0 {
                center / 3.0
            } else {
                center / 10.0
            }
        };

        // Outside +/-2 the gradient decides on its own, so the noise is never sampled -- which
        // is why vanilla threads the sampled value through as a lazily-filled holder.
        let noise = if !(-2.0..=2.0).contains(&gradient) {
            0.0
        } else {
            match barrier_noise {
                Some(value) => *value,
                None => {
                    let value = self.router.sample(RouterSlot::Barrier, x, y, z);
                    *barrier_noise = Some(value);
                    value
                }
            }
        };
        2.0 * (noise + gradient)
    }

    fn preliminary_surface_level(&self, x: i32, z: i32) -> i32 {
        self.router
            .sample_in_chunk(self.caches, RouterSlot::PreliminarySurfaceLevel, x, 0, z)
            as i32
    }
}

fn adjust_surface_level(preliminary: i32) -> i32 {
    preliminary + 8
}

fn similarity(distance1: i32, distance2: i32) -> f64 {
    1.0 - (distance2 - distance1) as f64 / 25.0
}

fn grid_x(block: i32) -> i32 {
    block >> 4
}

fn from_grid_x(grid: i32, offset: i32) -> i32 {
    (grid << 4) + offset
}

fn grid_y(block: i32) -> i32 {
    block.div_euclid(Y_SPACING)
}

fn from_grid_y(grid: i32, offset: i32) -> i32 {
    grid * Y_SPACING + offset
}

fn grid_z(block: i32) -> i32 {
    block >> 4
}

fn from_grid_z(grid: i32, offset: i32) -> i32 {
    (grid << 4) + offset
}

/// Vanilla `Mth.quantize`.
fn quantize(value: f64, resolution: i32) -> i32 {
    (value / resolution as f64).floor() as i32 * resolution
}

/// Vanilla `Mth.clampedMap`.
fn clamped_map(value: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    let factor = (value - from_min) / (from_max - from_min);
    if factor < 0.0 {
        to_min
    } else if factor > 1.0 {
        to_max
    } else {
        to_min + factor * (to_max - to_min)
    }
}

/// Vanilla `Mth.map`, unclamped.
fn map(value: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    to_min + (value - from_min) * (to_max - to_min) / (from_max - from_min)
}
