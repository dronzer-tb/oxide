//! `.mca` region file reading and writing.
//!
//! The 32x32-chunk header/sector layout, zlib chunk compression, and in-place sector
//! reuse/allocation are all handled by [`fastanvil::Region`] — per `docs/ARCHITECTURE.md`
//! ("prefers established crates for the NBT/Anvil layer"), that logic is not hand-rolled here.
//! `fastanvil::Region::write_compressed_chunk` already implements exactly the allocation strategy
//! the task calls for: reuse the chunk's existing sector run when the new payload still fits,
//! otherwise append at the end of the file and drop the old sectors from its free list. See
//! `fastanvil-0.31.0/src/region.rs` for that logic.
//!
//! What `fastanvil::Region` does *not* do, and what this module adds:
//! - The **timestamp table** (the second 4096-byte half of the 8KiB header): `Region::new` zeroes
//!   it and nothing in `fastanvil` ever writes to it afterwards. [`touch_timestamp`] patches the
//!   correct 4-byte big-endian slot directly so the header is genuinely complete, not just
//!   structurally present.
//! - Advisory locking ([`crate::lock::RegionGuard`]) and crash-safe whole-file replacement
//!   (temp-file-then-atomic-rename) — a live Folia server may hold the same file open (see
//!   `crate::lock` for exactly what that guard does and does not protect against).

use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::AnvilError;
use crate::lock::RegionGuard;
use crate::provenance;
use oxide_core::ChunkPos;

/// Region-file coordinates a chunk belongs to (`chunk_coord.div_euclid(32)`).
pub fn region_coords_for(pos: &ChunkPos) -> (i32, i32) {
    (pos.x.div_euclid(32), pos.z.div_euclid(32))
}

/// The chunk's 0..32 local coordinates within its region file.
pub fn local_coords(pos: &ChunkPos) -> (usize, usize) {
    (pos.x.rem_euclid(32) as usize, pos.z.rem_euclid(32) as usize)
}

/// Standard `r.<x>.<z>.mca` region file name for the region containing `pos`.
pub fn region_file_name(pos: &ChunkPos) -> String {
    let (rx, rz) = region_coords_for(pos);
    format!("r.{rx}.{rz}.mca")
}

/// Same 3-byte-offset/1-byte-sector-count header slot formula `fastanvil` uses internally
/// (`crate::region::header_pos` in fastanvil, not exposed publicly), duplicated here only to
/// address the timestamp table, which lives at `+4096` from the equivalent offset-table slot.
fn timestamp_header_pos(x: usize, z: usize) -> u64 {
    4096 + 4 * ((x % 32) + (z % 32) * 32) as u64
}

/// Write the current unix time into the timestamp-table slot for chunk `(x, z)` in an
/// already-written region file. Best-effort completeness on top of `fastanvil::Region`, which
/// never touches this table itself.
fn touch_timestamp(file: &mut File, x: usize, z: usize) -> std::io::Result<()> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as u32;
    file.seek(SeekFrom::Start(timestamp_header_pos(x, z)))?;
    file.write_all(&secs.to_be_bytes())?;
    Ok(())
}

/// Crash-safe whole-file replacement (temp-file-then-atomic-rename). Shared with
/// [`crate::provenance`], which writes its sidecar the same way.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), AnvilError> {
    let dir: PathBuf = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "region".to_string());
    let tmp_path = dir.join(format!(".{file_name}.oxide-tmp-{}", std::process::id()));

    {
        let mut f = File::create(&tmp_path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    // Rename is atomic as long as source and destination are on the same filesystem, which is
    // guaranteed here since tmp_path is a sibling of path.
    fs::rename(&tmp_path, path).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        AnvilError::Io(e)
    })
}

