//! Samples the 6 climate axes vanilla's biome search uses, from an `oxide-noise` router.

use oxide_noise::{NoiseRouterEvaluator, RouterSlot};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClimateSample {
    pub temperature: f64,
    pub humidity: f64,
    pub continentalness: f64,
    pub erosion: f64,
    pub depth: f64,
    pub weirdness: f64,
}

impl ClimateSample {
    // Verified 2026-08-22 against decompiled `RandomState`'s constructor: `new
    // Climate.Sampler(router.temperature(), router.vegetation(), router.continents(),
    // router.erosion(), router.depth(), router.ridges(), ...)` — humidity <- vegetation and
    // weirdness <- ridges are exactly this mapping, not a guess.
    pub fn sample(router: &NoiseRouterEvaluator, x: i32, y: i32, z: i32) -> Self {
        Self {
            temperature: router.sample(RouterSlot::Temperature, x, y, z),
            humidity: router.sample(RouterSlot::Vegetation, x, y, z),
            continentalness: router.sample(RouterSlot::Continents, x, y, z),
            erosion: router.sample(RouterSlot::Erosion, x, y, z),
            depth: router.sample(RouterSlot::Depth, x, y, z),
            weirdness: router.sample(RouterSlot::Ridges, x, y, z),
        }
    }

    /// Same climate sample, taken through a chunk's caches. The six climate slots share most
    /// of their subtrees with each other and with the terrain density, so inside a chunk this
    /// costs a fraction of [`Self::sample`].
    pub fn sample_in_chunk(
        router: &NoiseRouterEvaluator,
        caches: &oxide_noise::ChunkCaches,
        x: i32,
        y: i32,
        z: i32,
    ) -> Self {
        Self {
            temperature: router.sample_in_chunk(caches, RouterSlot::Temperature, x, y, z),
            humidity: router.sample_in_chunk(caches, RouterSlot::Vegetation, x, y, z),
            continentalness: router.sample_in_chunk(caches, RouterSlot::Continents, x, y, z),
            erosion: router.sample_in_chunk(caches, RouterSlot::Erosion, x, y, z),
            depth: router.sample_in_chunk(caches, RouterSlot::Depth, x, y, z),
            weirdness: router.sample_in_chunk(caches, RouterSlot::Ridges, x, y, z),
        }
    }
}
