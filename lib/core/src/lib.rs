//! The jotbay sync engine.
//!
//! Every front end. The CLI, the Tauri GUI, the macOS app, drives this crate
//! rather than reimplementing git handling. That is the whole point: the
//! conflict policy in particular is subtle enough that a second implementation
//! would drift from the first.

pub mod about;
pub mod browse;
pub mod changes;
pub mod conflict;
pub mod error;
pub mod git;
pub mod limits;
pub mod lock;
pub mod model;
pub mod notes;
pub mod presence;
pub mod proc;
pub mod schedule;
pub mod settings;
pub mod setup;
pub mod shortcut;
pub mod status;
pub mod sync;
pub mod update;
pub mod watch;

pub use error::{Error, Result};
pub use git::Git;
pub use model::*;

use std::path::{Path, PathBuf};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How long a machine may reasonably take to answer a roll call. Used only to
/// decide when a node counts as absent, at three times this value.
///
/// Matches `watch::POLL_REMOTE_MAX`: a machine that has been quiet for a while
/// checks the remote every five minutes, so it can take that long to notice it
/// was asked. Three of those, fifteen minutes of being asked and not
/// answering, is a machine that is genuinely not there.
///
/// Was 600, from when every machine synced on a ten-minute timer. Nothing has
/// run on that schedule since 1.6.0.
pub const SYNC_INTERVAL_SECS: i64 = 300;

pub struct Jotbay {
    git: Git,
}

impl Jotbay {
    /// Find Jotbay containing `start`, or fall back to `~/jotbay`.
    pub fn discover(start: Option<&Path>) -> Result<Self> {
        let start = match start {
            Some(p) => p.to_path_buf(),
            None => std::env::current_dir()?,
        };

        if let Ok(git) = Git::discover(&start) {
            return Ok(Self { git });
        }

        // Then whatever first-run setup recorded. This is the branch an
        // installed GUI always takes.
        if let Some(recorded) = settings::Settings::load().vault_path {
            let path = PathBuf::from(recorded);
            if path.join(".git").exists() {
                return Ok(Self { git: Git::new(path) });
            }
        }

        let fallback = default_root();
        if fallback.join(".git").exists() {
            return Ok(Self { git: Git::new(fallback) });
        }

        Err(Error::NotAJotbay(start))
    }

