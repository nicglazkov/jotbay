//! Finding notes, and seeing a note over time.
//!
//! Three features that look separate in a menu and are one idea underneath:
//! history, undelete, and conflicts are all *versions of a note*. A past
//! version, a version that ended, and two versions that both exist right now.
//! They share this module so they cannot drift into three different answers to
//! the same question.
//!
//! All of it is already in the repository. Git is the reason Jotbay can offer
//! any of this, and none of it needs a server, an index, or a second copy of
//! anyone's notes.

use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::git::Git;

// --- finding -----------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    /// Path relative to the notes directory.
    pub path: String,
    /// The matching line, when the match was in the text rather than the name.
    pub line: Option<u32>,
    pub excerpt: Option<String>,
    /// True when the file's name matched, which is usually what someone means.
    pub name_match: bool,
}

/// Search names first, then contents.
///
/// `git grep` rather than walking the tree: it respects .gitignore, skips
/// binaries on its own, and is fast enough on a large vault that the field can
/// search as you type. A file that is not committed yet will not appear, which
/// is a real limit and the reason untracked files are searched by name too.
pub fn search(git: &Git, notes: &Path, query: &str, limit: usize) -> Result<Vec<Hit>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let mut hits: Vec<Hit> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let needle = query.to_lowercase();

    // Names first. Someone typing "postgres" almost always wants
    // postgres-tuning.md, not the twelve notes that mention postgres.
    if let Ok(listing) = git.run(&["ls-files"]) {
        for path in listing.lines() {
            let rel = relative_to_notes(git, notes, path);
            if rel.to_lowercase().contains(&needle) {
                seen.push(path.to_string());
                hits.push(Hit {
                    path: rel,
                    line: None,
                    excerpt: None,
                    name_match: true,
                });
            }
            if hits.len() >= limit {
                return Ok(hits);
            }
        }
    }

    // Then contents. -I skips binaries, -n numbers the lines, -i is what a
    // person expects from a search box.
    let out = git.run(&["grep", "-I", "-n", "-i", "--", query]);
    if let Ok(text) = out {
        for line in text.lines() {
            // path:line:text
            let mut parts = line.splitn(3, ':');
            let (Some(path), Some(no), Some(body)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            if seen.iter().any(|p| p == path) {
                continue;
            }
            seen.push(path.to_string());
            hits.push(Hit {
                path: relative_to_notes(git, notes, path),
                line: no.parse().ok(),
                excerpt: Some(trim_excerpt(body)),
                name_match: false,
            });
            if hits.len() >= limit {
                break;
            }
        }
    }

    Ok(hits)
}

/// A long line is useless in a result row, and the interesting part is rarely
/// at the start of it.
fn trim_excerpt(body: &str) -> String {
    let body = body.trim();
    if body.chars().count() <= 120 {
        return body.to_string();
    }
    body.chars().take(117).collect::<String>() + "..."
}

/// Git reports paths from the repository root; the interface speaks in paths
/// under the notes directory, which are the same thing only when the vault is
/// flat.
fn relative_to_notes(git: &Git, notes: &Path, repo_path: &str) -> String {
    let full = git.root().join(repo_path);
    full.strip_prefix(notes)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| repo_path.to_string())
}

// --- a note over time ---------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Version {
    pub sha: String,
    pub short: String,
    /// RFC 3339, so every surface formats it the same way.
    pub at: String,
    /// The machine that made it, read from the commit message Jotbay writes.
    pub machine: Option<String>,
    pub subject: String,
    /// True when this version deleted the file.
    pub deleted: bool,
}

