//! Exercises the exported `extern "C"` functions directly (this crate's `rlib` target makes
//! them callable as normal — if `unsafe` — Rust functions here, no actual FFI hop needed to
//! test the contract). Reuses `oxide-datapack`'s `valid_pack` fixture rather than duplicating
//! one.

use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};

use oxide_ffi::{
    oxide_close, oxide_default_block_name, oxide_default_fluid_name, oxide_generate_chunk,
    oxide_height, oxide_last_error, oxide_min_y, oxide_open, oxide_sea_level,
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

        let len = 256usize * 384;
        let mut buf = vec![0xFFu8; len]; // poison value: every byte must be overwritten to 0/1/2
        let written = oxide_generate_chunk(handle, 0, 0, buf.as_mut_ptr(), buf.len());
        assert_eq!(written, len as i64);
        assert!(
            buf.iter().all(|&b| b <= 2),
            "buffer contains a byte outside {{0,1,2}} — poison value leaked through"
        );

        // fixture's final_density is a flat 0.0 (not > 0), so every block is fluid below sea
        // level and air above it — never the "1" (solid) value. Confirms the buffer reflects
        // real generation, not just zero-fill.
        assert!(buf.contains(&2), "expected at least one fluid block");
        assert!(!buf.contains(&1), "density 0.0 should never produce solid");

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

        let mut tiny_buf = [0u8; 4];
        let result = oxide_generate_chunk(handle, 0, 0, tiny_buf.as_mut_ptr(), tiny_buf.len());
        assert_eq!(result, -2);

        oxide_close(handle);
    }
}

#[test]
fn null_handle_is_reported_not_a_crash() {
    unsafe {
        assert_eq!(oxide_min_y(std::ptr::null()), i32::MIN);
        assert_eq!(oxide_height(std::ptr::null()), -1);
        let mut buf = [0u8; 16];
        assert_eq!(
            oxide_generate_chunk(std::ptr::null_mut(), 0, 0, buf.as_mut_ptr(), buf.len()),
            -1
        );
        oxide_close(std::ptr::null_mut()); // must not crash
    }
}
