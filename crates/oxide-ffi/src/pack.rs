//! Packs a chunk's block-index buffer into vanilla's per-section palette + `SimpleBitStorage`
//! layout, so the server can install each section with `PalettedContainer.unpack` instead of
//! 4096 `setBlockState` calls. The 26.2 block-state strategy decides the layout: one entry is
//! zero bits, 2-16 entries are 4 bits (linear palette), 17-256 are 5-8 bits (hash-map palette).
//! Above 256 vanilla switches to the global palette; such a section is reported with a count
//! of 0 and the caller writes it the old way.

use std::cell::RefCell;

pub const SECTION_BLOCKS: usize = 4096;
pub const MAX_LOCAL_PALETTE: usize = 256;
/// Longs for the widest packed layout this module produces (8 bits, 8 values per long).
pub const MAX_PACKED_LONGS: usize = SECTION_BLOCKS / 8;

/// Bits vanilla stores per entry for a block-state palette of `n` entries (`n` in 1..=256).
pub fn bits_for(n: usize) -> u32 {
    debug_assert!((1..=MAX_LOCAL_PALETTE).contains(&n));
    if n <= 1 {
        0
    } else {
        (usize::BITS - (n - 1).leading_zeros()).max(4)
    }
}

/// Longs `SimpleBitStorage` needs for a section at `bits` per entry (values never span longs).
pub fn longs_for(bits: u32) -> usize {
    if bits == 0 {
        return 0;
    }
    let per_long = (64 / bits) as usize;
    SECTION_BLOCKS.div_ceil(per_long)
}

struct Scratch {
    stamp: Vec<u32>,
    local: Vec<u8>,
    generation: u32,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch {
        stamp: vec![0; 1 << 16],
        local: vec![0; 1 << 16],
        generation: 0,
    });
}

/// Packs one section. Writes the local palette (global indices, first-seen order) into
/// `palette` and the bit-packed entries into `packed`; returns the palette size, or 0 when the
/// section has more than [`MAX_LOCAL_PALETTE`] distinct states.
pub fn pack_section(blocks: &[u16], palette: &mut [u16], packed: &mut [u64]) -> usize {
    debug_assert_eq!(blocks.len(), SECTION_BLOCKS);
    SCRATCH.with(|cell| {
        let s = &mut *cell.borrow_mut();
        s.generation = s.generation.wrapping_add(1);
        if s.generation == 0 {
            s.stamp.fill(0);
            s.generation = 1;
        }
        let generation = s.generation;

        let mut n = 0usize;
        for &b in blocks {
            let g = b as usize;
            if s.stamp[g] != generation {
                if n == MAX_LOCAL_PALETTE {
                    return 0;
                }
                s.stamp[g] = generation;
                s.local[g] = n as u8;
                palette[n] = b;
                n += 1;
            }
        }

        let bits = bits_for(n);
        if bits == 0 {
            return n;
        }
        let per_long = (64 / bits) as usize;
        let longs = longs_for(bits);
        for (li, out) in packed[..longs].iter_mut().enumerate() {
            let start = li * per_long;
            let end = (start + per_long).min(SECTION_BLOCKS);
            let mut word = 0u64;
            for (k, &b) in blocks[start..end].iter().enumerate() {
                word |= (s.local[b as usize] as u64) << (k as u32 * bits);
            }
            *out = word;
        }
        n
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unpack(palette: &[u16], n: usize, packed: &[u64]) -> Vec<u16> {
        let bits = bits_for(n);
        if bits == 0 {
            return vec![palette[0]; SECTION_BLOCKS];
        }
        let per_long = (64 / bits) as usize;
        let mask = (1u64 << bits) - 1;
        (0..SECTION_BLOCKS)
            .map(|i| {
                let word = packed[i / per_long];
                let local = (word >> ((i % per_long) as u32 * bits)) & mask;
                palette[local as usize]
            })
            .collect()
    }

    fn section_with(distinct: usize) -> Vec<u16> {
        // Deterministic spread with every value present; global ids far apart to exercise the
        // full lookup table.
        (0..SECTION_BLOCKS)
            .map(|i| ((i * 7919) % distinct) as u16 * 211 % 60_000)
            .collect()
    }

    #[test]
    fn layout_matches_vanilla_block_strategy() {
        assert_eq!(bits_for(1), 0);
        assert_eq!(bits_for(2), 4);
        assert_eq!(bits_for(16), 4);
        assert_eq!(bits_for(17), 5);
        assert_eq!(bits_for(256), 8);
        assert_eq!(longs_for(4), 256);
        assert_eq!(longs_for(5), 342);
        assert_eq!(longs_for(7), 456);
        assert_eq!(longs_for(8), MAX_PACKED_LONGS);
    }

    #[test]
    fn round_trips() {
        for distinct in [1, 2, 16, 17, 33, 100, 256] {
            let blocks = section_with(distinct);
            let mut palette = [0u16; MAX_LOCAL_PALETTE];
            let mut packed = [0u64; MAX_PACKED_LONGS];
            let n = pack_section(&blocks, &mut palette, &mut packed);
            assert_eq!(n, distinct, "palette size for {distinct}");
            assert_eq!(
                unpack(&palette, n, &packed),
                blocks,
                "round trip for {distinct}"
            );
        }
    }

    #[test]
    fn over_256_falls_back() {
        let blocks = section_with(300);
        let mut palette = [0u16; MAX_LOCAL_PALETTE];
        let mut packed = [0u64; MAX_PACKED_LONGS];
        assert_eq!(pack_section(&blocks, &mut palette, &mut packed), 0);
        // The next section on this thread is unaffected by the aborted one.
        let blocks = section_with(3);
        let n = pack_section(&blocks, &mut palette, &mut packed);
        assert_eq!(unpack(&palette, n, &packed), blocks);
    }
}
