//! Integration tests for the `oxide-pregen` CLI: region files land where expected, chunks read
//! back byte-identically through `oxide-anvil`'s own read path, heightmaps agree with the blocks
//! actually placed, and `--dry-run` touches nothing.

use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use oxide_anvil::nbt::{self, ChunkNbtWriteOptions};
use oxide_anvil::{provenance, region};
use oxide_core::{BlockState, ChunkData, ChunkPos, HeightmapType};

use oxide_pregen::stub::{self, Pattern};

/// A fresh scratch directory under `$CARGO_TARGET_DIR` (never `/tmp` directly).
fn scratch_dir(name: &str) -> PathBuf {
    let base = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = base
        .join("oxide-pregen-test-scratch")
        .join(format!("{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test scratch dir");
    dir
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxide-pregen"))
}

// Placeholder for tests only — never used to write a real world. Matches the convention
// `oxide-anvil`'s own tests use.
const TEST_DATA_VERSION: &str = "4189";

fn write_opts() -> ChunkNbtWriteOptions {
    ChunkNbtWriteOptions {
        data_version: TEST_DATA_VERSION.parse().unwrap(),
        last_update: 0,
        inhabited_time: 0,
    }
}

/// The block at absolute `(x, y, z)` (chunk-local x/z) as actually stored in `chunk`.
fn block_at_column(chunk: &ChunkData, x: usize, z: usize, y: i32) -> BlockState {
    let section_y = y.div_euclid(16);
    let local_y = y.rem_euclid(16) as usize;
    let section = chunk
        .sections
        .iter()
        .find(|s| s.y as i32 == section_y)
        .unwrap_or_else(|| panic!("no section for y={y}"));
    let index = (local_y * 16 + z) * 16 + x;
    section.block_states.get(index).clone()
}

#[test]
fn writes_region_files_at_expected_paths() {
    let dir = scratch_dir("region-paths");
    let world = dir.join("world");

    // Center at (5,5): a 3x3 square (radius=1) around it (x,z in 4..=6) stays clear of any
    // region boundary (multiples of 32), so it lands entirely in region (0,0).
    let status = bin()
        .args([
            "--world",
            world.to_str().unwrap(),
            "--center-x",
            "5",
            "--center-z",
            "5",
            "--radius",
            "1",
            "--data-version",
            TEST_DATA_VERSION,
        ])
        .status()
        .expect("run oxide-pregen");
    assert!(status.success());

    let region_path = world.join("region").join("r.0.0.mca");
    assert!(
        region_path.exists(),
        "expected region file at {}",
        region_path.display()
    );

    let sidecar_path = provenance::sidecar_path_for(&region_path);
    assert!(
        sidecar_path.exists(),
        "expected provenance sidecar at {}",
        sidecar_path.display()
    );
    let sidecar = provenance::load(&sidecar_path).unwrap();
    assert_eq!(sidecar.count_set(), 9);
    for x in 4..=6 {
        for z in 4..=6 {
            assert!(sidecar.get(ChunkPos::new(x, z)));
        }
    }
}

#[test]
fn chunks_read_back_identically_and_heightmaps_agree_with_placed_blocks() {
    let dir = scratch_dir("readback");
    let world = dir.join("world");

    let status = bin()
        .args([
            "--world",
            world.to_str().unwrap(),
            "--center-x",
            "5",
            "--center-z",
            "5",
            "--radius",
            "1",
            "--data-version",
            TEST_DATA_VERSION,
            "--pattern",
            "checkerboard",
        ])
        .status()
        .expect("run oxide-pregen");
    assert!(status.success());

    let region_path = world.join("region").join("r.0.0.mca");
    let opts = write_opts();

    for x in 4..=6 {
        for z in 4..=6 {
            let pos = ChunkPos::new(x, z);
            let expected_chunk = stub::build_chunk(pos, Pattern::Checkerboard);
            let expected_root = nbt::build_chunk_root(&expected_chunk, &opts).unwrap();

            let actual_root = region::read_chunk_nbt(&region_path, &pos)
                .unwrap()
                .unwrap_or_else(|| panic!("chunk {pos:?} missing from region"));
            assert_eq!(
                actual_root, expected_root,
                "chunk {pos:?} did not round-trip byte-identically through oxide-anvil"
            );

            // Heightmap correctness against the blocks actually placed: for every column, the
            // block one below the heightmap value must be solid, and the block at the heightmap
            // value itself must be air (vanilla's "one above the highest solid block" contract).
            let hm = expected_chunk
                .heightmaps
                .get(&HeightmapType::WorldSurface)
                .unwrap();
            for lx in 0..16usize {
                for lz in 0..16usize {
                    let surface_y = hm.get(lx, lz) + expected_chunk.min_y - 1;
                    let solid = block_at_column(&expected_chunk, lx, lz, surface_y);
                    assert_ne!(
                        solid.name.path(),
                        "air",
                        "chunk {pos:?} col ({lx},{lz}): heightmap points below a non-solid block"
                    );
                    let above = block_at_column(&expected_chunk, lx, lz, surface_y + 1);
                    assert_eq!(
                        above.name.path(),
                        "air",
                        "chunk {pos:?} col ({lx},{lz}): block above heightmap surface isn't air"
                    );
                }
            }
        }
    }
}

#[test]
fn dry_run_creates_no_files() {
    let dir = scratch_dir("dry-run");
    let world = dir.join("world");

    let status = bin()
        .args([
            "--world",
            world.to_str().unwrap(),
            "--center-x",
            "5",
            "--center-z",
            "-3",
            "--radius",
            "2",
            "--data-version",
            TEST_DATA_VERSION,
            "--dry-run",
        ])
        .status()
        .expect("run oxide-pregen");
    assert!(status.success());

    assert!(
        !world.exists(),
        "--dry-run must not create the world directory or anything in it"
    );
}

#[test]
fn missing_data_version_is_rejected() {
    let dir = scratch_dir("missing-data-version");
    let world = dir.join("world");

    let status = bin()
        .args([
            "--world",
            world.to_str().unwrap(),
            "--center-x",
            "0",
            "--center-z",
            "0",
            "--radius",
            "0",
        ])
        .status()
        .expect("run oxide-pregen");
    assert!(!status.success(), "must fail without --data-version");
    assert!(!world.exists());
}
