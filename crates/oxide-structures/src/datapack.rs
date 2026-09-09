//! Loading the two things jigsaw assembly reads from a pack: `.nbt` structure templates and
//! typed `worldgen/template_pool` definitions.
//!
//! `oxide-datapack` deliberately holds template pools as raw JSON — pool internals are this
//! crate's concern, not that one's — so the typed parse lives here, next to the code that walks
//! them. Templates are `.nbt`, which no other crate wants at all.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use oxide_core::ResourceLocation;

use crate::jigsaw::TemplateSource;
use crate::pool::TemplatePool;
use crate::template::StructureTemplate;

/// Templates and pools resolved from one pack root, keyed the way the pack's own ids refer to
/// each other.
#[derive(Debug, Default)]
pub struct JigsawData {
    pub templates: HashMap<ResourceLocation, StructureTemplate>,
    pub pools: HashMap<ResourceLocation, TemplatePool>,
}

impl TemplateSource for JigsawData {
    fn template(&self, id: &ResourceLocation) -> Option<&StructureTemplate> {
        self.templates.get(id)
    }

    fn pool(&self, id: &ResourceLocation) -> Option<&TemplatePool> {
        self.pools.get(id)
    }
}

impl JigsawData {
    /// Walks `data/<namespace>/structure/**/*.nbt` and
    /// `data/<namespace>/worldgen/template_pool/**/*.json` under `root`.
    ///
    /// A template that fails to parse is skipped rather than failing the load: packs ship
    /// templates from older data versions, and one unreadable house should cost that house, not
    /// the whole village. The count of skips comes back so a caller can report it.
    pub fn load(root: &Path) -> Result<(Self, usize)> {
        let mut data = JigsawData::default();
        let mut skipped = 0usize;

        let data_dir = root.join("data");
        let namespaces = std::fs::read_dir(&data_dir)
            .with_context(|| format!("reading {}", data_dir.display()))?;

        for namespace in namespaces.flatten() {
            if !namespace.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let Some(namespace_name) = namespace.file_name().to_str().map(str::to_owned) else {
                continue;
            };

            let structures = namespace.path().join("structure");
            for (id, bytes) in walk(&structures, "nbt") {
                let id = ResourceLocation::new(&namespace_name, &id);
                match StructureTemplate::from_bytes(&bytes) {
                    Ok(template) => {
                        data.templates.insert(id, template);
                    }
                    Err(_) => skipped += 1,
                }
            }

            let pools = namespace.path().join("worldgen").join("template_pool");
            for (id, bytes) in walk(&pools, "json") {
                let id = ResourceLocation::new(&namespace_name, &id);
                match serde_json::from_slice::<TemplatePool>(&bytes) {
                    Ok(pool) => {
                        data.pools.insert(id, pool);
                    }
                    Err(_) => skipped += 1,
                }
            }
        }

        Ok((data, skipped))
    }
}

/// Every file under `root` with `extension`, keyed by its path relative to `root` with the
/// extension dropped — which is how a datapack id is spelled.
fn walk(root: &Path, extension: &str) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];

    while let Some((dir, prefix)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, format!("{prefix}{name}/")));
            } else if let Some(stem) = name.strip_suffix(&format!(".{extension}")) {
                if let Ok(bytes) = std::fs::read(&path) {
                    out.push((format!("{prefix}{stem}"), bytes));
                }
            }
        }
    }

    out
}