    /// Discovery for a windowed app.
    ///
    /// `discover` starts from the working directory, which is right for a CLI
    /// `cd` into another vault and it follows you. An installed app has no
    /// meaningful working directory: it is wherever the launcher happened to
    /// leave it. On Windows that was the install directory, so the app reported
    /// "no jotbay found at C:\Users\\AppData\Local\Jotbay" before setup and
    /// could bind to the wrong repository after it, if one happened to sit
    /// above the launch directory.
    ///
    /// So: what setup recorded, then the default location, and nothing else.
    pub fn for_app() -> Result<Self> {
        if let Some(recorded) = settings::Settings::load().vault_path {
            let path = PathBuf::from(recorded);
            if path.join(".git").exists() {
                return Ok(Self { git: Git::new(path) });
            }
        }
        let fallback = default_root();
        if fallback.join(".git").exists() {
            return Ok(Self { git: Git::new(fallback) });
        }
        Err(Error::NotAJotbay(fallback))
    }

    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        if !root.join(".git").exists() {
            return Err(Error::NotAJotbay(root));
        }
        Ok(Self { git: Git::new(root) })
    }

    pub fn git(&self) -> &Git {
        &self.git
    }

    pub fn root(&self) -> &Path {
        self.git.root()
    }

    /// The synced notes directory. The one users make shortcuts to.
    ///
    /// Discovered rather than assumed, because vaults come in two shapes.
    ///
    /// Notes used to live in `data/`, which put two folders between somebody
    /// and their note and named the second one after an implementation detail.
    /// It was never a technical boundary either: the watcher fingerprints the
    /// repository root and sync has always run `git add -A` from it, so `data/`
    /// only ever affected what the interface pointed at.
    ///
    /// New vaults are flat. Existing ones keep their `data/` and go on working
    /// on every version, which is the point of deciding this by looking rather
    /// than by version number. A machine running an older build against a vault
    /// that somebody has since flattened would otherwise report zero notes
    /// while syncing them perfectly well.
    ///
    /// Migrating is therefore a thing a person does to their files, not
    /// something an upgrade does to them: move the notes up, remove `data/`,
    /// and the next call here notices.
    pub fn data_dir(&self) -> PathBuf {
        let root = self.git.root();
        let nested = root.join("data");
        if has_visible_entries(&nested) {
            return nested;
        }
        root.to_path_buf()
    }

    /// This machine's name, as published to its status ref.
    ///
    /// macOS reports `Some-MacBook.local` via gethostname while nearly every
    /// other surface calls the same machine `some-macbook`. Trimming the mDNS
    /// suffix keeps one machine from appearing as two nodes if it is ever
    /// queried through a different path.
    pub fn hostname() -> String {
        let raw = gethostname::gethostname().to_string_lossy().to_string();
        raw.strip_suffix(".local").unwrap_or(&raw).to_string()
    }

    /// Snapshot of local state plus every node's published status.
    ///
    /// `refresh` controls whether the network is touched. The TUI passes false
    /// on its fast redraw path so a repaint never blocks on a fetch.
    pub fn status(&self, refresh: bool) -> Result<JotbayStatus> {
        if refresh {
            // Bounded like every other network call: this runs on the GUI's
            // refresh path, and an unbounded fetch there would freeze the
            // window for as long as the remote stayed silent.
            let _ = self
                .git
                .run_networked(&["fetch", "--quiet", "origin"], git::NETWORK_TIMEOUT);
            let _ = status::fetch_all(&self.git);
            // Only here, never on the offline repaint path: this is the one
            // call that already accepts network latency.
            update::refresh_remote();
        }

        let (ahead, behind) = self.git.ahead_behind()?;
        let rebase_in_progress = self.git.rebase_in_progress();

        Ok(JotbayStatus {
            root: self.git.root().to_string_lossy().to_string(),
            notes: self.data_dir().to_string_lossy().to_string(),
            branch: self.git.current_branch().unwrap_or_else(|_| "HEAD".into()),
            head: self.git.head().unwrap_or_default(),
            head_short: self.git.head_short().unwrap_or_default(),
            ahead,
            behind,
            dirty_files: self.git.dirty_files()?,
            rebase_in_progress,
            conflicts: if rebase_in_progress {
                self.git.conflicted_paths().unwrap_or_default()
            } else {
                Vec::new()
            },
            data_files: count_files(&self.data_dir()),
            warnings: limits::scan(&self.git).unwrap_or_default(),
            update_available: {
                let u = update::check(self.git.root());
                if u.available { u.latest } else { None }
            },
            nodes: status::read_all(&self.git).unwrap_or_default(),
        })
    }

    pub fn sync(&self) -> Result<SyncReport> {
        sync::run(self)
    }

    pub fn nodes(&self, refresh: bool) -> Result<Vec<NodeStatus>> {
        if refresh {
            status::fetch_all(&self.git)?;
        }
        status::read_all(&self.git)
    }

    pub fn forget_node(&self, hostname: &str) -> Result<()> {
        status::forget(&self.git, hostname)
    }

    /// Every machine's activity, merged newest-first.
    ///
    /// This is what actually happened, syncs that moved content, conflicts
    /// resolved, failures, as distinct from `log`, which is the commit
    /// history of what changed.
    pub fn activity(&self, refresh: bool, limit: usize) -> Result<Vec<ActivityEvent>> {
        if refresh {
            status::fetch_all(&self.git)?;
        }
        let mut events = status::read_all_events(&self.git)?;
        events.truncate(limit);
        Ok(events)
    }

    /// The feed as a person reads it: one line per change, not one per machine.
    pub fn changes(&self, refresh: bool, limit: usize) -> Result<Vec<crate::changes::Change>> {
        // Read wide, then fold. Grouping happens across machines, so taking
        // only `limit` raw events first would split a change whose reports
        // straddled the cut and show it twice.
        let events = self.activity(refresh, limit.saturating_mul(4).max(200))?;
        let mut folded = crate::changes::summarise(&events);
        folded.truncate(limit);
        Ok(folded)
    }

    // --- notes: finding them, and seeing them over time ---------------------
    //
    // Thin on purpose. Each one pairs the notes directory with the repository,
    // which is the one thing a caller would otherwise have to work out, and
    // getting it wrong is how the folder button broke when vaults went flat.

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<notes::Hit>> {
        notes::search(&self.git, &self.data_dir(), query, limit)
    }

    pub fn history(&self, rel: &str, limit: u32) -> Result<Vec<notes::Version>> {
        notes::history(&self.git, &self.data_dir(), rel, limit)
    }

    pub fn version_content(&self, rel: &str, sha: &str) -> Result<String> {
        notes::version_content(&self.git, &self.data_dir(), rel, sha)
    }

    pub fn restore_version(&self, rel: &str, sha: &str) -> Result<std::path::PathBuf> {
        notes::restore(&self.git, &self.data_dir(), rel, sha)
    }

    pub fn deleted_notes(&self, limit: u32) -> Result<Vec<notes::Deleted>> {
        notes::deleted(&self.git, &self.data_dir(), limit)
    }

    pub fn restore_deleted(&self, rel: &str, sha: &str) -> Result<std::path::PathBuf> {
        notes::restore_deleted(&self.git, &self.data_dir(), rel, sha)
    }

    pub fn create_note(&self, name: &str, body: &str) -> Result<std::path::PathBuf> {
        notes::create(&self.data_dir(), name, body)
    }

    pub fn append_note(&self, rel: &str, text: &str) -> Result<std::path::PathBuf> {
        notes::append(&self.data_dir(), rel, text)
    }

    pub fn open_note_externally(&self, rel: &str) -> Result<()> {
        notes::open_externally(&self.data_dir().join(rel))
    }

    pub fn log(&self, limit: u32) -> Result<Vec<CommitInfo>> {
        // Unit separator between fields, record separator between commits:
        // neither can appear in a commit subject, unlike any printable choice.
        let fmt = "--pretty=format:%H\x1f%h\x1f%s\x1f%an\x1f%ad\x1e";
        let out = self.git.run(&[
            "log",
            &format!("-{limit}"),
            "--date=format:%Y-%m-%d %H:%M",
            fmt,
        ])?;

        Ok(out
            .split('\x1e')
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .filter_map(|record| {
                let f: Vec<&str> = record.split('\x1f').collect();
                if f.len() < 5 {
                    return None;
                }
                Some(CommitInfo {
                    sha: f[0].to_string(),
                    short: f[1].to_string(),
                    subject: f[2].to_string(),
                    author: f[3].to_string(),
                    timestamp: f[4].to_string(),
                    node: parse_node(f[2]),
                })
            })
            .collect())
    }

    /// Whether a newer release is known, from the marker the sync already
    /// carries. No network call.
    pub fn update_status(&self) -> update::UpdateStatus {
        update::check(self.root())
    }

    /// Fetch that release and replace this machine's binaries.
    /// Upgrade this machine, whatever it was installed with.
    ///
    /// One call for every surface. It works out whether these files belong to
    /// Homebrew, to apt, to the Windows installer or to nobody, does the right
    /// thing, and restarts the background sync so the machine is actually
    /// running what was installed rather than merely holding it on disk.
    pub fn upgrade(&self) -> Result<update::Outcome> {
        let status = self.update_status();
        let version = status
            .latest
            .ok_or_else(|| Error::Other("no release marker in the repository yet".into()))?;
        if !status.available {
            return Err(Error::Other(format!("already on {}", status.current)));
        }
        update::perform(self.root(), &version)
    }

    /// Remember this vault as the one this machine uses.
    pub fn remember(&self) -> Result<()> {
        let mut s = settings::Settings::load();
        s.vault_path = Some(self.root().to_string_lossy().to_string());
        s.save()
    }

    /// Whether any vault is reachable, without constructing one.
    pub fn exists() -> bool {
        Self::discover(None).is_ok()
    }

    pub fn abort_rebase(&self) -> Result<()> {
        self.git.try_run(&["rebase", "--abort"])?;
        Ok(())
    }
}

