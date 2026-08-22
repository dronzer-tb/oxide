//! `PalettedContainer<T>`: vanilla's palette + packed-index storage for block states / biomes.

use super::{ceil_log2, pack_bits};
use ahash::AHashMap;
use std::hash::Hash;

/// A fixed-size grid of `T` values stored as indices into a deduplicated palette, matching
/// vanilla's `PalettedContainer`. `min_bits` is the floor applied to `bits_per_entry` once the
/// palette holds more than one value (4 for block states, 1 for biomes); a single-value
/// palette always packs at 0 bits regardless of `min_bits`.
#[derive(Debug, Clone)]
pub struct PalettedContainer<T> {
    size: usize,
    min_bits: u8,
    palette: Vec<T>,
    /// Palette index per slot, `size` entries long.
    data: Vec<u32>,
    /// Reverse lookup `value -> palette index`, kept in sync with `palette`.
    index_of: AHashMap<T, u32>,
}

impl<T: Clone + Eq + Hash> PalettedContainer<T> {
    /// Creates a container with `size` entries, all initially `default`, and a floor of
    /// `min_bits` on the packed bit width once more than one distinct value is present.
    pub fn new(size: usize, min_bits: u8, default: T) -> Self {
        let mut index_of = AHashMap::default();
        index_of.insert(default.clone(), 0);
        Self {
            size,
            min_bits,
            palette: vec![default],
            data: vec![0; size],
            index_of,
        }
    }

    pub fn len(&self) -> usize {
        self.size
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    pub fn get(&self, index: usize) -> &T {
        &self.palette[self.data[index] as usize]
    }

    pub fn set(&mut self, index: usize, value: T) {
        let palette_index = match self.index_of.get(&value) {
            Some(&i) => i,
            None => {
                let i = self.palette.len() as u32;
                self.palette.push(value.clone());
                self.index_of.insert(value, i);
                i
            }
        };
        self.data[index] = palette_index;
    }

    /// The current palette, in insertion order (index 0 is the initial default value).
    pub fn palette(&self) -> &[T] {
        &self.palette
    }

    /// Packed bit width per entry: `0` for a single-value palette, otherwise
    /// `max(min_bits, ceil(log2(palette.len())))`.
    pub fn bits_per_entry(&self) -> u8 {
        if self.palette.len() <= 1 {
            0
        } else {
            ceil_log2(self.palette.len()).max(self.min_bits)
        }
    }

    /// Exports vanilla's packed-`i64` representation: entries never span a long boundary, and
    /// a single-value palette produces zero data longs.
    pub fn to_packed_longs(&self) -> Vec<i64> {
        pack_bits(self.data.iter().map(|&v| v as u64), self.bits_per_entry())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_value_palette_packs_to_zero_longs() {
        let c: PalettedContainer<u8> = PalettedContainer::new(4096, 4, 0);
        assert_eq!(c.bits_per_entry(), 0);
        assert!(c.to_packed_longs().is_empty());
    }

    #[test]
    fn min_bits_floor_applies_once_palette_grows() {
        let mut c: PalettedContainer<u8> = PalettedContainer::new(4096, 4, 0);
        c.set(0, 1); // 2 distinct values -> ceil_log2(2) = 1, floored to min_bits=4
        assert_eq!(c.bits_per_entry(), 4);
    }

    #[test]
    fn biome_min_bits_is_one() {
        let mut c: PalettedContainer<u16> = PalettedContainer::new(64, 1, 0);
        c.set(0, 1);
        assert_eq!(c.bits_per_entry(), 1);
    }

    #[test]
    fn bits_grow_past_floor_with_more_distinct_values() {
        let mut c: PalettedContainer<u16> = PalettedContainer::new(64, 1, 0);
        for i in 1..20u16 {
            c.set(i as usize, i);
        }
        // 20 distinct values -> ceil(log2(20)) = 5
        assert_eq!(c.bits_per_entry(), 5);
    }

    #[test]
    fn packed_round_trip_bits_4() {
        let mut c: PalettedContainer<u8> = PalettedContainer::new(8, 4, 0);
        let values = [0u8, 3, 7, 15, 1, 0, 3, 15];
        for (i, &v) in values.iter().enumerate() {
            c.set(i, v);
        }
        assert_eq!(c.bits_per_entry(), 4);
        let longs = c.to_packed_longs();
        // 8 entries * 4 bits = 32 bits, fits in a single i64 word.
        assert_eq!(longs.len(), 1);

        // Unpack manually and compare against the palette-mapped original values.
        let word = longs[0] as u64;
        for (i, &expected) in values.iter().enumerate() {
            let idx = (word >> (i * 4)) & 0xF;
            assert_eq!(*c.get(i), expected);
            assert_eq!(c.palette()[idx as usize], expected);
        }
    }

    #[test]
    fn packed_entries_never_span_a_long_boundary() {
        // 20 distinct values -> bits_per_entry = ceil(log2(20)) = 5, entries_per_long =
        // 64/5 = 12 (60 bits used, 4 wasted per word). With 20 entries this spans two words,
        // and entry #12 must start a fresh word rather than straddling the boundary.
        let mut c: PalettedContainer<u8> = PalettedContainer::new(20, 1, 0);
        for i in 0..20u8 {
            c.set(i as usize, i);
        }
        assert_eq!(c.bits_per_entry(), 5);
        let longs = c.to_packed_longs();
        let entries_per_long = 64 / 5;
        assert_eq!(entries_per_long, 12);
        assert_eq!(longs.len(), 20usize.div_ceil(entries_per_long));

        for i in 0..20 {
            let word = longs[i / entries_per_long] as u64;
            let offset = (i % entries_per_long) * 5;
            let idx = (word >> offset) & 0x1F;
            assert_eq!(c.palette()[idx as usize], c.data[i] as u8);
        }
    }

    #[test]
    fn get_reflects_last_set() {
        let mut c: PalettedContainer<u8> = PalettedContainer::new(4, 4, 9);
        assert_eq!(*c.get(0), 9);
        c.set(0, 5);
        assert_eq!(*c.get(0), 5);
        c.set(0, 9);
        assert_eq!(*c.get(0), 9);
    }
}