/// Every version of one note, newest first.
pub fn history(git: &Git, notes: &Path, rel: &str, limit: u32) -> Result<Vec<Version>> {
    let path = repo_path(git, notes, rel);
    // The record separator leads, and that is load bearing. With --name-status
    // git prints the changed paths *after* the formatted header, so a trailing
    // separator puts each commit's paths at the start of the next record: the
    // first commit parsed and the rest were dropped as headerless.
    let fmt = "--pretty=format:%x1e%H\x1f%h\x1f%aI\x1f%s";
    let n = format!("-{limit}");
    // --follow so a renamed note keeps its past. --diff-filter records what
    // this commit did to the file, which is how a deletion is spotted.
    let raw = git.run(&[
        "log", &n, "--follow", "--name-status", fmt, "--", &path,
    ])?;

    let mut out = Vec::new();
    for record in raw.split('\x1e') {
        let record = record.trim_start_matches('\n').trim_start();
        if record.is_empty() {
            continue;
        }
        let mut lines = record.lines();
        let Some(head) = lines.next() else { continue };
        let f: Vec<&str> = head.split('\x1f').collect();
        if f.len() < 4 {
            continue;
        }
        // The name-status line looks like "D\tpath" or "M\tpath".
        let deleted = lines
            .find(|l| !l.trim().is_empty())
            .map(|l| l.starts_with('D'))
            .unwrap_or(false);

        out.push(Version {
            sha: f[0].to_string(),
            short: f[1].to_string(),
            at: f[2].to_string(),
            machine: machine_from_subject(f[3]),
            subject: f[3].to_string(),
            deleted,
        });
    }
    Ok(out)
}

/// Jotbay writes "jotbay: <machine> <date> <time>", so the machine is readable
/// without a second lookup. A hand-written commit has no machine, and saying
/// nothing is better than guessing.
fn machine_from_subject(subject: &str) -> Option<String> {
    subject
        .strip_prefix("jotbay: ")
        .and_then(|rest| rest.split_whitespace().next())
        .map(|s| s.to_string())
}

/// What the note said at that version.
pub fn version_content(git: &Git, notes: &Path, rel: &str, sha: &str) -> Result<String> {
    let path = repo_path(git, notes, rel);
    git.run(&["show", &format!("{sha}:{path}")])
}

/// Put an old version back in the working tree.
///
/// Written to the file and left there, uncommitted. The watcher notices it the
/// way it notices any other edit, so restoring behaves exactly like typing the
/// old text back in: it syncs, it is undoable, and it never rewrites history.
pub fn restore(git: &Git, notes: &Path, rel: &str, sha: &str) -> Result<PathBuf> {
    let content = version_content(git, notes, rel, sha)?;
    let target = notes.join(rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, content)?;
    Ok(target)
}

#[derive(Debug, Clone, Serialize)]
pub struct Deleted {
    pub path: String,
    /// The commit that removed it, which is also the one to restore from.
    pub sha: String,
    pub short: String,
    pub at: String,
    pub machine: Option<String>,
}

/// Notes that used to exist and do not now.
///
/// The commit that deleted a file cannot be the one to read it back from, so
/// each entry carries the deleting commit and `restore_deleted` reads its
/// parent. Getting that wrong yields an empty file, which looks like a restore
/// that worked.
pub fn deleted(git: &Git, notes: &Path, limit: u32) -> Result<Vec<Deleted>> {
    let n = format!("-{limit}");
    let raw = git.run(&[
        "log", &n, "--diff-filter=D", "--name-only",
        // Leading separator, for the same reason as history above.
        "--pretty=format:%x1e%H\x1f%h\x1f%aI\x1f%s",
    ])?;

    let mut out: Vec<Deleted> = Vec::new();
    for record in raw.split('\x1e') {
        let record = record.trim_start_matches('\n').trim_start();
        if record.is_empty() {
            continue;
        }
        let mut lines = record.lines();
        let Some(head) = lines.next() else { continue };
        let f: Vec<&str> = head.split('\x1f').collect();
        if f.len() < 4 {
            continue;
        }
        for path in lines.filter(|l| !l.trim().is_empty()) {
            // Still gone? A note deleted and later written again is not
            // waiting to be restored, and listing it would be a lie.
            if notes.join(relative_to_notes(git, notes, path)).exists() {
                continue;
            }
            out.push(Deleted {
                path: relative_to_notes(git, notes, path),
                sha: f[0].to_string(),
                short: f[1].to_string(),
                at: f[2].to_string(),
                machine: machine_from_subject(f[3]),
            });
        }
    }
    out.dedup_by(|a, b| a.path == b.path);
    Ok(out)
}

