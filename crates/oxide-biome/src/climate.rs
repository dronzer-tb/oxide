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
    // PARITY-CHECK: the router-slot -> climate-axis mapping (humidity <- `vegetation`,
    // weirdness <- `ridges`) is reconstructed from memory of `NoiseRouterData`'s overworld
    // wiring, not verified against a real Minecraft 26.2 data export.
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
}
