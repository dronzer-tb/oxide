//! `Heightmap`: 256 (16x16) surface-height values, packed the same way as a `PalettedContainer`.

use super::pack_bits;

/// The six vanilla heightmap types written per chunk (see `docs/ARCHITECTURE.md` § "Chunk
/// output contract").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HeightmapType {
    WorldSurface,
    WorldSurfaceWg,
    OceanFloor,
    OceanFloorWg,
    MotionBlocking,
    MotionBlockingNoLeaves,
}

/// A 16x16 grid of surface heights, stored relative to the chunk's `min_y` (so `0` means "at
/// or below the bottom of the world"). Packs to vanilla's `SimpleBitStorage` long-array
/// format at `ceil(log2(world_height + 1))` bits per entry — `9` bits for the standard
/// 384-tall world (`ceil(log2(385)) = 9`).
#[derive(Debug, Clone)]
pub struct Heightmap {
    /// y relative to `min_y`, one per (x, z) in row-major (z * 16 + x) order.
    values: [i32; 256],
    /// World height in blocks (`max_y - min_y`), used to size `bits_per_entry`.
    world_height: i32,
}

impl Heightmap {
    pub fn new(world_height: i32) -> Self {
        Self {
            values: [0; 256],
            world_height,
        }
    }

    pub fn get(&self, x: usize, z: usize) -> i32 {
        self.values[z * 16 + x]
    }

    pub fn set(&mut self, x: usize, z: usize, y_relative_to_min: i32) {
        self.values[z * 16 + x] = y_relative_to_min;
    }

    /// `ceil(log2(world_height + 1))`: enough bits to represent every relative height from
    /// `0` to `world_height` inclusive.
    pub fn bits_per_entry(&self) -> u8 {
        let n = (self.world_height as u32 + 1) as u64;
        (u64::BITS - (n.saturating_sub(1)).leading_zeros()) as u8
    }

    /// Packed `i64` export, same non-spanning layout as `PalettedContainer::to_packed_longs`.
    ///
    /// // PARITY-CHECK: vanilla heightmaps use `SimpleBitStorage`, the same non-spanning
    /// // packer used post-1.16 for block/biome palettes, so this should already match — but
    /// // it hasn't been checked against a real 26.2 NBT dump.
    pub fn to_packed_longs(&self) -> Vec<i64> {
        pack_bits(self.values.iter().map(|&v| v as u64), self.bits_per_entry())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_per_entry_384_height_is_9() {
        let h = Heightmap::new(384);
        assert_eq!(h.bits_per_entry(), 9);
    }

    #[test]
    fn get_set_round_trip() {
        let mut h = Heightmap::new(384);
        h.set(5, 10, 123);
        assert_eq!(h.get(5, 10), 123);
        assert_eq!(h.get(0, 0), 0);
    }

    #[test]
    fn packed_round_trip() {
        let mut h = Heightmap::new(384);
        for x in 0..16 {
            for z in 0..16 {
                h.set(x, z, ((x * 16 + z) % 385) as i32);
            }
        }
        let longs = h.to_packed_longs();
        let bits = h.bits_per_entry() as usize;
        let entries_per_long = 64 / bits;
        assert_eq!(longs.len(), 256usize.div_ceil(entries_per_long));

        for x in 0..16 {
            for z in 0..16 {
                let i = z * 16 + x;
                let word = longs[i / entries_per_long] as u64;
                let offset = (i % entries_per_long) * bits;
                let mask = (1u64 << bits) - 1;
                let got = (word >> offset) & mask;
                assert_eq!(got as i32, h.get(x, z));
            }
        }
    }
}
