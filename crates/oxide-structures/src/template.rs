//! Minecraft `.nbt` structure templates: the block palette a jigsaw piece is made of, and the
//! jigsaw connectors that decide what may attach to it.
//!
//! Ported from 26.2's `StructureTemplate`. The rotation-aware accessors ([`StructureTemplate::
//! jigsaws_at`], [`StructureTemplate::bounding_box`], [`StructureTemplate::blocks_at`]) mirror
//! `getJigsaws` / `getBoundingBox` / the placement pass: vanilla stores one unrotated copy and
//! transforms on read, so every rotation of a piece comes from the same parsed template.

use std::collections::BTreeMap;
use std::io::Read;
use std::str::FromStr;

use flate2::read::GzDecoder;
use oxide_core::{BlockPos, BlockState, ResourceLocation};
use serde::{Deserialize, Serialize};

use crate::jigsaw::BoundingBox;
use crate::rotation::{
    parse_front_and_top, rotate_block_state, transform, Direction, Mirror, Rotation,
};

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

/// A parsed 3D structure piece template, stored unrotated.
#[derive(Debug, Clone)]
pub struct StructureTemplate {
    pub size: [i32; 3],
    /// Every block except the jigsaw blocks, which are held in `jigsaws` and contribute their
    /// `final_state` at placement time.
    pub blocks: Vec<PlacedTemplateBlock>,
    pub jigsaws: Vec<JigsawConnector>,
}

#[derive(Debug, Clone)]
pub struct PlacedTemplateBlock {
    pub pos: BlockPos,
    pub state: BlockState,
}

/// One jigsaw block: where it is, which way it faces, what it will accept, and what it turns
/// into once the structure is placed.
#[derive(Debug, Clone)]
pub struct JigsawConnector {
    pub pos: BlockPos,
    /// The face this connector reaches out of, from the block's `orientation` property.
    pub front: Direction,
    /// The connector's "up", also from `orientation`. Only an `aligned` joint checks it.
    pub top: Direction,
    pub name: String,
    pub target: String,
    pub pool: ResourceLocation,
    pub final_state: BlockState,
    pub joint: JigsawJoint,
    /// `SequencedPriorityIterator` order: which connectors get expanded first, across pieces.
    pub placement_priority: i32,
    /// Order *within* one piece's connector list, applied after the shuffle.
    pub selection_priority: i32,
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
                    // A jigsaw block with no readable orientation cannot be attached to and is
                    // not a wall either -- vanilla would still place it, so it stays a block.
                    let orientation = state
                        .properties
                        .get("orientation")
                        .and_then(|value| parse_front_and_top(value));
                    if let Some((front, top)) = orientation {
                        let final_str = extract_string(map, "final_state")
                            .unwrap_or_else(|| "minecraft:air".to_string());
                        let final_state = parse_state_spec(&final_str);
                        jigsaws.push(JigsawConnector {
                            pos,
                            front,
                            top,
                            name: extract_string(map, "name").unwrap_or_default(),
                            target: extract_string(map, "target").unwrap_or_default(),
                            pool: extract_string(map, "pool")
                                .and_then(|p| ResourceLocation::from_str(&p).ok())
                                .unwrap_or_else(|| ResourceLocation::minecraft("empty")),
                            final_state,
                            joint: match extract_string(map, "joint").as_deref() {
                                Some("aligned") => JigsawJoint::Aligned,
                                // Vanilla defaults by orientation: a connector facing up or down
                                // is aligned, a horizontal one is rollable.
                                Some("rollable") => JigsawJoint::Rollable,
                                _ if front.is_vertical() => JigsawJoint::Aligned,
                                _ => JigsawJoint::Rollable,
                            },
                            placement_priority: extract_int(map, "placement_priority"),
                            selection_priority: extract_int(map, "selection_priority"),
                        });
                        continue;
                    }
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

    /// `StructureTemplate.getBoundingBox`: transform the two opposite corners, then move to
    /// `position`. The pivot is the origin and the mirror is none, matching the
    /// `StructurePlaceSettings` jigsaw placement builds.
    pub fn bounding_box(&self, position: BlockPos, rotation: Rotation) -> BoundingBox {
        let origin = BlockPos::new(0, 0, 0);
        let delta = BlockPos::new(self.size[0] - 1, self.size[1] - 1, self.size[2] - 1);
        let a = transform(origin, Mirror::None, rotation, origin);
        let b = transform(delta, Mirror::None, rotation, origin);
        BoundingBox::new(
            a.x.min(b.x) + position.x,
            a.y.min(b.y) + position.y,
            a.z.min(b.z) + position.z,
            a.x.max(b.x) + position.x,
            a.y.max(b.y) + position.y,
            a.z.max(b.z) + position.z,
        )
    }

    /// `StructureTemplate.getJigsaws`: connectors with their positions transformed and their
    /// facings turned, offset to `position`.
    pub fn jigsaws_at(&self, position: BlockPos, rotation: Rotation) -> Vec<JigsawConnector> {
        let origin = BlockPos::new(0, 0, 0);
        self.jigsaws
            .iter()
            .map(|jigsaw| {
                let moved = transform(jigsaw.pos, Mirror::None, rotation, origin);
                JigsawConnector {
                    pos: BlockPos::new(
                        moved.x + position.x,
                        moved.y + position.y,
                        moved.z + position.z,
                    ),
                    front: rotation.rotate(jigsaw.front),
                    top: rotation.rotate(jigsaw.top),
                    ..jigsaw.clone()
                }
            })
            .collect()
    }

