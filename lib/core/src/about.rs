//! What this machine is, in one struct.
//!
//! Every surface wants the same handful of facts to show in a settings panel:
//! which version is installed, where the notes are, which remote they go to,
//! and whether the background sync is doing its job. Each surface used to
//! answer those separately, or not at all, and the answers drifted.
//!
//! Nothing here touches the network. `update_available` reports the marker the
//! repository already carries, so opening settings is not a request to GitHub.

use std::path::PathBuf;

use serde::Serialize;

use time::OffsetDateTime;

use crate::{count_files, schedule, settings, status, update, Jotbay, Result};

#[derive(Debug, Clone, Serialize)]
pub struct About {
    /// The version of the binary answering this call.
    pub version: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,

    pub root: PathBuf,
    pub notes: PathBuf,
    pub branch: String,
    /// Credentials removed. See [`sanitize_remote`].
    pub remote: Option<String>,
    pub files: usize,

    pub config_path: PathBuf,
    /// Where `jotbay upgrade` looks, so a fork or a mirror is visible rather
    /// than surprising.
    pub tool_repo: String,
    pub update_available: Option<String>,

    pub sync: SyncHealth,
}

/// Whether the background sync is installed, and whether it is the version
/// that is installed on this machine.
///
/// The second half matters more than it sounds. Replacing the binaries does
/// not restart the watcher, so a fully upgraded machine can keep syncing with
/// the previous version indefinitely, and it publishes that version as this
/// machine's. Anyone reading the node list to check a rollout is told the
/// reassuring thing rather than the true one.
///
/// This is derived from the status ref this machine last published rather than
/// by looking for a process, which needs no process table and no guessing at
/// what a watcher's command line looks like.
#[derive(Debug, Clone, Serialize)]
pub struct SyncHealth {
    /// A schedule is registered with launchd, systemd or Task Scheduler.
    pub scheduled: bool,
    /// The version that last published this machine's status, which is the
    /// version actually doing the syncing.
    pub running_version: Option<String>,
    /// Seconds since this machine last published. None if it never has.
    pub last_report_secs: Option<i64>,
    /// True when the running version is behind the installed one, so the
    /// remedy is a restart rather than an upgrade.
    pub restart_needed: bool,
}

/// Strip anything secret from a remote URL before showing it.
///
/// A remote cloned over HTTPS with a token embedded reads
/// `https://x-access-token:ghp_...@github.com/you/notes.git`, and a settings
/// panel is exactly the sort of place a person screenshots for a bug report.
/// The userinfo is the whole risk, so it goes, and the rest stays readable.
pub fn sanitize_remote(url: &str) -> String {
    // scheme://userinfo@host/path -> scheme://host/path. SSH's scp-like form
    // (git@github.com:you/notes.git) carries a username but never a secret, so
    // it is left as it is.
    if let Some(scheme_end) = url.find("://") {
        let (scheme, rest) = url.split_at(scheme_end + 3);
        if let Some(at) = rest.find('@') {
            // Only userinfo, never a path segment that happens to contain '@'.
            let host_start = rest.find('/').unwrap_or(rest.len());
            if at < host_start {
                return format!("{scheme}{}", &rest[at + 1..]);
            }
        }
    }
    url.to_string()
}

impl Jotbay {
    /// Collect everything a settings panel needs. Local reads only.
    pub fn about(&self) -> Result<About> {
        let installed = env!("CARGO_PKG_VERSION").to_string();
        let hostname = Jotbay::hostname();

        let remote = self
            .git
            .run(&["remote", "get-url", "origin"])
            .ok()
            .map(|url| sanitize_remote(url.trim()));

        let branch = self
            .git
            .run(&["rev-parse", "--abbrev-ref", "HEAD"])
            .unwrap_or_default()
            .trim()
            .to_string();

        // What this machine published about itself, which is what the rest of
        // the mesh sees.
        let mine = status::read_all(&self.git)
            .unwrap_or_default()
            .into_iter()
            .find(|n| n.hostname == hostname);

        let running_version = mine.as_ref().map(|n| n.agent_version.clone());
        let last_report_secs = mine
            .as_ref()
            .map(|n| (OffsetDateTime::now_utc() - n.last_sync).whole_seconds());
        let restart_needed = running_version
            .as_deref()
            .is_some_and(|running| running != installed);

        Ok(About {
            version: installed,
            hostname,
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            root: self.git.root().to_path_buf(),
            notes: self.data_dir(),
            branch,
            remote,
            files: count_files(&self.data_dir()) as usize,
            config_path: settings::settings_path(),
            tool_repo: update::tool_repo(),
            update_available: {
                let u = update::check(self.git.root());
                if u.available { u.latest } else { None }
            },
            sync: SyncHealth {
                scheduled: schedule::is_installed(),
                running_version,
                last_report_secs,
                restart_needed,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_remote;

    #[test]
    fn strips_a_token_from_an_https_remote() {
        assert_eq!(
            sanitize_remote("https://x-access-token:ghp_secret@github.com/you/notes.git"),
            "https://github.com/you/notes.git"
        );
        assert_eq!(
            sanitize_remote("https://user:pass@gitlab.com/you/notes.git"),
            "https://gitlab.com/you/notes.git"
        );
    }

    #[test]
    fn leaves_ordinary_remotes_alone() {
        for url in [
            "https://github.com/you/notes.git",
            "git@github.com:you/notes.git",
            "ssh://git@codeberg.org/you/notes.git",
            "/srv/git/notes.git",
        ] {
            let cleaned = sanitize_remote(url);
            // The SSH forms carry a username, never a secret, so they survive
            // whole. Only an https userinfo is removed.
            if url.starts_with("ssh://") {
                assert_eq!(cleaned, "ssh://codeberg.org/you/notes.git");
            } else {
                assert_eq!(cleaned, url);
            }
        }
    }

    #[test]
    fn an_at_sign_in_the_path_is_not_userinfo() {
        assert_eq!(
            sanitize_remote("https://example.com/~user/no@ise.git"),
            "https://example.com/~user/no@ise.git"
        );
    }
}
