//! Chunk payload compression for region files.
//!
//! A region file stores each chunk as `length (u32 BE), scheme (u8), payload`. The scheme byte
//! says how the payload is compressed; the game reads every scheme listed here whatever its
//! own `region-file-compression` setting is, so the choice only trades file size for time.

use std::io::Write;

use flate2::write::ZlibEncoder;

use crate::error::AnvilError;

/// How a chunk's NBT is compressed inside a region file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// Scheme 2, what vanilla writes by default. `level` is zlib's 0..=9; vanilla uses 6.
    Zlib { level: u32 },
    /// Scheme 3: the NBT bytes as they are.
    None,
}

impl Compression {
    /// Vanilla's own default: zlib at level 6.
    pub const VANILLA: Self = Self::Zlib { level: 6 };

    /// The scheme byte written in front of the payload.
    pub fn scheme(self) -> u8 {
        match self {
            Self::Zlib { .. } => 2,
            Self::None => 3,
        }
    }

    /// Compresses `nbt` (uncompressed chunk NBT) into `out`, which is cleared first. Taking the
    /// buffer lets a worker reuse one allocation across chunks.
    pub fn compress_into(self, nbt: &[u8], out: &mut Vec<u8>) -> Result<(), AnvilError> {
        out.clear();
        match self {
            Self::Zlib { level } => {
                let mut encoder =
                    ZlibEncoder::new(std::mem::take(out), flate2::Compression::new(level));
                encoder.write_all(nbt)?;
                *out = encoder.finish()?;
            }
            Self::None => out.extend_from_slice(nbt),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn zlib_round_trips() {
        let nbt: Vec<u8> = (0..20_000u32).map(|i| (i % 7) as u8).collect();
        let mut out = vec![1, 2, 3];
        Compression::VANILLA.compress_into(&nbt, &mut out).unwrap();
        assert!(out.len() < nbt.len());
        let mut back = Vec::new();
        flate2::read::ZlibDecoder::new(&out[..])
            .read_to_end(&mut back)
            .unwrap();
        assert_eq!(back, nbt);
    }

    #[test]
    fn none_copies() {
        let mut out = vec![9];
        Compression::None.compress_into(b"abc", &mut out).unwrap();
        assert_eq!(out, b"abc");
        assert_eq!(Compression::None.scheme(), 3);
    }
}
