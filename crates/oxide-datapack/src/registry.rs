//! Generic `ResourceLocation`-keyed registry plus the directory walk that
//! turns `data/<namespace>/worldgen/<registry>/<path>.json` trees into one.

use crate::error::{parse_json, DatapackError, Result};
use oxide_core::ResourceLocation;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Registry<T> {
    entries: BTreeMap<ResourceLocation, T>,
    sources: BTreeMap<ResourceLocation, PathBuf>,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            sources: BTreeMap::new(),
        }
    }
}

impl<T> Registry<T> {
    pub fn get(&self, id: &ResourceLocation) -> Option<&T> {
        self.entries.get(id)
    }

    pub fn contains(&self, id: &ResourceLocation) -> bool {
        self.entries.contains_key(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ResourceLocation, &T)> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn source_of(&self, id: &ResourceLocation) -> Option<&Path> {
        self.sources.get(id).map(PathBuf::as_path)
    }

    /// Merges entries from another registry into this one, overwriting any colliding keys.
    /// This enables layered multi-datapack overlay semantics.
    pub fn merge(&mut self, other: Registry<T>) {
        for (id, val) in other.entries {
            self.entries.insert(id, val);
        }
        for (id, src) in other.sources {
            self.sources.insert(id, src);
        }
    }

    fn insert(
        &mut self,
        registry_name: &'static str,
        id: ResourceLocation,
        value: T,
        source: PathBuf,
    ) -> Result<()> {
        if let Some(first) = self.sources.get(&id) {
            return Err(DatapackError::Duplicate {
                registry: registry_name,
                id: id.to_string(),
                first: first.clone(),
                second: source,
            });
        }
        self.sources.insert(id.clone(), source);
        self.entries.insert(id, value);
        Ok(())
    }
}

/// Walk `data/*/worldgen/<registry_dir>/**/*.json` under `pack_root`, parsing
/// each file as `T` and keying the registry by `<namespace>:<relative path
/// without .json>`.
pub fn load_registry<T: serde::de::DeserializeOwned>(
    pack_root: &Path,
    registry_dir: &str,
    registry_name: &'static str,
) -> Result<Registry<T>> {
    load_registry_at(
        pack_root,
        &Path::new("worldgen").join(registry_dir),
        registry_name,
    )
}

/// Like [`load_registry`] but `registry_subpath` is relative to
/// `data/<namespace>/` directly, for registries that don't live under
/// `worldgen/` (e.g. `dimension/`, `dimension_type/`).
pub fn load_registry_at<T: serde::de::DeserializeOwned>(
    pack_root: &Path,
    registry_subpath: &Path,
    registry_name: &'static str,
) -> Result<Registry<T>> {
    let mut registry = Registry::default();
    let data_dir = pack_root.join("data");
    if !data_dir.is_dir() {
        return Err(DatapackError::NotADatapack(pack_root.to_path_buf()));
    }

    let namespaces = read_dir_sorted(&data_dir)?;
    for ns_entry in namespaces {
        if !ns_entry.is_dir() {
            continue;
        }
        let namespace = ns_entry
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let reg_root = ns_entry.join(registry_subpath);
        if !reg_root.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        collect_json_files(&reg_root, &mut files)?;
        files.sort();
        for file in files {
            let rel = file
                .strip_prefix(&reg_root)
                .expect("file was found under reg_root");
            let rel_no_ext = rel.with_extension("");
            let path_str = rel_no_ext.to_string_lossy().replace('\\', "/");
            let id = ResourceLocation::new(namespace.clone(), path_str);

            let bytes = std::fs::read(&file).map_err(|source| DatapackError::Io {
                path: file.clone(),
                source,
            })?;
            let value: T = parse_json(&file, &bytes)?;
            registry.insert(registry_name, id, value, file)?;
        }
    }
    Ok(registry)
}

fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let rd = std::fs::read_dir(dir).map_err(|source| DatapackError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in rd {
        let entry = entry.map_err(|source| DatapackError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        out.push(entry.path());
    }
    out.sort();
    Ok(out)
}

fn collect_json_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in read_dir_sorted(dir)? {
        if entry.is_dir() {
            collect_json_files(&entry, out)?;
        } else if entry.extension().and_then(|e| e.to_str()) == Some("json") {
            out.push(entry);
        }
    }
    Ok(())
}
