//! The sync pass: commit, integrate, publish. Idempotent and re-entrant.

use crate::conflict;
use crate::error::{Error, Result};
use crate::git::{Git, NETWORK_TIMEOUT};
use crate::limits;
use crate::lock::SyncLock;
use crate::model::{ActivityEvent, EventKind, NodeStatus, SyncReport};
use crate::status;
use crate::Jotbay;
use time::OffsetDateTime;

/// Run one full sync. Holding the lock for the whole pass means a scheduled
/// run that overlaps a manual one exits cleanly instead of interleaving git
/// operations with it.
/// Fetch with a deadline, reporting what git said rather than discarding it.
fn fetch(git: &Git) -> Result<()> {
    let out = git.run_networked(&["fetch", "--quiet", "origin"], NETWORK_TIMEOUT)?;
    if !out.success {
        return Err(Error::Other(out.describe("fetch")));
    }
    Ok(())
}

/// The cheapest question you can ask a remote: what do your refs point at?
///
/// One round trip, no object negotiation, no pack. It exists so the watcher can
/// find out that nothing has changed without paying for a fetch, a status fetch
/// and a push to discover the same thing — which is what it used to do, every
/// twenty seconds, forever. On an idle machine that was roughly thirteen
/// thousand git operations a day against someone's git host.
///
/// Branches only. **Status refs are deliberately excluded, and that is load
/// bearing.**
///
/// Every sync republishes this node's status with a fresh `last_sync`, so its
/// ref moves every single time. Include those here and one machine's sync
/// changes what every other machine is watching: B and C wake because A
/// synced, their own syncs move their status refs, which wakes A, forever. The
/// backoff would never engage and the whole exercise would cost more than the
/// fixed interval it replaced.
///
/// Watching branches asks the question that actually has consequences — has
/// the content moved, do I need to pull — and that settles after one round,
/// because a machine that only pulls pushes nothing back. Status is telemetry;
/// `sync` already fetches it, and the interface refreshes it on demand.
///
/// Tags and pull-request refs are excluded for a duller reason: on a busy
/// repository they make the answer large without making it more useful.
///
/// Returns None when the remote could not be reached, which is deliberately
/// *not* an error: a probe is an optimisation, and a machine that cannot reach
/// its remote should fall through to a real sync and report the failure from
/// there, where the reporting already exists.
pub fn remote_fingerprint(git: &Git) -> Option<String> {
    let (out, stdout) = git
        .run_networked_out(&["ls-remote", "origin", "refs/heads/*"], NETWORK_TIMEOUT)
        .ok()?;
    if !out.success {
        return None;
    }
    // Sorted: ls-remote's order is the server's, and a server that reorders
    // refs between calls would otherwise look like a change every time.
    let mut lines: Vec<&str> = stdout.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines.sort_unstable();
    Some(lines.join("\n"))
}

pub fn run(jotbay: &Jotbay) -> Result<SyncReport> {
    let git = jotbay.git();

    let _lock = match SyncLock::acquire(git.root()) {
        Ok(l) => l,
        Err(Error::Locked) => {
            return Ok(SyncReport {
                skipped_locked: true,
                ..Default::default()
            })
        }
        Err(e) => return Err(e),
    };

    // A rebase left over from a previous run would make everything below
    // operate on a half-applied tree.
    if git.rebase_in_progress() {
        return Err(Error::RebaseInProgress);
    }

    let hostname = Jotbay::hostname();
    let mut report = SyncReport::default();
    let outcome = sync_inner(jotbay, &hostname, &mut report);

    // Publish status even when the pass failed — a node that reports "auth
    // error at 14:22" is far more useful than one that silently goes stale.
    let error = outcome.as_ref().err().map(|e| e.to_string());
    let _ = publish_status(jotbay, &hostname, &report, error);

    outcome?;
    Ok(report)
}

