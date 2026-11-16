//! oxide-ffi — C ABI surface for the Folia plugin, called over Java's Panama FFI
//! (`java.lang.foreign`). See `docs/ARCHITECTURE.md`. Wave 5 per `docs/ROADMAP.md`.
//!
//! Every exported function is wrapped in [`std::panic::catch_unwind`] — a Rust panic
//! propagating across an `extern "C"` boundary is undefined behavior, so a panic here becomes
//! an error return instead of a crash. Every pointer argument is null-checked before use.
//!
//! Scope of what this generates: exactly what `oxide-chunkgen::generate_chunk` produces today
//! — noise-shaped terrain with aquifers and ore veins, then surface rules (so grass/dirt/sand/
//! bedrock, not bare stone), then carvers, in vanilla's order — plus the biome grid, all
//! transmitted as palette indices. Still absent: structures and features. See
//! `oxide_chunkgen::fill` and `oxide_chunkgen::surface` module docs for the scope-cut list this
//! inherits.

mod handle;
pub mod pack;
mod pacside_ffi;

/// Exposed for the `bench_native` example: lets a pure-Rust harness measure generator
/// throughput with no JVM, Bukkit or Folia scheduler in the way.
pub use handle::OxideGenerator as OxideGeneratorHandle;

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use handle::OxideGenerator;
use oxide_core::HeightmapType;

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

