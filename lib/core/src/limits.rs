//! What Jotbay can and cannot carry, and why.
//!
//! The ceilings are GitHub's, not ours. The one that matters is 100 MB: a push
//! containing a larger file is rejected outright. Committing such a file first
//! and discovering that at push time is the worst outcome available — the
//! commit is already in local history, every later sync fails on the same
//! rejected push, and getting out requires a reset the user did not ask for.
//!
//! So oversized files are detected *before* staging and simply left
//! uncommitted. Everything else still syncs, and the file stays visible in
//! `jotbay status` until the user deals with it.

use crate::error::Result;
use crate::git::Git;
use serde::{Deserialize, Serialize};

/// GitHub rejects any push containing a file at or above this size.
pub const BLOCK_BYTES: u64 = 100 * 1024 * 1024;
/// GitHub warns above this, and clones start to feel it.
pub const WARN_BYTES: u64 = 50 * 1024 * 1024;
/// Our own guidance. Git keeps a near-complete copy of every version of a
/// binary forever, so repeated edits to a file this size are what actually
/// ruins a repository — long before any single file hits the hard ceiling.
pub const ADVISE_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Will be refused by the remote. Never staged.
    Blocked,
    /// Allowed, but the remote complains and clones get slower.
    Warning,
    /// Allowed and quiet, but a poor fit for a git-backed jotbay.
    Advisory,
}

impl Severity {
    pub fn of(bytes: u64) -> Option<Severity> {
        if bytes >= BLOCK_BYTES {
            Some(Severity::Blocked)
        } else if bytes >= WARN_BYTES {
            Some(Severity::Warning)
        } else if bytes >= ADVISE_BYTES {
            Some(Severity::Advisory)
        } else {
            None
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Severity::Blocked => "too large to sync",
            Severity::Warning => "very large",
            Severity::Advisory => "large",
        }
    }

    /// What the user should actually do about a group of files at this
    /// severity.
    ///
    /// This is the only copy of that advice. The CLI reads it directly and
    /// both GUIs receive it on every `FileWarning` (see the `Serialize` impl
    /// below), because the previous arrangement — each UI holding its own
    /// literal — is exactly how all three came to keep recommending Git LFS
    /// for months after this function stopped.
    pub fn advice(&self) -> &'static str {
        match self {
            Severity::Blocked => {
                // Deliberately does NOT suggest Git LFS. Conflict resolution
                // reads merge stages with `git show :2:`, which returns an LFS
                // *pointer* rather than the file — verified — so a conflict on
                // an LFS-tracked file would write a 130-byte stub over real
                // content. Advise LFS only once conflict.rs smudges stages.
                "GitHub refuses files of 100 MB or more, so these were left out of the sync. \
                 Move them somewhere outside your notes folder. Everything else synced normally."
            }
            Severity::Warning => {
                "These sync, but GitHub warns about them and every machine pays the \
                 download on every clone."
            }
            Severity::Advisory => {
                "These sync, but git keeps every version forever — editing them repeatedly \
                 grows the repository permanently on every machine."
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileWarning {
    pub path: String,
    pub bytes: u64,
    pub severity: Severity,
}

/// Serialized by hand so every warning carries its own `advice` string to the
/// GUIs. They render whatever Rust sends rather than keeping a copy, which is
/// what stops the two from drifting apart again. `advice` is derived, so
/// `Deserialize` stays derived and simply ignores the field on the way back in.
impl Serialize for FileWarning {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("FileWarning", 4)?;
        s.serialize_field("path", &self.path)?;
        s.serialize_field("bytes", &self.bytes)?;
        s.serialize_field("severity", &self.severity)?;
        s.serialize_field("advice", self.severity.advice())?;
        s.end()
    }
}

impl FileWarning {
    pub fn human_size(&self) -> String {
        human_size(self.bytes)
    }
}

pub fn human_size(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.0} MB", b / MB)
    } else {
        format!("{:.0} KB", b / 1024.0)
    }
}

/// Every file in the working tree that is at or over the advisory threshold.
///
/// Uses `git ls-files` rather than walking the directory so that ignored paths
/// — build output above all — are never considered.
pub fn scan(git: &Git) -> Result<Vec<FileWarning>> {
    let listing = git.run(&["ls-files", "--cached", "--others", "--exclude-standard"])?;
    let root = git.root();

    let mut found: Vec<FileWarning> = listing
        .lines()
        .filter(|p| !p.is_empty())
        .filter_map(|path| {
            let bytes = std::fs::metadata(root.join(path)).ok()?.len();
            let severity = Severity::of(bytes)?;
            Some(FileWarning {
                path: path.to_string(),
                bytes,
                severity,
            })
        })
        .collect();

    found.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    Ok(found)
}

/// Just the paths that must not be staged.
pub fn blocked_paths(warnings: &[FileWarning]) -> Vec<String> {
    warnings
        .iter()
        .filter(|w| w.severity == Severity::Blocked)
        .map(|w| w.path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_map_to_the_right_severity() {
        assert_eq!(Severity::of(1024), None);
        assert_eq!(Severity::of(ADVISE_BYTES), Some(Severity::Advisory));
        assert_eq!(Severity::of(WARN_BYTES), Some(Severity::Warning));
        assert_eq!(Severity::of(BLOCK_BYTES), Some(Severity::Blocked));
        // The boundary is inclusive: GitHub rejects at exactly 100 MB.
        assert_eq!(Severity::of(BLOCK_BYTES - 1), Some(Severity::Warning));
    }

    #[test]
    fn sizes_render_readably() {
        assert_eq!(human_size(512 * 1024), "512 KB");
        assert_eq!(human_size(30 * 1024 * 1024), "30 MB");
        assert_eq!(human_size(2 * 1024 * 1024 * 1024), "2.0 GB");
    }
}
