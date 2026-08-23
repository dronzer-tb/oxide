//! `worldgen/configured_carver`: the cave and canyon carvers a biome names.
//!
//! Deserialization only, like the rest of this crate -- carving itself lives in
//! `oxide-chunkgen`. Field names and shapes are taken from the real 26.2 export, and the
//! sampling semantics of the providers below are ported from the decompiled
//! `net.minecraft.util.valueproviders` classes rather than recalled.

use oxide_core::{RandomSource, ResourceLocation};
use serde::{Deserialize, Serialize};

use crate::surface_rule::VerticalAnchor;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ConfiguredCarver {
    #[serde(rename = "minecraft:cave")]
    Cave { config: CaveCarverConfig },

    /// Same algorithm and configuration as `cave`, with the nether's replaceable set and
    /// without the surface-aware handling; the difference lives in the carver, not the data.
    #[serde(rename = "minecraft:nether_cave")]
    NetherCave { config: CaveCarverConfig },

    #[serde(rename = "minecraft:canyon")]
    Canyon { config: CanyonCarverConfig },
}

/// Shared by every carver: how often it starts, what it may replace, and where lava sits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CarverConfig {
    pub probability: f32,
    /// Block tag (e.g. `#minecraft:overworld_carver_replaceables`) or a single block id. Block
    /// tags are not modelled by this crate, so the carver treats this as advisory -- see
    /// `oxide_chunkgen::carver`.
    pub replaceable: String,
    pub lava_level: VerticalAnchor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaveCarverConfig {
    #[serde(flatten)]
    pub base: CarverConfig,
    pub y: HeightProvider,
    #[serde(rename = "yScale")]
    pub y_scale: FloatProvider,
    pub horizontal_radius_multiplier: FloatProvider,
    pub vertical_radius_multiplier: FloatProvider,
    pub floor_level: FloatProvider,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanyonCarverConfig {
    #[serde(flatten)]
    pub base: CarverConfig,
    pub y: HeightProvider,
    #[serde(rename = "yScale")]
    pub y_scale: FloatProvider,
    pub vertical_rotation: FloatProvider,
    pub shape: CanyonShape,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanyonShape {
    pub distance_factor: FloatProvider,
    pub thickness: FloatProvider,
    pub width_smoothness: i32,
    pub horizontal_radius_factor: FloatProvider,
    pub vertical_radius_default_factor: f32,
    pub vertical_radius_center_factor: f32,
}

/// `net.minecraft.util.valueproviders.FloatProvider`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FloatProvider {
    Constant(f32),
    Tagged(TaggedFloatProvider),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TaggedFloatProvider {
    #[serde(rename = "minecraft:constant")]
    Constant { value: f32 },
    #[serde(rename = "minecraft:uniform")]
    Uniform {
        min_inclusive: f32,
        max_exclusive: f32,
    },
    #[serde(rename = "minecraft:trapezoid")]
    Trapezoid { min: f32, max: f32, plateau: f32 },
    #[serde(rename = "minecraft:clamped_normal")]
    ClampedNormal {
        mean: f32,
        deviation: f32,
        min: f32,
        max: f32,
    },
}

impl FloatProvider {
    pub fn sample(&self, random: &mut impl RandomSource) -> f32 {
        match self {
            FloatProvider::Constant(value) => *value,
            FloatProvider::Tagged(tagged) => tagged.sample(random),
        }
    }
}

impl TaggedFloatProvider {
    pub fn sample(&self, random: &mut impl RandomSource) -> f32 {
        match self {
            Self::Constant { value } => *value,
            // UniformFloat: minInclusive + nextFloat() * (maxExclusive - minInclusive)
            Self::Uniform {
                min_inclusive,
                max_exclusive,
            } => min_inclusive + random.next_float() * (max_exclusive - min_inclusive),
            // TrapezoidFloat: a plateau with linear ramps either side. Vanilla samples
            // `min + plateau + (range - plateau) * (nextFloat() - nextFloat()) / 2` where
            // range = max - min.
            Self::Trapezoid { min, max, plateau } => {
                let range = max - min;
                if *plateau >= range {
                    // Degenerates to uniform, per vanilla's own guard.
                    return min + random.next_float() * range;
                }
                let ramp = (range - plateau) / 2.0;
                let base = range - ramp;
                min + random.next_float() * base + random.next_float() * ramp
            }
            // ClampedNormalFloat: a gaussian, clamped.
            Self::ClampedNormal {
                mean,
                deviation,
                min,
                max,
            } => (mean + random.next_gaussian() as f32 * deviation).clamp(*min, *max),
        }
    }
}

/// `net.minecraft.world.level.levelgen.heightproviders.HeightProvider`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HeightProvider {
    Anchor(VerticalAnchor),
    Tagged(Box<TaggedHeightProvider>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TaggedHeightProvider {
    #[serde(rename = "minecraft:constant")]
    Constant { value: VerticalAnchor },
    #[serde(rename = "minecraft:uniform")]
    Uniform {
        min_inclusive: VerticalAnchor,
        max_inclusive: VerticalAnchor,
    },
    #[serde(rename = "minecraft:biased_to_bottom")]
    BiasedToBottom {
        min_inclusive: VerticalAnchor,
        max_inclusive: VerticalAnchor,
        #[serde(default = "one")]
        inner: i32,
    },
    #[serde(rename = "minecraft:very_biased_to_bottom")]
    VeryBiasedToBottom {
        min_inclusive: VerticalAnchor,
        max_inclusive: VerticalAnchor,
        #[serde(default = "one")]
        inner: i32,
    },
    #[serde(rename = "minecraft:trapezoid")]
    Trapezoid {
        min_inclusive: VerticalAnchor,
        max_inclusive: VerticalAnchor,
        #[serde(default)]
        plateau: i32,
    },
}

fn one() -> i32 {
    1
}

impl HeightProvider {
    /// `resolve` turns a [`VerticalAnchor`] into an absolute y for this world.
    pub fn sample(
        &self,
        random: &mut impl RandomSource,
        resolve: &impl Fn(&VerticalAnchor) -> i32,
    ) -> i32 {
        match self {
            HeightProvider::Anchor(anchor) => resolve(anchor),
            HeightProvider::Tagged(tagged) => tagged.sample(random, resolve),
        }
    }
}

impl TaggedHeightProvider {
    pub fn sample(
        &self,
        random: &mut impl RandomSource,
        resolve: &impl Fn(&VerticalAnchor) -> i32,
    ) -> i32 {
        match self {
            Self::Constant { value } => resolve(value),
            Self::Uniform {
                min_inclusive,
                max_inclusive,
            } => {
                let min = resolve(min_inclusive);
                let max = resolve(max_inclusive);
                if min >= max {
                    min
                } else {
                    random.next_int_between(min, max)
                }
            }
            // BiasedToBottomHeight: pick a bound, then a value under it, favouring low values.
            Self::BiasedToBottom {
                min_inclusive,
                max_inclusive,
                inner,
            } => {
                let min = resolve(min_inclusive);
                let max = resolve(max_inclusive);
                if min + inner > max {
                    return min;
                }
                let bound = random.next_int_between(min + inner, max);
                random.next_int_between(min, bound - 1) + 1
            }
            // VeryBiasedToBottomHeight: the same idea applied twice.
            Self::VeryBiasedToBottom {
                min_inclusive,
                max_inclusive,
                inner,
            } => {
                let min = resolve(min_inclusive);
                let max = resolve(max_inclusive);
                if min + inner > max {
                    return min;
                }
                let bound = random.next_int_between(min + inner, max);
                let second = random.next_int_between(min + inner, bound - 1);
                random.next_int_between(min, second - 1) + 1
            }
            Self::Trapezoid {
                min_inclusive,
                max_inclusive,
                plateau,
            } => {
                let min = resolve(min_inclusive);
                let max = resolve(max_inclusive);
                if min > max {
                    return min;
                }
                let range = max - min;
                if *plateau >= range {
                    return random.next_int_between(min, max);
                }
                let ramp = (range - plateau) / 2;
                let base = range - ramp;
                min + random.next_int_between(0, base) + random.next_int_between(0, ramp)
            }
        }
    }
}

/// Carver ids a biome names, e.g. `minecraft:cave`.
pub type CarverId = ResourceLocation;
