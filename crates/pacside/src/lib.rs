//! Pacside — High-Performance Native Chunk Packet Caching & Streaming Engine.
//!
//! Pre-encodes and caches network chunk packets directly in native off-heap memory,
//! bypassing Java object inflation and repeated Netty Zlib compression passes.

pub mod cache;
pub mod compress;

use std::sync::OnceLock;
use cache::{ChunkKey, PacsideCache};

static GLOBAL_CACHE: OnceLock<PacsideCache> = OnceLock::new();

fn get_cache() -> &'static PacsideCache {
    GLOBAL_CACHE.get_or_init(|| PacsideCache::new(32768)) // Default 32k chunks (~1 GB)
}

/// Initializes or resizes the global Pacside packet cache with `capacity_chunks`.
#[no_mangle]
pub extern "C" fn pacside_init(capacity_chunks: usize) {
    let _ = GLOBAL_CACHE.set(PacsideCache::new(capacity_chunks));
}

/// Stores a pre-compressed chunk packet payload in the native off-heap cache.
#[no_mangle]
pub extern "C" fn pacside_put(
    world_id: i64,
    chunk_x: i32,
    chunk_z: i32,
    data_ptr: *const u8,
    data_len: usize,
) -> i32 {
    if data_ptr.is_null() || data_len == 0 {
        return -1;
    }

    let slice = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
    let key = ChunkKey::new(world_id, chunk_x, chunk_z);
    get_cache().put(key, slice.to_vec());
    0
}

/// Retrieves a cached chunk packet payload into `out_ptr`.
/// Returns the length written on success, 0 on cache miss, or negative on buffer overflow.
#[no_mangle]
pub extern "C" fn pacside_get(
    world_id: i64,
    chunk_x: i32,
    chunk_z: i32,
    out_ptr: *mut u8,
    max_len: usize,
) -> i32 {
    if out_ptr.is_null() {
        return -1;
    }

    let key = ChunkKey::new(world_id, chunk_x, chunk_z);
    match get_cache().get(&key) {
        Some(data) => {
            if data.len() > max_len {
                return -(data.len() as i32); // Buffer too small, returns needed size
            }
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), out_ptr, data.len());
            }
            data.len() as i32
        }
        None => 0, // Cache miss
    }
}

/// Invalidates a single chunk from the cache.
#[no_mangle]
pub extern "C" fn pacside_invalidate(world_id: i64, chunk_x: i32, chunk_z: i32) {
    let key = ChunkKey::new(world_id, chunk_x, chunk_z);
    get_cache().invalidate(&key);
}

/// Clears all entries from the cache.
#[no_mangle]
pub extern "C" fn pacside_clear() {
    get_cache().clear();
}

/// Writes out cache statistics into the provided pointers.
#[no_mangle]
pub extern "C" fn pacside_stats(
    out_cached_chunks: *mut u64,
    out_bytes: *mut u64,
    out_hits: *mut u64,
    out_misses: *mut u64,
    out_evictions: *mut u64,
) {
    let stats = get_cache().stats();
    unsafe {
        if !out_cached_chunks.is_null() {
            *out_cached_chunks = stats.cached_chunks as u64;
        }
        if !out_bytes.is_null() {
            *out_bytes = stats.bytes_cached;
        }
        if !out_hits.is_null() {
            *out_hits = stats.hits;
        }
        if !out_misses.is_null() {
            *out_misses = stats.misses;
        }
        if !out_evictions.is_null() {
            *out_evictions = stats.evictions;
        }
    }
}
