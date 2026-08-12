use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// What a single machine publishes about itself to
/// `refs/jotbay-status/<hostname>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStatus {
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub agent_version: String,
    #[serde(with = "time::serde::rfc3339")]
    pub last_sync: OffsetDateTime,
    pub head: String,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: u32,
    pub conflicts_resolved: u32,
    pub last_error: Option<String>,
    /// The last failure was the machine being unable to reach the remote,
    /// rather than anything being wrong with it.
    ///
    /// Published alongside `last_error` rather than replacing it: the text is
    /// still worth keeping, and a node running an older agent that never sets
    /// this simply reads as false, which is what it meant before.
    #[serde(default)]
    pub offline: bool,
    /// Read-time annotation, not part of what a node publishes about itself:
    /// true when this node's head is an ancestor of the local head, i.e. the
    /// node is merely behind and will catch up on its next pull. Filled in by
    /// `status::read_all`; meaningless (always false) in a published record,
    /// because "behind" is only defined relative to the machine reading it.
    #[serde(default)]
    pub behind_local: bool,
}

impl NodeStatus {
    /// A node counts as absent once it has missed several roll calls.
    ///
    /// Derived at read time rather than stored, so the threshold can change
    /// without rewriting what every node published.
    ///
    /// This is a weaker claim than it used to be, and deliberately so. It once
    /// meant "has not synced recently", which worked only because every machine
    /// synced on a fixed timer whether or not it had anything to do. Idle
    /// machines no longer sync, so absence of syncing stopped meaning absence
    /// of machine. A healthy fleet with nothing to do reported itself
    /// entirely offline. Opening a window now asks the others to report in, so
    /// what this measures is a machine that was asked and did not answer.
    pub fn is_stale(&self, interval_secs: i64) -> bool {
        let age = (OffsetDateTime::now_utc() - self.last_sync).whole_seconds();
        age > interval_secs * 3
    }

    pub fn age_secs(&self) -> i64 {
        (OffsetDateTime::now_utc() - self.last_sync).whole_seconds()
    }

    pub fn health(&self, interval_secs: i64, local_head: &str) -> NodeHealth {
        if self.offline {
            // Checked before `last_error`, which is also set: a machine that
            // cannot reach the network is not a machine with a problem, and
            // showing it in red trains people to ignore red.
            NodeHealth::Offline
        } else if self.last_error.is_some() {
            NodeHealth::Error
        } else if self.is_stale(interval_secs) {
            NodeHealth::Stale
        } else if self.head != local_head {
            // "Diverged" used to cover every head mismatch, which overstated
            // the common case: right after this machine pushes, every peer
            // shows a different head purely because it has not pulled yet.
            if self.behind_local {
                NodeHealth::Behind
            } else {
                NodeHealth::Diverged
            }
        } else {
            NodeHealth::Healthy
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeHealth {
    Healthy,
    /// Strictly behind the local head; resolves itself on the node's next pull.
    Behind,
    /// Holds commits the local head does not, needs a sync to reconcile.
    Diverged,
    Stale,
    /// Reachable and working, but currently without a route to the remote.
    Offline,
    Error,
}

impl NodeHealth {
    pub fn glyph(&self) -> &'static str {
        match self {
            NodeHealth::Healthy => "●",
            NodeHealth::Behind => "◑",
            NodeHealth::Diverged => "◐",
            NodeHealth::Stale => "○",
            NodeHealth::Offline => "◌",
            NodeHealth::Error => "✖",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            NodeHealth::Healthy => "in sync",
            NodeHealth::Behind => "behind",
            NodeHealth::Diverged => "diverged",
            NodeHealth::Stale => "not answering",
            NodeHealth::Offline => "offline",
            NodeHealth::Error => "error",
        }
    }
}

/// A snapshot of the local repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JotbayStatus {
    pub root: String,
    /// Where the notes actually are, which is not always `root`.
    ///
    /// Published rather than left for each interface to work out, because the
    /// macOS app worked it out separately by appending "data" to the root. When
    /// vaults became flat that button pointed at a directory which no longer
    /// existed, and NSWorkspace declines to open a missing path without saying
    /// anything, so the folder button simply stopped doing anything.
    ///
    /// One place decides the layout. Everything else is told.
    #[serde(default)]
    pub notes: String,
    pub branch: String,
    pub head: String,
    pub head_short: String,
    pub ahead: u32,
    pub behind: u32,
    pub dirty_files: Vec<String>,
    pub rebase_in_progress: bool,
    pub conflicts: Vec<String>,
    pub data_files: u32,
    #[serde(default)]
    pub warnings: Vec<crate::limits::FileWarning>,
    /// Set when the repository's release marker names a newer version than
    /// this binary. Costs nothing to compute. The marker already synced.
    #[serde(default)]
    pub update_available: Option<String>,
    pub nodes: Vec<NodeStatus>,
}

impl JotbayStatus {
    pub fn is_clean(&self) -> bool {
        self.dirty_files.is_empty() && self.ahead == 0 && self.behind == 0 && !self.rebase_in_progress
    }
}

/// What one `sync` pass did.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncReport {
    pub committed: bool,
    /// How many files went into that commit. Captured before staging, so the
    /// feed can say "committed 3 files" instead of "committed local changes".
    #[serde(default)]
    pub committed_files: u32,
    /// Every path this pass moved, whether committed here or arriving in a
    /// pull. Taken from the head delta rather than the staging list, so it
    /// answers "what changed" the same way regardless of which side changed it.
    #[serde(default)]
    pub changed_paths: Vec<String>,
    pub commit_message: Option<String>,
    pub pulled: u32,
    pub pushed: bool,
    pub conflicts: Vec<ConflictResolution>,
    pub head_short: String,
    pub skipped_locked: bool,
    /// Files at or over the advisory size threshold. Anything marked
    /// `Blocked` was deliberately left unstaged.
    #[serde(default)]
    pub warnings: Vec<crate::limits::FileWarning>,
}

