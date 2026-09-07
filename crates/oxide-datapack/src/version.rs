//! Pack version metadata: `pack.mcmeta` and the optional `version.json`
//! written by the data generator's `--reports` output. The DataVersion is
//! never hardcoded (see docs/ARCHITECTURE.md) — it must come from one of
//! these files, or loading fails.

use crate::error::{parse_json, DatapackError, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackMeta {
    pub pack: PackMetaInner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackMetaInner {
    pub description: serde_json::Value,
    pub pack_format: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_formats: Option<serde_json::Value>,
    /// Non-standard extension some exports embed directly in pack.mcmeta.
    /// Vanilla `pack.mcmeta` alone does not normally carry this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_version: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_name: Option<String>,
}

/// Data generator `--reports` output: `generated/reports/version.json` in a
/// real export, expected copied alongside the pack root as `version.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionJson {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub stable: bool,
    #[serde(alias = "world_version")]
    pub data_version: i32,
}

/// Resolved version metadata for a loaded pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataVersion {
    pub data_version: i32,
    pub version_name: String,
}

/// Resolve DataVersion + version name. `version.json` (the data generator's
/// `--reports` output) is authoritative when present. Otherwise falls back to
/// a `data_version`/`version_name` extension in `pack.mcmeta`, if the export
/// embeds one. Vanilla `pack.mcmeta` alone (just `pack_format`) does not
/// carry a DataVersion integer, so if neither file supplies one, this
/// fails loudly rather than guessing.
pub fn load_data_version(pack_root: &Path) -> Result<DataVersion> {
    let version_json_path = pack_root.join("version.json");
    let pack_mcmeta_path = pack_root.join("pack.mcmeta");

    if version_json_path.is_file() {
        let bytes = std::fs::read(&version_json_path).map_err(|source| DatapackError::Io {
            path: version_json_path.clone(),
            source,
        })?;
        let v: VersionJson = parse_json(&version_json_path, &bytes)?;
        return Ok(DataVersion {
            data_version: v.data_version,
            version_name: v.name,
        });
    }

    if let Some(meta) = load_pack_meta(pack_root)? {
        if let Some(data_version) = meta.pack.data_version {
            return Ok(DataVersion {
                data_version,
                version_name: meta.pack.version_name.unwrap_or_default(),
            });
        }
    }

    Err(DatapackError::MissingDataVersion {
        version_json: version_json_path,
        pack_mcmeta: pack_mcmeta_path,
    })
}

/// Load `pack.mcmeta` if present. Independent of DataVersion resolution —
/// `pack_format` is not a DataVersion and must not be conflated with one.
pub fn load_pack_meta(pack_root: &Path) -> Result<Option<PackMeta>> {
    let path = pack_root.join("pack.mcmeta");
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).map_err(|source| DatapackError::Io {
        path: path.clone(),
        source,
    })?;
    let meta: PackMeta = parse_json(&path, &bytes)?;
    Ok(Some(meta))
}
