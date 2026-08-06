//! Keep-both-sides conflict resolution.
//!
//! # The inversion that makes this subtle
//!
//! During a rebase, git replays *your* commits on top of the upstream branch.
//! `HEAD` is therefore upstream, which means:
//!
//! * stage 2, `--ours`   → the **upstream** (remote) version
//! * stage 3, `--theirs` → **your** local commit being replayed
//!
//! This is inverted from every intuition about "ours" and "theirs", and getting
//! it backwards would silently swap which version keeps the canonical filename
//! while still exiting zero. `tests/conflict.rs` asserts on content precisely
//! because an exit-status check cannot catch that.
//!
//! # Policy
//!
//! Upstream keeps the canonical name; the local version is written beside it as
//! `<stem>.conflict-<hostname>-<timestamp>.<ext>`. When only one side has
//! content. The delete/modify cases, that surviving content is kept at the
//! canonical path. No branch of this function can lose a byte.

use crate::error::Result;
use crate::git::Git;
use crate::model::{ConflictKind, ConflictResolution};
use std::path::Path;
use time::OffsetDateTime;

const OURS_UPSTREAM: u8 = 2;
const THEIRS_LOCAL: u8 = 3;

/// Build the sibling path that holds the local version.
fn conflict_path(path: &str, hostname: &str, stamp: &str) -> String {
    let p = Path::new(path);
    let parent = p.parent().filter(|d| !d.as_os_str().is_empty());
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = p.extension().map(|e| e.to_string_lossy().to_string());

    let name = match ext {
        Some(ext) => format!("{stem}.conflict-{hostname}-{stamp}.{ext}"),
        None => format!("{stem}.conflict-{hostname}-{stamp}"),
    };

    match parent {
        Some(dir) => dir.join(name).to_string_lossy().replace('\\', "/"),
        None => name,
    }
}

fn timestamp() -> String {
    let now = OffsetDateTime::now_utc();
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

/// Resolve every unmerged path, preserving both sides. Returns what was done.
pub fn resolve_all(git: &Git, hostname: &str) -> Result<Vec<ConflictResolution>> {
    let stamp = timestamp();
    let mut out = Vec::new();

    for path in git.conflicted_paths()? {
        let (has_upstream, has_local) = git.conflict_stages(&path)?;
        let abs = git.root().join(&path);
        if let Some(dir) = abs.parent() {
            std::fs::create_dir_all(dir)?;
        }

        let resolution = match (has_upstream, has_local) {
            // Both sides changed it: upstream keeps the name, ours goes beside.
            (true, true) => {
                let upstream = git.stage_blob(OURS_UPSTREAM, &path)?;
                let local = git.stage_blob(THEIRS_LOCAL, &path)?;

                std::fs::write(&abs, &upstream)?;

                let copy = conflict_path(&path, hostname, &stamp);
                std::fs::write(git.root().join(&copy), &local)?;
                git.run(&["add", "--", &path, &copy])?;

                ConflictResolution {
                    path,
                    kept_copy: Some(copy),
                    kind: ConflictKind::BothModified,
                }
            }

            // We deleted it, upstream changed it. Keep upstream's content:
            // a deletion is cheaper to redo than a lost file is to recover.
            (true, false) => {
                let upstream = git.stage_blob(OURS_UPSTREAM, &path)?;
                std::fs::write(&abs, &upstream)?;
                git.run(&["add", "--", &path])?;
                ConflictResolution {
                    path,
                    kept_copy: None,
                    kind: ConflictKind::DeletedLocally,
                }
            }

            // Upstream deleted it, we changed it. Keep our content.
            (false, true) => {
                let local = git.stage_blob(THEIRS_LOCAL, &path)?;
                std::fs::write(&abs, &local)?;
                git.run(&["add", "--", &path])?;
                ConflictResolution {
                    path,
                    kept_copy: None,
                    kind: ConflictKind::DeletedUpstream,
                }
            }

            // Neither stage present: both sides deleted it. Nothing to keep.
            (false, false) => {
                git.run(&["rm", "-q", "--ignore-unmatch", "--", &path])?;
                continue;
            }
        };

        out.push(resolution);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_path_preserves_directory_and_extension() {
        assert_eq!(
            conflict_path("data/notes.md", "macbook", "20260801T1427Z"),
            "data/notes.conflict-macbook-20260801T1427Z.md"
        );
    }

    #[test]
    fn conflict_path_handles_root_level_files() {
        assert_eq!(
            conflict_path("README.md", "workstation", "20260801T1427Z"),
            "README.conflict-workstation-20260801T1427Z.md"
        );
    }

    #[test]
    fn conflict_path_handles_no_extension() {
        assert_eq!(
            conflict_path("data/LICENSE", "box", "20260801T1427Z"),
            "data/LICENSE.conflict-box-20260801T1427Z"
        );
    }

    #[test]
    fn conflict_path_handles_dotted_names() {
        // Only the final extension is split off; the rest stays in the stem.
        assert_eq!(
            conflict_path("data/notes.draft.md", "box", "20260801T1427Z"),
            "data/notes.draft.conflict-box-20260801T1427Z.md"
        );
    }
}
