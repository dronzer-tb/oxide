//! Block/chunk/section coordinate types.

/// Chunk column coordinates (in chunks, not blocks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// Minimum block X coordinate covered by this chunk column.
    pub const fn min_block_x(self) -> i32 {
        self.x << 4
    }

    /// Minimum block Z coordinate covered by this chunk column.
    pub const fn min_block_z(self) -> i32 {
        self.z << 4
    }
}

/// Absolute block coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub const fn chunk_pos(self) -> ChunkPos {
        ChunkPos::new(self.x >> 4, self.z >> 4)
    }
}

/// 16x16x16 section coordinates (chunk section grid, `y` included since sections stack
/// vertically and the world can start below y=0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SectionPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl SectionPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub const fn min_block_x(self) -> i32 {
        self.x << 4
    }

    pub const fn min_block_y(self) -> i32 {
        self.y << 4
    }

    pub const fn min_block_z(self) -> i32 {
        self.z << 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_pos_to_block() {
        let c = ChunkPos::new(3, -2);
        assert_eq!(c.min_block_x(), 48);
        assert_eq!(c.min_block_z(), -32);
    }

    #[test]
    fn block_pos_to_chunk_pos() {
        let b = BlockPos::new(-17, 64, 31);
        assert_eq!(b.chunk_pos(), ChunkPos::new(-2, 1));
    }
}
