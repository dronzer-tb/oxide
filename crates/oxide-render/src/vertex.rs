//! Compact 16-byte packed vertex layout for zero-copy GPU rendering.

use bytemuck::{Pod, Zeroable};

/// 6 primary voxel faces matching Minecraft convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Face {
    Down = 0,  // -Y
    Up = 1,    // +Y
    North = 2, // -Z
    South = 3, // +Z
    West = 4,  // -X
    East = 5,  // +X
}

impl Face {
    pub const ALL: [Face; 6] = [
        Face::Down,
        Face::Up,
        Face::North,
        Face::South,
        Face::West,
        Face::East,
    ];

    #[inline(always)]
    pub fn normal(self) -> [i8; 3] {
        match self {
            Face::Down => [0, -1, 0],
            Face::Up => [0, 1, 0],
            Face::North => [0, 0, -1],
            Face::South => [0, 0, 1],
            Face::West => [-1, 0, 0],
            Face::East => [1, 0, 0],
        }
    }
}

/// Compact 16-byte vertex structure directly consumed by GPU vertex buffers.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct PackedVertex {
    /// Local section block coordinates (0..16).
    pub x: u8,
    pub y: u8,
    pub z: u8,
    /// Voxel face index (0..5).
    pub face: u8,
    /// 16-bit texture UV coordinates.
    pub u: u16,
    pub v: u16,
    /// Palette block state identifier.
    pub block_state: u16,
    /// Packed block light (0..15) and sky light (0..15).
    pub light: u8,
    /// Ambient occlusion level (0..3).
    pub ao: u8,
    pub _pad: u16,
}

impl PackedVertex {
    #[inline(always)]
    pub fn new(
        x: u8,
        y: u8,
        z: u8,
        face: Face,
        u: u16,
        v: u16,
        block_state: u16,
        light: u8,
        ao: u8,
    ) -> Self {
        Self {
            x,
            y,
            z,
            face: face as u8,
            u,
            v,
            block_state,
            light,
            ao,
            _pad: 0,
        }
    }
}

/// A merged rectangular quad produced by the greedy mesher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MergedQuad {
    pub x: u8,
    pub y: u8,
    pub z: u8,
    pub width: u8,
    pub height: u8,
    pub face: Face,
    pub block_state: u16,
    pub light: u8,
    pub ao: u8,
}

impl MergedQuad {
    /// Emits the 4 packed vertices corresponding to this quad.
    #[inline(always)]
    pub fn emit_vertices(&self, out: &mut Vec<PackedVertex>) {
        let (x, y, z) = (self.x, self.y, self.z);
        let (w, h) = (self.width, self.height);
        let bs = self.block_state;
        let lt = self.light;
        let ao = self.ao;
        let f = self.face;

        match f {
            Face::Up => {
                out.push(PackedVertex::new(x, y + 1, z, f, 0, 0, bs, lt, ao));
                out.push(PackedVertex::new(x, y + 1, z + h, f, 0, h as u16 * 16, bs, lt, ao));
                out.push(PackedVertex::new(x + w, y + 1, z + h, f, w as u16 * 16, h as u16 * 16, bs, lt, ao));
                out.push(PackedVertex::new(x + w, y + 1, z, f, w as u16 * 16, 0, bs, lt, ao));
            }
            Face::Down => {
                out.push(PackedVertex::new(x, y, z, f, 0, 0, bs, lt, ao));
                out.push(PackedVertex::new(x + w, y, z, f, w as u16 * 16, 0, bs, lt, ao));
                out.push(PackedVertex::new(x + w, y, z + h, f, w as u16 * 16, h as u16 * 16, bs, lt, ao));
                out.push(PackedVertex::new(x, y, z + h, f, 0, h as u16 * 16, bs, lt, ao));
            }
            Face::North => {
                out.push(PackedVertex::new(x + w, y, z, f, 0, 0, bs, lt, ao));
                out.push(PackedVertex::new(x, y, z, f, w as u16 * 16, 0, bs, lt, ao));
                out.push(PackedVertex::new(x, y + h, z, f, w as u16 * 16, h as u16 * 16, bs, lt, ao));
                out.push(PackedVertex::new(x + w, y + h, z, f, 0, h as u16 * 16, bs, lt, ao));
            }
            Face::South => {
                out.push(PackedVertex::new(x, y, z + 1, f, 0, 0, bs, lt, ao));
                out.push(PackedVertex::new(x + w, y, z + 1, f, w as u16 * 16, 0, bs, lt, ao));
                out.push(PackedVertex::new(x + w, y + h, z + 1, f, w as u16 * 16, h as u16 * 16, bs, lt, ao));
                out.push(PackedVertex::new(x, y + h, z + 1, f, 0, h as u16 * 16, bs, lt, ao));
            }
            Face::West => {
                out.push(PackedVertex::new(x, y, z, f, 0, 0, bs, lt, ao));
                out.push(PackedVertex::new(x, y, z + w, f, w as u16 * 16, 0, bs, lt, ao));
                out.push(PackedVertex::new(x, y + h, z + w, f, w as u16 * 16, h as u16 * 16, bs, lt, ao));
                out.push(PackedVertex::new(x, y + h, z, f, 0, h as u16 * 16, bs, lt, ao));
            }
            Face::East => {
                out.push(PackedVertex::new(x + 1, y, z + w, f, 0, 0, bs, lt, ao));
                out.push(PackedVertex::new(x + 1, y, z, f, w as u16 * 16, 0, bs, lt, ao));
                out.push(PackedVertex::new(x + 1, y + h, z, f, w as u16 * 16, h as u16 * 16, bs, lt, ao));
                out.push(PackedVertex::new(x + 1, y + h, z + w, f, 0, h as u16 * 16, bs, lt, ao));
            }
        }
    }
}