impl SyncReport {
    pub fn did_nothing(&self) -> bool {
        !self.committed && self.pulled == 0 && !self.pushed && self.conflicts.is_empty()
    }

    /// One line describing what happened, in the words a non-git user needs.
    ///
    /// Lives here rather than in each front end because it was written three
    /// times, once per UI, and the three had already drifted apart on
    /// pluralisation and on whether "committed" was a word worth showing
    /// somebody who never asked for a commit.
    pub fn summary(&self) -> String {
        if self.skipped_locked {
            return "Another sync is already running".into();
        }
        if self.did_nothing() {
            return "Everything is up to date".into();
        }

        let mut parts = Vec::new();
        if self.committed || self.pushed {
            parts.push("Saved your changes".to_string());
        }
        if self.pulled > 0 {
            parts.push(format!(
                "brought in {} update{}",
                self.pulled,
                if self.pulled == 1 { "" } else { "s" }
            ));
        }
        if !self.conflicts.is_empty() {
            parts.push(format!(
                "kept both versions of {} file{}",
                self.conflicts.len(),
                if self.conflicts.len() == 1 { "" } else { "s" }
            ));
        }
        let mut text = parts.join(", ");
        if let Some(first) = text.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        text
    }
}

/// One file whose two versions were both preserved.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictResolution {
    /// The path that kept its canonical name (the upstream version).
    pub path: String,
    /// Where the local version was written, if one existed.
    pub kept_copy: Option<String>,
    pub kind: ConflictKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// Both sides changed the file. Both versions retained.
    BothModified,
    /// Upstream deleted it, we changed it. Our version retained.
    DeletedUpstream,
    /// We deleted it, upstream changed it. Upstream's version retained.
    DeletedLocally,
}

/// One thing that happened on one machine.
///
/// Recorded per sync, and only when the sync actually did something. A no-op
/// check every ten minutes on six machines would bury the events that matter.
/// "Nothing happened but I am alive" is already carried by `NodeStatus`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEvent {
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub hostname: String,
    pub kind: EventKind,
    /// One short human sentence. Always safe to show.
    pub summary: String,
    /// The paths this event touched, so a row can be expanded to answer
    /// "pushed 2 files, which two?" without going to the commit log.
    #[serde(default)]
    pub files: Vec<String>,
    /// How many times in a row this machine reported exactly this.
    ///
    /// One condition, one entry. A failure retried on the watcher's 20 second
    /// poll used to append a record per attempt: one real incident lasting
    /// eleven minutes wrote 34 identical events, which is 68% of everything
    /// this node can remember, and it permanently evicted the history behind
    /// it. Counting is the difference between "this is still happening" and
    /// "your history is gone".
    #[serde(default = "one")]
    pub repeats: u32,
    /// When the run started, as against `at`, which tracks the latest report.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub first_at: Option<OffsetDateTime>,
    /// The raw underlying text, when there is one, full git stderr for a
    /// failure. Shown only in verbose mode: a push rejected for a private
    /// email produced five lines of `remote: error: ` that told a first-time
    /// user nothing and swamped the pane.
    #[serde(default)]
    pub detail: Option<String>,
    pub head: String,
}

impl ActivityEvent {
    pub fn age_secs(&self) -> i64 {
        (OffsetDateTime::now_utc() - self.at).whole_seconds()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// Content moved: committed, pulled, pushed, or some combination.
    Changed,
    /// A file was edited on two machines and both versions were kept.
    Conflict,
    /// The sync failed.
    Error,
    /// The machine could not reach the remote. Not a failure of anything on
    /// it, and it resolves itself when the network comes back.
    Offline,
}

impl EventKind {
    pub fn glyph(&self) -> &'static str {
        match self {
            EventKind::Changed => "↕",
            EventKind::Conflict => "⚠",
            EventKind::Error => "✖",
            EventKind::Offline => "◌",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            EventKind::Changed => "changed",
            EventKind::Conflict => "conflict",
            EventKind::Error => "error",
            EventKind::Offline => "offline",
        }
    }
}

fn one() -> u32 { 1 }

/// How many events each machine keeps. The feed merges every machine's buffer,
/// so the visible history is this times the number of machines.
pub const MAX_EVENTS_PER_NODE: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    pub timestamp: String,
    /// Parsed out of automated commit subjects of the form `jotbay: <host> `.
    pub node: Option<String>,
}
