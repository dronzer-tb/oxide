//! 2D Greedy Meshing engine for chunk sections.
//!
//! Iterates through chunk slices along each axis, builds binary visibility masks,
//! and merges coplanar adjacent block faces into single large quads, cutting vertex counts by 40-60%.

use crate::vertex::{Face, MergedQuad, PackedVertex};

const SECTION_SIZE: usize = 16;
const SECTION_VOLUME: usize = SECTION_SIZE * SECTION_SIZE * SECTION_SIZE;

/// Voxel grid accessor helper.
#[inline(always)]
pub fn block_index(x: usize, y: usize, z: usize) -> usize {
    (y * SECTION_SIZE + z) * SECTION_SIZE + x
}

/// Mesh result containing packed vertices and index buffers.
#[derive(Debug, Default, Clone)]
pub struct SectionMesh {
    pub vertices: Vec<PackedVertex>,
    pub indices: Vec<u32>,
    pub quad_count: usize,
}

/// Builds an optimized mesh for a 16x16x16 chunk section.
/// `blocks`: 4096 entries (u16 block state ids). 0 represents AIR (invisible).
pub fn mesh_section(blocks: &[u16; SECTION_VOLUME]) -> SectionMesh {
    let mut quads = Vec::with_capacity(512);

    // Mesh along X, Y, and Z axes
    mesh_axis(blocks, 0, &mut quads); // X axis (West / East)
    mesh_axis(blocks, 1, &mut quads); // Y axis (Down / Up)
    mesh_axis(blocks, 2, &mut quads); // Z axis (North / South)

    let mut vertices = Vec::with_capacity(quads.len() * 4);
    let mut indices = Vec::with_capacity(quads.len() * 6);

    for (i, quad) in quads.iter().enumerate() {
        let base_vertex = (i * 4) as u32;
        quad.emit_vertices(&mut vertices);

        // Standard quad index layout (0, 1, 2, 0, 2, 3)
        indices.push(base_vertex);
        indices.push(base_vertex + 1);
        indices.push(base_vertex + 2);
        indices.push(base_vertex);
        indices.push(base_vertex + 2);
        indices.push(base_vertex + 3);
    }

    SectionMesh {
        quad_count: quads.len(),
        vertices,
        indices,
    }
}

