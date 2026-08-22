//! oxide-anvil — chunk NBT serialization + `.mca` region file writer/reader.
//!
//! See `docs/ARCHITECTURE.md` for the crate graph and the "Chunk output contract" this crate
//! implements (relight marking, heightmap set). Built against the real `oxide-core` types
//! (`oxide_core::{ChunkData, ChunkSection, ChunkStatus, PalettedContainer, Heightmap,
//! HeightmapType, BlockState, BiomeId, ResourceLocation, ChunkPos}`) — no local shim.
//!
//! - [`nbt`] — `ChunkData` -> modern (1.18+) flattened chunk NBT, and back.
//! - [`region`] — `.mca` region file read/write, including in-place single-chunk updates.
//! - [`lock`] — advisory locking for region files a live Java server may hold open.
//! - [`error`] — [`error::AnvilError`], this crate's error enum.

pub mod error;
pub mod lock;
pub mod nbt;
pub mod region;

pub use error::AnvilError;

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A fresh scratch directory under `$CARGO_TARGET_DIR` (falling back to this crate's own
    /// `target/`) for disk-touching tests — never under `/tmp` directly, per the task brief.
    pub fn scratch_dir(name: &str) -> PathBuf {
        let base = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = base
            .join("oxide-anvil-test-scratch")
            .join(format!("{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test scratch dir");
        dir
    }
}