fn sync_inner(jotbay: &Jotbay, hostname: &str, report: &mut SyncReport) -> Result<()> {
    let git = jotbay.git();

    // Where this pass began, so the paths it moved can be named at the end.
    let head_before = git.head().unwrap_or_default();

    // 1. Commit whatever changed locally, minus anything the remote would
    //    refuse. Staging a 100MB file and only finding out at push time leaves
    //    an unpushable commit in local history that breaks every later sync.
    report.warnings = limits::scan(git)?;
    let blocked = limits::blocked_paths(&report.warnings);

    let pending: Vec<String> = git
        .dirty_files()?
        .into_iter()
        .filter(|f| !blocked.iter().any(|b| b == f))
        .collect();

    if !pending.is_empty() {
        let mut add: Vec<String> = vec!["add".into(), "-A".into(), "--".into(), ".".into()];
        // Pathspec exclusion keeps the file in the working tree, untracked and
        // visible in `jotbay status`, rather than silently deleting or hiding it.
        add.extend(blocked.iter().map(|p| format!(":(exclude){p}")));
        let add_refs: Vec<&str> = add.iter().map(String::as_str).collect();
        git.run(&add_refs)?;
        let now = OffsetDateTime::now_utc();
        let message = format!(
            "jotbay: {hostname} {:04}-{:02}-{:02} {:02}:{:02}",
            now.year(),
            now.month() as u8,
            now.day(),
            now.hour(),
            now.minute()
        );
        // Signing is off for automated commits by design: a passphrase prompt
        // in a scheduled job would hang forever holding the sync lock, turning
        // a visible failure into silent sync death.
        //
        // If the commit itself fails — overwhelmingly because the machine has
        // no user.name/user.email — everything we just staged would stay
        // staged, so the next `git status` the user runs shows a half-prepared
        // commit they never made. Put the index back before reporting.
        if let Err(e) = git.run(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", &message]) {
            let _ = git.run(&["reset", "-q"]);
            return Err(e);
        }
        report.committed = true;
        report.committed_files = pending.len() as u32;
        report.commit_message = Some(message);
    }

    // 2. Fetch content and every node's status in the same trip.
    fetch(git)?;
    let _ = status::fetch_all(git);

    if !git.has_upstream() {
        return Err(Error::NoUpstream);
    }

    // 3. Rebase only when upstream genuinely has commits we lack. Comparing
    //    HEAD to @{u} instead would also fire when merely ahead, making every
    //    ordinary local edit trigger a pointless rebase.
    let behind: u32 = git
        .run(&["rev-list", "--count", "@..@{u}"])?
        .trim()
        .parse()
        .unwrap_or(0);

    if behind > 0 {
        integrate(jotbay, hostname, report)?;
        report.pulled = behind;
    }

    // 4. Publish. A push rejected because someone else pushed between our
    //    fetch and ours is normal, not an error: integrate and retry once.
    //
    //    Whether anything is actually going out is decided *before* the push,
    //    because afterwards the answer is always no. `pushed` used to be set
    //    unconditionally, which made did_nothing() permanently false — a lie
    //    nobody noticed at ten-minute intervals and that the watcher turned
    //    into "Saved your changes" every twenty seconds, forever.
    let ahead: u32 = git
        .run(&["rev-list", "--count", "@{u}..@"])
        .unwrap_or_default()
        .trim()
        .parse()
        .unwrap_or(0);
    // Nothing to send, so nothing is sent. `ahead` was already computed for
    // the report; the push ran regardless of it, which meant every poll opened
    // a connection, authenticated, exchanged a ref advertisement and pushed
    // zero objects. On a twenty-second timer that was a third of all the
    // traffic this program generated, and all of it was to say nothing.
    //
    // Skipping it cannot lose work: with `ahead == 0` there is no local commit
    // the remote lacks, and the next sync recomputes this from scratch.
    if ahead > 0 {
        let branch = git.current_branch()?;
        let first = git.run_networked(&["push", "--quiet", "origin", &branch], NETWORK_TIMEOUT)?;
        if first.timed_out {
            return Err(Error::Other(first.describe("push")));
        }
        if !first.success {
            fetch(git)?;
            integrate(jotbay, hostname, report)?;
            let retry =
                git.run_networked(&["push", "--quiet", "origin", &branch], NETWORK_TIMEOUT)?;
            if !retry.success {
                // The first rejection is expected — someone else pushed between
                // our fetch and ours. A second one is not, and carries why.
                return Err(Error::Other(retry.describe("push")));
            }
        }
    }
    report.pushed = ahead > 0;
    report.head_short = git.head_short()?;

    let head_after = git.head().unwrap_or_default();
    if !head_before.is_empty() && head_before != head_after {
        if let Ok(out) = git.run(&["diff", "--name-only", &format!("{head_before}..{head_after}")]) {
            report.changed_paths = out.lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
        }
    }

    Ok(())
}

