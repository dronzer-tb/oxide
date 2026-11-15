//! Configuration IR of the End decoration features that carry data.
//!
//! `chorus_plant`, `end_island`, `end_platform`, `void_start_platform` and `bonus_chest` have
//! empty configs and keep their raw-value variants in `feature.rs`. `end_spike` and `end_gateway`
//! are typed here: the 26.2 export has `{ "spikes": [] }` for the spike (an empty list means the
//! layout is derived from the world seed) and `{ "exact": bool, "exit": [x, y, z] }` for the
//! gateway, `exit` being absent on the delayed one.
//!
//! Deserialization only; the generator compiles these in `oxide-chunkgen`.

use serde::{Deserialize, Serialize};

/// One obsidian pillar of an `end_spike` config.
///
/// PARITY-CHECK: the key spellings (`centerX`, `centerZ`, `radius`, `height`, `guarded`) come
/// from the reference implementation; the 26.2 export only ever lists an empty `spikes` array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpikeData {
    #[serde(rename = "centerX", default)]
    pub center_x: i32,
    #[serde(rename = "centerZ", default)]
    pub center_z: i32,
    #[serde(default)]
    pub radius: i32,
    #[serde(default)]
    pub height: i32,
    #[serde(default)]
    pub guarded: bool,
}

/// `end_spike`: the pillars around the main End island, each topped by a crystal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndSpikeConfig {
    #[serde(default)]
    pub spikes: Vec<SpikeData>,
}

/// `end_gateway`: a gateway portal block inside its bedrock frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndGatewayConfig {
    #[serde(default)]
    pub exact: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<[i32; 3]>,
}
