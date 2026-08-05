//! Asking the other machines whether they are still there.
//!
//! Liveness used to be a side effect: every machine synced on a fixed timer, so
//! its status ref was always fresh, and "recently published" stood in for
//! "alive". Once polling became conditional, an idle machine stopped syncing —
//! correctly, there was nothing to sync — and therefore stopped publishing. A
//! healthy fleet with nothing to do reported itself as entirely offline.
//!
//! The obvious repair is a heartbeat: publish every few minutes regardless. It
//! works, and it is the wrong trade. It spends traffic all day answering a
//! question nobody is asking, and it still cannot tell "idle but alive" from
//! "published five minutes ago, then the lid closed".
//!
//! So this asks instead. Opening a window bumps one ref; every watcher is
//! already running an `ls-remote` on its poll and sees it move for free; each
//! one answers by publishing its status. A machine that is asleep does not
//! answer, which is the actual question. Zero traffic until somebody looks,
//! and a real answer when they do.
//!
//! **The roll-call ref must never be moved by answering it.** Watching status
//! refs directly is what this deliberately avoids: every sync republishes
//! status, so machines would wake each other in a loop that never settles.
//! Nothing here writes the roll-call ref except an explicit request from a
//! person, and publishing a status ref cannot move it.

use crate::error::Result;
use crate::git::Git;
use time::OffsetDateTime;

/// The single ref every machine watches for a roll call.
///
/// Outside `refs/jotbay-status/*` on purpose, so the status refspec neither
/// fetches nor overwrites it.
///
/// The trailing component is not decoration. `receive-pack` rejects a ref one
/// level under `refs/` as a "funny ref" — `refs/jotbay-rollcall` is refused
/// remotely even though it is a perfectly good local ref, which is why the
/// status refs have always had a hostname after them.
pub const ROLLCALL_REF: &str = "refs/jotbay-rollcall/current";

/// How long after a roll call the fleet stays attentive.
///
/// While inside this window a watcher holds its poll at the base interval
/// instead of backing off, so a window left open sees presence stay current
/// rather than decay. Long enough to cover someone reading their notes,
/// short enough that a forgotten window does not poll all night.
pub const ATTENTION: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Ask every machine to report in.
///
/// Best-effort by design: this is a courtesy to whoever is looking at a
/// window, and it must never be the reason an app fails to open.
pub fn request(git: &Git) -> Result<()> {
    // The content only has to differ from last time. A timestamp does that and
    // is worth reading in a pinch; the ref is force-pushed, so no history
    // accumulates behind it.
    let body = format!(
        "roll call from {} at {}\n",
        crate::Jotbay::hostname(),
        OffsetDateTime::now_utc()
    );
    let blob = git.run_with_stdin(&["hash-object", "-w", "--stdin"], body.as_bytes())?;
    let tree = git.run_with_stdin(
        &["mktree"],
        format!("100644 blob {blob}\trollcall\n").as_bytes(),
    )?;
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
        "roll call",
    ])?;

    git.run(&["update-ref", ROLLCALL_REF, &commit])?;
    let out = git.run_networked(
        &["push", "--quiet", "--force", "origin", &format!("{ROLLCALL_REF}:{ROLLCALL_REF}")],
        crate::git::NETWORK_TIMEOUT,
    )?;
    if !out.success {
        return Err(crate::error::Error::Other(out.describe("asking the other machines to report in")));
    }
    Ok(())
}