/// Writes a region file from scratch, containing exactly the given chunks (each already
/// serialized to *uncompressed* NBT bytes, e.g. via `nbt::serialize_chunk`).
///
/// This is the documented "naive" fallback: it does not preserve any chunk not present in
/// `chunks`, so it must not be used to touch one chunk of a region that already has others — use
/// [`update_chunk_in_place`] for that. It exists for building a brand-new region (or fully
/// regenerating one Oxide fully owns) and is always crash-safe: the file is built in memory, then
/// persisted via write-temp-then-atomic-rename, so a crash mid-write never leaves a truncated
/// region on disk.
///
/// Also marks every written chunk Oxide-generated in the region's provenance sidecar (see
/// `crate::provenance`), under the same [`RegionGuard`] as the region write itself, so the two
/// files cannot be observed to disagree because of a race with another Oxide writer.
pub fn write_region_file(path: &Path, chunks: &[(ChunkPos, Vec<u8>)]) -> Result<(), AnvilError> {
    let _guard = RegionGuard::acquire(path)?;

    let mut region = fastanvil::Region::new(Cursor::new(Vec::new()))?;
    for (pos, nbt_bytes) in chunks {
        let (x, z) = local_coords(pos);
        region.write_chunk(x, z, nbt_bytes)?;
    }
    let bytes = region.into_inner()?.into_inner();
    atomic_write(path, &bytes)?;

    // Best-effort timestamp completeness: reopen and patch the timestamp table now that the
    // file exists on disk under its final name.
    if let Ok(mut file) = OpenOptions::new().write(true).open(path) {
        for (pos, _) in chunks {
            let (x, z) = local_coords(pos);
            let _ = touch_timestamp(&mut file, x, z);
        }
    }

    for (pos, _) in chunks {
        provenance::mark_chunk_locked(path, *pos)?;
    }

    Ok(())
}

/// Update (or insert) a single chunk inside an existing region file in place. Falls back to
/// [`write_region_file`] when `path` does not exist yet.
///
/// `nbt_bytes` must be uncompressed chunk NBT (e.g. from `nbt::serialize_chunk`); this compresses
/// it with zlib (scheme 2) via `fastanvil::Region::write_chunk`, which reuses the chunk's current
/// sector run when the new payload still fits and otherwise appends at the end of the file and
/// frees the old sectors — see the module docs above.
///
/// Also marks the chunk Oxide-generated in the region's provenance sidecar, under the same
/// [`RegionGuard`] as the region write — see [`write_region_file`].
pub fn update_chunk_in_place(
    path: &Path,
    pos: &ChunkPos,
    nbt_bytes: &[u8],
) -> Result<(), AnvilError> {
    let (x, z) = local_coords(pos);

    let _guard = RegionGuard::acquire(path)?;

    if !path.exists() {
        drop(_guard); // write_region_file acquires its own guard.
        return write_region_file(path, &[(*pos, nbt_bytes.to_vec())]);
    }

    let file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut region = fastanvil::Region::from_stream(file)?;
    region.write_chunk(x, z, nbt_bytes)?;
    drop(region); // release the File before reopening it below.

    if let Ok(mut file) = OpenOptions::new().write(true).open(path) {
        let _ = touch_timestamp(&mut file, x, z);
    }

    provenance::mark_chunk_locked(path, *pos)?;

    Ok(())
}

