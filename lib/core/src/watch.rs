//! Sync without being asked.
//!
//! The scheduled sync ran every ten minutes, which meant a note written on one
//! machine could sit there for ten minutes before existing anywhere else, and
//! the answer to "did that save?" was "probably, eventually". That is a poor
//! deal for someone who does not think in commits, which is everyone this is
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
//! happened while the process was not running; and for a notes folder. Which
//! is measured in hundreds of files. A scan costs single-digit milliseconds.
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
/// Long enough that saving a file in a loop, which every editor does, and
/// which some do several times a second, produces one commit rather than
/// twenty. Short enough that "did it save?" is never a real question.
///
/// With a one-second scan this puts a change on the remote in under ten
/// seconds end to end, most of which is the network round trip rather than
/// anything waited for here. Measured at twelve seconds with a three-second
/// settle, which was noticeably longer than it needed to be.
pub const SETTLE: Duration = Duration::from_secs(2);

/// How often the folder is inspected for local edits.
pub const SCAN_EVERY: Duration = Duration::from_secs(1);

/// How soon the remote is checked after anything happens.
///
/// The other half of the promise: pushing quickly is worth little if the
/// machine you walk over to is still ten minutes behind.
///
/// This used to be a fixed interval, on the belief, written down, never
/// measured. That checking an unchanged remote was "one small round trip". It
/// was three: a fetch, a status fetch, and a push that had nothing to send.
/// Four thousand times a day, on a machine nobody was using, against somebody
/// else's git host. A self-hosted Gitea or a small Codeberg account would feel
/// that; so, eventually, would GitHub.
pub const POLL_REMOTE: Duration = Duration::from_secs(20);

/// The slowest the remote is checked once nothing has happened for a while.
///
/// The interval doubles from `POLL_REMOTE` on every check that finds nothing,
/// so a machine reaches this after roughly ten minutes of total quiet. Every
/// machine idle, nobody editing. Five minutes of staleness on a machine nobody
/// has touched in ten is a fair trade for cutting idle traffic by ninety-odd
/// percent, and any local edit drops it straight back to `POLL_REMOTE`.
///
/// Deliberately not larger. The failure this must never produce is walking over
/// to a laptop, opening it, and reading yesterday's note.
pub const POLL_REMOTE_MAX: Duration = Duration::from_secs(300);

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
/// belongs to the operating system, launchd, systemd, Task Scheduler, all of
/// which restart a process that dies and log what it printed. A hand-rolled
/// daemon would reimplement that badly.
/// Whether to report where the seconds go, via `JOTBAY_TIMING`.
///
/// Off by default: the watcher's log should stay quiet enough that a line in it
/// means something. Turn it on for a foreground run, `JOTBAY_TIMING=1 jotbay
/// watch`. When a machine seems slow.
///
/// It exists because a fresh install measured 18.7 s from writing a file to it
/// reaching the remote on Windows, against 5–7 s on macOS and 8 s on Linux, and
/// nobody could say which part was slow. The module docs above claim a scan
/// costs "single-digit milliseconds"; that was measured on one platform and
/// asserted for three. This is how that claim gets checked rather than
/// repeated.
fn timing_enabled() -> bool {
    match std::env::var("JOTBAY_TIMING") {
        Ok(v) => !v.is_empty() && v != "0",
        Err(_) => false,
    }
}

/// One line per sync, breaking the wait into the parts we control and the part
/// we do not.
///
/// `scan` is our directory walk, `settle` is the deliberate quiet period, and
/// `sync` is git, add, commit, push, fetch, and the network. Only the first is
/// a thing this code could make faster; if Windows is slow in `sync`, the cause
/// is git or the network and no amount of tuning here will touch it.
fn report(label: &str, files: usize, scan: Duration, sync: Duration, total: Duration) {
    if !timing_enabled() {
        return;
    }
    eprintln!(
        "timing {label}: files={files} scan={:.3}s sync={:.2}s total={:.2}s",
        scan.as_secs_f64(),
        sync.as_secs_f64(),
        total.as_secs_f64(),
    );
}

