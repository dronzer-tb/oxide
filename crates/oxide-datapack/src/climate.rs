//! Shared climate-parameter type used by `spawn_target` entries and
//! multi-noise biome-source parameter points. Vanilla's `Climate.Parameter`
//! codec accepts either a single value or a `[min, max]` range.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ClimateParam {
    Single(f32),
    Range([f32; 2]),
}
