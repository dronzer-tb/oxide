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
    write_then_rename(path, bytes, true)
}

/// [`atomic_write`] with the flush to disk optional: the rename is atomic either way, so a
/// reader never sees a half-written file; only durability across a power cut is given up.
pub(crate) fn write_then_rename(path: &Path, bytes: &[u8], sync: bool) -> Result<(), AnvilError> {
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
        if sync {
            f.sync_all()?;
        }
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

/// One chunk's payload, already compressed with `scheme` (see [`crate::compress`]).
#[derive(Debug, Clone)]
pub struct CompressedChunk {
    pub pos: ChunkPos,
    pub scheme: u8,
    pub payload: Vec<u8>,
}

const SECTOR: usize = 4096;

/// Lays out a whole region file in memory from compressed chunks: the 8 KiB header, then each
/// chunk in the order given, each padded to a 4 KiB sector. `timestamp` goes in every written
/// chunk's slot of the timestamp table.
///
/// `region` is the region's own coordinates; a chunk from any other region is an error rather
/// than a silently misplaced chunk.
pub fn build_region_bytes(
    region: (i32, i32),
    chunks: &[CompressedChunk],
    timestamp: u32,
) -> Result<Vec<u8>, AnvilError> {
    let sectors_of = |chunk: &CompressedChunk| (chunk.payload.len() + 5).div_ceil(SECTOR);
    let total: usize = 2 + chunks.iter().map(sectors_of).sum::<usize>();
    let mut bytes = vec![0u8; total * SECTOR];
    let mut next = 2usize;
    for chunk in chunks {
        let wrong_region = || AnvilError::WrongRegion {
            x: chunk.pos.x,
            z: chunk.pos.z,
            path: PathBuf::from(format!("r.{}.{}.mca", region.0, region.1)),
        };
        if region_coords_for(&chunk.pos) != region {
            return Err(wrong_region());
        }
        let count = sectors_of(chunk);
        // A count over 255 does not fit the header; vanilla moves such a chunk to its own
        // `.mcc` file, which nothing here writes.
        if count > 255 {
            return Err(AnvilError::ChunkTooLarge {
                x: chunk.pos.x,
                z: chunk.pos.z,
                bytes: chunk.payload.len(),
            });
        }
        let (x, z) = local_coords(&chunk.pos);
        let slot = 4 * (x + z * 32);
        if bytes[slot..slot + 4] != [0; 4] {
            return Err(wrong_region()); // The same chunk twice.
        }
        bytes[slot..slot + 3].copy_from_slice(&(next as u32).to_be_bytes()[1..]);
        bytes[slot + 3] = count as u8;
        bytes[SECTOR + slot..SECTOR + slot + 4].copy_from_slice(&timestamp.to_be_bytes());
        let at = next * SECTOR;
        bytes[at..at + 4].copy_from_slice(&(chunk.payload.len() as u32 + 1).to_be_bytes());
        bytes[at + 4] = chunk.scheme;
        bytes[at + 5..at + 5 + chunk.payload.len()].copy_from_slice(&chunk.payload);
        next += count;
    }
    Ok(bytes)
}

/// Writes a brand-new region file at `path` holding exactly `chunks`, replacing any file
/// already there, and marks every one Oxide-generated in the provenance sidecar.
///
/// This is the bulk path for offline generation: one allocation, one write and one rename per
/// region, and one sidecar write however many chunks there are. `sync` asks for the file to
/// reach the disk before the rename; without it a power cut can lose the region, never
/// corrupt one that was there before.
pub fn write_region_compressed(
    path: &Path,
    region: (i32, i32),
    chunks: &[CompressedChunk],
    sync: bool,
) -> Result<u64, AnvilError> {
    let _guard = RegionGuard::acquire(path)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as u32;
    let bytes = build_region_bytes(region, chunks, timestamp)?;
    write_then_rename(path, &bytes, sync)?;

    let mut map = provenance::ProvenanceMap::new();
    for chunk in chunks {
        map.set(chunk.pos, true);
    }
    provenance::store_with(&provenance::sidecar_path_for(path), &map, sync)?;
    Ok(bytes.len() as u64)
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
    let file = File::open(path)?;
    let mut region = fastanvil::Region::from_stream(file)?;
    Ok(region.read_chunk(x, z)?)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compress::Compression;
    use crate::test_support::scratch_dir;

    fn compressed(pos: ChunkPos, nbt: &[u8], how: Compression) -> CompressedChunk {
        let mut payload = Vec::new();
        how.compress_into(nbt, &mut payload).unwrap();
        CompressedChunk {
            pos,
            scheme: how.scheme(),
            payload,
        }
    }

    #[test]
    fn bulk_region_reads_back_through_fastanvil() {
        let dir = scratch_dir("bulk-region");
        let path = dir.join("r.-1.0.mca");
        // One small chunk, one spanning several sectors, in a negative-x region.
        let small: Vec<u8> = b"small chunk".to_vec();
        let large: Vec<u8> = (0..30_000u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
            .collect();
        let chunks = [
            compressed(ChunkPos::new(-1, 0), &small, Compression::VANILLA),
            compressed(ChunkPos::new(-32, 31), &large, Compression::VANILLA),
            compressed(ChunkPos::new(-7, 3), &large, Compression::None),
        ];
        let written = write_region_compressed(&path, (-1, 0), &chunks, false).unwrap();
        assert_eq!(written, fs::metadata(&path).unwrap().len());
        assert_eq!(written % 4096, 0);

        assert_eq!(
            read_chunk(&path, &ChunkPos::new(-1, 0)).unwrap().unwrap(),
            small
        );
        assert_eq!(
            read_chunk(&path, &ChunkPos::new(-32, 31)).unwrap().unwrap(),
            large
        );
        assert_eq!(
            read_chunk(&path, &ChunkPos::new(-7, 3)).unwrap().unwrap(),
            large
        );
        assert!(read_chunk(&path, &ChunkPos::new(-2, 0)).unwrap().is_none());

        let map = provenance::load(&provenance::sidecar_path_for(&path)).unwrap();
        assert_eq!(map.count_set(), 3);
        assert!(map.get(ChunkPos::new(-7, 3)));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bulk_region_refuses_a_chunk_from_another_region() {
        let chunk = compressed(ChunkPos::new(40, 0), b"x", Compression::None);
        assert!(matches!(
            build_region_bytes((0, 0), &[chunk], 0),
            Err(AnvilError::WrongRegion { .. })
        ));
    }
}
