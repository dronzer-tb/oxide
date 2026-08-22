//! Advisory locking for `.mca` region files a live Folia/Java server may hold open.
//!
//! **What this protects against:** two Oxide writers (or an Oxide writer and a concurrent
//! `oxide-harness` reader) racing on the same region file.
//!
//! **What this does NOT protect against:** the Java server itself. This is a plain sidecar file
//! (`<region>.mca.oxide-lock`) that nothing outside this crate consults. A Java process that
//! opens and writes `r.0.0.mca` directly will never see it, never wait on it, and can interleave
//! writes with Oxide's in a way that corrupts the region. Real mutual exclusion with the running
//! server is out of scope here and has to be enforced by whatever coordinates when Oxide is
//! allowed to touch a region at all (e.g. only writing chunks the server hasn't loaded yet) — this
//! lock file exists to stop Oxide from racing itself, not to fence off Java.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::AnvilError;

/// A lock is considered stale (safe to steal) once it is older than this, *if* we cannot confirm
/// the owning pid is alive by other means (e.g. non-Linux, or `/proc` unavailable). On Linux we
/// primarily trust `/proc/<pid>` existing; this is the cross-platform fallback.
const STALE_AGE_FALLBACK: Duration = Duration::from_secs(60);

fn lock_path_for(region_path: &Path) -> PathBuf {
    let mut os = region_path.as_os_str().to_owned();
    os.push(".oxide-lock");
    PathBuf::from(os)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(target_os = "linux")]
fn pid_is_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(not(target_os = "linux"))]
fn pid_is_alive(_pid: u32) -> bool {
    // No cheap portable liveness check without extra deps; fall back to age-based staleness.
    true
}

struct LockInfo {
    pid: u32,
    acquired_unix: u64,
}

fn parse_lock(contents: &str) -> Option<LockInfo> {
    let mut lines = contents.lines();
    let pid: u32 = lines.next()?.trim().parse().ok()?;
    let acquired_unix: u64 = lines.next()?.trim().parse().ok()?;
    Some(LockInfo { pid, acquired_unix })
}

fn is_stale(info: &LockInfo) -> bool {
    if !pid_is_alive(info.pid) {
        return true;
    }
    let age = now_unix().saturating_sub(info.acquired_unix);
    age > STALE_AGE_FALLBACK.as_secs()
}

/// RAII guard for the advisory lock on one region file. Acquire with [`RegionGuard::acquire`];
/// the lock file is removed on drop (including on early-return / panic-unwind paths), so a
/// held lock cannot be leaked by forgetting to release it explicitly.
pub struct RegionGuard {
    lock_path: PathBuf,
}

impl RegionGuard {
    /// Acquire the advisory lock for `region_path`. Fails with
    /// [`AnvilError::RegionLocked`] if a live (non-stale) lock is already held by another
    /// process; a stale lock is stolen automatically.
    pub fn acquire(region_path: &Path) -> Result<Self, AnvilError> {
        let lock_path = lock_path_for(region_path);

        match Self::try_create(&lock_path) {
            Ok(()) => return Ok(Self { lock_path }),
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
                return Err(AnvilError::Io(e))
            }
            Err(_) => {} // AlreadyExists: fall through to staleness check below.
        }

        let contents = fs::read_to_string(&lock_path).map_err(AnvilError::Io)?;
        let info =
            parse_lock(&contents).ok_or_else(|| AnvilError::MalformedLock(lock_path.clone()))?;

        if is_stale(&info) {
            // Best-effort steal: remove and recreate. If another process wins the race here,
            // the subsequent try_create will fail and we surface that as still-locked rather
            // than looping forever.
            let _ = fs::remove_file(&lock_path);
            Self::try_create(&lock_path).map_err(AnvilError::Io)?;
            return Ok(Self { lock_path });
        }

        Err(AnvilError::RegionLocked {
            path: region_path.to_path_buf(),
            lock_path,
            pid: info.pid,
        })
    }

    fn try_create(lock_path: &Path) -> std::io::Result<()> {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock_path)?;
        write!(f, "{}\n{}\n", std::process::id(), now_unix())?;
        f.sync_all()?;
        Ok(())
    }
}

impl Drop for RegionGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn scratch_dir(name: &str) -> PathBuf {
        crate::test_support::scratch_dir(name)
    }

    #[test]
    fn acquire_then_drop_releases_lock() {
        let dir = scratch_dir("lock-basic");
        let region = dir.join("r.0.0.mca");
        let lock_path = lock_path_for(&region);

        {
            let _guard = RegionGuard::acquire(&region).unwrap();
            assert!(lock_path.exists());
        }
        assert!(!lock_path.exists());
    }

    #[test]
    fn live_lock_blocks_second_acquire() {
        let dir = scratch_dir("lock-contended");
        let region = dir.join("r.0.0.mca");
        let _guard = RegionGuard::acquire(&region).unwrap();

        let result = RegionGuard::acquire(&region);
        assert!(matches!(result, Err(AnvilError::RegionLocked { .. })));
    }

    #[test]
    fn stale_lock_is_stolen() {
        let dir = scratch_dir("lock-stale");
        let region = dir.join("r.0.0.mca");
        let lock_path = lock_path_for(&region);

        // Simulate a lock left behind by a pid that can't be running (pid 1 is init and would
        // be "alive", so use a fabricated old timestamp instead, which is the portable path this
        // crate relies on off Linux too).
        let mut f = File::create(&lock_path).unwrap();
        write!(f, "999999\n1\n").unwrap(); // acquired at unix time 1: guaranteed stale by age.
        drop(f);

        let _guard = RegionGuard::acquire(&region).unwrap();
        let contents = fs::read_to_string(&lock_path).unwrap();
        assert!(contents.starts_with(&std::process::id().to_string()));
    }
}
