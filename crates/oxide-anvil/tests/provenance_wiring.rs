//! Integration tests: the region write paths (`write_region_file`, `update_chunk_in_place`)
//! must set exactly the provenance bit for the chunk(s) they write, and no others.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use oxide_core::{
    BlockState, ChunkData, ChunkPos, ChunkSection, ChunkStatus, Heightmap, HeightmapType,
    ResourceLocation,
};

use oxide_anvil::nbt::{self, ChunkNbtWriteOptions};
use oxide_anvil::{provenance, region};

fn scratch_dir(name: &str) -> PathBuf {
    let base = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = base
        .join("oxide-anvil-test-scratch")
        .join(format!("{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test scratch dir");
    dir
}

fn synthetic_chunk(pos: ChunkPos) -> ChunkData {
    let stone = BlockState::new(ResourceLocation::minecraft("stone"));
    let plains = ResourceLocation::minecraft("plains");
    let section = ChunkSection::new(0, stone, plains);
    let mut heightmaps = HashMap::new();
    let mut hm = Heightmap::new(384);
    for x in 0..16 {
        for z in 0..16 {
            hm.set(x, z, 128);
        }
    }
    heightmaps.insert(HeightmapType::WorldSurface, hm);
    ChunkData {
        pos,
        min_y: -64,
        height: 384,
        sections: vec![section],
        heightmaps,
        status: ChunkStatus::Full,
        needs_relight: true,
    }
}

fn opts() -> ChunkNbtWriteOptions {
    ChunkNbtWriteOptions {
        data_version: 4189,
        last_update: 1000,
        inhabited_time: 0,
    }
}

#[test]
fn write_region_file_sets_exactly_the_written_bits() {
    let dir = scratch_dir("provenance-write-region");
    let region_path = dir.join("r.0.0.mca");
    let pos_a = ChunkPos::new(2, 3);
    let pos_b = ChunkPos::new(30, 31);

    let bytes_a = nbt::serialize_chunk(&synthetic_chunk(pos_a), &opts()).unwrap();
    let bytes_b = nbt::serialize_chunk(&synthetic_chunk(pos_b), &opts()).unwrap();
    region::write_region_file(&region_path, &[(pos_a, bytes_a), (pos_b, bytes_b)]).unwrap();

    let sidecar_path = provenance::sidecar_path_for(&region_path);
    let map = provenance::load(&sidecar_path).unwrap();

    assert_eq!(map.count_set(), 2);
    assert!(map.get(pos_a));
    assert!(map.get(pos_b));
    assert!(!map.get(ChunkPos::new(0, 0)));

    assert!(provenance::is_oxide_generated(&dir, pos_a).unwrap());
    assert!(!provenance::is_oxide_generated(&dir, ChunkPos::new(5, 5)).unwrap());
}

#[test]
fn update_chunk_in_place_sets_exactly_one_bit_leaves_rest_clear() {
    let dir = scratch_dir("provenance-update-in-place");
    let region_path = dir.join("r.0.0.mca");
    let pos = ChunkPos::new(7, 11);

    let bytes = nbt::serialize_chunk(&synthetic_chunk(pos), &opts()).unwrap();
    region::update_chunk_in_place(&region_path, &pos, &bytes).unwrap();

    let sidecar_path = provenance::sidecar_path_for(&region_path);
    let map = provenance::load(&sidecar_path).unwrap();

    assert_eq!(map.count_set(), 1);
    assert!(map.get(pos));

    // Every other one of the 1024 bits in this region must remain clear.
    for x in 0..32i32 {
        for z in 0..32i32 {
            let candidate = ChunkPos::new(x, z);
            if candidate == pos {
                continue;
            }
            assert!(
                !map.get(candidate),
                "bit for {candidate:?} unexpectedly set"
            );
        }
    }
}

#[test]
fn mark_chunk_convenience_matches_wired_write_path() {
    let dir = scratch_dir("provenance-mark-chunk");
    let pos = ChunkPos::new(1, 1);

    provenance::mark_chunk(&dir, pos).unwrap();

    assert!(provenance::is_oxide_generated(&dir, pos).unwrap());
    assert!(!provenance::is_oxide_generated(&dir, ChunkPos::new(2, 2)).unwrap());
}