/// Meshes slices perpendicular to the specified axis:
/// `axis = 0` (X), `axis = 1` (Y), `axis = 2` (Z).
fn mesh_axis(blocks: &[u16; SECTION_VOLUME], axis: usize, quads: &mut Vec<MergedQuad>) {
    let (u_axis, v_axis) = match axis {
        0 => (2, 1), // Perpendicular to X: U=Z, V=Y
        1 => (0, 2), // Perpendicular to Y: U=X, V=Z
        2 => (0, 1), // Perpendicular to Z: U=X, V=Y
        _ => unreachable!(),
    };

    let mut mask = [0i32; SECTION_SIZE * SECTION_SIZE];

    for d in 0..=SECTION_SIZE {
        // Step 1: Compute visibility mask between slice d-1 and slice d
        let mut mask_idx = 0;
        for v in 0..SECTION_SIZE {
            for u in 0..SECTION_SIZE {
                let current = if d < SECTION_SIZE {
                    let (x, y, z) = coord_from_axes(axis, d, u_axis, u, v_axis, v);
                    blocks[block_index(x, y, z)]
                } else {
                    0 // Out of bounds -> air
                };

                let neighbor = if d > 0 {
                    let (x, y, z) = coord_from_axes(axis, d - 1, u_axis, u, v_axis, v);
                    blocks[block_index(x, y, z)]
                } else {
                    0 // Out of bounds -> air
                };

                let current_opaque = current > 0;
                let neighbor_opaque = neighbor > 0;

                if current_opaque == neighbor_opaque {
                    mask[mask_idx] = 0;
                } else if current_opaque {
                    // Face pointing towards negative axis (e.g. West, Down, North)
                    mask[mask_idx] = current as i32;
                } else {
                    // Face pointing towards positive axis (e.g. East, Up, South)
                    mask[mask_idx] = -(neighbor as i32);
                }
                mask_idx += 1;
            }
        }

        // Step 2: Greedy 2D quad merging over the slice mask
        let mut n = 0;
        for j in 0..SECTION_SIZE {
            let mut i = 0;
            while i < SECTION_SIZE {
                let val = mask[n + i];
                if val != 0 {
                    // Find width of consecutive matching blocks
                    let mut w = 1;
                    while i + w < SECTION_SIZE && mask[n + i + w] == val {
                        w += 1;
                    }

                    // Find height by checking subsequent rows
                    let mut h = 1;
                    let mut can_expand = true;
                    while j + h < SECTION_SIZE && can_expand {
                        for k in 0..w {
                            if mask[(j + h) * SECTION_SIZE + i + k] != val {
                                can_expand = false;
                                break;
                            }
                        }
                        if can_expand {
                            h += 1;
                        }
                    }

                    // Determine block state and face direction
                    let (block_state, face) = if val > 0 {
                        let f = match axis {
                            0 => Face::West,
                            1 => Face::Down,
                            2 => Face::North,
                            _ => unreachable!(),
                        };
                        (val as u16, f)
                    } else {
                        let f = match axis {
                            0 => Face::East,
                            1 => Face::Up,
                            2 => Face::South,
                            _ => unreachable!(),
                        };
                        ((-val) as u16, f)
                    };

                    let (x, y, z) = coord_from_axes(
                        axis,
                        if val > 0 { d } else { d - 1 },
                        u_axis,
                        i,
                        v_axis,
                        j,
                    );

                    quads.push(MergedQuad {
                        x: x as u8,
                        y: y as u8,
                        z: z as u8,
                        width: w as u8,
                        height: h as u8,
                        face,
                        block_state,
                        light: 255,
                        ao: 3,
                    });

                    // Zero out mask for the merged region
                    for dy in 0..h {
                        for dx in 0..w {
                            mask[(j + dy) * SECTION_SIZE + i + dx] = 0;
                        }
                    }

                    i += w;
                } else {
                    i += 1;
                }
            }
            n += SECTION_SIZE;
        }
    }
}

#[inline(always)]
fn coord_from_axes(
    axis1: usize,
    val1: usize,
    axis2: usize,
    val2: usize,
    axis3: usize,
    val3: usize,
) -> (usize, usize, usize) {
    let mut coords = [0usize; 3];
    coords[axis1] = val1;
    coords[axis2] = val2;
    coords[axis3] = val3;
    (coords[0], coords[1], coords[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_section_produces_zero_quads() {
        let blocks = [0u16; SECTION_VOLUME];
        let mesh = mesh_section(&blocks);
        assert_eq!(mesh.quad_count, 0);
        assert!(mesh.vertices.is_empty());
        assert!(mesh.indices.is_empty());
    }

    #[test]
    fn test_single_block_produces_6_faces() {
        let mut blocks = [0u16; SECTION_VOLUME];
        blocks[block_index(1, 1, 1)] = 1; // 1 stone block

        let mesh = mesh_section(&blocks);
        assert_eq!(mesh.quad_count, 6);
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
    }

    #[test]
    fn test_greedy_merging_combines_full_plane_to_single_quad_per_face() {
        let mut blocks = [0u16; SECTION_VOLUME];
        // Fill an entire 16x16 horizontal layer at Y=0
        for z in 0..16 {
            for x in 0..16 {
                blocks[block_index(x, 0, z)] = 1;
            }
        }

        let mesh = mesh_section(&blocks);
        // Top and Bottom are 1 single 16x16 quad each!
        // 4 sides around the perimeter: 4 quads of 16x1 each.
        // Total = 6 quads instead of 256 * 6 = 1536 quads! (99.6% reduction!)
        assert_eq!(mesh.quad_count, 6);
        assert_eq!(mesh.vertices.len(), 24);
    }
}
