//! oxide-ffi — C ABI surface for the Folia plugin, called over Java's Panama FFI
//! (`java.lang.foreign`). See `docs/ARCHITECTURE.md`. Wave 5 per `docs/ROADMAP.md`.
//!
//! Every exported function is wrapped in [`std::panic::catch_unwind`] — a Rust panic
//! propagating across an `extern "C"` boundary is undefined behavior, so a panic here becomes
//! an error return instead of a crash. Every pointer argument is null-checked before use.
//!
//! Scope of what this generates: exactly what `oxide-chunkgen::fill_chunk` produces today —
//! noise-shaped solid/fluid/air, no surface rules, no biome transmission yet (see
//! `oxide_generate_chunk`'s doc), no aquifers/ore veins/carvers/structures. See
//! `oxide_chunkgen::fill`'s module doc for the full scope-cut list this inherits.

mod handle;

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use handle::OxideGenerator;

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn set_last_error(msg: impl std::fmt::Display) {
    let c = CString::new(msg.to_string()).unwrap_or_else(|_| {
        CString::new("error message contained a NUL byte").expect("literal has no NUL")
    });
    LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(c));
}

/// Returns the last error message set on this thread by a failed call, or null if none. The
/// returned pointer is valid until the next failed call on this thread; the caller must not
/// free it.
#[no_mangle]
pub extern "C" fn oxide_last_error() -> *const c_char {
    LAST_ERROR.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|c| c.as_ptr())
            .unwrap_or(std::ptr::null())
    })
}

/// # Safety
/// `datapack_path` and `dimension_id` must be valid null-terminated UTF-8 C strings, live for
/// the duration of this call. Returns null on any failure (check `oxide_last_error`); the
/// caller owns the returned pointer and must pass it to `oxide_close` exactly once.
#[no_mangle]
pub unsafe extern "C" fn oxide_open(
    datapack_path: *const c_char,
    dimension_id: *const c_char,
    seed: i64,
) -> *mut OxideGenerator {
    if datapack_path.is_null() || dimension_id.is_null() {
        set_last_error("oxide_open: datapack_path/dimension_id must not be null");
        return std::ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let path_str = CStr::from_ptr(datapack_path)
            .to_str()
            .map_err(|e| format!("datapack_path is not valid UTF-8: {e}"))?;
        let dim_str = CStr::from_ptr(dimension_id)
            .to_str()
            .map_err(|e| format!("dimension_id is not valid UTF-8: {e}"))?;
        // `{:#}` and not `to_string()`: anyhow's Display prints only the outermost
        // context, so "loading datapack at <path>" reached callers with the actual
        // cause -- missing version.json, a bad JSON file, an unknown density
        // function -- silently dropped. The alternate form prints the whole chain.
        OxideGenerator::open(Path::new(path_str), dim_str, seed).map_err(|e| format!("{e:#}"))
    }));
    match result {
        Ok(Ok(generator)) => Box::into_raw(Box::new(generator)),
        Ok(Err(msg)) => {
            set_last_error(msg);
            std::ptr::null_mut()
        }
        Err(_) => {
            set_last_error("oxide_open panicked");
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `handle` must be a pointer previously returned by `oxide_open` and not yet closed. After
/// this call `handle` is invalid and must not be used again.
#[no_mangle]
pub unsafe extern "C" fn oxide_close(handle: *mut OxideGenerator) {
    if handle.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        drop(Box::from_raw(handle));
    }));
}

/// # Safety
/// `handle` must be a live pointer from `oxide_open`.
#[no_mangle]
pub unsafe extern "C" fn oxide_min_y(handle: *const OxideGenerator) -> i32 {
    with_handle(handle, i32::MIN, |g| g.min_y())
}

/// # Safety
/// `handle` must be a live pointer from `oxide_open`.
#[no_mangle]
pub unsafe extern "C" fn oxide_height(handle: *const OxideGenerator) -> i32 {
    with_handle(handle, -1, |g| g.height())
}

/// # Safety
/// `handle` must be a live pointer from `oxide_open`.
#[no_mangle]
pub unsafe extern "C" fn oxide_sea_level(handle: *const OxideGenerator) -> i32 {
    with_handle(handle, i32::MIN, |g| g.sea_level())
}

/// Null-terminated namespaced block id (e.g. `"minecraft:stone"`) for value `1` in
/// `oxide_generate_chunk`'s output buffer. Valid as long as `handle` is open; the caller must
/// not free it.
///
/// # Safety
/// `handle` must be a live pointer from `oxide_open`.
#[no_mangle]
pub unsafe extern "C" fn oxide_default_block_name(handle: *const OxideGenerator) -> *const c_char {
    with_handle(handle, std::ptr::null(), |g| g.default_block_name_ptr())
}

/// Null-terminated namespaced block id for value `2` in `oxide_generate_chunk`'s output buffer.
///
/// # Safety
/// `handle` must be a live pointer from `oxide_open`.
#[no_mangle]
pub unsafe extern "C" fn oxide_default_fluid_name(handle: *const OxideGenerator) -> *const c_char {
    with_handle(handle, std::ptr::null(), |g| g.default_fluid_name_ptr())
}

/// Fills `out_buf` with one byte per block: `0` = air, `1` = the block named by
/// `oxide_default_block_name`, `2` = the block named by `oxide_default_fluid_name`. Index order
/// is `(y * 16 + z) * 16 + x` within each 16-block-tall horizontal slice, slices stacked
/// bottom-to-top from `oxide_min_y`; required buffer length is `256 * oxide_height(handle)`.
///
/// No surface-rule block variety (grass/dirt/sand), no biome, no structures, no features — see
/// this crate's module doc. Returns the number of bytes written, or a negative value on error
/// (check `oxide_last_error`): `-1` null/misused handle, `-2` buffer too small, `-3` panic.
///
/// # Safety
/// `handle` must be a live pointer from `oxide_open`. `out_buf` must be valid for
/// `out_buf_len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn oxide_generate_chunk(
    handle: *mut OxideGenerator,
    chunk_x: i32,
    chunk_z: i32,
    out_buf: *mut u8,
    out_buf_len: usize,
) -> i64 {
    if handle.is_null() || out_buf.is_null() {
        set_last_error("oxide_generate_chunk: handle/out_buf must not be null");
        return -1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let generator = &*handle;
        let required = generator.buffer_len();
        if out_buf_len < required {
            return Err(format!(
                "out_buf too small: need {required} bytes, got {out_buf_len}"
            ));
        }
        let buf = std::slice::from_raw_parts_mut(out_buf, required);
        generator.generate_chunk(chunk_x, chunk_z, buf);
        Ok(required)
    }));
    match result {
        Ok(Ok(written)) => written as i64,
        Ok(Err(msg)) => {
            set_last_error(msg);
            -2
        }
        Err(_) => {
            set_last_error("oxide_generate_chunk panicked");
            -3
        }
    }
}

unsafe fn with_handle<T>(
    handle: *const OxideGenerator,
    on_null: T,
    f: impl FnOnce(&OxideGenerator) -> T,
) -> T {
    if handle.is_null() {
        set_last_error("null handle");
        return on_null;
    }
    match catch_unwind(AssertUnwindSafe(|| f(&*handle))) {
        Ok(v) => v,
        Err(_) => {
            set_last_error("panicked");
            on_null
        }
    }
}
