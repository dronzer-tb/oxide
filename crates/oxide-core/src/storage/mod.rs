//! Chunk storage: paletted containers, heightmaps, and the chunk data model.

mod chunk;
mod heightmap;
mod paletted;

pub use chunk::{ChunkData, ChunkSection, ChunkStatus};
pub use heightmap::{Heightmap, HeightmapType};
pub use paletted::PalettedContainer;

/// Shared bit-packing routine for vanilla's `SimpleBitStorage` layout: entries are packed
/// into `i64` words at `bits_per_entry` bits each, entries never span a word boundary (the
/// remaining high bits of each word beyond a whole number of entries are left zero). Used by
/// both `PalettedContainer::to_packed_longs` and `Heightmap::to_packed_longs`.
pub(crate) fn pack_bits(
    values: impl ExactSizeIterator<Item = u64>,
    bits_per_entry: u8,
) -> Vec<i64> {
    if bits_per_entry == 0 {
        return Vec::new();
    }
    let bits = bits_per_entry as usize;
    let entries_per_long = 64 / bits;
    let len = values.len();
    let num_longs = len.div_ceil(entries_per_long);
    let mut out = vec![0u64; num_longs];
    let mask: u64 = (1u64 << bits) - 1;
    for (i, v) in values.enumerate() {
        let word = i / entries_per_long;
        let offset = (i % entries_per_long) * bits;
        out[word] |= (v & mask) << offset;
    }
    out.into_iter().map(|w| w as i64).collect()
}

/// `ceil(log2(n))` for `n >= 1`, returning `0` for `n <= 1` (a single-value palette needs no
/// bits to select its one entry).
pub(crate) fn ceil_log2(n: usize) -> u8 {
    if n <= 1 {
        0
    } else {
        (usize::BITS - (n - 1).leading_zeros()) as u8
    }
}
