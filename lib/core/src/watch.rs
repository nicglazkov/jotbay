//! Sync without being asked.
//!
//! The scheduled sync ran every ten minutes, which meant a note written on one
//! machine could sit there for ten minutes before existing anywhere else, and
//! the answer to "did that save?" was "probably, eventually". That is a poor
//! deal for someone who does not think in commits — which is everyone this is
//! built for.
//!
//! This watches the notes folder and syncs shortly after edits stop, and polls
//! the remote often enough that the other machines are current too. The user's
//! side of the contract becomes: put a file in the folder. Nothing else.
//!
//! Change detection is a directory scan comparing size and mtime rather than
//! an OS notification API. Three reasons, in order of weight: it behaves
//! identically on macOS, Windows and Linux, where the notification APIs differ
//! enough to have their own bug classes each; it cannot miss a change that
//! happened while the process was not running; and for a notes folder — which
//! is measured in hundreds of files — a scan costs single-digit milliseconds.
//!
//! It is deliberately not a general file-sync engine. A folder of markdown is
//! a small, quiet thing, and the design leans on that.

use crate::error::Result;
use crate::Jotbay;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// How long the folder must be quiet before a sync runs.
///
/// Long enough that saving a file in a loop — which every editor does, and
/// which some do several times a second — produces one commit rather than
/// twenty. Short enough that "did it save?" is never a real question.
///
/// With a one-second scan this puts a change on the remote in under ten
/// seconds end to end, most of which is the network round trip rather than
/// anything waited for here. Measured at twelve seconds with a three-second
/// settle, which was noticeably longer than it needed to be.
pub const SETTLE: Duration = Duration::from_secs(2);

/// How often the folder is inspected for local edits.
pub const SCAN_EVERY: Duration = Duration::from_secs(1);

/// How often the remote is checked for other machines' work.
///
/// The other half of the promise: pushing quickly is worth little if the
/// machine you walk over to is still ten minutes behind. A fetch against an
/// unchanged remote is one small round trip, so this is affordable in a way
/// that a full sync at the same cadence would not be.
pub const POLL_REMOTE: Duration = Duration::from_secs(20);

/// A fingerprint of the notes folder: path, size and modification time.
///
/// Content is deliberately not hashed. The question is only "did anything
/// change", git answers "what exactly" immediately afterwards, and hashing
/// every file every two seconds would spend real power to learn nothing new.
type Fingerprint = BTreeMap<String, (u64, i64)>;

fn fingerprint(dir: &Path) -> Fingerprint {
    fn walk(dir: &Path, base: &Path, out: &mut Fingerprint, depth: u32) {
        // A notes folder is not deep. The bound stops a symlink cycle from
        // turning a two-second scan into an infinite one.
        if depth > 32 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // .git changes on every sync; watching it would make the watcher
            // trigger itself forever.
            if name.starts_with('.') {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                walk(&path, base, out, depth + 1);
            } else {
                let key = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                out.insert(key, (meta.len(), mtime));
            }
        }
    }

    let mut out = Fingerprint::new();
    walk(dir, dir, &mut out, 0);
    out
}

/// What the watcher just did, for whoever is displaying it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Local edits were found and synced.
    Local,
    /// A scheduled check of the remote.
    Remote,
    /// A sync was attempted and failed.
    Failed,
}

/// Watch until the process is killed, calling `on_event` after each sync.
///
/// Runs in the foreground on purpose: the supervisor that keeps it alive
/// belongs to the operating system — launchd, systemd, Task Scheduler — all of
/// which restart a process that dies and log what it printed. A hand-rolled
/// daemon would reimplement that badly.
pub fn run(jotbay: &Jotbay, mut on_event: impl FnMut(Event, Option<String>)) -> Result<()> {
    let data = jotbay.data_dir();
    let mut seen = fingerprint(&data);
    let mut dirty_since: Option<Instant> = None;
    let mut last_remote = Instant::now();

    // One at startup, so a machine that was asleep while another pushed is
    // current by the time anyone looks at it.
    sync_now(jotbay, Event::Remote, &mut on_event);

    loop {
        std::thread::sleep(SCAN_EVERY);

        let current = fingerprint(&data);
        if current != seen {
            seen = current;
            // Restarted on every change, so a burst of saves collapses into a
            // single sync once the burst ends.
            dirty_since = Some(Instant::now());
            continue;
        }

        if let Some(since) = dirty_since {
            if since.elapsed() >= SETTLE {
                dirty_since = None;
                last_remote = Instant::now();
                sync_now(jotbay, Event::Local, &mut on_event);
                // The sync writes status refs and may pull; re-read so its own
                // work is never mistaken for the user's.
                seen = fingerprint(&data);
                continue;
            }
        }

        if last_remote.elapsed() >= POLL_REMOTE {
            last_remote = Instant::now();
            sync_now(jotbay, Event::Remote, &mut on_event);
            seen = fingerprint(&data);
        }
    }
}

fn sync_now(jotbay: &Jotbay, kind: Event, on_event: &mut impl FnMut(Event, Option<String>)) {
    match jotbay.sync() {
        // A sync that changed nothing is the common case and says nothing
        // worth saying; only report the ones that moved something.
        Ok(report) if report.did_nothing() => {}
        Ok(report) => on_event(kind, Some(report.summary())),
        // Never fatal. A remote that is unreachable now will be reachable
        // later, and a watcher that exits on the first failure is a watcher
        // that is not running when it matters.
        Err(e) => on_event(Event::Failed, Some(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("jotbay-watch-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.md"), "one").unwrap();
        std::fs::write(dir.join("sub/b.md"), "two").unwrap();
        dir
    }

    #[test]
    fn notices_new_edited_and_removed_files() {
        let dir = scratch("changes");
        let first = fingerprint(&dir);
        assert_eq!(first.len(), 2);

        std::fs::write(dir.join("c.md"), "three").unwrap();
        let added = fingerprint(&dir);
        assert_ne!(first, added);

        std::fs::write(dir.join("c.md"), "three and a half").unwrap();
        assert_ne!(added, fingerprint(&dir), "a size change must register");

        std::fs::remove_file(dir.join("c.md")).unwrap();
        assert_eq!(first, fingerprint(&dir));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ignores_dotfiles_so_the_watcher_cannot_trigger_itself() {
        // .git churns on every sync; counting it would mean each sync caused
        // the next one, forever.
        let dir = scratch("dotfiles");
        let before = fingerprint(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main").unwrap();
        std::fs::write(dir.join(".hidden"), "x").unwrap();
        assert_eq!(before, fingerprint(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
