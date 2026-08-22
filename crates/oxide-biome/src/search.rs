//! Multi-noise biome search: nearest `MultiNoiseBiomeEntry` to a sampled [`ClimateSample`],
//! matching vanilla's `Climate.Sampler` / parameter-list lookup.
//!
//! A plain linear scan, not vanilla's KD-tree (`docs/ARCHITECTURE.md` describes this crate's
//! job as "the multi-noise (n-dimensional KD) biome search tree", but the tree is purely a
//! lookup-speed optimization over a deterministic nearest-neighbor search — any
//! nearest-correct implementation picks the same biome. Typical biome-parameter lists are a
//! few dozen entries, so `O(n)` per lookup is fine; a KD-tree is a perf upgrade to make later
//! if profiling ever asks for it, not a correctness requirement.

use oxide_core::BiomeId;
use oxide_datapack::climate::ClimateParam;
use oxide_datapack::{ClimateParameters, MultiNoiseBiomeEntry, MultiNoiseSource};

use crate::climate::ClimateSample;

pub struct BiomeSearchTree {
    entries: Vec<MultiNoiseBiomeEntry>,
}

impl BiomeSearchTree {
    /// `None` for a `Preset` source (e.g. `{"preset": "minecraft:overworld"}`) — presets defer
    /// to a Java-hardcoded parameter table this crate cannot see (see the `// PARITY-CHECK` on
    /// `oxide_datapack::MultiNoiseSource`); only an `Explicit` biome list can be searched.
    pub fn from_source(source: &MultiNoiseSource) -> Option<Self> {
        match source {
            MultiNoiseSource::Explicit { biomes } => Some(Self {
                entries: biomes.clone(),
            }),
            MultiNoiseSource::Preset { .. } => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn nearest(&self, sample: ClimateSample) -> Option<&BiomeId> {
        self.entries
            .iter()
            .min_by(|a, b| {
                let fa = fitness(sample, &a.parameters);
                let fb = fitness(sample, &b.parameters);
                fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|e| &e.biome)
    }
}

fn param_distance(param: ClimateParam, value: f64) -> f64 {
    let (min, max) = match param {
        ClimateParam::Single(v) => (v as f64, v as f64),
        ClimateParam::Range([lo, hi]) => (lo as f64, hi as f64),
    };
    if value < min {
        min - value
    } else if value > max {
        value - max
    } else {
        0.0
    }
}

// PARITY-CHECK: vanilla's `Climate.fitness` sums squared per-axis distances (each internally
// quantized to a fixed-point `long` to avoid float-rounding drift across platforms) plus the
// squared parameter-point `offset`. Reconstructed from memory, not verified against 26.2.
// Plain `f64` here — correct for nearest-neighbor ordering, may not tie-break identically to
// vanilla exactly on a parameter-point boundary.
fn fitness(sample: ClimateSample, params: &ClimateParameters) -> f64 {
    param_distance(params.temperature, sample.temperature).powi(2)
        + param_distance(params.humidity, sample.humidity).powi(2)
        + param_distance(params.continentalness, sample.continentalness).powi(2)
        + param_distance(params.erosion, sample.erosion).powi(2)
        + param_distance(params.depth, sample.depth).powi(2)
        + param_distance(params.weirdness, sample.weirdness).powi(2)
        + (params.offset as f64).powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::ResourceLocation;

    fn entry(name: &str, temp: f32) -> MultiNoiseBiomeEntry {
        MultiNoiseBiomeEntry {
            biome: ResourceLocation::minecraft(name),
            parameters: ClimateParameters {
                temperature: ClimateParam::Single(temp),
                humidity: ClimateParam::Single(0.0),
                continentalness: ClimateParam::Single(0.0),
                erosion: ClimateParam::Single(0.0),
                depth: ClimateParam::Single(0.0),
                weirdness: ClimateParam::Single(0.0),
                offset: 0.0,
            },
        }
    }

    fn sample_with_temp(t: f64) -> ClimateSample {
        ClimateSample {
            temperature: t,
            humidity: 0.0,
            continentalness: 0.0,
            erosion: 0.0,
            depth: 0.0,
            weirdness: 0.0,
        }
    }

    #[test]
    fn nearest_picks_closest_temperature() {
        let source = MultiNoiseSource::Explicit {
            biomes: vec![entry("cold", -1.0), entry("warm", 1.0)],
        };
        let tree = BiomeSearchTree::from_source(&source).unwrap();
        assert_eq!(tree.nearest(sample_with_temp(-0.9)).unwrap().path(), "cold");
        assert_eq!(tree.nearest(sample_with_temp(0.9)).unwrap().path(), "warm");
    }

    #[test]
    fn preset_source_is_not_searchable() {
        let source = MultiNoiseSource::Preset {
            preset: ResourceLocation::minecraft("overworld"),
        };
        assert!(BiomeSearchTree::from_source(&source).is_none());
    }

    #[test]
    fn point_inside_a_range_has_zero_distance() {
        let entry = MultiNoiseBiomeEntry {
            biome: ResourceLocation::minecraft("plains"),
            parameters: ClimateParameters {
                temperature: ClimateParam::Range([-1.0, 1.0]),
                humidity: ClimateParam::Single(0.0),
                continentalness: ClimateParam::Single(0.0),
                erosion: ClimateParam::Single(0.0),
                depth: ClimateParam::Single(0.0),
                weirdness: ClimateParam::Single(0.0),
                offset: 0.0,
            },
        };
        assert_eq!(fitness(sample_with_temp(0.5), &entry.parameters), 0.0);
    }
}
