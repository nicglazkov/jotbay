//! Conflict copies that are still sitting in the vault, and what to do with
//! them.
//!
//! Sync never loses a version: a file edited on two machines keeps the
//! incoming text under the original name and saves yours beside it as
//! `<stem>.conflict-<machine>-<time>.<ext>`. That is the right behaviour and
//! the wrong ending, because the app then abandoned you with the pair. The
//! copies sat in the folder indefinitely, syncing everywhere, looking like
//! clutter that appeared on its own.
//!
//! This module finds the pairs and settles them. Settling is ordinary file
//! work, committed by the next sync like any other edit, so it is itself
//! undoable through version history and never rewrites anything.

use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize)]
pub struct ConflictPair {
    /// The note under its own name, holding the other machine's text.
    pub original: String,
    /// The saved copy, holding this side's text at the time of the conflict.
    pub copy: String,
    /// Which machine's edit lost the filename, read from the copy's name.
    pub machine: Option<String>,
    /// When, from the same place. RFC 3339 when parseable.
    pub at: Option<String>,
    /// True when the two files currently have identical bytes, which means
    /// the conflict resolved itself, someone already merged by hand, or the
    /// edits were the same edit. Settling it discards nothing either way.
    pub identical: bool,
}

/// Every conflict copy in the vault, paired with its original.
pub fn list(notes: &Path) -> Result<Vec<ConflictPair>> {
    let mut out = Vec::new();
    walk(notes, notes, &mut out)?;
    // Newest first, matching every other list in the app.
    out.sort_by(|a, b| b.at.cmp(&a.at));
    Ok(out)
}

fn walk(notes: &Path, dir: &Path, out: &mut Vec<ConflictPair>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            walk(notes, &path, out)?;
            continue;
        }
        let Some((original_name, machine, at)) = parse_conflict_name(&name) else {
            continue;
        };
        let original_path = dir.join(&original_name);
        let rel = |p: &Path| {
            p.strip_prefix(notes)
                .map(|r| r.to_string_lossy().to_string())
                .unwrap_or_else(|_| p.to_string_lossy().to_string())
        };
        // A copy whose original vanished is still worth listing: settling it
        // by "keep this version" restores the note outright.
        let identical = match (std::fs::read(&original_path), std::fs::read(&path)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        out.push(ConflictPair {
            original: rel(&original_path),
            copy: rel(&path),
            machine,
            at,
            identical,
        });
    }
    Ok(())
}

/// `notes.conflict-macbook-20260801T1427Z.md` -> ("notes.md", macbook, time).
///
/// The name is the only record of the pairing, so this must accept exactly
/// what `conflict.rs` produces, including the extensionless form. Hostnames
/// can contain hyphens; the timestamp cannot, which is what anchors the split.
fn parse_conflict_name(name: &str) -> Option<(String, Option<String>, Option<String>)> {
    let marker = ".conflict-";
    let idx = name.find(marker)?;
    let stem = &name[..idx];
    let rest = &name[idx + marker.len()..];

    // rest = "<machine>-<stamp>" or "<machine>-<stamp>.<ext>"
    let (middle, ext) = match rest.rfind('.') {
        Some(dot) if dot > 0 => (&rest[..dot], Some(&rest[dot + 1..])),
        _ => (rest, None),
    };
    // The stamp is the part after the last hyphen and looks like
    // 20260801T1427Z; machines may have hyphens of their own.
    let dash = middle.rfind('-')?;
    let (machine, stamp) = (&middle[..dash], &middle[dash + 1..]);
    if stamp.len() < 8 || !stamp.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }

    let original = match ext {
        Some(e) => format!("{stem}.{e}"),
        None => stem.to_string(),
    };
    let at = to_rfc3339(stamp);
    Some((original, Some(machine.to_string()), at))
}

/// 20260801T1427Z -> 2026-08-01T14:27:00Z, so every surface can format it the
/// way it formats every other time.
fn to_rfc3339(stamp: &str) -> Option<String> {
    let b = stamp.as_bytes();
    if b.len() < 13 {
        return None;
    }
    let (y, mo, d, h, mi) = (
        stamp.get(0..4)?,
        stamp.get(4..6)?,
        stamp.get(6..8)?,
        stamp.get(9..11)?,
        stamp.get(11..13)?,
    );
    Some(format!("{y}-{mo}-{d}T{h}:{mi}:00Z"))
}

/// The three ways to settle a pair. In every case both texts remain in
/// history, because settling happens in the working tree and syncs as an
/// ordinary change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Settle {
    /// The original already holds the text to keep; remove the copy.
    KeepCurrent,
    /// The copy holds the text to keep; it becomes the note.
    KeepCopy,
    /// Keep both: the copy stops being a conflict marker and becomes an
    /// ordinary note named `<stem> (from <machine>).<ext>`.
    KeepBoth,
}

