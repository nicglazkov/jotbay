//! Per-node status published to `refs/jotbay-status/<hostname>`.
//!
//! Each machine owns exactly one ref, so two nodes can never conflict and no
//! heartbeat ever lands on `main` — the branch carries content and nothing
//! else. Reading every node's state costs one fetch.

use crate::error::Result;
use crate::git::Git;
use crate::model::{ActivityEvent, NodeStatus, MAX_EVENTS_PER_NODE};

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
    git.try_run(&["push", "--quiet", "--force", "origin", &format!("{refname}:{refname}")])?;
    Ok(())
}

/// Fetch every node's status ref. Best-effort: being offline is not an error
/// worth aborting a sync over.
pub fn fetch_all(git: &Git) -> Result<()> {
    git.try_run(&["fetch", "--quiet", "origin", REFSPEC])?;
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
    events.push(event);
    events.sort_by(|a, b| b.at.cmp(&a.at));
    events.truncate(MAX_EVENTS_PER_NODE);
}

/// Drop a decommissioned machine, locally and on the remote.
pub fn forget(git: &Git, hostname: &str) -> Result<()> {
    let refname = ref_name(hostname);
    git.try_run(&["update-ref", "-d", &refname])?;
    git.try_run(&["push", "--quiet", "origin", "--delete", &refname])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_hostnames_into_valid_refs() {
        assert_eq!(ref_name("Some-MacBook"), "refs/jotbay-status/some-macbook");
        assert_eq!(ref_name("Big Linux Box"), "refs/jotbay-status/big-linux-box");
        assert_eq!(ref_name("host:weird*name"), "refs/jotbay-status/host-weird-name");
    }
}