    /// Every block this template places at `position` under `rotation`, jigsaw connectors
    /// included as their `final_state` — which is what a jigsaw block leaves behind once the
    /// structure is generated.
    pub fn blocks_at(&self, position: BlockPos, rotation: Rotation) -> Vec<(BlockPos, BlockState)> {
        let origin = BlockPos::new(0, 0, 0);
        let place = |pos: BlockPos| {
            let moved = transform(pos, Mirror::None, rotation, origin);
            BlockPos::new(
                moved.x + position.x,
                moved.y + position.y,
                moved.z + position.z,
            )
        };

        let mut out = Vec::with_capacity(self.blocks.len() + self.jigsaws.len());
        for block in &self.blocks {
            out.push((place(block.pos), rotate_block_state(&block.state, rotation)));
        }
        for jigsaw in &self.jigsaws {
            out.push((
                place(jigsaw.pos),
                rotate_block_state(&jigsaw.final_state, rotation),
            ));
        }
        out
    }
}

/// `final_state` is a block-state *spec* (`minecraft:oak_planks[axis=y]`), not a bare id.
fn parse_state_spec(spec: &str) -> BlockState {
    let (name, properties) = match spec.split_once('[') {
        Some((name, rest)) => (name, rest.trim_end_matches(']')),
        None => (spec, ""),
    };
    let mut state = BlockState::new(
        ResourceLocation::from_str(name).unwrap_or_else(|_| ResourceLocation::minecraft("air")),
    );
    for pair in properties.split(',').filter(|p| !p.is_empty()) {
        if let Some((key, value)) = pair.split_once('=') {
            state
                .properties
                .insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    state
}

fn extract_string(map: &std::collections::HashMap<String, fastnbt::Value>, key: &str) -> Option<String> {
    match map.get(key) {
        Some(fastnbt::Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Priorities are optional ints that default to 0, and NBT may hand them over in any integer
/// width depending on what wrote the file.
fn extract_int(map: &std::collections::HashMap<String, fastnbt::Value>, key: &str) -> i32 {
    match map.get(key) {
        Some(fastnbt::Value::Int(v)) => *v,
        Some(fastnbt::Value::Short(v)) => *v as i32,
        Some(fastnbt::Value::Byte(v)) => *v as i32,
        Some(fastnbt::Value::Long(v)) => *v as i32,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connector(pos: BlockPos, front: Direction) -> JigsawConnector {
        JigsawConnector {
            pos,
            front,
            top: Direction::Up,
            name: "side".into(),
            target: "side".into(),
            pool: ResourceLocation::minecraft("empty"),
            final_state: BlockState::new(ResourceLocation::minecraft("air")),
            joint: JigsawJoint::Rollable,
            placement_priority: 0,
            selection_priority: 0,
        }
    }

    fn template() -> StructureTemplate {
        StructureTemplate {
            size: [5, 3, 7],
            blocks: vec![PlacedTemplateBlock {
                pos: BlockPos::new(0, 0, 0),
                state: BlockState::new(ResourceLocation::minecraft("stone")),
            }],
            jigsaws: vec![connector(BlockPos::new(4, 1, 0), Direction::East)],
        }
    }

    #[test]
    fn a_quarter_turn_swaps_the_footprint() {
        let unrotated = template().bounding_box(BlockPos::new(0, 0, 0), Rotation::None);
        assert_eq!((unrotated.max_x, unrotated.max_z), (4, 6));
        let turned = template().bounding_box(BlockPos::new(0, 0, 0), Rotation::Clockwise90);
        assert_eq!(
            (turned.max_x - turned.min_x, turned.max_z - turned.min_z),
            (6, 4),
            "a 5x7 footprint must become 7x5"
        );
    }

    #[test]
    fn a_rotated_connector_moves_and_turns_with_its_piece() {
        let jigsaws = template().jigsaws_at(BlockPos::new(100, 64, 100), Rotation::Clockwise90);
        let jigsaw = &jigsaws[0];
        assert_eq!(jigsaw.front, Direction::South, "east turns to south");
        // (4, 1, 0) clockwise about the origin is (0, 1, 4), then offset.
        assert_eq!(jigsaw.pos, BlockPos::new(100, 65, 104));
    }

    #[test]
    fn a_jigsaw_block_leaves_its_final_state_behind() {
        let mut template = template();
        template.jigsaws[0].final_state =
            BlockState::new(ResourceLocation::minecraft("oak_planks"));
        let placed = template.blocks_at(BlockPos::new(0, 0, 0), Rotation::None);
        assert!(placed
            .iter()
            .any(|(pos, state)| *pos == BlockPos::new(4, 1, 0)
                && state.name.path() == "oak_planks"));
    }

    #[test]
    fn a_final_state_spec_keeps_its_properties() {
        let state = parse_state_spec("minecraft:oak_stairs[facing=north,half=top]");
        assert_eq!(state.name.path(), "oak_stairs");
        assert_eq!(state.properties.get("facing").unwrap(), "north");
        assert_eq!(state.properties.get("half").unwrap(), "top");
    }
}
