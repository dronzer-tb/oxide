//! Exercises the exported `extern "C"` functions directly (this crate's `rlib` target makes
//! them callable as normal — if `unsafe` — Rust functions here, no actual FFI hop needed to
//! test the contract). Reuses `oxide-datapack`'s `valid_pack` fixture rather than duplicating
//! one.

use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};

use oxide_ffi::{
    oxide_block_palette_len, oxide_block_palette_name, oxide_close, oxide_default_block_name,
    oxide_default_fluid_name, oxide_generate_chunk, oxide_height, oxide_last_error, oxide_min_y,
    oxide_open, oxide_sea_level,
};

fn fixture_path() -> PathBuf {
    // `oxide-datapack`'s fixtures, not duplicated here — CARGO_MANIFEST_DIR is this crate's
    // own `crates/oxide-ffi`, so walk up to `crates/` then into the sibling crate.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../oxide-datapack/tests/fixtures/valid_pack")
}

#[test]
fn open_generate_close_round_trip() {
    let path = CString::new(fixture_path().to_str().unwrap()).unwrap();
    let dim = CString::new("testmod:overworld").unwrap();

    unsafe {
        let handle = oxide_open(path.as_ptr(), dim.as_ptr(), 42);
        assert!(
            !handle.is_null(),
            "oxide_open failed: {:?}",
            CStr::from_ptr(oxide_last_error())
        );

        assert_eq!(oxide_min_y(handle), -64);
        assert_eq!(oxide_height(handle), 384);
        assert_eq!(oxide_sea_level(handle), 63);

        let block_name = CStr::from_ptr(oxide_default_block_name(handle))
            .to_str()
            .unwrap();
        assert_eq!(block_name, "minecraft:stone");
        let fluid_name = CStr::from_ptr(oxide_default_fluid_name(handle))
            .to_str()
            .unwrap();
        assert_eq!(fluid_name, "minecraft:water");

        let blocks_len = 256usize * 384;
        let biomes_len = 64usize * (384 / 16);
        // Poison values: every entry must be overwritten with a real palette index.
        let mut blocks = vec![0xFFFFu16; blocks_len];
        let mut biomes = vec![0xFFFFu16; biomes_len];
        let written = oxide_generate_chunk(
            handle,
            0,
            0,
            blocks.as_mut_ptr(),
            blocks.len(),
            biomes.as_mut_ptr(),
            biomes.len(),
        );
        assert_eq!(written, blocks_len as i64);

        let palette_len = oxide_block_palette_len(handle);
        assert!(palette_len > 0, "generation must intern at least one state");
        assert!(
            blocks.iter().all(|&i| (i as i32) < palette_len),
            "a block index points outside the palette — poison value leaked through"
        );

        let names: Vec<String> = (0..palette_len)
            .map(|i| {
                CStr::from_ptr(oxide_block_palette_name(handle, i))
                    .to_str()
                    .unwrap()
                    .to_string()
            })
            .collect();

        // The fixture's final_density is a flat 0.0 (not > 0), so every block is fluid below
        // sea level and air above it, and no solid is ever placed. Confirms the buffers
        // reflect real generation, not just zero-fill.
        assert!(
            names.iter().any(|n| n == "minecraft:water"),
            "expected water in the palette, got {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "minecraft:air"),
            "expected air in the palette, got {names:?}"
        );
        // Stone *is* expected here, even at a flat density of 0.0: the fixture leaves
        // aquifers enabled, and where two aquifers of different fluid levels meet, the
        // barrier-pressure term makes the boundary solid. Before aquifers existed this
        // asserted the opposite.
        assert!(
            names.iter().any(|n| n == "minecraft:stone"),
            "aquifer barriers should place some solid, got {names:?}"
        );

        assert_eq!(
            oxide_block_palette_name(handle, palette_len),
            std::ptr::null()
        );

        oxide_close(handle);
    }
}

#[test]
fn open_rejects_unknown_dimension() {
    let path = CString::new(fixture_path().to_str().unwrap()).unwrap();
    let dim = CString::new("testmod:does_not_exist").unwrap();
    unsafe {
        let handle = oxide_open(path.as_ptr(), dim.as_ptr(), 1);
        assert!(handle.is_null());
        let err = CStr::from_ptr(oxide_last_error()).to_str().unwrap();
        assert!(err.contains("does_not_exist"), "unexpected error: {err}");
    }
}

#[test]
fn open_rejects_bad_datapack_path() {
    let path = CString::new("/nonexistent/path/for/sure").unwrap();
    let dim = CString::new("testmod:overworld").unwrap();
    unsafe {
        let handle = oxide_open(path.as_ptr(), dim.as_ptr(), 1);
        assert!(handle.is_null());
    }
}

#[test]
fn generate_chunk_reports_buffer_too_small() {
    let path = CString::new(fixture_path().to_str().unwrap()).unwrap();
    let dim = CString::new("testmod:overworld").unwrap();
    unsafe {
        let handle = oxide_open(path.as_ptr(), dim.as_ptr(), 1);
        assert!(!handle.is_null());

        let mut tiny_blocks = [0u16; 4];
        let mut tiny_biomes = [0u16; 4];
        let result = oxide_generate_chunk(
            handle,
            0,
            0,
            tiny_blocks.as_mut_ptr(),
            tiny_blocks.len(),
            tiny_biomes.as_mut_ptr(),
            tiny_biomes.len(),
        );
        assert_eq!(result, -2);

        oxide_close(handle);
    }
}

#[test]
fn null_handle_is_reported_not_a_crash() {
    unsafe {
        assert_eq!(oxide_min_y(std::ptr::null()), i32::MIN);
        assert_eq!(oxide_height(std::ptr::null()), -1);
        let mut blocks = [0u16; 16];
        let mut biomes = [0u16; 16];
        assert_eq!(
            oxide_generate_chunk(
                std::ptr::null_mut(),
                0,
                0,
                blocks.as_mut_ptr(),
                blocks.len(),
                biomes.as_mut_ptr(),
                biomes.len(),
            ),
            -1
        );
        assert_eq!(oxide_block_palette_len(std::ptr::null()), -1);
        assert_eq!(
            oxide_block_palette_name(std::ptr::null(), 0),
            std::ptr::null()
        );
        oxide_close(std::ptr::null_mut()); // must not crash
    }
}
