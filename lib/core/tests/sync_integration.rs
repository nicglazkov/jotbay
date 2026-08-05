//! End-to-end tests against real git repositories.
//!
//! These assert on file *content*, not exit status. The conflict policy's most
//! likely failure mode — swapping which side keeps the canonical filename,
//! because `--ours` means upstream during a rebase — exits zero either way.

use std::path::Path;
use std::process::Command;
use jotbay_core::{ConflictKind, EventKind, Jotbay};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A bare origin plus two clones, mimicking two machines.
struct Fixture {
    _tmp: tempfile::TempDir,
    a: std::path::PathBuf,
    b: std::path::PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let origin = tmp.path().join("origin.git");
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");

    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "--bare", "-q", "-b", "main", "."]);

    git(tmp.path(), &["clone", "-q", origin.to_str().unwrap(), "a"]);
    configure(&a);
    std::fs::create_dir_all(a.join("data")).unwrap();
    // A real jotbay always carries the normalisation attributes; without them
    // the fixture is at the mercy of the host's core.autocrlf, and Git for
    // Windows installs with autocrlf=true - clone B then checks files out
    // with CRLF and every content assertion misses. Found the first time the
    // suite ran on Windows.
    std::fs::write(a.join(".gitattributes"), "* text=auto eol=lf\n").unwrap();
    std::fs::write(a.join("data/notes.md"), "line one\n").unwrap();
    git(&a, &["add", "-A"]);
    git(&a, &["commit", "-q", "-m", "seed"]);
    git(&a, &["push", "-q", "-u", "origin", "main"]);

    git(tmp.path(), &["clone", "-q", origin.to_str().unwrap(), "b"]);
    configure(&b);

    Fixture { _tmp: tmp, a, b }
}

fn configure(repo: &Path) {
    git(repo, &["config", "user.name", "Test"]);
    git(repo, &["config", "user.email", "test@example.com"]);
    git(repo, &["config", "commit.gpgsign", "false"]);
}

#[test]
fn sync_is_a_noop_when_nothing_changed() {
    let f = fixture();
    let jotbay = Jotbay::open(&f.a).unwrap();

    let report = jotbay.sync().expect("sync should succeed");
    assert!(!report.committed, "nothing was edited, so nothing to commit");
    assert!(report.conflicts.is_empty());
}

#[test]
fn the_remote_fingerprint_moves_only_when_the_remote_does() {
    // The watcher leans on this to decide whether a full sync is worth three
    // round trips. If it reported a change on every call, the backoff would
    // never take hold and the polling would be exactly as heavy as before —
    // silently, because everything would still work.
    let f = fixture();
    let a = Jotbay::open(&f.a).unwrap();

    let first = jotbay_core::sync::remote_fingerprint(a.git()).expect("remote is reachable");
    assert!(!first.is_empty(), "a seeded remote has refs");

    let again = jotbay_core::sync::remote_fingerprint(a.git()).expect("remote is reachable");
    assert_eq!(first, again, "an untouched remote must fingerprint the same");

    // Somebody else pushes.
    std::fs::write(f.b.join("data/from-b.md"), "hello\n").unwrap();
    Jotbay::open(&f.b).unwrap().sync().expect("b syncs");

    let after = jotbay_core::sync::remote_fingerprint(a.git()).expect("remote is reachable");
    assert_ne!(
        first, after,
        "the remote moved and the fingerprint did not — the watcher would \
         never notice another machine's work"
    );
}

#[test]
fn the_fingerprint_ignores_status_refs() {
    // The subtle one. Every sync republishes its node's status with a fresh
    // last_sync, so those refs move constantly and for reasons nobody needs to
    // act on. Watch them here and each machine's sync wakes every other
    // machine, whose syncs wake it back: the backoff never engages and the
    // polling costs more than the fixed interval it replaced. Nothing would
    // look broken — it would just quietly be worse.
    let f = fixture();
    let a = Jotbay::open(&f.a).unwrap();

    let before = jotbay_core::sync::remote_fingerprint(a.git()).unwrap();

    // b does a pure no-op sync: no content, but it still publishes status.
    Jotbay::open(&f.b).unwrap().sync().expect("b syncs");
    let status_refs = git(&f.b, &["ls-remote", "origin", "refs/jotbay-status/*"]);
    assert!(
        !status_refs.is_empty(),
        "the fixture never published a status ref, so this proves nothing"
    );

    let after = jotbay_core::sync::remote_fingerprint(a.git()).unwrap();
    assert_eq!(
        before, after,
        "a status-only change moved the fingerprint — every machine will now \
         wake every other machine indefinitely"
    );
}

