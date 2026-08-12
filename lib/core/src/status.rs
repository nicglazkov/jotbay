//! Per-node status published to `refs/jotbay-status/<hostname>`.
//!
//! Each machine owns exactly one ref, so two nodes can never conflict and no
//! heartbeat ever lands on `main`. The branch carries content and nothing
//! else. Reading every node's state costs one fetch.

use crate::error::{Error, Result};
use crate::git::Git;
use crate::model::{ActivityEvent, EventKind, NodeStatus, MAX_EVENTS_PER_NODE};

pub const REFSPEC: &str = "+refs/jotbay-status/*:refs/jotbay-status/*";
const STATUS_FILE: &str = "status.json";
const EVENTS_FILE: &str = "events.json";

pub fn ref_name(hostname: &str) -> String {
    format!("refs/jotbay-status/{}", sanitize(hostname))
}

/// Git refs cannot contain spaces, `~^:?*[`, or a trailing `.lock`.
fn sanitize(hostname: &str) -> String {
    hostname
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
            _ => '-',
        })
        .collect::<String>()
        .trim_matches('.')
        .to_lowercase()
}

/// Publish this node's status and its event buffer. Always a force-push: the
/// ref is a scratch pointer, not history, so there is nothing to preserve.
pub fn publish(git: &Git, status: &NodeStatus, events: &[ActivityEvent]) -> Result<()> {
    let status_json = serde_json::to_vec_pretty(status)?;
    let events_json = serde_json::to_vec_pretty(events)?;

    let status_blob = git.run_with_stdin(&["hash-object", "-w", "--stdin"], &status_json)?;
    let events_blob = git.run_with_stdin(&["hash-object", "-w", "--stdin"], &events_json)?;

    // mktree wants entries in sorted order; "events.json" sorts before
    // "status.json", so this listing is already correct.
    let tree_entries = format!(
        "100644 blob {events_blob}\t{EVENTS_FILE}\n100644 blob {status_blob}\t{STATUS_FILE}\n"
    );
    let tree = git.run_with_stdin(&["mktree"], tree_entries.as_bytes())?;

    // An explicit identity keeps this working on machines where user.name is
    // unset, such as a fresh CI runner.
    let commit = git.run(&[
        "-c",
        "user.name=jotbay-agent",
        "-c",
        "user.email=jotbay-agent@localhost",
        "-c",
        "commit.gpgsign=false",
        "commit-tree",
        &tree,
        "-m",
        "jotbay status",
    ])?;

    let refname = ref_name(&status.hostname);
    git.run(&["update-ref", &refname, &commit])?;
    // Bounded: this is the exact call that once sat for four minutes inside a
    // scheduled sync. Publishing status is best-effort, so a failure here is
    // not worth aborting over, but it must not hang the scheduler either.
    let out = git.run_networked(
        &["push", "--quiet", "--force", "origin", &format!("{refname}:{refname}")],
        crate::git::NETWORK_TIMEOUT,
    )?;
    if out.timed_out {
        return Err(Error::Other(out.describe("publishing this machine's status")));
    }
    Ok(())
}

/// Fetch every node's status ref. Best-effort: being offline is not an error
/// worth aborting a sync over.
pub fn fetch_all(git: &Git) -> Result<()> {
    let out = git.run_networked(
        &["fetch", "--quiet", "origin", REFSPEC],
        crate::git::NETWORK_TIMEOUT,
    )?;
    if out.timed_out {
        return Err(Error::Other(out.describe("fetching machine status")));
    }
    Ok(())
}

