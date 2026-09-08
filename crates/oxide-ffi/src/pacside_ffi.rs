// Pacside native packet cache FFI exports

/// Initializes global Pacside off-heap chunk packet cache.
#[no_mangle]
pub unsafe extern "C" fn pacside_ffi_init(capacity_chunks: i64) {
    let cap = if capacity_chunks <= 0 { 32768 } else { capacity_chunks as usize };
    pacside::pacside_init(cap);
}

/// Puts a pre-compressed chunk packet payload into native RAM.
#[no_mangle]
pub unsafe extern "C" fn pacside_ffi_put(
    world_id: i64,
    chunk_x: i32,
    chunk_z: i32,
    data_ptr: *const u8,
    data_len: i64,
) -> i32 {
    if data_ptr.is_null() || data_len <= 0 {
        return -1;
    }
    pacside::pacside_put(world_id, chunk_x, chunk_z, data_ptr, data_len as usize)
}

/// Retrieves a cached chunk packet payload into `out_ptr`.
/// Returns bytes written (>0) on hit, 0 on miss, or negative if buffer is too small.
#[no_mangle]
pub unsafe extern "C" fn pacside_ffi_get(
    world_id: i64,
    chunk_x: i32,
    chunk_z: i32,
    out_ptr: *mut u8,
    max_len: i64,
) -> i32 {
    if out_ptr.is_null() || max_len <= 0 {
        return -1;
    }
    pacside::pacside_get(world_id, chunk_x, chunk_z, out_ptr, max_len as usize)
}

/// Invalidates a chunk from the native packet cache.
#[no_mangle]
pub unsafe extern "C" fn pacside_ffi_invalidate(world_id: i64, chunk_x: i32, chunk_z: i32) {
    pacside::pacside_invalidate(world_id, chunk_x, chunk_z);
}

/// Clears all entries from the native packet cache.
#[no_mangle]
pub unsafe extern "C" fn pacside_ffi_clear() {
    pacside::pacside_clear();
}
