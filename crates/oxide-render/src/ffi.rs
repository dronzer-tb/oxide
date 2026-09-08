//! C-ABI Foreign Function Interface (FFI) bindings for Java 22+ Panama FFI.

use std::ptr;
use crate::cull::{cull_sections, Frustum, SectionAABB};
use crate::greedy::{mesh_section, SectionMesh};
use crate::vertex::PackedVertex;

/// Heap-allocated mesh container pinned for Panama FFI access.
pub struct RawMeshHandle {
    pub mesh: SectionMesh,
}

/// Meshes a 16x16x16 chunk section (4096 u16 block states) and returns pointers
/// to the contiguous packed vertex array and index array in native RAM.
///
/// # Safety
/// `blocks_ptr` must point to at least 4096 contiguous `u16` values.
/// All `out_*` pointers must be valid and writable.
#[no_mangle]
pub unsafe extern "C" fn oxide_render_mesh_section(
    blocks_ptr: *const u16,
    out_vertices_ptr: *mut *const PackedVertex,
    out_vertex_count: *mut usize,
    out_indices_ptr: *mut *const u32,
    out_index_count: *mut usize,
) -> *mut RawMeshHandle {
    if blocks_ptr.is_null()
        || out_vertices_ptr.is_null()
        || out_vertex_count.is_null()
        || out_indices_ptr.is_null()
        || out_index_count.is_null()
    {
        return ptr::null_mut();
    }

    let blocks_slice = std::slice::from_raw_parts(blocks_ptr, 4096);
    let mut blocks_arr = [0u16; 4096];
    blocks_arr.copy_from_slice(blocks_slice);

    let mesh = mesh_section(&blocks_arr);

    let handle = Box::new(RawMeshHandle { mesh });

    *out_vertices_ptr = handle.mesh.vertices.as_ptr();
    *out_vertex_count = handle.mesh.vertices.len();
    *out_indices_ptr = handle.mesh.indices.as_ptr();
    *out_index_count = handle.mesh.indices.len();

    Box::into_raw(handle)
}

/// Frees a native mesh handle allocated by `oxide_render_mesh_section`.
///
/// # Safety
/// `handle` must be a valid pointer returned by `oxide_render_mesh_section` and not yet freed.
#[no_mangle]
pub unsafe extern "C" fn oxide_render_free_mesh(handle: *mut RawMeshHandle) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// Batch culls an array of section AABBs against the current camera view frustum.
///
/// # Safety
/// `frustum_ptr` must point to a valid `Frustum` struct.
/// `aabbs_ptr` must point to `count` contiguous `SectionAABB` structs.
/// `visible_out` must be a writable buffer of at least `count` bytes.
#[no_mangle]
pub unsafe extern "C" fn oxide_render_cull_sections(
    frustum_ptr: *const Frustum,
    aabbs_ptr: *const SectionAABB,
    count: usize,
    visible_out: *mut u8,
) -> usize {
    if frustum_ptr.is_null() || aabbs_ptr.is_null() || visible_out.is_null() || count == 0 {
        return 0;
    }

    let frustum = &*frustum_ptr;
    let aabbs = std::slice::from_raw_parts(aabbs_ptr, count);
    let out_slice = std::slice::from_raw_parts_mut(visible_out, count);

    cull_sections(frustum, aabbs, out_slice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ffi_mesh_and_free_roundtrip() {
        let blocks = [1u16; 4096]; // Solid stone chunk
        let mut vertices_ptr: *const PackedVertex = ptr::null();
        let mut vertex_count: usize = 0;
        let mut indices_ptr: *const u32 = ptr::null();
        let mut index_count: usize = 0;

        unsafe {
            let handle = oxide_render_mesh_section(
                blocks.as_ptr(),
                &mut vertices_ptr,
                &mut vertex_count,
                &mut indices_ptr,
                &mut index_count,
            );
            assert!(!handle.is_null());
            assert!(vertex_count > 0);
            assert!(index_count > 0);
            assert!(!vertices_ptr.is_null());
            assert!(!indices_ptr.is_null());

            oxide_render_free_mesh(handle);
        }
    }
}
