//! oxide-pregen: writes Oxide-generated chunks straight into a world's region files, with the
//! server stopped. See `src/main.rs` for what the chunks hold and what the server adds later.

pub mod snbt;

use std::collections::HashMap;

use anyhow::{Context, Result};
use fastnbt::Value;
use oxide_anvil::nbt::{build_chunk_root, ChunkNbtWriteOptions};
use oxide_anvil::region::compress_chunk;
use oxide_chunkgen::ChunkExtras;
use oxide_core::{BlockPos, ChunkData};

/// `chunk` and its decoration extras as compressed chunk NBT, ready for a region file.
pub fn chunk_bytes(
    chunk: &ChunkData,
    extras: &ChunkExtras,
    opts: &ChunkNbtWriteOptions,
) -> Result<Vec<u8>> {
    let mut root = build_chunk_root(chunk, opts)?;
    for entity in &extras.block_entities {
        root.block_entities.push(
            snbt::parse(&entity.snbt).with_context(|| format!("block entity {}", entity.snbt))?,
        );
    }
    root.fluid_ticks = extras
        .fluid_ticks
        .iter()
        .map(|(p, id)| tick(*p, &id.to_string()))
        .collect();
    root.block_ticks = extras
        .block_ticks
        .iter()
        .map(|(p, id)| tick(*p, &id.to_string()))
        .collect();
    for p in &extras.post_process {
        let section = (p.y - chunk.min_y).div_euclid(16) as usize;
        if let Some(list) = root.post_processing.get_mut(section) {
            // ProtoChunk.packOffsetCoordinate
            list.push(((p.x & 15) | (p.y & 15) << 4 | (p.z & 15) << 8) as i16);
        }
    }
    let nbt = fastnbt::to_bytes(&root).context("serialising chunk NBT")?;
    Ok(compress_chunk(&nbt)?)
}

/// `SavedTick.save` as a proto chunk keeps it: delay and priority 0.
fn tick(pos: BlockPos, id: &str) -> Value {
    Value::Compound(HashMap::from([
        ("i".to_string(), Value::String(id.to_string())),
        ("x".to_string(), Value::Int(pos.x)),
        ("y".to_string(), Value::Int(pos.y)),
        ("z".to_string(), Value::Int(pos.z)),
        ("t".to_string(), Value::Int(0)),
        ("p".to_string(), Value::Int(0)),
    ]))
}
