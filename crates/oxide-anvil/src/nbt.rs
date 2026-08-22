//! Modern (1.18+) flattened chunk NBT layout.
//!
//! Lighting: this module never writes `BlockLight`/`SkyLight`. Per `docs/ARCHITECTURE.md` §
//! "Chunk output contract", Rust-generated chunks are marked as needing relight rather than
//! carrying self-computed light data. The key used for that is **`isLightOn`** (root-level
//! `TAG_Byte`, `0`/`1`), the same key vanilla itself uses to mark a chunk's light as stale after
//! e.g. a `/fill` or structure paste — `serialize_chunk` sets it to `!chunk.needs_relight`, and
//! `oxide-chunkgen` is expected to always hand us `needs_relight = true` today, so in practice
//! this crate always writes `isLightOn: false`.
// PARITY-CHECK: confirm `isLightOn` is still the relight-marker key in 26.2 (some Minecraft
// versions have renamed/relocated light-state bookkeeping); if it moved, this is the one place to
// update.

use std::collections::BTreeMap;

use fastnbt::{LongArray, Value};
use serde::{Deserialize, Serialize};

use oxide_core::{ChunkData, ChunkStatus, HeightmapType};

use crate::error::AnvilError;

/// The exact string vanilla stores in the `Status` NBT tag for each generation stage.
///
/// `oxide_core::ChunkStatus` carries its own PARITY-CHECK (`storage/chunk.rs`): the variant list
/// itself is unverified against 26.2's `ChunkStatus` registry. This mapping just renders whatever
/// that ladder is to snake_case; if the ladder changes, update this match arm-for-arm.
fn status_nbt_value(status: ChunkStatus) -> &'static str {
    match status {
        ChunkStatus::Empty => "empty",
        ChunkStatus::StructureStarts => "structure_starts",
        ChunkStatus::StructureReferences => "structure_references",
        ChunkStatus::Biomes => "biomes",
        ChunkStatus::Noise => "noise",
        ChunkStatus::Surface => "surface",
        ChunkStatus::Carvers => "carvers",
        ChunkStatus::Features => "features",
        ChunkStatus::InitializeLight => "initialize_light",
        ChunkStatus::Light => "light",
        ChunkStatus::Spawn => "spawn",
        ChunkStatus::Full => "full",
    }
}