pub fn run(jotbay: &Jotbay, mut on_event: impl FnMut(Event, Option<String>)) -> Result<()> {
    // The whole vault, not just `data/`. `sync` commits the repository with
    // `git add -A`, so watching a subdirectory of it meant the watcher's idea
    // of "changed" was narrower than git's. A report written to
    // `install/agent/` sat for eight minutes and never committed, because
    // nothing that fires the fast path had happened inside `data/`.
    //
    // `.git` is excluded for free: the walk skips dotfiles, which is also why
    // its own churn cannot self-trigger.
    let data = jotbay.git().root().to_path_buf();
    let mut seen = fingerprint(&data);
    let mut dirty_since: Option<Instant> = None;
    let mut last_remote = Instant::now();
    // Grows while nothing happens, resets the moment anything does.
    let mut poll_every = POLL_REMOTE;
    // One at startup, so a machine that was asleep while another pushed is
    // current by the time anyone looks at it.
    sync_now(jotbay, Event::Remote, &mut on_event);

    // What the remote looked like when we last agreed with it. None means "no
    // idea", which always resolves to doing the real work.
    let mut remote_seen: Option<String> = crate::sync::remote_fingerprint(jotbay.git());
    // The roll-call ref as we last saw it, and how long to stay responsive
    // after somebody asks. Both start empty: whatever is already on the remote
    // at startup is not a request aimed at us.
    let mut rollcall_seen: Option<Option<String>> = None;
    let mut attentive_until: Option<Instant> = None;

    loop {
        std::thread::sleep(SCAN_EVERY);

        let scan_started = Instant::now();
        let current = fingerprint(&data);
        let scan_cost = scan_started.elapsed();
        let files = current.len();

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
                let sync_started = Instant::now();
                sync_now(jotbay, Event::Local, &mut on_event);
                // `since` is when the change was noticed, so this total omits
                // the up-to-SCAN_EVERY wait before noticing it. Stated rather
                // than corrected: the wall-clock a user perceives starts at the
                // save, and this cannot see that moment.
                report("local", files, scan_cost, sync_started.elapsed(), since.elapsed());
                // Somebody is working. Whatever backoff had accumulated is
                // wrong now. The other machines are about to matter again.
                poll_every = POLL_REMOTE;
                remote_seen = crate::sync::remote_fingerprint(jotbay.git());
                // The sync writes status refs and may pull; re-read so its own
                // work is never mistaken for the user's.
                seen = fingerprint(&data);
                continue;
            }
        }

        if last_remote.elapsed() >= poll_every {
            last_remote = Instant::now();

            // Ask the cheap question first. One round trip, no pack, no
            // negotiation, against the three operations a full sync costs.
            let probe_started = Instant::now();
            let answer = crate::sync::probe(jotbay.git());
            let probe_cost = probe_started.elapsed();
            let probe = answer.as_ref().map(|p| p.heads.clone());

            // Somebody opened a window and wants to know who is out there.
            // Answering costs one small force-push of our own status ref. No
            // fetch, no push of main. A full sync would answer the same
            // question for three times the traffic *and* move a branch tip,
            // waking every other machine into doing the same.
            let asked = answer.as_ref().map(|p| p.rollcall.clone());
            let rollcall_moved = match (&asked, &rollcall_seen) {
                (Some(now), Some(before)) => now != before,
                // First sight of a roll call after starting up is not a
                // request: it is whatever was already on the remote.
                (Some(_), None) => false,
                (None, _) => false,
            };
            if asked.is_some() {
                rollcall_seen = asked;
            }
            if rollcall_moved {
                let _ = crate::sync::announce_presence(jotbay);
                // Somebody is looking. Stay responsive while they are, so
                // presence does not decay in front of them.
                attentive_until = Some(Instant::now() + crate::presence::ATTENTION);
            }

            // And the free ones, both local. `ahead` stops a failed push from
            // being forgotten until the next edit; `dirty` catches everything
            // the folder scan cannot see. The whole vault outside `data/`,
            // and every dotfile.
            let ahead = jotbay.git().ahead_behind().map(|(a, _)| a).unwrap_or(0);
            let dirty = jotbay.git().is_dirty().unwrap_or(false);

            if worth_syncing(probe.as_deref(), remote_seen.as_deref(), ahead, dirty) {
                let sync_started = Instant::now();
                sync_now(jotbay, Event::Remote, &mut on_event);
                report("remote", files, probe_cost, sync_started.elapsed(), probe_started.elapsed());
                // Re-read after the sync rather than reusing the pre-sync
                // probe: our own push moved the remote, and storing the older
                // answer would make the next poll see a phantom change.
                remote_seen = crate::sync::remote_fingerprint(jotbay.git());
                poll_every = POLL_REMOTE;
                seen = fingerprint(&data);
            } else {
                if remote_seen.is_none() {
                    remote_seen = probe;
                }
                // Hold the base interval while somebody has a window open,
                // so presence stays current in front of them rather than
                // decaying to five-minute granularity.
                let watched = attentive_until.map(|t| Instant::now() < t).unwrap_or(false);
                poll_every = if watched { POLL_REMOTE } else { backed_off(poll_every) };
                report("idle", files, probe_cost, Duration::ZERO, probe_started.elapsed());
            }
        }
    }
}

