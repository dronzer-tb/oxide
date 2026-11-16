//! Biome temperature, freezing and snow: what `Biome.shouldFreeze` and `shouldSnow` answer.
//!
//! The datapack carries a biome's base temperature, its `temperature_modifier` and whether it
//! has precipitation; [`BiomeClimate`] keeps those three. A position's temperature is the base
//! (or `0.2` inside the `frozen` modifier's noise patches) lowered above sea level + 17 by a
//! height term with its own noise. Three fixed simplex noises feed it, seeded 1234, 3456 and
//! 2345 from the legacy random source, exactly as vanilla's statics are.
//!
//! Float and double widths follow the reference implementation: since the noise classes were
//! changed to return `float`, each sample is truncated to `f32` before it is scaled.
//!
//! PARITY-CHECK: the `SimplexNoise` in `decorator/simplex.rs` is itself unverified against a real
//! 26.2 run (see its module doc); every temperature depends on it above sea level + 17 and inside
//! frozen patches. Compare `BiomeClimate::temperature` with `Biome.getTemperature` at a few dozen
//! positions of a frozen biome and of a tall mountain.
//!
//! PARITY-CHECK: vanilla also requires `getBrightness(LightLayer.BLOCK, pos) < 10`. Decoration
//! runs before the light engine, where that brightness is 0, so the check is omitted rather than
//! evaluated against a light level Oxide does not have.

use std::sync::LazyLock;

use anyhow::{bail, Result};
use oxide_core::{BlockPos, BlockState, LegacyRandom};
use oxide_datapack::{Biome, BlockFact};

use super::super::context::{block_in, FeatureContext};
use super::super::simplex::{info_noise, SimplexNoise};
use super::{is_air, is_block};

/// `Biome.TEMPERATURE_NOISE`.
static HEIGHT_NOISE: LazyLock<SimplexNoise> =
    LazyLock::new(|| SimplexNoise::new(&mut LegacyRandom::new(1234)));

/// `Biome.FROZEN_TEMPERATURE_NOISE`: three octaves from one random source, finest first.
static FROZEN_NOISE: LazyLock<[SimplexNoise; 3]> = LazyLock::new(|| {
    let mut random = LegacyRandom::new(3456);
    [
        SimplexNoise::new(&mut random),
        SimplexNoise::new(&mut random),
        SimplexNoise::new(&mut random),
    ]
});

/// The octaves of [`FROZEN_NOISE`]: input frequency and amplitude, in order.
const FROZEN_OCTAVES: [(f64, f32); 3] = [(1.0, 1.0 / 7.0), (0.5, 2.0 / 7.0), (0.25, 4.0 / 7.0)];

/// The `f32` a `SimplexNoise` sample is truncated to.
fn sample(noise: &SimplexNoise, x: f64, z: f64) -> f32 {
    noise.value(x, z) as f32
}

/// The climate fields of a biome that decide its temperature and precipitation.
#[derive(Debug, Clone, Copy)]
pub struct BiomeClimate {
    base_temperature: f32,
    frozen: bool,
    has_precipitation: bool,
}

impl BiomeClimate {
    /// Reads the climate of `biome`; its `temperature_modifier` must be `none` or `frozen`.
    pub fn from_biome(biome: &Biome) -> Result<Self> {
        let frozen = match biome.temperature_modifier.as_deref() {
            None | Some("none") => false,
            Some("frozen") => true,
            Some(other) => bail!("unknown temperature_modifier {other:?}"),
        };
        Ok(Self {
            base_temperature: biome.temperature,
            frozen,
            has_precipitation: biome.has_precipitation,
        })
    }

    /// `TemperatureModifier.modifyTemperature`.
    fn modified(&self, pos: BlockPos) -> f32 {
        if self.frozen {
            let (x, z) = (f64::from(pos.x), f64::from(pos.z));
            let mut stack = 0.0f32;
            for (noise, (frequency, amplitude)) in FROZEN_NOISE.iter().zip(FROZEN_OCTAVES) {
                stack += amplitude * sample(noise, x * 0.05 * frequency, z * 0.05 * frequency);
            }
            let frozen = f64::from(stack * 7.0);
            let info = f64::from(info_noise(x * 0.2, z * 0.2) as f32);
            if frozen + info < 0.3 && f64::from(info_noise(x * 0.09, z * 0.09) as f32) < 0.8 {
                return 0.2;
            }
        }
        self.base_temperature
    }

    /// `Biome.getHeightAdjustedTemperature`.
    pub fn temperature(&self, pos: BlockPos, sea_level: i32) -> f32 {
        let base = self.modified(pos);
        let start = sea_level + 17;
        if pos.y > start {
            let variation = sample(
                &HEIGHT_NOISE,
                f64::from(pos.x as f32 / 8.0),
                f64::from(pos.z as f32 / 8.0),
            ) * 8.0;
            base - ((variation + pos.y as f32 - start as f32) * 0.05 / 40.0)
        } else {
            base
        }
    }

    fn warm_enough_to_rain(&self, pos: BlockPos, sea_level: i32) -> bool {
        self.temperature(pos, sea_level) >= 0.15
    }

    /// Whether precipitation at `pos` is snow.
    fn snows(&self, pos: BlockPos, sea_level: i32) -> bool {
        self.has_precipitation && !self.warm_enough_to_rain(pos, sea_level)
    }
}

/// The climate of the biome at `pos`, or `None` when `pos` is outside the window or its biome has
/// no climate on record.
fn climate_at<'a>(cx: &'a FeatureContext<'_, '_, '_>, pos: BlockPos) -> Option<&'a BiomeClimate> {
    cx.region
        .biome(pos)
        .and_then(|biome| cx.env.biome_climate.get(biome))
}

/// `Biome.shouldFreeze(level, pos, requireNotSurrounded)` for the biome at `biome_pos`: whether
/// the water source at `pos` is cold enough to ice over, and, with `require_edge`, has a
/// non-water neighbour to freeze from.
pub fn should_freeze(
    cx: &FeatureContext<'_, '_, '_>,
    biome_pos: BlockPos,
    pos: BlockPos,
    require_edge: bool,
) -> bool {
    let Some(climate) = climate_at(cx, biome_pos) else {
        return false;
    };
    if climate.warm_enough_to_rain(pos, cx.env.sea_level) || cx.outside_build_height(pos.y) {
        return false;
    }
    let state = block_in(cx.region, pos);
    if !(is_block(state, "water") && cx.env.facts.is(state, BlockFact::Water)) {
        return false;
    }
    if !require_edge {
        return true;
    }
    [[-1, 0], [1, 0], [0, -1], [0, 1]].iter().any(|[dx, dz]| {
        !cx.region.is(
            BlockPos::new(pos.x + dx, pos.y, pos.z + dz),
            BlockFact::Water,
        )
    })
}

/// `Biome.shouldSnow(level, pos)` for the biome at `pos`: snow falls here and a snow layer could
/// be placed, which needs air (or snow) at `pos` and the layer's survival rule to hold. An
/// unknown survival rule is tallied and counts as no.
pub fn should_snow(cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos, snow: &BlockState) -> bool {
    let Some(climate) = climate_at(cx, pos) else {
        return false;
    };
    if !climate.snows(pos, cx.env.sea_level) || cx.outside_build_height(pos.y) {
        return false;
    }
    if !(is_air(cx, pos) || is_block(block_in(cx.region, pos), "snow")) {
        return false;
    }
    match cx.env.behaviours.can_survive(snow, cx.region, pos) {
        Some(survives) => survives,
        None => {
            cx.unsupported.record("survival", "minecraft:snow");
            false
        }
    }
}