/// Bring back a deleted note, reading from before it was removed.
pub fn restore_deleted(git: &Git, notes: &Path, rel: &str, deleting_sha: &str) -> Result<PathBuf> {
    restore(git, notes, rel, &format!("{deleting_sha}^"))
}

// --- writing ------------------------------------------------------------------

/// Create a note, and never overwrite one.
///
/// The only place Jotbay writes a file of its own accord, so it refuses to
/// touch an existing path rather than deciding what a collision means.
pub fn create(notes: &Path, name: &str, body: &str) -> Result<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Other("a note needs a name".into()));
    }
    if name.contains("..") || name.starts_with('/') || name.starts_with('\\') {
        return Err(Error::Other("that name would leave the notes folder".into()));
    }

    let with_ext = if Path::new(name).extension().is_some() {
        name.to_string()
    } else {
        format!("{name}.md")
    };
    let target = notes.join(&with_ext);
    if target.exists() {
        return Err(Error::Other(format!("{with_ext} already exists")));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, body)?;
    Ok(target)
}

/// Add to the end of a note, creating it if it is not there.
///
/// Append rather than rewrite, deliberately: two machines appending to the
/// same note produce a conflict git can usually merge on its own, where two
/// machines rewriting it cannot.
pub fn append(notes: &Path, rel: &str, text: &str) -> Result<PathBuf> {
    let target = notes.join(rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let existing = std::fs::read_to_string(&target).unwrap_or_default();
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(text.trim_end());
    next.push('\n');
    std::fs::write(&target, next)?;
    Ok(target)
}

/// Hand a file to whatever the person actually writes in.
///
/// Jotbay is not an editor and should not become one. Opening the file in the
/// editor already chosen costs nothing and answers most of what "let me edit
/// in the app" is asking for.
pub fn open_externally(path: &Path) -> Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    crate::proc::quiet(opener)
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| Error::Other(format!("could not open {}: {e}", path.display())))
}

fn repo_path(git: &Git, notes: &Path, rel: &str) -> String {
    notes
        .join(rel)
        .strip_prefix(git.root())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| rel.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_gets_an_extension_but_keeps_one_it_has() {
        let dir = std::env::temp_dir().join(format!("jotbay-notes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let a = create(&dir, "shopping", "").unwrap();
        assert!(a.ends_with("shopping.md"));
        let b = create(&dir, "config.toml", "").unwrap();
        assert!(b.ends_with("config.toml"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn creating_never_overwrites_and_never_escapes() {
        let dir = std::env::temp_dir().join(format!("jotbay-notes2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        create(&dir, "once", "first").unwrap();
        assert!(create(&dir, "once", "second").is_err(), "must not overwrite");
        assert_eq!(std::fs::read_to_string(dir.join("once.md")).unwrap(), "first");

        // The only place Jotbay writes a path someone typed.
        assert!(create(&dir, "../escape", "x").is_err());
        assert!(create(&dir, "/etc/passwd", "x").is_err());
        assert!(create(&dir, "   ", "x").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn appending_keeps_what_was_there_and_ends_with_one_newline() {
        let dir = std::env::temp_dir().join(format!("jotbay-notes3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        append(&dir, "log.md", "one").unwrap();
        append(&dir, "log.md", "two").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("log.md")).unwrap(), "one\ntwo\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_machine_is_read_from_jotbays_own_commit_subject() {
        assert_eq!(
            machine_from_subject("jotbay: studio-mac 2026-08-12 14:03"),
            Some("studio-mac".to_string())
        );
        // Somebody's own commit, which says nothing about a machine.
        assert_eq!(machine_from_subject("fix the tuning notes"), None);
    }

    #[test]
    fn an_excerpt_is_trimmed_rather_than_wrapped() {
        let long = "x".repeat(400);
        let out = trim_excerpt(&long);
        assert_eq!(out.chars().count(), 120);
        assert!(out.ends_with("..."));
        assert_eq!(trim_excerpt("  short  "), "short");
    }
}
