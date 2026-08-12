//! The activity feed as a person reads it, rather than as machines report it.
//!
//! Each machine publishes what *it* did: committed, pushed, pulled. That is the
//! honest record and it has to stay, because the point of the feed is that a
//! machine which is failing says so itself. But it means one edited note
//! produces one event per machine, and a fleet of four turns a single change
//! into four lines that all say the same thing happened.
//!
//! Measured on a real vault: 61% of every entry was a machine announcing it had
//! received someone else's work, and three quarters of all changes appeared two
//! to four times.
//!
//! So this groups the raw events by the commit they describe. One change, one
//! line, naming the machine that made it and how many have it. The raw view is
//! still there behind a setting, because when something is wrong the mechanics
//! are exactly what you want.

use serde::Serialize;
use std::collections::BTreeMap;
use time::OffsetDateTime;

use crate::model::{ActivityEvent, EventKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// Notes moved. The ordinary case, and the only one that is good news.
    Updated,
    /// Both versions of a file were kept.
    Conflict,
    /// A machine could not reach the remote.
    Offline,
    /// Something needs a person.
    Problem,
}

impl ChangeKind {
    pub fn glyph(&self) -> &'static str {
        match self {
            ChangeKind::Updated => "✎",
            ChangeKind::Conflict => "⚠",
            ChangeKind::Offline => "◌",
            ChangeKind::Problem => "✖",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Change {
    /// Most recent report of this change.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub kind: ChangeKind,
    /// One line, written for a person: "notes.md updated".
    pub summary: String,
    pub files: Vec<String>,
    /// The machine that made the change, when one can be identified. A change
    /// this machine only received has an origin somewhere else.
    pub origin: Option<String>,
    /// Every machine that has reported this change, origin included.
    pub machines: Vec<String>,
    /// How many times the underlying condition repeated. Only ever above one
    /// for a problem that kept happening.
    pub repeats: u32,
    pub detail: Option<String>,
}

/// Fold raw per-machine events into what actually happened.
pub fn summarise(events: &[ActivityEvent]) -> Vec<Change> {
    // Commits are the join. Two machines reporting the same head are two
    // reports of one change, whoever made it.
    let mut by_commit: BTreeMap<&str, Vec<&ActivityEvent>> = BTreeMap::new();
    let mut standalone: Vec<&ActivityEvent> = Vec::new();

    for e in events {
        match e.kind {
            // Grouped only when there is a commit to group on. An event with
            // no head cannot be matched to anything and would otherwise all
            // collapse into one bogus group.
            EventKind::Changed if !e.head.is_empty() => {
                by_commit.entry(e.head.as_str()).or_default().push(e)
            }
            _ => standalone.push(e),
        }
    }

    let mut out: Vec<Change> = Vec::new();

    for (_head, group) in by_commit {
        // The machine that committed is the author; everyone else pulled it.
        // Identified by what it said it did rather than by comparing heads,
        // because after a pull every machine's head is the same.
        let author = group
            .iter()
            .find(|e| e.summary.contains("committed"))
            .map(|e| e.hostname.clone());

        let mut machines: Vec<String> = group.iter().map(|e| e.hostname.clone()).collect();
        machines.sort();
        machines.dedup();

        // The widest file list any machine reported. A machine that pulled two
        // commits at once names both sets; one that pulled a single commit
        // names fewer. Taking the longest avoids under-reporting the change.
        let files = group
            .iter()
            .map(|e| e.files.clone())
            .max_by_key(|f| f.len())
            .unwrap_or_default();

        let newest = group.iter().max_by_key(|e| e.at).unwrap();

        out.push(Change {
            at: newest.at,
            kind: ChangeKind::Updated,
            summary: describe_files(&files, &group[0].summary),
            files,
            origin: author,
            machines,
            repeats: 1,
            detail: None,
        });
    }

    for e in standalone {
        let kind = match e.kind {
            EventKind::Conflict => ChangeKind::Conflict,
            EventKind::Offline => ChangeKind::Offline,
            EventKind::Error => ChangeKind::Problem,
            // A Changed event with no head: keep it rather than drop it.
            EventKind::Changed => ChangeKind::Updated,
        };
        out.push(Change {
            at: e.at,
            kind,
            summary: if kind == ChangeKind::Updated {
                describe_files(&e.files, &e.summary)
            } else {
                e.summary.clone()
            },
            files: e.files.clone(),
            origin: Some(e.hostname.clone()),
            machines: vec![e.hostname.clone()],
            repeats: e.repeats,
            detail: e.detail.clone(),
        });
    }

    out.sort_by(|a, b| b.at.cmp(&a.at));
    merge_editing_runs(out)
}

/// How close two edits of the same files have to be to count as one session.
///
/// Saving a note twice while working on it makes two commits, and both are
/// real. But two lines a minute apart saying "imac-check.md updated" read as
/// the feed repeating itself, which is the complaint this whole module exists
/// to answer. Beyond this gap they are treated as separate pieces of work.
const SESSION: time::Duration = time::Duration::minutes(5);

/// Fold runs of the same thing into one line.
///
/// Two rules, for two different kinds of repetition.
///
/// Edits: the same files saved twice inside a short window are one piece of
/// work, not two.
///
/// Problems: the same machine reporting the same condition again is that
/// condition still being true. `push_event` now collapses these as they are
/// written, but every buffer already in the wild was filled before that
/// existed, so the same fold has to happen on read or history stays broken
/// until it churns out. On the vault this was written against, that is 44
/// error records describing two incidents.
fn merge_editing_runs(changes: Vec<Change>) -> Vec<Change> {
    let mut out: Vec<Change> = Vec::with_capacity(changes.len());
    for c in changes {
        // Sorted newest first, so `last` is the one just before this in time.
        if let Some(prev) = out.last_mut() {
            let same_condition = prev.kind != ChangeKind::Updated
                && prev.kind == c.kind
                && prev.summary == c.summary
                && prev.origin == c.origin;
            if same_condition {
                prev.repeats = prev.repeats.saturating_add(c.repeats);
                continue;
            }

            let same_files = prev.files == c.files && !c.files.is_empty();
            let both_updates = prev.kind == ChangeKind::Updated && c.kind == ChangeKind::Updated;
            if both_updates && same_files && (prev.at - c.at) <= SESSION {
                for m in c.machines {
                    if !prev.machines.contains(&m) {
                        prev.machines.push(m);
                    }
                }
                prev.machines.sort();
                // Keep the earliest author seen; `prev.at` already holds the
                // most recent time, which is the one a reader cares about.
                if prev.origin.is_none() {
                    prev.origin = c.origin;
                }
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// "notes.md updated", "3 files updated".
///
/// Falls back to whatever the machine said when it named no files, rather than
/// inventing a count of zero.
fn describe_files(files: &[String], fallback: &str) -> String {
    match files.len() {
        0 => fallback.to_string(),
        1 => {
            let name = files[0].rsplit('/').next().unwrap_or(&files[0]);
            format!("{name} updated")
        }
        n => format!("{n} files updated"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(host: &str, head: &str, summary: &str, files: &[&str], secs: i64) -> ActivityEvent {
        ActivityEvent {
            at: OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(secs),
            hostname: host.into(),
            kind: EventKind::Changed,
            summary: summary.into(),
            files: files.iter().map(|s| s.to_string()).collect(),
            detail: None,
            head: head.into(),
            repeats: 1,
            first_at: None,
        }
    }

    #[test]
    fn one_edit_seen_by_four_machines_is_one_line() {
        // Exactly the shape found in the real vault: the author commits, the
        // other three each announce that they pulled it.
        let events = vec![
            ev("mac", "abc123", "committed 1 file · pushed", &["measurements.md"], 100),
            ev("win", "abc123", "pulled 1 commit", &["measurements.md"], 110),
            ev("linux", "abc123", "pulled 1 commit", &["measurements.md"], 120),
            ev("imac", "abc123", "pulled 1 commit", &["measurements.md"], 130),
        ];
        let changes = summarise(&events);
        assert_eq!(changes.len(), 1, "four reports, one change");
        let c = &changes[0];
        assert_eq!(c.summary, "measurements.md updated");
        assert_eq!(c.origin.as_deref(), Some("mac"), "the machine that committed");
        assert_eq!(c.machines.len(), 4);
        assert_eq!(c.at, OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(130));
    }

    #[test]
    fn separate_commits_stay_separate() {
        let events = vec![
            ev("mac", "aaa", "committed 1 file · pushed", &["a.md"], 100),
            ev("mac", "bbb", "committed 1 file · pushed", &["b.md"], 200),
        ];
        assert_eq!(summarise(&events).len(), 2);
    }

    #[test]
    fn saving_the_same_note_twice_in_a_minute_is_one_line() {
        // Observed in the real vault: the same file, two commits 40 seconds
        // apart, reported by three machines each. Six events, one edit.
        let events = vec![
            ev("mac", "aaa", "committed 1 file · pushed", &["imac-check.md"], 0),
            ev("win", "aaa", "pulled 1 commit", &["imac-check.md"], 5),
            ev("mac", "bbb", "committed 1 file · pushed", &["imac-check.md"], 40),
            ev("win", "bbb", "pulled 1 commit", &["imac-check.md"], 45),
        ];
        let changes = summarise(&events);
        assert_eq!(changes.len(), 1, "one editing session");
        assert_eq!(changes[0].summary, "imac-check.md updated");
        assert_eq!(changes[0].machines.len(), 2);
        assert_eq!(changes[0].at, OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(45));
    }

    #[test]
    fn the_same_note_edited_hours_later_is_a_new_line() {
        let events = vec![
            ev("mac", "aaa", "committed 1 file · pushed", &["notes.md"], 0),
            ev("mac", "bbb", "committed 1 file · pushed", &["notes.md"], 3 * 3600),
        ];
        assert_eq!(summarise(&events).len(), 2, "two separate pieces of work");
    }

    #[test]
    fn history_written_before_coalescing_existed_is_folded_on_read() {
        // Buffers already in the wild hold one record per retry. Reading them
        // has to produce one line, or the fix does nothing until they churn.
        let mut events = vec![];
        for i in 0..34 {
            let mut e = ev("mac", "", "Push rejected: your commit email is private.", &[], i * 20);
            e.kind = EventKind::Error;
            events.push(e);
        }
        let changes = summarise(&events);
        assert_eq!(changes.len(), 1, "34 records, one incident");
        assert_eq!(changes[0].repeats, 34);
    }

    #[test]
    fn two_machines_with_the_same_problem_stay_separate() {
        // Same message, different machines: two problems, and folding them
        // would hide one of them entirely.
        let mut a = ev("mac", "", "Push rejected.", &[], 10);
        a.kind = EventKind::Error;
        let mut b = ev("win", "", "Push rejected.", &[], 20);
        b.kind = EventKind::Error;
        assert_eq!(summarise(&[a, b]).len(), 2);
    }

    #[test]
    fn different_files_are_never_merged() {
        let events = vec![
            ev("mac", "aaa", "committed 1 file · pushed", &["a.md"], 0),
            ev("mac", "bbb", "committed 1 file · pushed", &["b.md"], 10),
        ];
        assert_eq!(summarise(&events).len(), 2);
    }

    #[test]
    fn counts_files_rather_than_naming_all_of_them() {
        let events = vec![ev("mac", "abc", "committed 3 files · pushed", &["a.md", "b.md", "c.md"], 1)];
        assert_eq!(summarise(&events)[0].summary, "3 files updated");
    }

    #[test]
    fn a_repeating_problem_keeps_its_count_and_stays_its_own_line() {
        let mut e = ev("mac", "abc", "Push rejected: your commit email is private.", &[], 10);
        e.kind = EventKind::Error;
        e.repeats = 34;
        let changes = summarise(&[e]);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Problem);
        assert_eq!(changes[0].repeats, 34);
        assert_eq!(changes[0].summary, "Push rejected: your commit email is private.");
    }

    #[test]
    fn events_without_a_commit_do_not_all_merge_into_one() {
        // Two unrelated failures, neither carrying a head. Grouping on an empty
        // key would have folded them into a single misleading line.
        let mut a = ev("mac", "", "Offline.", &[], 10);
        a.kind = EventKind::Offline;
        let mut b = ev("win", "", "Offline.", &[], 20);
        b.kind = EventKind::Offline;
        assert_eq!(summarise(&[a, b]).len(), 2);
    }
}