/// The next polling interval after a check that found nothing.
///
/// Doubling rather than a fixed ladder so the ramp is short while someone is
/// working and long once they clearly are not, and capped so that "idle"
/// never becomes "asleep".
fn backed_off(current: Duration) -> Duration {
    (current * 2).min(POLL_REMOTE_MAX)
}

/// Whether a poll should pay for a full sync, or stop at the cheap probe.
///
/// Pure so the rule can be asserted rather than inferred from a loop, because
/// getting it wrong is silent: nothing errors, nothing logs, work simply stops
/// leaving the machine.
///
/// `dirty` is the one that matters and the one 1.7.3 shipped without. The
/// watcher fingerprints `data/`, but `sync` commits the whole repository with
/// `git add -A`, so anything in the vault outside `data/`, and anything
/// dotfiled, which the scan skips, is invisible to the watcher and still
/// perfectly visible to git. Before the poll became conditional this never
/// showed, because a full sync ran every twenty seconds regardless and swept
/// those files up as a side effect. Making the poll conditional removed the
/// accident that was carrying them, and edits outside `data/` silently stopped
/// syncing: no error, no log line, the file just never left the machine.
///
/// `git status` is local, so asking costs nothing on the wire.
fn worth_syncing(
    remote_now: Option<&str>,
    remote_seen: Option<&str>,
    ahead: u32,
    dirty: bool,
) -> bool {
    // Local work always wins: it is ours to publish and no remote answer
    // changes that.
    if ahead > 0 || dirty {
        return true;
    }
    match (remote_now, remote_seen) {
        (Some(now), Some(before)) => now != before,
        // No baseline yet, establish one by doing the real thing.
        (Some(_), None) => true,
        // Unreachable. Back off rather than retry hard: an outage is exactly
        // when hammering someone's host helps least, and there is nothing of
        // ours waiting to go out.
        (None, _) => false,
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

    const A: &str = "aaa\trefs/heads/main";
    const B: &str = "bbb\trefs/heads/main";

    #[test]
    fn a_quiet_machine_with_nothing_of_its_own_does_not_sync() {
        assert!(!worth_syncing(Some(A), Some(A), 0, false));
    }

    #[test]
    fn local_work_outside_the_data_folder_still_syncs() {
        // The 1.7.3 regression, in one line. `dirty` is true and everything
        // else says "nothing to do": the remote has not moved and there is no
        // commit waiting, because the edit was never committed in the first
        // place. Return false here and the file silently never leaves the
        // machine. No error, no log, nothing to notice.
        assert!(
            worth_syncing(Some(A), Some(A), 0, true),
            "an uncommitted local change must force a sync, wherever in the \
             vault it lives"
        );
    }

    #[test]
    fn a_commit_the_remote_lacks_still_syncs() {
        // A push that failed earlier must not wait for the next edit.
        assert!(worth_syncing(Some(A), Some(A), 1, false));
    }

    #[test]
    fn a_moved_remote_syncs_and_an_unmoved_one_does_not() {
        assert!(worth_syncing(Some(B), Some(A), 0, false));
        assert!(!worth_syncing(Some(A), Some(A), 0, false));
    }

    #[test]
    fn no_baseline_means_do_the_real_thing() {
        assert!(worth_syncing(Some(A), None, 0, false));
    }

    #[test]
    fn an_unreachable_remote_backs_off_unless_we_are_holding_work() {
        // Nothing of ours is waiting, so retrying hard during an outage only
        // hurts whoever is hosting the repository.
        assert!(!worth_syncing(None, Some(A), 0, false));
        // But our own work still has to get out when the network returns.
        assert!(worth_syncing(None, Some(A), 1, false));
        assert!(worth_syncing(None, Some(A), 0, true));
    }

    #[test]
    fn backoff_ramps_from_the_base_and_stops_at_the_cap() {
        // Short enough that a machine somebody is using stays responsive, long
        // enough that an idle one stops asking. Ten minutes of total quiet to
        // reach the cap, which is the number the doubling was chosen for.
        let mut d = POLL_REMOTE;
        let mut elapsed = Duration::ZERO;
        let mut steps = 0;
        while d < POLL_REMOTE_MAX {
            elapsed += d;
            d = backed_off(d);
            steps += 1;
            assert!(steps < 20, "backoff is not converging on the cap");
        }
        assert_eq!(d, POLL_REMOTE_MAX);
        assert!(
            elapsed <= Duration::from_secs(15 * 60),
            "took {elapsed:?} of quiet to reach the slowest interval, which is \
             long enough that the cap stops doing anything useful"
        );

        // And it stays there rather than growing without bound. An unclamped
        // doubling reaches hours before the day is out, and a machine that
        // checks once an hour is one somebody will call broken.
        for _ in 0..40 {
            d = backed_off(d);
            assert_eq!(d, POLL_REMOTE_MAX);
        }
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