/// Read one chunk's raw *uncompressed* NBT bytes back out of a region file. `Ok(None)` means the
/// chunk has not been generated (no error). Decompression (zlib/gzip/uncompressed, per whatever
/// scheme the chunk was actually stored with) is handled by `fastanvil::Region::read_chunk`.
pub fn read_chunk(path: &Path, pos: &ChunkPos) -> Result<Option<Vec<u8>>, AnvilError> {
    let (x, z) = local_coords(pos);
    let mut file = File::open(path)?;
    if let Some(bytes) = read_lz4_chunk(&mut file, x, z)? {
        return Ok(Some(bytes));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut region = fastanvil::Region::from_stream(file)?;
    Ok(region.read_chunk(x, z)?)
}

/// The chunk's NBT if it is stored with scheme 4 (LZ4), which `fastanvil` cannot read;
/// `None` for an absent chunk or any other scheme, left to `fastanvil`.
fn read_lz4_chunk(file: &mut File, x: usize, z: usize) -> Result<Option<Vec<u8>>, AnvilError> {
    use std::io::Read;
    let mut location = [0u8; 4];
    file.seek(SeekFrom::Start(4 * (x + z * 32) as u64))?;
    file.read_exact(&mut location)?;
    let sector = u32::from_be_bytes([0, location[0], location[1], location[2]]) as u64;
    if sector == 0 {
        return Ok(None);
    }
    let mut header = [0u8; 5];
    file.seek(SeekFrom::Start(sector * 4096))?;
    file.read_exact(&mut header)?;
    if header[4] != 4 {
        return Ok(None);
    }
    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    let mut body = vec![0u8; length.saturating_sub(1)];
    file.read_exact(&mut body)?;
    Ok(Some(crate::lz4::decode_block_stream(&body)?))
}

/// Convenience: read a chunk and parse it straight into [`crate::nbt::ChunkNbtRoot`] — what the
/// validation harness wants for comparing Oxide output against vanilla reference regions.
pub fn read_chunk_nbt(
    path: &Path,
    pos: &ChunkPos,
) -> Result<Option<crate::nbt::ChunkNbtRoot>, AnvilError> {
    match read_chunk(path, pos)? {
        Some(bytes) => Ok(Some(crate::nbt::parse_chunk(&bytes)?)),
        None => Ok(None),
    }
}

/// Zlib-compresses uncompressed chunk NBT at the fast level, as a region stores it (scheme 2).
/// Separate from [`write_compressed_region`] so callers can compress chunks in parallel.
pub fn compress_chunk(nbt_bytes: &[u8]) -> Result<Vec<u8>, AnvilError> {
    use flate2::{write::ZlibEncoder, Compression};
    let mut encoder =
        ZlibEncoder::new(Vec::with_capacity(nbt_bytes.len() / 4), Compression::fast());
    encoder.write_all(nbt_bytes)?;
    Ok(encoder.finish()?)
}

/// Writes a brand-new region from already-compressed chunks (see [`compress_chunk`]) in one
/// pass: header, timestamps and sectors built in memory, one atomic write, one provenance store.
///
/// For a region nothing else owns. Like [`write_region_file`] it replaces whatever is at `path`.
/// The difference is cost: that one recompresses serially and rewrites the sidecar once per
/// chunk, which at pregen rates is most of the time spent.
pub fn write_compressed_region(
    path: &Path,
    chunks: &[(ChunkPos, Vec<u8>)],
) -> Result<(), AnvilError> {
    const SECTOR: usize = 4096;
    let _guard = RegionGuard::acquire(path)?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as u32;
    let body: usize = chunks
        .iter()
        .map(|(_, data)| (data.len() + 5).div_ceil(SECTOR) * SECTOR)
        .sum();
    let mut bytes = vec![0u8; 2 * SECTOR];
    bytes.reserve(body);

    for (pos, data) in chunks {
        let (x, z) = local_coords(pos);
        let slot = 4 * (x + z * 32);
        let sector = bytes.len() / SECTOR;
        let count = (data.len() + 5).div_ceil(SECTOR);
        if count > 255 {
            // Vanilla moves chunks this large to a .mcc file; a generated chunk never gets near.
            return Err(AnvilError::Io(std::io::Error::other(format!(
                "chunk {pos:?} compresses to {} bytes, over the 1 MiB a region slot holds",
                data.len()
            ))));
        }
        bytes[slot..slot + 4]
            .copy_from_slice(&(((sector as u32) << 8) | count as u32).to_be_bytes());
        bytes[SECTOR + slot..SECTOR + slot + 4].copy_from_slice(&now.to_be_bytes());
        bytes.extend_from_slice(&((data.len() + 1) as u32).to_be_bytes());
        bytes.push(2);
        bytes.extend_from_slice(data);
        bytes.resize(bytes.len().div_ceil(SECTOR) * SECTOR, 0);
    }
    atomic_write(path, &bytes)?;

    let mut map = provenance::ProvenanceMap::new();
    for (pos, _) in chunks {
        map.set(*pos, true);
    }
    provenance::store(&provenance::sidecar_path_for(path), &map)
}

#[cfg(test)]
mod compressed_region_tests {
    use super::*;
    use crate::test_support::scratch_dir;

    #[test]
    fn write_compressed_region_reads_back() {
        let dir = scratch_dir("compressed-region");
        let path = dir.join("r.-1.0.mca");
        // Sizes on both sides of a sector boundary, and one spanning several sectors.
        let chunks: Vec<(ChunkPos, Vec<u8>)> = [(-32, 0, 10), (-1, 31, 4091), (-17, 5, 9000)]
            .iter()
            .map(|&(x, z, n)| {
                let nbt: Vec<u8> = (0..n).map(|i| (i * 7 % 251) as u8).collect();
                (ChunkPos::new(x, z), nbt)
            })
            .collect();
        let compressed: Vec<(ChunkPos, Vec<u8>)> = chunks
            .iter()
            .map(|(p, nbt)| (*p, compress_chunk(nbt).unwrap()))
            .collect();
        write_compressed_region(&path, &compressed).unwrap();
        for (pos, nbt) in &chunks {
            assert_eq!(read_chunk(&path, pos).unwrap().as_ref(), Some(nbt));
        }
        assert_eq!(read_chunk(&path, &ChunkPos::new(-2, 2)).unwrap(), None);
        assert_eq!(
            provenance::load(&provenance::sidecar_path_for(&path))
                .unwrap()
                .count_set(),
            3
        );
    }
}