/// Rebase onto upstream, applying the keep-both-sides policy if it stops.
fn integrate(jotbay: &Jotbay, hostname: &str, report: &mut SyncReport) -> Result<()> {
    let git = jotbay.git();

    if git.try_run(&["rebase", "--quiet", "@{u}"])? {
        return Ok(());
    }

    // The rebase stopped. Resolve every conflicted commit in turn — one rebase
    // can halt repeatedly, once per replayed commit that collides.
    let mut guard = 0;
    while git.rebase_in_progress() {
        guard += 1;
        if guard > 100 {
            return Err(Error::Other(
                "rebase did not converge after 100 conflict rounds; \
                 run `jotbay resolve --abort`"
                    .into(),
            ));
        }

        let resolved = conflict::resolve_all(git, hostname)?;
        report.conflicts.extend(resolved);

        // With every path staged, --continue advances to the next commit.
        // GIT_EDITOR=true (set in Git::command) keeps it from opening an editor.
        if !git.try_run(&["rebase", "--continue"])? {
            // Nothing left to resolve but the rebase still refuses: most often
            // a commit that became empty once its changes landed upstream.
            if !git.try_run(&["rebase", "--skip"])? {
                return Err(Error::Other(
                    "rebase could not continue; run `jotbay resolve --abort`".into(),
                ));
            }
        }
    }

    Ok(())
}

/// Turn a finished pass into at most one feed entry.
///
/// Only syncs that did something are recorded. Six machines checking in every
/// ten minutes would otherwise push roughly 850 "nothing happened" entries a
/// day and bury the handful that matter; liveness is already carried by
/// `NodeStatus::last_sync`.
fn describe(
    hostname: &str,
    report: &SyncReport,
    error: &Option<String>,
    head: &str,
) -> Option<ActivityEvent> {
    let blocked = limits::blocked_paths(&report.warnings);
    let mut files: Vec<String> = Vec::new();
    let mut detail: Option<String> = None;

    let (kind, summary) = if let Some(err) = error {
        // Keep the raw text, but lead with a sentence. Verbose mode shows the
        // rest; without this the feed fills with `remote: error: …` and URLs.
        detail = Some(err.clone());
        (EventKind::Error, summarise_error(err))
    } else if !blocked.is_empty() {
        // Worth a feed entry even when the pass was otherwise a no-op: from
        // the user's side a file they added simply is not syncing, and the
        // only other place that surfaces is `jotbay status`.
        let names = blocked
            .iter()
            .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        files = blocked.clone();
        (
            EventKind::Error,
            format!(
                "{} file{} too large to sync: {names}",
                blocked.len(),
                if blocked.len() == 1 { "" } else { "s" }
            ),
        )
    } else if report.did_nothing() {
        return None;
    } else {
        files = report.changed_paths.clone();
        let mut parts = Vec::new();

        if report.committed {
            match &report.commit_message {
                // A hand-written message is the most informative thing
                // available; the generated "jotbay: host date" is not, so fall
                // back to the file count.
                Some(m) if !m.starts_with("jotbay: ") => parts.push(format!("committed \"{m}\"")),
                _ => parts.push(format!(
                    "committed {} file{}",
                    report.committed_files,
                    if report.committed_files == 1 { "" } else { "s" }
                )),
            }
        }
        if report.pulled > 0 {
            parts.push(format!(
                "pulled {} commit{}",
                report.pulled,
                if report.pulled == 1 { "" } else { "s" }
            ));
        }
        if !report.conflicts.is_empty() {
            let n = report.conflicts.len();
            parts.push(format!(
                "{n} conflict{} — both versions kept",
                if n == 1 { "" } else { "s" }
            ));
        }
        if report.pushed && (report.committed || !report.conflicts.is_empty()) {
            parts.push("pushed".to_string());
        }

        if parts.is_empty() {
            return None;
        }

        let kind = if report.conflicts.is_empty() {
            EventKind::Changed
        } else {
            EventKind::Conflict
        };
        (kind, parts.join(" · "))
    };

    Some(ActivityEvent {
        at: OffsetDateTime::now_utc(),
        hostname: hostname.to_string(),
        kind,
        summary,
        files,
        detail,
        head: head.to_string(),
    })
}

