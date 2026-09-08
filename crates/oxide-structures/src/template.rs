//! Deserialization and parsing of Minecraft 3D .nbt structure templates.

use std::collections::BTreeMap;
use std::io::Read;
use std::str::FromStr;

use flate2::read::GzDecoder;
use oxide_core::{BlockPos, BlockState, ResourceLocation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawBlockState {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Properties", default)]
    pub properties: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawBlockEntry {
    pub pos: [i32; 3],
    pub state: usize,
    #[serde(default)]
    pub nbt: Option<fastnbt::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawStructureNbt {
    #[serde(rename = "DataVersion", default)]
    pub data_version: i32,
    pub size: [i32; 3],
    #[serde(default)]
    pub palette: Vec<RawBlockState>,
    #[serde(default)]
    pub blocks: Vec<RawBlockEntry>,
}

/// A parsed 3D structure piece template.
#[derive(Debug, Clone)]
pub struct StructureTemplate {
    pub size: [i32; 3],
    pub blocks: Vec<PlacedTemplateBlock>,
    pub jigsaws: Vec<JigsawConnector>,
}

#[derive(Debug, Clone)]
pub struct PlacedTemplateBlock {
    pub pos: BlockPos,
    pub state: BlockState,
}

#[derive(Debug, Clone)]
pub struct JigsawConnector {
    pub pos: BlockPos,
    pub name: String,
    pub target: String,
    pub pool: ResourceLocation,
    pub final_state: BlockState,
    pub joint: JigsawJoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JigsawJoint {
    Rollable,
    Aligned,
}

impl StructureTemplate {
    /// Loads a structure template from raw NBT bytes (automatically handling gzip decompression).
    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        let raw: RawStructureNbt = if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut decoder = GzDecoder::new(bytes);
            let mut decompressed = Vec::new();
            decoder.read_to_end(&mut decompressed)?;
            fastnbt::from_bytes(&decompressed)?
        } else {
            fastnbt::from_bytes(bytes)?
        };

        let palette: Vec<BlockState> = raw
            .palette
            .into_iter()
            .map(|raw_state| {
                let rl = ResourceLocation::from_str(&raw_state.name)
                    .unwrap_or_else(|_| ResourceLocation::minecraft("air"));
                BlockState {
                    name: rl,
                    properties: raw_state.properties,
                }
            })
            .collect();

        let mut blocks = Vec::with_capacity(raw.blocks.len());
        let mut jigsaws = Vec::new();

        for b in raw.blocks {
            if b.state >= palette.len() {
                continue;
            }
            let pos = BlockPos::new(b.pos[0], b.pos[1], b.pos[2]);
            let state = palette[b.state].clone();

            if state.name.path() == "jigsaw" {
                if let Some(fastnbt::Value::Compound(map)) = &b.nbt {
                    let name = extract_string(map, "name").unwrap_or_default();
                    let target = extract_string(map, "target").unwrap_or_default();
                    let pool_str = extract_string(map, "pool").unwrap_or_else(|| "minecraft:empty".to_string());
                    let pool = ResourceLocation::from_str(&pool_str)
                        .unwrap_or_else(|_| ResourceLocation::minecraft("empty"));
                    let final_str = extract_string(map, "final_state").unwrap_or_else(|| "minecraft:air".to_string());
                    let final_state = BlockState::new(
                        ResourceLocation::from_str(&final_str)
                            .unwrap_or_else(|_| ResourceLocation::minecraft("air")),
                    );
                    let joint_str = extract_string(map, "joint").unwrap_or_default();
                    let joint = if joint_str == "aligned" {
                        JigsawJoint::Aligned
                    } else {
                        JigsawJoint::Rollable
                    };

                    jigsaws.push(JigsawConnector {
                        pos,
                        name,
                        target,
                        pool,
                        final_state: final_state.clone(),
                        joint,
                    });

                    // In assembled structures, jigsaw block is replaced by its final state
                    blocks.push(PlacedTemplateBlock {
                        pos,
                        state: final_state,
                    });
                    continue;
                }
            }

            blocks.push(PlacedTemplateBlock { pos, state });
        }

        Ok(Self {
            size: raw.size,
            blocks,
            jigsaws,
        })
    }
}

fn extract_string(map: &std::collections::HashMap<String, fastnbt::Value>, key: &str) -> Option<String> {
    match map.get(key) {
        Some(fastnbt::Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_template_creation() {
        let template = StructureTemplate {
            size: [5, 5, 5],
            blocks: vec![PlacedTemplateBlock {
                pos: BlockPos::new(0, 0, 0),
                state: BlockState::new(ResourceLocation::minecraft("stone")),
            }],
            jigsaws: vec![],
        };
        assert_eq!(template.blocks.len(), 1);
        assert_eq!(template.size, [5, 5, 5]);
    }
}
