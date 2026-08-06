//! The three first-run routes, against real git repositories.
//!
//! Setup offers three ways in, create a repository, clone one you already
//! have, adopt a folder that is already a clone, and they are three different
//! functions that happen to share a tail. Nothing tested that tail, and in
//! 1.7.1 it cost us: the git-identity fix landed below an early return that
//! only the *create* route falls through, so cloning and adopting kept the old
//! behaviour and the first push kept dying on GH007. The fix looked proven
//! because the one route anyone had exercised was the one that worked.
//!
//! These tests exist so that a route cannot silently skip a step again.
//!
//! `create_and_clone` is not covered: it requires `gh` to be installed and
//! signed in, which a test process cannot arrange. It is also the route that
//! was already working, being the only one anybody had run.

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// A repository-local config value, or None when it is not set locally.
///
/// `--local` matters as much here as it does in the code under test: the plain
/// form falls back to the global config, so a developer's own git identity
/// would satisfy this assertion on their machine and nowhere else. That exact
/// confusion is what shipped the bug.
fn local_config(repo: &Path, key: &str) -> Option<String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["config", "--local", "--get", key])
        .output()
        .expect("git should run");
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() { None } else { Some(v) }
}

/// A bare origin with one commit on `main`, so a clone of it has an upstream.
///
/// That is the whole point: an *empty* repository produces a clone with no
/// upstream, which is the case the old code handled. Every real "I already have
/// one" starts from a repository with something in it.
fn origin_with_history(tmp: &Path) -> PathBuf {
    let origin = tmp.join("origin.git");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "--bare", "-q", "-b", "main", "."]);

    let seed = tmp.join("seed");
    git(tmp, &["clone", "-q", origin.to_str().unwrap(), "seed"]);
    git(&seed, &["config", "user.name", "Test"]);
    git(&seed, &["config", "user.email", "test@example.com"]);
    git(&seed, &["config", "commit.gpgsign", "false"]);
    std::fs::create_dir_all(seed.join("data")).unwrap();
    std::fs::write(seed.join("data/notes.md"), "existing note\n").unwrap();
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "-q", "-m", "seed"]);
    git(&seed, &["push", "-q", "-u", "origin", "main"]);
    std::fs::remove_dir_all(&seed).unwrap();

    origin
}

/// Hide any git identity this machine already has.
///
/// Without this the test asserts nothing on a machine with a global
/// `user.email`, which is every developer's laptop and every GitHub runner.
/// `ensure_identity` legitimately accepts a global identity and returns
/// success without writing anything locally, and that outcome is *identical*
/// to the bug, where it was never called at all.
///
/// Missing this shipped a test that passed on my machine and failed on both CI
/// platforms, which is the exact failure mode the test was written to prevent,
/// arriving one level up. `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` point
/// git at an empty file, so the only identity that can exist is one the code
/// under test put there.
///
/// One shared path rather than a per-test temporary: these run in parallel in
/// one process, and every caller writing the same value makes the race
/// harmless.
fn hide_ambient_git_identity() {
    let empty = std::env::temp_dir().join("jotbay-tests-empty.gitconfig");
    let _ = std::fs::write(&empty, "");
    std::env::set_var("GIT_CONFIG_GLOBAL", &empty);
    std::env::set_var("GIT_CONFIG_SYSTEM", &empty);
}

/// Whichever way it goes, prove the identity step was not skipped.
///
/// With ambient identity hidden, exactly two honest outcomes remain: `gh` is
/// signed in and the route writes a noreply address, or it is not and the
/// route refuses and says so. Asserting either alone would test the machine
/// rather than the code. What cannot happen is returning success having
/// neither set an identity nor complained, precisely what the bug did, on the
/// route a user with existing notes actually picks.
fn assert_identity_was_settled(outcome: &Result<PathBuf, jotbay_core::Error>, repo: &Path) {
    let email = local_config(repo, "user.email");
    assert!(
        outcome.is_err() || email.is_some(),
        "setup reported success without settling a git identity. The commit \
         that follows would be authored from the global config and the first \
         push would fail with GH007. Local user.email: {email:?}"
    );
}

#[test]
fn cloning_an_existing_repository_settles_the_git_identity() {
    hide_ambient_git_identity();
    let tmp = std::env::temp_dir().join("jotbay-route-clone");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let origin = origin_with_history(&tmp);
    let dest = tmp.join("vault");

    let outcome = jotbay_core::setup::clone_existing(origin.to_str().unwrap(), &dest);

    // The clone itself must have happened either way. The identity step runs
    // after it, so a failure here would mean something else broke.
    assert!(dest.join(".git").exists(), "the repository was not cloned");
    assert_eq!(
        git(&dest, &["rev-parse", "--abbrev-ref", "@{u}"]),
        "origin/main",
        "the fixture is wrong: this clone has no upstream, so it does not \
         exercise the early return that hid the bug"
    );
    assert_identity_was_settled(&outcome, &dest);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn adopting_an_existing_clone_settles_the_git_identity() {
    hide_ambient_git_identity();
    let tmp = std::env::temp_dir().join("jotbay-route-adopt");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let origin = origin_with_history(&tmp);
    let existing = tmp.join("already-here");
    git(&tmp, &["clone", "-q", origin.to_str().unwrap(), "already-here"]);

    let outcome = jotbay_core::setup::adopt(&existing);

    assert_identity_was_settled(&outcome, &existing);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn every_route_seeds_the_vault_layout() {
    hide_ambient_git_identity();
    // Shared tail, separately reachable. `seed` is what puts data/ and the
    // normalisation attributes in place, and a route that skipped it would
    // produce a vault whose line endings differ per platform. The failure the
    // sync suite had to learn about on Windows.
    let tmp = std::env::temp_dir().join("jotbay-route-seed");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let origin = origin_with_history(&tmp);
    let dest = tmp.join("vault");
    let _ = jotbay_core::setup::clone_existing(origin.to_str().unwrap(), &dest);

    assert!(dest.join("data").is_dir(), "clone_existing did not seed data/");
    let attrs = std::fs::read_to_string(dest.join(".gitattributes"))
        .expect("clone_existing did not seed .gitattributes");
    assert!(attrs.contains("eol=lf"), "normalisation attributes missing");

    // Adoption seeds too, and must not disturb what is already there.
    let existing = tmp.join("already-here");
    git(&tmp, &["clone", "-q", origin.to_str().unwrap(), "already-here"]);
    let before = std::fs::read_to_string(existing.join("data/notes.md")).unwrap();
    let _ = jotbay_core::setup::adopt(&existing);
    assert!(existing.join("data").is_dir(), "adopt did not seed data/");
    assert_eq!(
        std::fs::read_to_string(existing.join("data/notes.md")).unwrap(),
        before,
        "adopt overwrote a note that was already in the vault"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