#[test]
fn a_sync_with_nothing_to_send_does_not_push() {
    // The push used to run unconditionally, which on a twenty-second timer was
    // a third of all the traffic this program made, every byte of it spent
    // saying nothing. Proven by the remote's own reflog: a push that sends no
    // objects still updates nothing, so the check is that the remote is
    // untouched across a sync that had no work.
    let f = fixture();
    let jotbay = Jotbay::open(&f.a).unwrap();

    let before = jotbay_core::sync::remote_fingerprint(jotbay.git()).unwrap();
    let report = jotbay.sync().expect("sync should succeed");
    assert!(!report.pushed, "there was nothing to push");
    let after = jotbay_core::sync::remote_fingerprint(jotbay.git()).unwrap();
    assert_eq!(before, after, "an empty sync moved a ref on the remote");

    // And the converse, so this cannot pass by never pushing at all.
    std::fs::write(f.a.join("data/real-work.md"), "content\n").unwrap();
    let report = jotbay.sync().expect("sync should succeed");
    assert!(report.pushed, "real work must still reach the remote");
    let moved = jotbay_core::sync::remote_fingerprint(jotbay.git()).unwrap();
    assert_ne!(after, moved, "the commit never arrived");
}

#[test]
fn local_edits_reach_the_other_clone() {
    let f = fixture();

    std::fs::write(f.a.join("data/new.md"), "from a\n").unwrap();
    Jotbay::open(&f.a).unwrap().sync().expect("a syncs");

    Jotbay::open(&f.b).unwrap().sync().expect("b syncs");
    let got = std::fs::read_to_string(f.b.join("data/new.md")).expect("b received the file");
    assert_eq!(got, "from a\n");
}

#[test]
fn conflicting_edits_keep_both_sides_with_upstream_holding_the_name() {
    let f = fixture();

    // A publishes first, so A becomes upstream.
    std::fs::write(f.a.join("data/notes.md"), "ALPHA\n").unwrap();
    Jotbay::open(&f.a).unwrap().sync().expect("a syncs");

    // B edits the same line without having seen A's change.
    std::fs::write(f.b.join("data/notes.md"), "BRAVO\n").unwrap();
    let report = Jotbay::open(&f.b).unwrap().sync().expect("b syncs through the conflict");

    assert_eq!(report.conflicts.len(), 1, "exactly one file conflicted");
    let res = &report.conflicts[0];
    assert_eq!(res.kind, ConflictKind::BothModified);
    assert_eq!(res.path, "data/notes.md");

    // The canonical name holds UPSTREAM's content. If the ours/theirs
    // inversion were mishandled this would be "BRAVO" and still exit zero.
    let canonical = std::fs::read_to_string(f.b.join("data/notes.md")).unwrap();
    assert_eq!(canonical, "ALPHA\n", "upstream keeps the canonical filename");

    // B's version survives beside it.
    let copy = res.kept_copy.as_ref().expect("a conflict copy was written");
    assert!(copy.starts_with("data/notes.conflict-"), "copy sits next to the original: {copy}");
    let kept = std::fs::read_to_string(f.b.join(copy)).unwrap();
    assert_eq!(kept, "BRAVO\n", "the local version is preserved verbatim");
}

