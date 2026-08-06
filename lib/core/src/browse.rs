//! Read-only access to the notes folder, for the GUIs' file browser.
//!
//! Listing is per-directory rather than a whole-tree walk: the browser shows
//! one level at a time, and a vault that has grown to thousands of files
//! should not pay for all of them to render its root.
//!
//! Everything here refuses to leave the notes directory. The paths come from a
//! webview, and a webview's idea of a path is attacker-adjacent input even
//! when the only user is the owner. One `../` in a crafted event and a
//! "read-only notes browser" is reading `~/.ssh`.

use crate::error::{Error, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// One entry in a directory listing.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    /// Relative to the notes root, always with forward slashes. The webview
    /// hands it back verbatim, and Windows accepts both.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// Seconds since the epoch; 0 when the filesystem cannot say.
    pub modified: i64,
    /// How many entries a directory holds, so a folder row can say something
    /// more useful than a size of zero.
    pub children: u32,
}

/// What a file read produced.
#[derive(Debug, Clone, Serialize)]
pub struct FileContent {
    pub path: String,
    pub size: u64,
    /// `markdown`, `text`, or `binary`. The browser renders the first, shows
    /// the second plain, and only describes the third.
    pub kind: String,
    /// Empty for binary files.
    pub content: String,
    /// True when the file was larger than the cap and was cut short.
    pub truncated: bool,
}

/// Read no more than this into memory for a preview. A note is kilobytes; a
/// file this size in the vault is a PDF or an image, and the browser only
/// needs to say so.
const PREVIEW_CAP: u64 = 2 * 1024 * 1024;

/// Resolve `rel` inside `root`, refusing anything that escapes it.
///
/// Canonicalizes both sides so `..`, symlinks out of the vault, and Windows
/// path quirks are all judged on the real filesystem location rather than on
/// string prefixes.
fn resolve(root: &Path, rel: &str) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .map_err(|_| Error::Other("the notes folder is missing".into()))?;
    let joined = root.join(rel.trim_start_matches(['/', '\\']));
    let real = joined
        .canonicalize()
        .map_err(|_| Error::Other(format!("no such file: {rel}")))?;
    if !real.starts_with(&root) {
        return Err(Error::Other("path escapes the notes folder".into()));
    }
    Ok(real)
}

/// List one directory, folders first, then files, both alphabetically.
pub fn list(root: &Path, rel: &str) -> Result<Vec<Entry>> {
    let dir = resolve(root, rel)?;
    let mut entries = Vec::new();

    for item in std::fs::read_dir(&dir)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().to_string();
        // Dotfiles are sync plumbing (.gitattributes, .obsidian), not notes.
        if name.starts_with('.') {
            continue;
        }
        let meta = item.metadata()?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let children = if meta.is_dir() {
            std::fs::read_dir(item.path())
                .map(|it| {
                    it.flatten()
                        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                        .count() as u32
                })
                .unwrap_or(0)
        } else {
            0
        };

        let path = if rel.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", rel.trim_end_matches('/'), name)
        };

        entries.push(Entry {
            name,
            path,
            is_dir: meta.is_dir(),
            size: meta.len(),
            modified,
            children,
        });
    }

    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

/// Read one file for preview.
pub fn read(root: &Path, rel: &str) -> Result<FileContent> {
    let path = resolve(root, rel)?;
    let meta = std::fs::metadata(&path)?;
    if meta.is_dir() {
        return Err(Error::Other(format!("{rel} is a folder")));
    }

    let size = meta.len();
    let mut bytes = std::fs::read(&path)?;
    let truncated = size > PREVIEW_CAP;
    if truncated {
        bytes.truncate(PREVIEW_CAP as usize);
        // Cutting mid-codepoint would turn a large valid file "binary".
        while !bytes.is_empty() && std::str::from_utf8(&bytes).is_err() {
            bytes.pop();
        }
    }

    let (kind, content) = match String::from_utf8(bytes) {
        Ok(text) => {
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let kind = if matches!(ext.as_str(), "md" | "markdown") {
                "markdown"
            } else {
                "text"
            };
            (kind, text)
        }
        Err(_) => ("binary", String::new()),
    };

    Ok(FileContent {
        path: rel.to_string(),
        size,
        kind: kind.to_string(),
        content,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One directory per test, not per process: these run in parallel, and a
    /// shared directory that every test deletes on exit is a race the first
    /// parallel run loses.
    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("jotbay-browse-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("notes/deep")).unwrap();
        std::fs::write(dir.join("notes/a.md"), "# hello\n").unwrap();
        std::fs::write(dir.join("notes/deep/b.txt"), "plain\n").unwrap();
        std::fs::write(dir.join("notes/.hidden"), "x").unwrap();
        std::fs::write(dir.join("secret.txt"), "outside\n").unwrap();
        dir
    }

    #[test]
    fn lists_folders_first_and_hides_dotfiles() {
        let dir = scratch("list");
        let root = dir.join("notes");
        let entries = list(&root, "").unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["deep", "a.md"]);
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].children, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_markdown_as_markdown_and_txt_as_text() {
        let dir = scratch("kind");
        let root = dir.join("notes");
        assert_eq!(read(&root, "a.md").unwrap().kind, "markdown");
        assert_eq!(read(&root, "deep/b.txt").unwrap().kind, "text");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_to_leave_the_notes_folder() {
        let dir = scratch("escape");
        let root = dir.join("notes");
        // Whether the traversal fails to resolve or resolves outside the root,
        // the answer must be an error, never the file.
        assert!(read(&root, "../secret.txt").is_err());
        assert!(list(&root, "..").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
