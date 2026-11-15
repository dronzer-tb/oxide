//! Structure template files (`data/<namespace>/structure/<path>.nbt`), read as raw NBT.
//!
//! Two consumers need the same bytes interpreted two ways: jigsaw assembly (`oxide-structures`,
//! which pulls connectors out of the jigsaw blocks) and the `fossil` / `template` decoration
//! features (`oxide-chunkgen`, which needs every palette in vanilla's placement order). This
//! module is the shared layer under both: the on-disk shape of the file and where to find it.
//! It sits here because `oxide-chunkgen` cannot depend on `oxide-structures`, and because which
//! file a template id names is a fact about the datapack tree.
//!
//! Templates are not read at load: a world needs a few dozen of the hundreds a pack carries, so
//! [`Datapack::structure_template`] reads one when asked.

use std::collections::BTreeMap;
use std::io::Read;

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use oxide_core::ResourceLocation;
use serde::{Deserialize, Serialize};

use crate::loader::Datapack;

/// One palette entry: a block id and its properties, as `NbtUtils.writeBlockState` writes them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawBlockState {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Properties", default)]
    pub properties: BTreeMap<String, String>,
}

/// One entry of the `blocks` list: a template-relative position, a palette index, and the block
/// entity data, if the block carries any.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawBlockEntry {
    pub pos: [i32; 3],
    pub state: usize,
    #[serde(default)]
    pub nbt: Option<fastnbt::Value>,
}

/// A whole template file. `palettes` (several palettes sharing one `blocks` list) takes
/// precedence over `palette` when present, as in `StructureTemplate.load`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawStructureNbt {
    #[serde(rename = "DataVersion", default)]
    pub data_version: i32,
    pub size: [i32; 3],
    #[serde(default)]
    pub palette: Vec<RawBlockState>,
    #[serde(default)]
    pub palettes: Option<Vec<Vec<RawBlockState>>>,
    #[serde(default)]
    pub blocks: Vec<RawBlockEntry>,
    #[serde(default)]
    pub entities: Vec<fastnbt::Value>,
}

impl RawStructureNbt {
    /// Parses a template file's bytes, gzip-compressed (as the game writes them) or not.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut decompressed = Vec::new();
            GzDecoder::new(bytes).read_to_end(&mut decompressed)?;
            Ok(fastnbt::from_bytes(&decompressed)?)
        } else {
            Ok(fastnbt::from_bytes(bytes)?)
        }
    }

    /// Every palette, in file order: `palettes` if the file has it, else the single `palette`.
    pub fn all_palettes(&self) -> Vec<&[RawBlockState]> {
        match &self.palettes {
            Some(palettes) => palettes.iter().map(Vec::as_slice).collect(),
            None => vec![self.palette.as_slice()],
        }
    }
}

impl Datapack {
    /// The template `id` names, read from the last pack root that has it (later packs override
    /// earlier ones, as for every other resource). `Ok(None)` when no root has the file.
    pub fn structure_template(&self, id: &ResourceLocation) -> Result<Option<RawStructureNbt>> {
        let relative = format!("data/{}/structure/{}.nbt", id.namespace(), id.path());
        for root in self.pack_roots.iter().rev() {
            let path = root.join(&relative);
            if path.is_file() {
                let bytes =
                    std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
                return RawStructureNbt::from_bytes(&bytes)
                    .with_context(|| format!("parsing {}", path.display()))
                    .map(Some);
            }
        }
        Ok(None)
    }
}