/// Caller-supplied metadata `ChunkData` itself does not carry. `data_version` in particular must
/// never be hardcoded (see `docs/ARCHITECTURE.md` "Pinned decisions") — thread it through from
/// the loaded datapack's `version.json` / `pack.mcmeta`.
#[derive(Debug, Clone, Copy)]
pub struct ChunkNbtWriteOptions {
    /// The `DataVersion` to stamp on the chunk, read from exported pack metadata at load time.
    pub data_version: i32,
    /// World tick this chunk was (re)written at. Stored as `LastUpdate`.
    pub last_update: i64,
    /// Ticks players have spent in this chunk. `0` for a freshly generated chunk.
    pub inhabited_time: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockPaletteEntryNbt {
    #[serde(rename = "Name")]
    pub name: String,
    /// Omitted entirely (not an empty compound) when the block state has no properties — an
    /// empty `Properties: {}` is not the same NBT shape vanilla writes for property-less states.
    #[serde(
        rename = "Properties",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub properties: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockStatesNbt {
    pub palette: Vec<BlockPaletteEntryNbt>,
    /// `None` (key omitted) for a single-entry palette: a section that's uniformly one block
    /// state carries no `data` long-array at all. Emitting a zeroed array here is exactly the
    /// "subtly wrong chunk" the task brief calls out.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data: Option<LongArray>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BiomesNbt {
    pub palette: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data: Option<LongArray>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SectionNbt {
    #[serde(rename = "Y")]
    pub y: i8,
    pub block_states: BlockStatesNbt,
    pub biomes: BiomesNbt,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HeightmapsNbt {
    #[serde(
        rename = "WORLD_SURFACE",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub world_surface: Option<LongArray>,
    #[serde(
        rename = "WORLD_SURFACE_WG",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub world_surface_wg: Option<LongArray>,
    #[serde(
        rename = "OCEAN_FLOOR",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub ocean_floor: Option<LongArray>,
    #[serde(
        rename = "OCEAN_FLOOR_WG",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub ocean_floor_wg: Option<LongArray>,
    #[serde(
        rename = "MOTION_BLOCKING",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub motion_blocking: Option<LongArray>,
    #[serde(
        rename = "MOTION_BLOCKING_NO_LEAVES",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub motion_blocking_no_leaves: Option<LongArray>,
}

/// `structures.starts` / `structures.References`, both empty compounds when Oxide has placed (or
/// referenced) no structures — `oxide-structures` populates these once it lands.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct StructuresNbt {
    pub starts: BTreeMap<String, Value>,
    #[serde(rename = "References")]
    pub references: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChunkNbtRoot {
    #[serde(rename = "DataVersion")]
    pub data_version: i32,
    #[serde(rename = "xPos")]
    pub x_pos: i32,
    #[serde(rename = "zPos")]
    pub z_pos: i32,
    #[serde(rename = "yPos")]
    pub y_pos: i32,
    #[serde(rename = "Status")]
    pub status: String,
    #[serde(rename = "LastUpdate")]
    pub last_update: i64,
    #[serde(rename = "InhabitedTime")]
    pub inhabited_time: i64,
    #[serde(rename = "isLightOn")]
    pub is_light_on: bool,
    pub sections: Vec<SectionNbt>,
    #[serde(rename = "Heightmaps")]
    pub heightmaps: HeightmapsNbt,
    pub block_entities: Vec<Value>,
    pub block_ticks: Vec<Value>,
    pub fluid_ticks: Vec<Value>,
    pub structures: StructuresNbt,
    // PARITY-CHECK: vanilla's `PostProcessing` is one (usually empty) short-list per section;
    // verify against a real 26.2 save that it's still keyed 1:1 with `sections.len()` rather than
    // a fixed world-height count.
    #[serde(rename = "PostProcessing")]
    pub post_processing: Vec<Vec<i16>>,
}

fn block_palette_entry(bs: &oxide_core::BlockState) -> BlockPaletteEntryNbt {
    BlockPaletteEntryNbt {
        name: bs.name.to_string(),
        properties: (!bs.properties.is_empty()).then(|| bs.properties.clone()),
    }
}

fn build_section(section: &oxide_core::ChunkSection) -> SectionNbt {
    let block_palette = section.block_states.palette();
    let block_data =
        (block_palette.len() > 1).then(|| LongArray::new(section.block_states.to_packed_longs()));
    let biome_palette = section.biomes.palette();
    let biome_data =
        (biome_palette.len() > 1).then(|| LongArray::new(section.biomes.to_packed_longs()));

    SectionNbt {
        y: section.y,
        block_states: BlockStatesNbt {
            palette: block_palette.iter().map(block_palette_entry).collect(),
            data: block_data,
        },
        biomes: BiomesNbt {
            palette: biome_palette.iter().map(|b| b.to_string()).collect(),
            data: biome_data,
        },
    }
}

fn build_heightmaps(chunk: &ChunkData) -> HeightmapsNbt {
    let mut out = HeightmapsNbt::default();
    for (ty, hm) in &chunk.heightmaps {
        let packed = Some(LongArray::new(hm.to_packed_longs()));
        match ty {
            HeightmapType::WorldSurface => out.world_surface = packed,
            HeightmapType::WorldSurfaceWg => out.world_surface_wg = packed,
            HeightmapType::OceanFloor => out.ocean_floor = packed,
            HeightmapType::OceanFloorWg => out.ocean_floor_wg = packed,
            HeightmapType::MotionBlocking => out.motion_blocking = packed,
            HeightmapType::MotionBlockingNoLeaves => out.motion_blocking_no_leaves = packed,
        }
    }
    out
}

/// Build the root NBT compound for `chunk`. Does not compress or write to disk — see
/// `region::update_chunk_in_place` / `region::write_region_file` for that.
pub fn build_chunk_root(
    chunk: &ChunkData,
    opts: &ChunkNbtWriteOptions,
) -> Result<ChunkNbtRoot, AnvilError> {
    if chunk.min_y % 16 != 0 {
        return Err(AnvilError::UnalignedMinY(chunk.min_y));
    }
    let y_pos = chunk.min_y / 16;
    let section_count = chunk.sections.len();

    Ok(ChunkNbtRoot {
        data_version: opts.data_version,
        x_pos: chunk.pos.x,
        z_pos: chunk.pos.z,
        y_pos,
        status: status_nbt_value(chunk.status).to_string(),
        last_update: opts.last_update,
        inhabited_time: opts.inhabited_time,
        is_light_on: !chunk.needs_relight,
        sections: chunk.sections.iter().map(build_section).collect(),
        heightmaps: build_heightmaps(chunk),
        block_entities: Vec::new(),
        block_ticks: Vec::new(),
        fluid_ticks: Vec::new(),
        structures: StructuresNbt::default(),
        post_processing: vec![Vec::new(); section_count],
    })
}

/// Serialize `chunk` to uncompressed NBT bytes, ready for `region::update_chunk_in_place` /
/// `Region::write_chunk` (both compress).
pub fn serialize_chunk(
    chunk: &ChunkData,
    opts: &ChunkNbtWriteOptions,
) -> Result<Vec<u8>, AnvilError> {
    let root = build_chunk_root(chunk, opts)?;
    Ok(fastnbt::to_bytes(&root)?)
}

/// Parse uncompressed chunk NBT bytes (as returned by `region::read_chunk`) back into the typed
/// root. Used for round-tripping and by the validation harness to compare Oxide output against
/// vanilla reference chunks.
pub fn parse_chunk(bytes: &[u8]) -> Result<ChunkNbtRoot, AnvilError> {
    Ok(fastnbt::from_bytes(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::{
        BiomeId, BlockState, ChunkPos, ChunkSection, ChunkStatus, Heightmap, ResourceLocation,
    };
    use std::collections::HashMap;

    fn stone() -> BlockState {
        BlockState::new(ResourceLocation::minecraft("stone"))
    }

    fn dirt() -> BlockState {
        BlockState::new(ResourceLocation::minecraft("farmland")).with_property("moisture", "3")
    }

    fn plains() -> BiomeId {
        ResourceLocation::minecraft("plains")
    }

    fn uniform_section(y: i8) -> ChunkSection {
        ChunkSection::new(y, stone(), plains())
    }

    fn mixed_section(y: i8) -> ChunkSection {
        let mut s = ChunkSection::new(y, stone(), plains());
        s.block_states.set(0, dirt());
        s
    }

    fn flat_heightmap(relative_y: i32) -> Heightmap {
        let mut hm = Heightmap::new(384);
        for x in 0..16 {
            for z in 0..16 {
                hm.set(x, z, relative_y);
            }
        }
        hm
    }

    fn sample_chunk(sections: Vec<ChunkSection>) -> ChunkData {
        let mut heightmaps = HashMap::new();
        heightmaps.insert(HeightmapType::WorldSurface, flat_heightmap(128));
        heightmaps.insert(HeightmapType::MotionBlocking, flat_heightmap(129));
        ChunkData {
            pos: ChunkPos::new(3, -2),
            min_y: -64,
            height: 384,
            sections,
            heightmaps,
            status: ChunkStatus::Full,
            needs_relight: true,
        }
    }

    fn opts() -> ChunkNbtWriteOptions {
        ChunkNbtWriteOptions {
            data_version: 4189, // placeholder for tests only, never used to write real chunks.
            last_update: 100,
            inhabited_time: 0,
        }
    }

    #[test]
    fn single_entry_palette_has_no_data_array() {
        let chunk = sample_chunk(vec![uniform_section(-4)]);
        let root = build_chunk_root(&chunk, &opts()).unwrap();
        let section = &root.sections[0];
        assert_eq!(section.block_states.palette.len(), 1);
        assert!(section.block_states.data.is_none());
        assert_eq!(section.biomes.palette.len(), 1);
        assert!(section.biomes.data.is_none());
    }

    #[test]
    fn multi_entry_palette_carries_data() {
        let chunk = sample_chunk(vec![mixed_section(-4)]);
        let root = build_chunk_root(&chunk, &opts()).unwrap();
        let section = &root.sections[0];
        assert_eq!(section.block_states.palette.len(), 2);
        assert!(section.block_states.data.is_some());
    }

    #[test]
    fn properties_omitted_when_empty() {
        let chunk = sample_chunk(vec![mixed_section(-4)]);
        let root = build_chunk_root(&chunk, &opts()).unwrap();
        let entries = &root.sections[0].block_states.palette;
        let stone_entry = entries
            .iter()
            .find(|e| e.name == "minecraft:stone")
            .unwrap();
        assert!(stone_entry.properties.is_none());
        let dirt_entry = entries
            .iter()
            .find(|e| e.name == "minecraft:farmland")
            .unwrap();
        assert_eq!(
            dirt_entry.properties.as_ref().unwrap().get("moisture"),
            Some(&"3".to_string())
        );
    }

    #[test]
    fn is_light_on_reflects_needs_relight() {
        let chunk = sample_chunk(vec![uniform_section(-4)]);
        let root = build_chunk_root(&chunk, &opts()).unwrap();
        assert!(!root.is_light_on);
    }

    #[test]
    fn rejects_unaligned_min_y() {
        let mut chunk = sample_chunk(vec![uniform_section(-4)]);
        chunk.min_y = -63;
        assert!(matches!(
            build_chunk_root(&chunk, &opts()),
            Err(AnvilError::UnalignedMinY(-63))
        ));
    }

    #[test]
    fn nbt_bytes_round_trip() {
        let chunk = sample_chunk(vec![uniform_section(-4), mixed_section(-3)]);
        let bytes = serialize_chunk(&chunk, &opts()).unwrap();
        let parsed = parse_chunk(&bytes).unwrap();
        assert_eq!(parsed.x_pos, 3);
        assert_eq!(parsed.z_pos, -2);
        assert_eq!(parsed.y_pos, -4);
        assert_eq!(parsed.status, "full");
        assert_eq!(parsed.sections.len(), 2);
        assert_eq!(
            parsed.heightmaps.world_surface.as_deref(),
            Some(flat_heightmap(128).to_packed_longs().as_slice())
        );
        assert!(parsed.structures.starts.is_empty());
        assert!(parsed.structures.references.is_empty());
        assert_eq!(parsed.post_processing.len(), 2);
    }
}
