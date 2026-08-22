//! `serde(with = ...)` helpers for `oxide_core::ResourceLocation` /
//! `BlockState`, which don't implement `serde::{Serialize, Deserialize}`
//! themselves. Implementing those foreign traits for those foreign types
//! directly is blocked by the orphan rule from this crate, so every JSON
//! field of one of these types is annotated with one of these modules
//! instead (`#[serde(with = "crate::ident_serde::rl")]` etc.).

use oxide_core::{BlockState, ResourceLocation};
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::str::FromStr;

pub mod rl {
    use super::*;

    pub fn serialize<S: Serializer>(id: &ResourceLocation, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&id.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<ResourceLocation, D::Error> {
        let s = String::deserialize(d)?;
        ResourceLocation::from_str(&s).map_err(|e| D::Error::custom(e.to_string()))
    }
}

pub mod rl_opt {
    use super::*;

    pub fn serialize<S: Serializer>(
        id: &Option<ResourceLocation>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        id.as_ref().map(|id| id.to_string()).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<ResourceLocation>, D::Error> {
        let s: Option<String> = Option::deserialize(d)?;
        s.map(|s| ResourceLocation::from_str(&s).map_err(|e| D::Error::custom(e.to_string())))
            .transpose()
    }
}

pub mod rl_vec {
    use super::*;

    pub fn serialize<S: Serializer>(ids: &[ResourceLocation], s: S) -> Result<S::Ok, S::Error> {
        let strs: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        strs.serialize(s)
    }

    /// Accepts either a list of ids or a single bare id -- 26.2's exports write
    /// `"biome_is": "minecraft:wooded_badlands"` where earlier ones always wrote a
    /// one-element array, and both forms mean the same thing.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<ResourceLocation>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum OneOrMany {
            One(String),
            Many(Vec<String>),
        }
        let strs = match OneOrMany::deserialize(d)? {
            OneOrMany::One(s) => vec![s],
            OneOrMany::Many(v) => v,
        };
        strs.iter()
            .map(|s| ResourceLocation::from_str(s).map_err(|e| D::Error::custom(e.to_string())))
            .collect()
    }
}

pub mod rl_vec_vec {
    use super::*;

    pub fn serialize<S: Serializer>(
        ids: &[Vec<ResourceLocation>],
        s: S,
    ) -> Result<S::Ok, S::Error> {
        let strs: Vec<Vec<String>> = ids
            .iter()
            .map(|inner| inner.iter().map(|id| id.to_string()).collect())
            .collect();
        strs.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Vec<Vec<ResourceLocation>>, D::Error> {
        let strs: Vec<Vec<String>> = Vec::deserialize(d)?;
        strs.iter()
            .map(|inner| {
                inner
                    .iter()
                    .map(|s| {
                        ResourceLocation::from_str(s).map_err(|e| D::Error::custom(e.to_string()))
                    })
                    .collect()
            })
            .collect()
    }
}

/// Matches the vanilla `BlockState.CODEC` JSON shape used in `noise_settings`
/// `default_block` / `default_fluid` and surface rule `result_state`:
/// `{"Name": "...", "Properties": {...}}`.
/// PARITY-CHECK: field casing ("Name"/"Properties") is recalled from
/// vanilla's `BlockState.CODEC`, unverified against a real 26.2 data export —
/// confirm against `generated/reports/worldgen` output before trusting this
/// on real vanilla noise_settings files.
pub mod block_state {
    use super::*;

    #[derive(Deserialize)]
    struct Raw {
        #[serde(rename = "Name")]
        name: String,
        #[serde(rename = "Properties", default)]
        properties: BTreeMap<String, String>,
    }

    #[derive(Serialize)]
    struct RawRef<'a> {
        #[serde(rename = "Name")]
        name: String,
        #[serde(rename = "Properties", skip_serializing_if = "BTreeMap::is_empty")]
        properties: &'a BTreeMap<String, String>,
    }

    pub fn serialize<S: Serializer>(state: &BlockState, s: S) -> Result<S::Ok, S::Error> {
        RawRef {
            name: state.name.to_string(),
            properties: &state.properties,
        }
        .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<BlockState, D::Error> {
        let raw = Raw::deserialize(d)?;
        let name =
            ResourceLocation::from_str(&raw.name).map_err(|e| D::Error::custom(e.to_string()))?;
        Ok(BlockState {
            name,
            properties: raw.properties,
        })
    }
}

/// Biome effect colours. 26.2 writes them as `"#rrggbb"` strings and omits the
/// keys it considers defaulted; earlier exports wrote packed RGB ints and always
/// wrote all four. Accepts either form, and absence.
pub mod color_opt {
    use super::*;

    pub fn serialize<S: Serializer>(value: &Option<i32>, s: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(color) => s.serialize_str(&format!("#{:06x}", color & 0x00ff_ffff)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i32>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Color {
            Packed(i32),
            Hex(String),
        }
        match Option::<Color>::deserialize(d)? {
            None => Ok(None),
            Some(Color::Packed(packed)) => Ok(Some(packed)),
            Some(Color::Hex(hex)) => i32::from_str_radix(hex.trim_start_matches('#'), 16)
                .map(Some)
                .map_err(|e| D::Error::custom(format!("invalid colour {hex:?}: {e}"))),
        }
    }
}