/// Read every node's published status, newest first.
pub fn read_all(git: &Git) -> Result<Vec<NodeStatus>> {
    let refs = git.run(&["for-each-ref", "--format=%(refname)", "refs/jotbay-status/"])?;
    let local_head = git.run(&["rev-parse", "HEAD"]).unwrap_or_default();
    let mut nodes = Vec::new();

    for refname in refs.lines().filter(|l| !l.is_empty()) {
        // A ref that fails to parse is a node running an incompatible version.
        // Skip it rather than failing the whole listing.
        if let Ok(bytes) = git.run_bytes(&["show", &format!("{refname}:{STATUS_FILE}")]) {
            if let Ok(mut node) = serde_json::from_slice::<NodeStatus>(&bytes) {
                // Annotate "merely behind" vs genuinely diverged, so a peer
                // that simply has not pulled this machine's latest push is not
                // reported with the same urgency as one holding unmerged work.
                node.behind_local = !local_head.is_empty()
                    && node.head != local_head
                    && git
                        .try_run(&["merge-base", "--is-ancestor", &node.head, &local_head])
                        .unwrap_or(false);
                nodes.push(node);
            }
        }
    }

    nodes.sort_by(|a, b| b.last_sync.cmp(&a.last_sync));
    Ok(nodes)
}