/// Automated commits are `jotbay: <hostname> <date>`; anything else is a human.
fn parse_node(subject: &str) -> Option<String> {
    subject
        .strip_prefix("jotbay: ")
        .and_then(|rest| rest.split_whitespace().next())
        .map(str::to_string)
}

pub fn default_root() -> PathBuf {
    home().join("jotbay")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Whether a directory holds anything a person would call a note.
///
/// `data_dir` asks this rather than testing for the directory, because
/// existence alone is too weak a signal. Flattening this machine's own vault
/// left `data/` behind holding nothing but an ignored `.remember/` from another
/// tool, and a bare `is_dir` check would have kept pointing the whole interface
/// at it. Any tool that ever creates a `data/` folder in somebody's notes would
/// have done the same, silently, long after the migration.
fn has_visible_entries(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        !e.file_name().to_string_lossy().starts_with('.')
    })
}

fn count_files(dir: &Path) -> u32 {
    fn walk(dir: &Path, n: &mut u32) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            // Dot directories are skipped, not just dotfiles. This counts the
            // notes directory, and once that can be the repository root, a walk
            // that descends into `.git` reports several thousand loose objects
            // as somebody's notes.
            if path.file_name().is_some_and(|f| f.to_string_lossy().starts_with('.')) {
                continue;
            }
            if path.is_dir() {
                walk(&path, n);
            } else if !path.file_name().is_some_and(|f| f.to_string_lossy().starts_with('.')) {
                *n += 1;
            }
        }
    }
    let mut n = 0;
    walk(dir, &mut n);
    n
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_vault_with_a_data_directory_still_points_at_it() {
        // Existing vaults keep working untouched, on every version. That is
        // what makes migrating optional rather than something an upgrade does
        // to somebody's files.
        let dir = std::env::temp_dir().join("jotbay-layout-nested");
        let _ = std::fs::remove_dir_all(&dir);
        // With a note in it, as every real nested vault has. An empty data/
        // says nothing about where the notes are, so it is treated as flat.
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("data/note.md"), b"hello").unwrap();
        let v = Jotbay { git: crate::git::Git::new(&dir) };
        assert_eq!(v.data_dir(), dir.join("data"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_data_folder_holding_only_hidden_files_does_not_count() {
        // What flattening this machine's vault actually produced: data/ could
        // not be removed because an ignored .remember/ from another tool was
        // sitting in it. Testing for the directory alone would have kept the
        // entire interface pointed at an empty folder, and any tool that
        // creates a `data/` in somebody's notes would do the same later.
        let dir = std::env::temp_dir().join("jotbay-layout-hidden");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("data/.remember")).unwrap();
        std::fs::write(dir.join("data/.DS_Store"), b"x").unwrap();
        std::fs::write(dir.join("a-note.md"), b"hello").unwrap();

        let v = Jotbay { git: crate::git::Git::new(&dir) };
        assert_eq!(v.data_dir(), dir, "an all-hidden data/ captured the vault");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_flat_vault_points_at_its_root() {
        let dir = std::env::temp_dir().join("jotbay-layout-flat");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let v = Jotbay { git: crate::git::Git::new(&dir) };
        assert_eq!(v.data_dir(), dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn counting_notes_never_descends_into_git() {
        // Once the notes directory can be the repository root, a walk that
        // recurses into dot directories reports several thousand loose objects
        // as somebody's notes. The count is the number on the front of the
        // window, so it would be wrong in the most visible place there is.
        let dir = std::env::temp_dir().join("jotbay-layout-count");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git/objects/ab")).unwrap();
        std::fs::write(dir.join(".git/objects/ab/deadbeef"), b"x").unwrap();
        std::fs::write(dir.join(".git/config"), b"x").unwrap();
        std::fs::write(dir.join("a-note.md"), b"hello").unwrap();
        std::fs::create_dir_all(dir.join("topic")).unwrap();
        std::fs::write(dir.join("topic/another.md"), b"hello").unwrap();

        assert_eq!(count_files(&dir), 2, "counted repository internals as notes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::*;

    #[test]
    fn parses_node_from_automated_subjects() {
        assert_eq!(parse_node("jotbay: workstation 2026-08-01 14:19"), Some("workstation".into()));
        assert_eq!(parse_node("Add a note about DNS"), None);
        assert_eq!(parse_node("jotbay: "), None);
    }
}
