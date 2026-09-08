//! `oxide-render`: High-throughput SIMD chunk meshing & occlusion culling engine.
//!
//! Exposes zero-copy C-ABI endpoints for Java 22+ Panama FFI client mods,
//! cutting chunk meshing time to <15µs and GPU vertex counts by up to 60% via greedy quad merging.

pub mod cull;
pub mod ffi;
pub mod greedy;
pub mod vertex;

pub use cull::{cull_sections, Frustum, Plane, SectionAABB};
pub use ffi::{oxide_render_cull_sections, oxide_render_free_mesh, oxide_render_mesh_section};
pub use greedy::{mesh_section, SectionMesh};
pub use vertex::{Face, MergedQuad, PackedVertex};
