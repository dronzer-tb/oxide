//! Reads region chunks stored with compression scheme 4: lz4-java's `LZ4BlockOutputStream`
//! framing around raw LZ4 blocks. Servers set this with `region-file-compression=lz4`;
//! `fastanvil` only knows schemes 1-3, so the harness could not diff against such a world.
//! Read-only: Oxide never writes this scheme.

use std::io::{Error, ErrorKind, Result};

const MAGIC: &[u8; 8] = b"LZ4Block";
const HEADER_LEN: usize = 8 + 1 + 4 + 4 + 4;
const METHOD_RAW: u8 = 0x10;
const METHOD_LZ4: u8 = 0x20;

fn bad(msg: &str) -> Error {
    Error::new(ErrorKind::InvalidData, format!("lz4 chunk: {msg}"))
}

fn le_u32(b: &[u8]) -> usize {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize
}

/// Decodes a whole `LZ4BlockOutputStream` stream. Checksums are not verified: the region is
/// only being read for comparison, and a corrupt block fails to decode anyway.
pub fn decode_block_stream(mut input: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    while !input.is_empty() {
        if input.len() < HEADER_LEN || &input[..8] != MAGIC {
            return Err(bad("missing LZ4Block header"));
        }
        let method = input[8] & 0xF0;
        let compressed = le_u32(&input[9..]);
        let original = le_u32(&input[13..]);
        let body = input
            .get(HEADER_LEN..HEADER_LEN + compressed)
            .ok_or_else(|| bad("block runs past the end"))?;
        if original == 0 {
            break; // end-of-stream marker
        }
        let start = out.len();
        match method {
            METHOD_RAW => out.extend_from_slice(body),
            METHOD_LZ4 => decode_block(body, &mut out)?,
            _ => return Err(bad("unknown block method")),
        }
        if out.len() - start != original {
            return Err(bad("block decoded to the wrong length"));
        }
        input = &input[HEADER_LEN + compressed..];
    }
    Ok(out)
}

fn read_len(input: &[u8], pos: &mut usize, mut len: usize) -> Result<usize> {
    if len == 15 {
        loop {
            let b = *input.get(*pos).ok_or_else(|| bad("truncated length"))?;
            *pos += 1;
            len += b as usize;
            if b != 255 {
                break;
            }
        }
    }
    Ok(len)
}

/// Decodes one raw LZ4 block, appending to `out`.
fn decode_block(input: &[u8], out: &mut Vec<u8>) -> Result<()> {
    let base = out.len();
    let mut pos = 0;
    while pos < input.len() {
        let token = input[pos];
        pos += 1;
        let literals = read_len(input, &mut pos, (token >> 4) as usize)?;
        let lit = input
            .get(pos..pos + literals)
            .ok_or_else(|| bad("truncated literals"))?;
        out.extend_from_slice(lit);
        pos += literals;
        if pos == input.len() {
            break; // the last sequence carries literals only
        }
        let offset = input
            .get(pos..pos + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
            .ok_or_else(|| bad("truncated offset"))?;
        pos += 2;
        let length = read_len(input, &mut pos, (token & 15) as usize)? + 4;
        if offset == 0 || offset > out.len() - base {
            return Err(bad("match offset out of range"));
        }
        let from = out.len() - offset;
        // Byte by byte: a match may overlap the bytes it is producing.
        for i in 0..length {
            let b = out[from + i];
            out.push(b);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(method: u8, body: &[u8], original: usize) -> Vec<u8> {
        let mut v = MAGIC.to_vec();
        v.push(method);
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(&(original as u32).to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn decodes_literals_overlapping_match_and_end_marker() {
        // "abc" then a match of 9 at offset 3 ("abcabcabc"), then literal "Z".
        // token: 3 literals, match length 9-4=5.
        let block = [0x35, b'a', b'b', b'c', 3, 0, 0x10, b'Z'];
        let mut stream = frame(METHOD_LZ4, &block, 13);
        stream.extend(frame(METHOD_RAW, b"xy", 2));
        stream.extend(frame(METHOD_RAW, b"", 0));
        assert_eq!(decode_block_stream(&stream).unwrap(), b"abcabcabcabcZxy");
    }

    #[test]
    fn long_lengths_use_continuation_bytes() {
        // 20 literal 'q's: token 0xF0, continuation 5.
        let mut block = vec![0xF0, 5];
        block.extend(std::iter::repeat_n(b'q', 20));
        let stream = frame(METHOD_LZ4, &block, 20);
        assert_eq!(decode_block_stream(&stream).unwrap(), vec![b'q'; 20]);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode_block_stream(b"notlz4block-header....").is_err());
    }
}
