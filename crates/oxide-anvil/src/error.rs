use std::path::PathBuf;

/// Library error enum for `oxide-anvil` (per `docs/ARCHITECTURE.md`: `thiserror` in library
/// code, `anyhow` reserved for binary/boundary callers).
#[derive(Debug, thiserror::Error)]
pub enum AnvilError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("nbt (de)serialization error: {0}")]
    Nbt(#[from] fastnbt::error::Error),

    #[error("region file error: {0}")]
    Region(#[from] fastanvil::Error),

    #[error("min_y {0} is not section-aligned (must be a multiple of 16)")]
    UnalignedMinY(i32),

    #[error("region {path} is locked by pid {pid} (lock file {lock_path})")]
    RegionLocked {
        path: PathBuf,
        lock_path: PathBuf,
        pid: u32,
    },

    #[error("lock file {0} is malformed")]
    MalformedLock(PathBuf),
}