pub fn settle(notes: &Path, copy_rel: &str, choice: Settle) -> Result<()> {
    let copy = notes.join(copy_rel);
    let name = copy
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| Error::Other("that is not a conflict copy".into()))?;
    let (original_name, machine, _) = parse_conflict_name(&name)
        .ok_or_else(|| Error::Other("that is not a conflict copy".into()))?;
    let dir = copy
        .parent()
        .ok_or_else(|| Error::Other("that is not a conflict copy".into()))?;
    let original = dir.join(&original_name);

    match choice {
        Settle::KeepCurrent => {
            std::fs::remove_file(&copy)?;
        }
        Settle::KeepCopy => {
            // Rename over the original rather than copy-and-delete: one
            // operation, and no moment where the note exists twice or zero
            // times.
            std::fs::rename(&copy, &original)?;
        }
        Settle::KeepBoth => {
            let stem = original
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| original_name.clone());
            let ext = original
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            let who = machine.unwrap_or_else(|| "other".into());
            let mut target = dir.join(format!("{stem} (from {who}){ext}"));
            // A second settle of the same shape must not overwrite the first.
            let mut n = 2;
            while target.exists() {
                target = dir.join(format!("{stem} (from {who} {n}){ext}"));
                n += 1;
            }
            std::fs::rename(&copy, &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jotbay-pairs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_pairs_and_reads_machine_and_time_from_the_name() {
        let dir = scratch("find");
        std::fs::write(dir.join("notes.md"), "theirs").unwrap();
        std::fs::write(dir.join("notes.conflict-my-macbook-20260801T1427Z.md"), "mine").unwrap();

        let pairs = list(&dir).unwrap();
        assert_eq!(pairs.len(), 1);
        let p = &pairs[0];
        assert_eq!(p.original, "notes.md");
        // Hostnames contain hyphens; the split anchors on the timestamp.
        assert_eq!(p.machine.as_deref(), Some("my-macbook"));
        assert_eq!(p.at.as_deref(), Some("2026-08-01T14:27:00Z"));
        assert!(!p.identical);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keep_current_removes_the_copy_and_keeps_the_note() {
        let dir = scratch("current");
        std::fs::write(dir.join("a.md"), "keep me").unwrap();
        std::fs::write(dir.join("a.conflict-box-20260801T1427Z.md"), "discard").unwrap();
        settle(&dir, "a.conflict-box-20260801T1427Z.md", Settle::KeepCurrent).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.md")).unwrap(), "keep me");
        assert_eq!(list(&dir).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keep_copy_makes_the_copy_the_note() {
        let dir = scratch("copy");
        std::fs::write(dir.join("a.md"), "replace me").unwrap();
        std::fs::write(dir.join("a.conflict-box-20260801T1427Z.md"), "the keeper").unwrap();
        settle(&dir, "a.conflict-box-20260801T1427Z.md", Settle::KeepCopy).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.md")).unwrap(), "the keeper");
        assert_eq!(list(&dir).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keep_both_renames_the_copy_into_an_ordinary_note() {
        let dir = scratch("both");
        std::fs::write(dir.join("a.md"), "one").unwrap();
        std::fs::write(dir.join("a.conflict-box-20260801T1427Z.md"), "two").unwrap();
        settle(&dir, "a.conflict-box-20260801T1427Z.md", Settle::KeepBoth).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.md")).unwrap(), "one");
        assert_eq!(
            std::fs::read_to_string(dir.join("a (from box).md")).unwrap(),
            "two"
        );
        assert_eq!(list(&dir).unwrap().len(), 0, "no longer reads as a conflict");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settling_twice_does_not_overwrite_the_first_keep_both() {
        let dir = scratch("twice");
        std::fs::write(dir.join("a.md"), "x").unwrap();
        std::fs::write(dir.join("a (from box).md"), "first").unwrap();
        std::fs::write(dir.join("a.conflict-box-20260801T1500Z.md"), "second").unwrap();
        settle(&dir, "a.conflict-box-20260801T1500Z.md", Settle::KeepBoth).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a (from box).md")).unwrap(), "first");
        assert_eq!(std::fs::read_to_string(dir.join("a (from box 2).md")).unwrap(), "second");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_identical_pair_is_flagged_so_the_ui_can_say_it_is_safe() {
        let dir = scratch("same");
        std::fs::write(dir.join("a.md"), "same").unwrap();
        std::fs::write(dir.join("a.conflict-box-20260801T1427Z.md"), "same").unwrap();
        assert!(list(&dir).unwrap()[0].identical);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extensionless_conflicts_pair_correctly() {
        let dir = scratch("noext");
        std::fs::write(dir.join("LICENSE"), "a").unwrap();
        std::fs::write(dir.join("LICENSE.conflict-box-20260801T1427Z"), "b").unwrap();
        let pairs = list(&dir).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].original, "LICENSE");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