/// This machine's own event buffer, as last published. Read before appending so
/// the buffer survives across syncs.
pub fn read_own_events(git: &Git, hostname: &str) -> Vec<ActivityEvent> {
    let refname = ref_name(hostname);
    git.run_bytes(&["show", &format!("{refname}:{EVENTS_FILE}")])
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Every machine's events, merged into one feed, newest first.
pub fn read_all_events(git: &Git) -> Result<Vec<ActivityEvent>> {
    let refs = git.run(&["for-each-ref", "--format=%(refname)", "refs/jotbay-status/"])?;
    let mut events = Vec::new();

    for refname in refs.lines().filter(|l| !l.is_empty()) {
        // A ref without events.json is a machine still running the older
        // agent; skip it rather than failing the whole feed.
        if let Ok(bytes) = git.run_bytes(&["show", &format!("{refname}:{EVENTS_FILE}")]) {
            if let Ok(mut node_events) = serde_json::from_slice::<Vec<ActivityEvent>>(&bytes) {
                events.append(&mut node_events);
            }
        }
    }

    events.sort_by(|a, b| b.at.cmp(&a.at));
    Ok(events)
}

/// Append an event to a buffer, keeping it bounded.
pub fn push_event(events: &mut Vec<ActivityEvent>, event: ActivityEvent) {
    // One condition, one entry, whatever the condition is.
    //
    // This began as a special case for being offline and should never have
    // been one. Every repeating state has the same shape: the watcher retries
    // on its poll interval, the attempt fails the same way, and a record is
    // written. A real push rejection that lasted eleven minutes wrote 34
    // identical events here, 68% of everything that node can remember, and
    // evicted the genuine history behind it. The buffer holds
    // MAX_EVENTS_PER_NODE, so roughly seventeen minutes of any repeating
    // failure erases a machine's entire past.
    //
    // Matched on kind and summary rather than on the raw detail, because the
    // detail carries a timestamp or a transient host and would never compare
    // equal. Changed events are excluded: two identical-looking syncs are two
    // real things that moved, and folding them would lose work.
    if event.kind != EventKind::Changed {
        if let Some(newest) = events.iter_mut().max_by_key(|e| e.at) {
            if newest.kind == event.kind && newest.summary == event.summary {
                newest.first_at = newest.first_at.or(Some(newest.at));
                newest.at = event.at;
                newest.head = event.head;
                newest.detail = event.detail;
                newest.repeats = newest.repeats.saturating_add(1);
                return;
            }
        }
    }
    events.push(event);
    events.sort_by(|a, b| b.at.cmp(&a.at));
    events.truncate(MAX_EVENTS_PER_NODE);
}

/// Drop a decommissioned machine, locally and on the remote.
pub fn forget(git: &Git, hostname: &str) -> Result<()> {
    let refname = ref_name(hostname);
    git.try_run(&["update-ref", "-d", &refname])?;
    let _ = git.run_networked(
        &["push", "--quiet", "origin", "--delete", &refname],
        crate::git::NETWORK_TIMEOUT,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: EventKind, at: time::OffsetDateTime, summary: &str) -> ActivityEvent {
        ActivityEvent {
            at,
            hostname: "h".into(),
            kind,
            summary: summary.into(),
            files: Vec::new(),
            detail: None,
            head: "abc".into(),
            repeats: 1,
            first_at: None,
        }
    }

    #[test]
    fn a_repeating_failure_stays_one_event_and_keeps_the_history() {
        // The real incident: a push rejected for a private email, retried on
        // the 20 second poll for eleven minutes. It wrote 34 records and took
        // most of this node's memory with it.
        let t0 = time::OffsetDateTime::now_utc();
        let mut events = vec![];
        push_event(&mut events, event(EventKind::Changed, t0, "pushed 3 files"));
        for i in 1..=34 {
            let at = t0 + time::Duration::seconds(i * 20);
            push_event(&mut events, event(EventKind::Error, at, "Push rejected: your commit email is private."));
        }
        assert_eq!(events.len(), 2, "one failure entry plus the real history");
        assert!(events.iter().any(|e| e.summary == "pushed 3 files"));
        let err = events.iter().find(|e| e.kind == EventKind::Error).unwrap();
        assert_eq!(err.repeats, 34);
        assert_eq!(err.first_at, Some(t0 + time::Duration::seconds(20)));
        assert_eq!(err.at, t0 + time::Duration::seconds(34 * 20));
    }

    #[test]
    fn two_real_changes_are_never_folded_together() {
        // Changed events are excluded from coalescing on purpose: two syncs
        // that look identical moved two different sets of work.
        let t0 = time::OffsetDateTime::now_utc();
        let mut events = vec![];
        push_event(&mut events, event(EventKind::Changed, t0, "pulled 1 commit"));
        push_event(&mut events, event(EventKind::Changed, t0 + time::Duration::seconds(30), "pulled 1 commit"));
        assert_eq!(events.len(), 2, "two pulls are two events");
    }

    #[test]
    fn an_offline_stretch_stays_one_event_and_keeps_the_history() {
        let t0 = time::OffsetDateTime::now_utc();
        let mut events = vec![];

        push_event(&mut events, event(EventKind::Changed, t0, "pushed 3 files"));
        // Far more retries than the buffer holds. Before coalescing, these
        // evicted the push above and left nothing but noise.
        for i in 1..=80 {
            let at = t0 + time::Duration::seconds(i * 20);
            push_event(&mut events, event(EventKind::Offline, at, "Offline."));
        }

        assert_eq!(events.len(), 2, "one offline entry plus the real one");
        assert!(events.iter().any(|e| e.summary == "pushed 3 files"));
        let offline = events.iter().find(|e| e.kind == EventKind::Offline).unwrap();
        assert_eq!(
            offline.at,
            t0 + time::Duration::seconds(80 * 20),
            "the surviving entry carries the most recent time"
        );
    }

    #[test]
    fn coming_back_online_starts_a_new_entry() {
        let t0 = time::OffsetDateTime::now_utc();
        let mut events = vec![];
        push_event(&mut events, event(EventKind::Offline, t0, "Offline."));
        push_event(&mut events, event(EventKind::Changed, t0 + time::Duration::seconds(1), "pushed"));
        push_event(&mut events, event(EventKind::Offline, t0 + time::Duration::seconds(2), "Offline."));
        assert_eq!(events.len(), 3, "a later offline is a separate stretch");
    }

    #[test]
    fn sanitizes_hostnames_into_valid_refs() {
        assert_eq!(ref_name("Some-MacBook"), "refs/jotbay-status/some-macbook");
        assert_eq!(ref_name("Big Linux Box"), "refs/jotbay-status/big-linux-box");
        assert_eq!(ref_name("host:weird*name"), "refs/jotbay-status/host-weird-name");
    }
}