/// Turn git's output into one sentence a person can act on.
///
/// The failures worth naming are the ones that stop a machine syncing at all
/// and are invisible until they bite. Anything unrecognised keeps its first
/// line, which is nearly always the useful part; the whole text survives in
/// `detail` for verbose mode either way.
fn summarise_error(raw: &str) -> String {
    let lower = raw.to_lowercase();

    if lower.contains("gh007") || lower.contains("publish a private email") {
        return "Push rejected: your commit email is private.                 Set the noreply address, or allow the push in GitHub's email settings."
            .to_string();
    }
    if lower.contains("author identity unknown") || lower.contains("please tell me who you are") {
        return "Cannot commit: git has no user.name or user.email on this machine.".to_string();
    }
    if lower.contains("authentication failed") || lower.contains("could not read username") {
        return "Authentication failed — the credential helper could not answer.".to_string();
    }
    if lower.contains("exceeds github's file size limit") || lower.contains("gh001") {
        return "Push rejected: a file exceeds GitHub's 100 MB limit.".to_string();
    }
    if lower.contains("could not resolve host") || lower.contains("network is unreachable") {
        return "Offline — could not reach the remote.".to_string();
    }
    if lower.contains("non-fast-forward") || lower.contains("rejected") && lower.contains("fetch first") {
        return "Push rejected: the remote moved on. The next sync reconciles it.".to_string();
    }

    let first = raw.lines().find(|l| !l.trim().is_empty()).unwrap_or(raw).trim();
    let first = first.strip_prefix("error: ").unwrap_or(first);
    if first.chars().count() > 140 {
        let cut: String = first.chars().take(137).collect();
        format!("{cut}…")
    } else {
        first.to_string()
    }
}

fn publish_status(
    jotbay: &Jotbay,
    hostname: &str,
    report: &SyncReport,
    error: Option<String>,
) -> Result<()> {
    let git = jotbay.git();
    let (ahead, behind) = git.ahead_behind().unwrap_or((0, 0));

    let node = NodeStatus {
        hostname: hostname.to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        agent_version: crate::VERSION.to_string(),
        last_sync: OffsetDateTime::now_utc(),
        head: git.head().unwrap_or_default(),
        ahead,
        behind,
        dirty: git.dirty_files().map(|f| f.len() as u32).unwrap_or(0),
        conflicts_resolved: report.conflicts.len() as u32,
        last_error: error.clone(),
        // Relative to whichever machine reads the record, so never published
        // with meaning; read_all recomputes it.
        behind_local: false,
    };

    let mut events = status::read_own_events(git, hostname);
    if let Some(event) = describe(hostname, report, &error, &node.head) {
        status::push_event(&mut events, event);
    }

    status::publish(git, &node, &events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_failures_that_strand_a_machine_get_named() {
        // Verbatim from the Windows deployment: five lines of git stderr with
        // URLs in it, which is what the user actually saw in the feed.
        let gh007 = "git push --quiet origin main: remote: error: GH007: Your push would \
                     publish a private email address. remote: You can make your email public \
                     or disable this protection by visiting: remote: \
                     https://github.com/settings/emails";
        let s = summarise_error(gh007);
        assert!(s.contains("email is private"), "{s}");
        assert!(!s.contains("http"), "no URLs in the summary: {s}");
        assert!(s.len() < 160, "one sentence, not a wall: {s}");

        // Verbatim from the Linux deployment.
        let identity = "Author identity unknown\n\n*** Please tell me who you are.\n\nRun\n\n  \
                        git config --global user.email \"you@example.com\"";
        assert!(summarise_error(identity).contains("no user.name"));

        assert!(summarise_error("fatal: Authentication failed for 'https://…'")
            .contains("Authentication failed"));
        assert!(summarise_error("fatal: unable to access: Could not resolve host: github.com")
            .contains("Offline"));
    }

    #[test]
    fn an_unrecognised_error_keeps_its_first_line_and_stays_short() {
        let s = summarise_error("error: something nobody anticipated\nline two\nline three");
        assert_eq!(s, "something nobody anticipated");

        let long = "x".repeat(400);
        let s = summarise_error(&long);
        assert!(s.chars().count() <= 140, "truncated: {}", s.chars().count());
        assert!(s.ends_with('…'));
    }
}