/// Turns native decoration (features: ores, plants, trees as they are ported) on or off.
/// Returns `1` when decoration is now on, `0` when off, `-1` on error (check
/// `oxide_last_error`) -- notably when asked to turn it on for a datapack that lacks the
/// block facts or tags decoration needs. Off is the default, and with it off a caller must
/// leave the server's own decoration pass on; with it on, off, or both apply.
///
/// # Safety
/// `handle` must be a live pointer from `oxide_open`. Call before generating chunks: chunks
/// generated on either side of a switch do not match each other.
#[no_mangle]
pub unsafe extern "C" fn oxide_set_decoration(handle: *const OxideGenerator, enabled: i32) -> i32 {
    if handle.is_null() {
        set_last_error("oxide_set_decoration: handle must not be null");
        return -1;
    }
    match catch_unwind(AssertUnwindSafe(|| (*handle).set_decoration(enabled != 0))) {
        Ok(Ok(on)) => i32::from(on),
        Ok(Err(e)) => {
            set_last_error(format!("{e:#}"));
            -1
        }
        Err(_) => {
            set_last_error("oxide_set_decoration panicked");
            -1
        }
    }
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

/// Surface height at `(x, z)` for one heightmap type, as a Bukkit
/// `ChunkGenerator.getBaseHeight` override answers it. `heightmap` selects the type by the
/// ordinal of Bukkit's `HeightMap` enum, which this maps explicitly rather than by cast:
/// `0` MOTION_BLOCKING, `1` MOTION_BLOCKING_NO_LEAVES, `2` OCEAN_FLOOR, `3` OCEAN_FLOOR_WG,
/// `4` WORLD_SURFACE, `5` WORLD_SURFACE_WG. An unknown value is treated as WORLD_SURFACE.
///
/// Returns `i32::MIN` on a null handle.
///
/// # Safety
/// `handle` must be a live pointer from `oxide_open`.
#[no_mangle]
pub unsafe extern "C" fn oxide_base_height(
    handle: *const OxideGenerator,
    x: i32,
    z: i32,
    heightmap: i32,
) -> i32 {
    let ty = match heightmap {
        0 => HeightmapType::MotionBlocking,
        1 => HeightmapType::MotionBlockingNoLeaves,
        2 => HeightmapType::OceanFloor,
        3 => HeightmapType::OceanFloorWg,
        5 => HeightmapType::WorldSurfaceWg,
        _ => HeightmapType::WorldSurface,
    };
    with_handle(handle, i32::MIN, |g| g.base_height(x, z, ty))
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

/// Generates one chunk -- noise fill plus surface rules -- into two palette-index buffers.
///
/// `out_blocks` takes one `u16` per block, index `(y * 16 + z) * 16 + x` within the whole
/// column (`y` relative to `oxide_min_y`, not restarted per section); its required length in
/// `u16`s is `256 * oxide_height(handle)`. `out_biomes` takes one `u16` per 4x4x4 biome quart,
/// index `section_index * 64 + (qy * 4 + qz) * 4 + qx`; its required length is
/// `64 * (oxide_height(handle) / 16)`.
///
/// Both hold indices into this handle's palettes, read back with `oxide_block_palette_len` /
/// `oxide_block_palette_name` and the biome pair. Indices never change meaning for a handle's
/// lifetime, so a caller resolves each one once and caches it.
///
/// Produces terrain that is already surfaced and carved: the noise fill runs with aquifers and
/// ore veins, then the surface rules, then the carvers -- vanilla's own order. A caller must
/// therefore suppress the server's surface and carver passes, or both apply twice.
///
/// With `oxide_set_decoration` on, features run too. Still absent: structures.
///
/// Returns the number of blocks written, or a negative value on error (check
/// `oxide_last_error`): `-1` null/misused handle, `-2` buffer too small or generation failed,
/// `-3` panic.
///
/// # Safety
/// `handle` must be a live pointer from `oxide_open`. `out_blocks` must be valid for
/// `out_blocks_len` writable `u16`s, `out_biomes` for `out_biomes_len`.
#[no_mangle]
pub unsafe extern "C" fn oxide_generate_chunk(
    handle: *mut OxideGenerator,
    chunk_x: i32,
    chunk_z: i32,
    out_blocks: *mut u16,
    out_blocks_len: usize,
    out_biomes: *mut u16,
    out_biomes_len: usize,
) -> i64 {
    if handle.is_null() || out_blocks.is_null() || out_biomes.is_null() {
        set_last_error("oxide_generate_chunk: handle/out_blocks/out_biomes must not be null");
        return -1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let generator = &*handle;
        let blocks_needed = generator.block_buffer_len();
        let biomes_needed = generator.biome_buffer_len();
        if out_blocks_len < blocks_needed {
            return Err(format!(
                "out_blocks too small: need {blocks_needed} u16s, got {out_blocks_len}"
            ));
        }
        if out_biomes_len < biomes_needed {
            return Err(format!(
                "out_biomes too small: need {biomes_needed} u16s, got {out_biomes_len}"
            ));
        }
        let blocks = std::slice::from_raw_parts_mut(out_blocks, blocks_needed);
        let biomes = std::slice::from_raw_parts_mut(out_biomes, biomes_needed);
        generator
            .generate_chunk(chunk_x, chunk_z, blocks, biomes)
            .map_err(|e| format!("{e:#}"))?;
        Ok(blocks_needed)
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

/// Copies out what native decoration left in chunk `(chunk_x, chunk_z)` besides block states,
/// for the last `oxide_generate_chunk` of that chunk on the calling thread (the extras are kept
/// per thread, one chunk at a time; call this right after generating, on the same thread).
///
/// The text is UTF-8, one item per line (`\n`-terminated), absolute block coordinates:
///
/// - `be <x> <y> <z> <snbt>`: a block entity as vanilla saves it (`id`, `x`, `y`, `z`, data).
///   A block-entity block with no data from its feature is `{id:"DUMMY",x:..,y:..,z:..}`, the
///   placeholder `WorldGenRegion.setBlock` stores in a proto chunk.
/// - `ft <x> <y> <z> <fluid_id>`: a scheduled fluid tick (`minecraft:water`, `minecraft:lava`,
///   `minecraft:flowing_water`, ..).
/// - `bt <x> <y> <z> <block_id>`: a scheduled block tick.
/// - `pp <x> <y> <z>`: a position marked for post-processing.
///
/// Returns the number of bytes the text needs. When that is `<= cap` the text has been written
/// to `out` (not NUL-terminated); when it is `> cap` nothing is written and the caller retries
/// with a buffer of at least that size. `0` (empty) when decoration is off. Negative on error
/// (check `oxide_last_error`): `-1` null handle (or null `out` with `cap > 0`), `-2` this
/// thread's last generated chunk is not that chunk of this handle, `-3` panic.
///
/// # Safety
/// `handle` must be a live pointer from `oxide_open`. `out` must be valid for `cap` writable
/// bytes (it may be null when `cap` is 0).
#[no_mangle]
pub unsafe extern "C" fn oxide_take_chunk_extras(
    handle: *const OxideGenerator,
    chunk_x: i32,
    chunk_z: i32,
    out: *mut u8,
    cap: usize,
) -> i64 {
    if handle.is_null() || (out.is_null() && cap > 0) {
        set_last_error("oxide_take_chunk_extras: handle (and out when cap > 0) must not be null");
        return -1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let text = (*handle)
            .chunk_extras(chunk_x, chunk_z)
            .map_err(|e| format!("{e:#}"))?;
        let bytes = text.as_bytes();
        if bytes.len() <= cap && !bytes.is_empty() {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
        }
        Ok::<usize, String>(bytes.len())
    }));
    match result {
        Ok(Ok(needed)) => needed as i64,
        Ok(Err(msg)) => {
            set_last_error(msg);
            -2
        }
        Err(_) => {
            set_last_error("oxide_take_chunk_extras panicked");
            -3
        }
    }
}

/// Packs a block buffer filled by `oxide_generate_chunk` into per-section palettes and
/// vanilla `SimpleBitStorage` longs (see [`pack`]), so the server installs whole sections.
///
/// For section `s` (`blocks_len / 4096` of them): `out_counts[s]` is its palette size, `0` when
/// it has more than 256 distinct states (write that section from `blocks` instead);
/// `out_palettes[s * 256..]` holds that many block-palette indices; `out_packed[s * 512..]`
/// holds the packed entries, as many longs as the palette size implies (none for one entry).
///
/// Returns the number of sections, or `-1` on a null pointer / bad length, `-3` on panic.
///
/// # Safety
/// `blocks` must be valid for `blocks_len` `u16`s; with `n = blocks_len / 4096`, `out_counts`
/// for `n` writable `u16`s, `out_palettes` for `n * 256`, `out_packed` for `n * 512` `u64`s.
#[no_mangle]
pub unsafe extern "C" fn oxide_pack_sections(
    blocks: *const u16,
    blocks_len: usize,
    out_counts: *mut u16,
    out_palettes: *mut u16,
    out_packed: *mut u64,
) -> i64 {
    use pack::{MAX_LOCAL_PALETTE, MAX_PACKED_LONGS, SECTION_BLOCKS};
    if blocks.is_null()
        || out_counts.is_null()
        || out_palettes.is_null()
        || out_packed.is_null()
        || blocks_len % SECTION_BLOCKS != 0
    {
        set_last_error("oxide_pack_sections: null pointer or blocks_len not a multiple of 4096");
        return -1;
    }
    let sections = blocks_len / SECTION_BLOCKS;
    let result = catch_unwind(AssertUnwindSafe(|| {
        let blocks = std::slice::from_raw_parts(blocks, blocks_len);
        let counts = std::slice::from_raw_parts_mut(out_counts, sections);
        let palettes = std::slice::from_raw_parts_mut(out_palettes, sections * MAX_LOCAL_PALETTE);
        let packed = std::slice::from_raw_parts_mut(out_packed, sections * MAX_PACKED_LONGS);
        for s in 0..sections {
            counts[s] = pack::pack_section(
                &blocks[s * SECTION_BLOCKS..(s + 1) * SECTION_BLOCKS],
                &mut palettes[s * MAX_LOCAL_PALETTE..(s + 1) * MAX_LOCAL_PALETTE],
                &mut packed[s * MAX_PACKED_LONGS..(s + 1) * MAX_PACKED_LONGS],
            ) as u16;
        }
    }));
    match result {
        Ok(()) => sections as i64,
        Err(_) => {
            set_last_error("oxide_pack_sections panicked");
            -3
        }
    }
}

/// Number of entries currently in this handle's block-state palette. Grows as generation meets
/// new states, so a caller re-reads it after each `oxide_generate_chunk` call.
///
/// # Safety
/// `handle` must be a live pointer from `oxide_open`.
#[no_mangle]
pub unsafe extern "C" fn oxide_block_palette_len(handle: *const OxideGenerator) -> i32 {
    with_handle(handle, -1, |g| match g.block_palette().read() {
        Ok(palette) => palette.len() as i32,
        Err(_) => -1,
    })
}

/// The block-state string at `index`, e.g. `minecraft:grass_block[snowy=false]` -- the exact
/// form Bukkit's `createBlockData` parses. Null if `index` is out of range. Valid for the
/// handle's lifetime; the caller must not free it.
///
/// # Safety
/// `handle` must be a live pointer from `oxide_open`.
#[no_mangle]
pub unsafe extern "C" fn oxide_block_palette_name(
    handle: *const OxideGenerator,
    index: i32,
) -> *const c_char {
    with_handle(handle, std::ptr::null(), |g| {
        if index < 0 {
            return std::ptr::null();
        }
        match g.block_palette().read() {
            Ok(palette) => palette.name_ptr(index as usize),
            Err(_) => std::ptr::null(),
        }
    })
}

/// Biome palette index for one block position, without generating a chunk -- what a caller
/// implementing a per-position biome lookup calls. Returns a negative value on error.
///
/// # Safety
/// `handle` must be a live pointer from `oxide_open`.
#[no_mangle]
pub unsafe extern "C" fn oxide_biome_at(
    handle: *const OxideGenerator,
    x: i32,
    y: i32,
    z: i32,
) -> i32 {
    with_handle(handle, -1, |g| match g.biome_at(x, y, z) {
        Ok(index) => index as i32,
        Err(e) => {
            set_last_error(format!("{e:#}"));
            -2
        }
    })
}

/// Number of entries currently in this handle's biome palette. See `oxide_block_palette_len`.
///
/// # Safety
/// `handle` must be a live pointer from `oxide_open`.
#[no_mangle]
pub unsafe extern "C" fn oxide_biome_palette_len(handle: *const OxideGenerator) -> i32 {
    with_handle(handle, -1, |g| match g.biome_palette().read() {
        Ok(palette) => palette.len() as i32,
        Err(_) => -1,
    })
}

/// The biome id at `index`, e.g. `minecraft:plains`. See `oxide_block_palette_name`.
///
/// # Safety
/// `handle` must be a live pointer from `oxide_open`.
#[no_mangle]
pub unsafe extern "C" fn oxide_biome_palette_name(
    handle: *const OxideGenerator,
    index: i32,
) -> *const c_char {
    with_handle(handle, std::ptr::null(), |g| {
        if index < 0 {
            return std::ptr::null();
        }
        match g.biome_palette().read() {
            Ok(palette) => palette.name_ptr(index as usize),
            Err(_) => std::ptr::null(),
        }
    })
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