#[test]
fn conflict_resolution_propagates_back_to_the_first_clone() {
    let f = fixture();

    std::fs::write(f.a.join("data/notes.md"), "ALPHA\n").unwrap();
    Jotbay::open(&f.a).unwrap().sync().unwrap();
    std::fs::write(f.b.join("data/notes.md"), "BRAVO\n").unwrap();
    Jotbay::open(&f.b).unwrap().sync().unwrap();

    // A pulls and should now see both versions.
    Jotbay::open(&f.a).unwrap().sync().unwrap();
    assert_eq!(std::fs::read_to_string(f.a.join("data/notes.md")).unwrap(), "ALPHA\n");

    let copies: Vec<_> = std::fs::read_dir(f.a.join("data"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains(".conflict-"))
        .collect();
    assert_eq!(copies.len(), 1, "the conflict copy synced across: {copies:?}");
}

#[test]
fn upstream_deletion_does_not_discard_a_local_edit() {
    let f = fixture();

    // A deletes the file.
    std::fs::remove_file(f.a.join("data/notes.md")).unwrap();
    Jotbay::open(&f.a).unwrap().sync().unwrap();

    // B edited it meanwhile. The edit must win: a deletion is cheap to redo,
    // a lost file is not.
    std::fs::write(f.b.join("data/notes.md"), "STILL WANTED\n").unwrap();
    let report = Jotbay::open(&f.b).unwrap().sync().unwrap();

    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(report.conflicts[0].kind, ConflictKind::DeletedUpstream);
    assert_eq!(
        std::fs::read_to_string(f.b.join("data/notes.md")).unwrap(),
        "STILL WANTED\n"
    );
}

#[test]
fn node_status_is_published_and_readable() {
    let f = fixture();
    let jotbay = Jotbay::open(&f.a).unwrap();
    jotbay.sync().unwrap();

    let nodes = jotbay.nodes(true).expect("status refs readable");
    assert_eq!(nodes.len(), 1, "one machine has reported in");
    assert_eq!(nodes[0].hostname, Jotbay::hostname());
    assert!(nodes[0].last_error.is_none());

    // And it must not pollute the branch history.
    let subjects = git(&f.a, &["log", "--pretty=%s"]);
    assert!(
        !subjects.contains("jotbay status"),
        "status commits must never land on main: {subjects}"
    );
}

#[test]
fn activity_records_real_work_and_ignores_no_op_syncs() {
    let f = fixture();
    let jotbay = Jotbay::open(&f.a).unwrap();

    // A sync with nothing to do must leave no trace, or six machines checking
    // in every ten minutes would bury everything that matters.
    jotbay.sync().unwrap();
    assert!(
        jotbay.activity(false, 50).unwrap().is_empty(),
        "a no-op sync must not produce an event"
    );

    std::fs::write(f.a.join("data/new.md"), "content\n").unwrap();
    jotbay.sync().unwrap();

    let events = jotbay.activity(false, 50).unwrap();
    assert_eq!(events.len(), 1, "one sync that did work, one event");
    assert_eq!(events[0].kind, EventKind::Changed);
    assert_eq!(events[0].hostname, Jotbay::hostname());
    assert!(
        events[0].summary.contains("committed 1 file"),
        "summary should say what happened: {}",
        events[0].summary
    );

    // And another no-op must not append to it.
    jotbay.sync().unwrap();
    assert_eq!(jotbay.activity(false, 50).unwrap().len(), 1);
}

#[test]
fn activity_merges_events_from_every_machine() {
    let f = fixture();

    std::fs::write(f.a.join("data/from-a.md"), "a\n").unwrap();
    Jotbay::open(&f.a).unwrap().sync().unwrap();

    std::fs::write(f.b.join("data/from-b.md"), "b\n").unwrap();
    Jotbay::open(&f.b).unwrap().sync().unwrap();

    // Both clones share a hostname here, so assert on the merged count rather
    // than on distinct names: two machines' buffers, two events.
    let events = Jotbay::open(&f.a).unwrap().activity(true, 50).unwrap();
    assert!(events.len() >= 2, "feed merges every machine: {events:?}");

    // Newest first.
    for pair in events.windows(2) {
        assert!(pair[0].at >= pair[1].at, "feed must be newest-first");
    }
}

#[test]
fn a_conflict_is_recorded_as_a_conflict_event() {
    let f = fixture();

    std::fs::write(f.a.join("data/notes.md"), "ALPHA\n").unwrap();
    Jotbay::open(&f.a).unwrap().sync().unwrap();
    std::fs::write(f.b.join("data/notes.md"), "BRAVO\n").unwrap();
    Jotbay::open(&f.b).unwrap().sync().unwrap();

    let events = Jotbay::open(&f.b).unwrap().activity(false, 50).unwrap();
    let conflict = events
        .iter()
        .find(|e| e.kind == EventKind::Conflict)
        .expect("the conflict should appear in the feed");
    assert!(
        conflict.summary.contains("both versions kept"),
        "the feed must say nothing was lost: {}",
        conflict.summary
    );
}

#[test]
fn the_event_buffer_stays_bounded() {
    use jotbay_core::MAX_EVENTS_PER_NODE;
    let f = fixture();
    let jotbay = Jotbay::open(&f.a).unwrap();

    // Each iteration changes a file, so each produces exactly one event.
    for i in 0..(MAX_EVENTS_PER_NODE + 5) {
        std::fs::write(f.a.join("data/counter.md"), format!("{i}\n")).unwrap();
        jotbay.sync().unwrap();
    }

    let events = jotbay.activity(false, 1000).unwrap();
    assert_eq!(
        events.len(),
        MAX_EVENTS_PER_NODE,
        "the per-machine buffer must not grow without bound"
    );
}

#[test]
fn status_reports_dirty_files_without_touching_the_network() {
    let f = fixture();
    std::fs::write(f.a.join("data/scratch.md"), "wip\n").unwrap();

    let status = Jotbay::open(&f.a).unwrap().status(false).unwrap();
    assert_eq!(status.dirty_files, vec!["data/scratch.md".to_string()]);
    assert!(!status.is_clean());
}

#[test]
fn oversized_files_are_never_staged_and_are_reported() {
    use jotbay_core::limits::{Severity, BLOCK_BYTES};
    let f = fixture();
    let jotbay = Jotbay::open(&f.a).unwrap();

    // A file over GitHub's hard ceiling, alongside an ordinary note.
    std::fs::write(f.a.join("data/huge.bin"), vec![0u8; (BLOCK_BYTES + 1024) as usize]).unwrap();
    std::fs::write(f.a.join("data/note.md"), "keep me\n").unwrap();

    let report = jotbay.sync().expect("sync succeeds despite the oversized file");

    // The note went through; the video-sized file did not.
    assert!(report.committed, "the ordinary note still syncs");
    let blocked: Vec<_> = report
        .warnings
        .iter()
        .filter(|w| w.severity == Severity::Blocked)
        .collect();
    assert_eq!(blocked.len(), 1, "the oversized file is reported");
    assert_eq!(blocked[0].path, "data/huge.bin");

    // Critically it must NOT be in history — a commit containing it could
    // never be pushed, and every later sync would fail on the same rejection.
    let tracked = git(&f.a, &["ls-files"]);
    assert!(!tracked.contains("huge.bin"), "oversized file must stay untracked: {tracked}");
    assert!(tracked.contains("note.md"));

    // It stays on disk, untouched.
    assert!(f.a.join("data/huge.bin").exists(), "the file itself is never removed");

    // And it surfaces in the activity feed rather than failing silently.
    let events = jotbay.activity(false, 10).unwrap();
    assert!(
        events.iter().any(|e| e.summary.contains("too large to sync")),
        "blocked files must appear in the feed: {events:?}"
    );
}

#[test]
fn a_roll_call_is_visible_to_other_machines_and_answering_does_not_move_it() {
    // The whole design rests on this asymmetry. Asking moves the roll-call ref;
    // answering moves only a status ref. Get it wrong and every machine's
    // answer is another machine's question, forever — the same loop that made
    // watching status refs untenable.
    let f = fixture();
    let a = Jotbay::open(&f.a).unwrap();
    let b = Jotbay::open(&f.b).unwrap();

    let before = jotbay_core::sync::probe(a.git()).expect("remote reachable");
    assert!(before.rollcall.is_none(), "the fixture starts with no roll call");

    // A asks.
    jotbay_core::presence::request(a.git()).expect("a asks");
    let asked = jotbay_core::sync::probe(b.git()).expect("remote reachable");
    assert!(
        asked.rollcall.is_some(),
        "b cannot see that anybody asked, so it will never report in"
    );
    assert_eq!(
        asked.heads, before.heads,
        "asking moved a branch tip — that would make a roll call trigger a full \
         sync on every machine instead of a status publish"
    );

    // B answers.
    jotbay_core::sync::announce_presence(&b).expect("b reports in");
    let after = jotbay_core::sync::probe(a.git()).expect("remote reachable");
    assert_eq!(
        after.rollcall, asked.rollcall,
        "answering the roll call moved it — every answer is now a new question \
         and the fleet will never go quiet"
    );
    assert_eq!(after.heads, before.heads, "answering moved a branch tip");
}

#[test]
fn asking_twice_is_seen_as_two_distinct_requests() {
    // Watchers detect a roll call by the ref *changing*. If two requests
    // produced the same value the second would be invisible, so a machine that
    // missed the first would stay unlit no matter how often you asked.
    let f = fixture();
    let a = Jotbay::open(&f.a).unwrap();

    jotbay_core::presence::request(a.git()).expect("first ask");
    let first = jotbay_core::sync::probe(a.git()).unwrap().rollcall;
    jotbay_core::presence::request(a.git()).expect("second ask");
    let second = jotbay_core::sync::probe(a.git()).unwrap().rollcall;

    assert!(first.is_some() && second.is_some());
    assert_ne!(first, second, "two roll calls were indistinguishable");
}

#[test]
fn announcing_presence_refreshes_last_sync_without_syncing() {
    // What answering costs, and what it achieves: this machine's last_sync
    // moves forward so peers can see it is alive, and nothing else happens.
    let f = fixture();
    let a = Jotbay::open(&f.a).unwrap();

    a.sync().expect("establish a status ref");
    let before = a.nodes(true).unwrap();
    let mine = jotbay_core::Jotbay::hostname();
    let first = before.iter().find(|n| n.hostname == mine).map(|n| n.last_sync);
    assert!(first.is_some(), "this machine published no status to begin with");

    std::thread::sleep(std::time::Duration::from_millis(1100));
    jotbay_core::sync::announce_presence(&a).expect("announce");

    let after = a.nodes(true).unwrap();
    let second = after.iter().find(|n| n.hostname == mine).map(|n| n.last_sync);
    assert!(
        second > first,
        "announcing presence did not move last_sync, so peers still cannot \
         tell this machine is alive"
    );
}
