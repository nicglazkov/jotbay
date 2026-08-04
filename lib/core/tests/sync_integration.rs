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
