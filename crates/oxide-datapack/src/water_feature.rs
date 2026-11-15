//! Configuration IR of the underwater decoration features that carry a config.
//!
//! `seagrass` takes vanilla's `ProbabilityFeatureConfiguration` (a `float` in `[0, 1]`, the
//! chance a plant is tall) and `sea_pickle` its `CountConfiguration` (an int provider in
//! `[0, 256]`, how many pickles are tried). `kelp`, the three coral shapes and `desert_well` take
//! `NoneFeatureConfiguration` and stay raw values in [`ConfiguredFeature`](crate::feature::ConfiguredFeature).
//!
//! Deserialization only; the generator compiles these in `oxide-chunkgen`.

use serde::{Deserialize, Serialize};

use crate::placement::IntProvider;

/// `seagrass`: one plant near the origin, tall with chance `probability`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeagrassConfig {
    /// Vanilla holds this as a Java `float` and compares a `double` draw against it widened.
    // PARITY-CHECK: serde reads the JSON number as `f64` and narrows it; Java narrows the parsed
    // number the same way through `Number.floatValue`. Equal for the 26.2 values (0.3, 0.6,
    // 0.8); verify for a datapack value with more than seven significant digits.
    pub probability: f32,
}

/// `sea_pickle`: `count` pickle clusters tried near the origin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeaPickleConfig {
    pub count: IntProvider,
}
