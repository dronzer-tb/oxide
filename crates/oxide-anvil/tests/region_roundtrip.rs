//! Integration tests for `oxide-anvil`'s region writer/reader, exercising real files on disk.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use oxide_core::{
    BlockState, ChunkData, ChunkPos, ChunkSection, ChunkStatus, Heightmap, HeightmapType,
    ResourceLocation,
};

use oxide_anvil::nbt::{self, ChunkNbtWriteOptions};
use oxide_anvil::region;

/// Scratch directory under `$CARGO_TARGET_DIR` (never `/tmp` directly), unique per test.
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

fn stone() -> BlockState {
    BlockState::new(ResourceLocation::minecraft("stone"))
}

fn air() -> BlockState {
    BlockState::new(ResourceLocation::minecraft("air"))
}

fn plains() -> ResourceLocation {
    ResourceLocation::minecraft("plains")
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

fn synthetic_chunk(pos: ChunkPos, section_count: usize) -> ChunkData {
    let mut sections = Vec::new();
    for i in 0..section_count {
        let mut section = ChunkSection::new((i as i8) - 4, stone(), plains());
        section.block_states.set(0, air()); // keep block palettes 2-entry so `data` is exercised.
        sections.push(section);
    }
    let mut heightmaps = HashMap::new();
    heightmaps.insert(HeightmapType::WorldSurface, flat_heightmap(128));
    heightmaps.insert(HeightmapType::MotionBlocking, flat_heightmap(129));
    ChunkData {
        pos,
        min_y: -64,
        height: 384,
        sections,
        heightmaps,
        status: ChunkStatus::Full,
        needs_relight: true,
    }
}

/// A chunk whose sections have a maximally-distinct 4096-entry block palette, so zlib can't
/// compress it back down under one sector — used to force sector reallocation.
fn maximal_chunk(pos: ChunkPos, section_count: usize) -> ChunkData {
    let mut chunk = synthetic_chunk(pos, section_count);
    for section in &mut chunk.sections {
        for i in 0..4096usize {
            let block = BlockState::new(ResourceLocation::minecraft(format!("block_{i}")));
            section.block_states.set(i, block);
        }
    }
    chunk
}

fn opts() -> ChunkNbtWriteOptions {
    ChunkNbtWriteOptions {
        // PARITY-CHECK: placeholder DataVersion for tests only. Real callers must supply the
        // value read from the loaded pack's version.json/pack.mcmeta, per the architecture doc.
        data_version: 4189,
        last_update: 1000,
        inhabited_time: 0,
    }
}

#[test]
fn write_read_roundtrip_through_region_file() {
    let dir = scratch_dir("roundtrip");
    let region_path = dir.join("r.0.0.mca");
    let pos = ChunkPos::new(3, 5);
    let chunk = synthetic_chunk(pos, 2);
    let write_opts = opts();

    let nbt_bytes = nbt::serialize_chunk(&chunk, &write_opts).unwrap();
    region::write_region_file(&region_path, &[(pos, nbt_bytes)]).unwrap();

    let read_back = region::read_chunk_nbt(&region_path, &pos).unwrap().unwrap();
    let expected = nbt::build_chunk_root(&chunk, &write_opts).unwrap();
    assert_eq!(read_back, expected);
}

#[test]
fn missing_chunk_reads_as_none() {
    let dir = scratch_dir("missing-chunk");
    let region_path = dir.join("r.0.0.mca");
    let present = ChunkPos::new(0, 0);
    let absent = ChunkPos::new(1, 1);
    let nbt_bytes = nbt::serialize_chunk(&synthetic_chunk(present, 1), &opts()).unwrap();
    region::write_region_file(&region_path, &[(present, nbt_bytes)]).unwrap();

    assert!(region::read_chunk(&region_path, &absent).unwrap().is_none());
}

#[test]
fn update_in_place_creates_region_when_missing() {
    let dir = scratch_dir("update-creates");
    let region_path = dir.join("r.0.0.mca");
    let pos = ChunkPos::new(0, 0);
    let nbt_bytes = nbt::serialize_chunk(&synthetic_chunk(pos, 1), &opts()).unwrap();

    region::update_chunk_in_place(&region_path, &pos, &nbt_bytes).unwrap();

    assert!(region::read_chunk(&region_path, &pos).unwrap().is_some());
}

/// Raw offset-table entry for chunk `(x, z)`: `(sector_offset, sector_count)`.
fn read_header_entry(file: &mut File, x: usize, z: usize) -> (u64, u8) {
    file.seek(SeekFrom::Start((4 * (x + z * 32)) as u64))
        .unwrap();
    let mut buf = [0u8; 4];
    file.read_exact(&mut buf).unwrap();
    let offset = ((buf[0] as u64) << 16) | ((buf[1] as u64) << 8) | buf[2] as u64;
    (offset, buf[3])
}

#[test]
fn header_offsets_land_on_4096_byte_boundaries() {
    let dir = scratch_dir("header-alignment");
    let region_path = dir.join("r.0.0.mca");
    let pos_a = ChunkPos::new(0, 0);
    let pos_b = ChunkPos::new(1, 0);
    let a = nbt::serialize_chunk(&synthetic_chunk(pos_a, 1), &opts()).unwrap();
    let b = nbt::serialize_chunk(&synthetic_chunk(pos_b, 3), &opts()).unwrap();
    region::write_region_file(&region_path, &[(pos_a, a), (pos_b, b)]).unwrap();

    let mut file = File::open(&region_path).unwrap();
    let len = file.metadata().unwrap().len();
    assert_eq!(len % 4096, 0, "region file length must be sector-aligned");

    for (x, z) in [(0usize, 0usize), (1, 0)] {
        let (offset, sectors) = read_header_entry(&mut file, x, z);
        assert!(offset >= 2, "chunk data must start after the 8KiB header");
        assert!(sectors >= 1);
        assert!(
            offset * 4096 + (sectors as u64) * 4096 <= len,
            "chunk sectors must fit inside the file"
        );
    }
}

#[test]
fn growing_a_chunk_reallocates_past_its_old_sector_run() {
    let dir = scratch_dir("sector-growth");
    let region_path = dir.join("r.0.0.mca");
    let pos = ChunkPos::new(0, 0);

    // Small chunk first: 1 section, highly compressible (uniform-ish) -> should fit in the
    // minimum 1-sector allocation.
    let small = nbt::serialize_chunk(&synthetic_chunk(pos, 1), &opts()).unwrap();
    region::update_chunk_in_place(&region_path, &pos, &small).unwrap();

    let (small_offset, small_sectors) = {
        let mut file = File::open(&region_path).unwrap();
        read_header_entry(&mut file, 0, 0)
    };

    // Now grow it well past what fits in that many sectors: many sections, each with a
    // maximally-distinct per-block palette so zlib can't compress it back down.
    let big_chunk = maximal_chunk(pos, 16);
    let big = nbt::serialize_chunk(&big_chunk, &opts()).unwrap();
    assert!(
        big.len() > small_sectors as usize * 4096,
        "test payload must actually exceed the old sector run to exercise reallocation"
    );
    region::update_chunk_in_place(&region_path, &pos, &big).unwrap();

    let (big_offset, big_sectors) = {
        let mut file = File::open(&region_path).unwrap();
        read_header_entry(&mut file, 0, 0)
    };

    assert!(big_sectors as usize > small_sectors as usize);
    assert!(
        big_offset >= small_offset,
        "growth must append at the end, not overlap the freed slot in place"
    );

    // And the region must still read back correctly after reallocation.
    let read_back = region::read_chunk_nbt(&region_path, &pos).unwrap().unwrap();
    let expected = nbt::build_chunk_root(&big_chunk, &opts()).unwrap();
    assert_eq!(read_back, expected);
}

#[test]
fn shrinking_a_chunk_reuses_its_old_sector_run() {
    let dir = scratch_dir("sector-reuse");
    let region_path = dir.join("r.0.0.mca");
    let pos = ChunkPos::new(0, 0);

    let big_chunk = maximal_chunk(pos, 16);
    let big = nbt::serialize_chunk(&big_chunk, &opts()).unwrap();
    region::update_chunk_in_place(&region_path, &pos, &big).unwrap();
    let (big_offset, big_sectors) = {
        let mut file = File::open(&region_path).unwrap();
        read_header_entry(&mut file, 0, 0)
    };

    let small = nbt::serialize_chunk(&synthetic_chunk(pos, 1), &opts()).unwrap();
    region::update_chunk_in_place(&region_path, &pos, &small).unwrap();
    let (small_offset, small_sectors) = {
        let mut file = File::open(&region_path).unwrap();
        read_header_entry(&mut file, 0, 0)
    };

    assert!(small_sectors <= big_sectors);
    // Reuse means the offset should not have moved: it still fits in the same (larger) run.
    assert_eq!(small_offset, big_offset);
}
