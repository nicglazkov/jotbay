use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

/// A whole-jotbay mutex, so a scheduled sync cannot land on top of a manual one.
///
/// `create_dir` is atomic on every filesystem we target, which `File::create`
/// is not — and unlike `flock` it exists on macOS, Linux and Windows alike.
pub struct SyncLock {
    path: PathBuf,
}

impl SyncLock {
    pub fn acquire(root: &Path) -> Result<Self> {
        let path = root.join(".jotbay-lock");
        match std::fs::create_dir(&path) {
            Ok(()) => Ok(Self { path }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // A lock left behind by a killed process would block every
                // future sync forever, so treat a clearly stale one as free.
                if Self::is_stale(&path) {
                    let _ = std::fs::remove_dir_all(&path);
                    std::fs::create_dir(&path)?;
                    Ok(Self { path })
                } else {
                    Err(Error::Locked)
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    fn is_stale(path: &Path) -> bool {
        const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(15 * 60);
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().unwrap_or_default() > STALE_AFTER)
            .unwrap_or(false)
    }
}

impl Drop for SyncLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
