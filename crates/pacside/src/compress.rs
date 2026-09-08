//! High-speed compression utilities for Minecraft chunk packet network payloads.

use std::io::Write;
use flate2::write::ZlibEncoder;
use flate2::Compression;

/// Compresses raw chunk data into Minecraft's Zlib network format.
pub fn compress_zlib(raw: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::with_capacity(raw.len() / 2), Compression::fast());
    encoder.write_all(raw)?;
    encoder.finish()
}

/// Decompresses Zlib payload.
pub fn decompress_zlib(compressed: &[u8]) -> std::io::Result<Vec<u8>> {
    use flate2::read::ZlibDecoder;
    use std::io::Read;
    let mut decoder = ZlibDecoder::new(compressed);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zlib_roundtrip() {
        let original = b"Hello Minecraft Chunk Section Data Worldgen Oxide Pacside!";
        let compressed = compress_zlib(original).expect("compress");
        let decompressed = decompress_zlib(&compressed).expect("decompress");
        assert_eq!(original.as_slice(), decompressed.as_slice());
    }
}
