//! A thin wrapper over the `git` command line.
//!
//! Deliberately shells out rather than linking libgit2. The user's credential
//! helper (`gh auth git-credential`), SSH commit signing, and per-repo config
//! all live in git's own configuration; reimplementing that surface with a
//! library would silently diverge from what `git` does on the command line.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Build a `git` Command that never flashes a console.
///
/// When the parent is a windowed app - the Tauri GUI, whose status poll runs
/// around ten git calls at a time - each child git.exe allocates its own
/// console window, and the desktop fills with cmd windows appearing and
/// vanishing in bursts, stealing focus as they go. The CLI never shows this
/// because its children inherit the console it already has. CREATE_NO_WINDOW
/// suppresses the console allocation; output is unaffected because every call
/// site pipes it.
fn git_command() -> Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = Command::new("git");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// How long a single network git call may take before it is killed.
///
/// Generous for a notes-sized push and far short of the four-minute stall that
/// prompted it. The scheduler runs every ten minutes, so a hung call must die
/// well before the next one starts or they queue up behind each other.
pub const NETWORK_TIMEOUT: Duration = Duration::from_secs(120);

/// What a network git call did, including what it said on the way out.
#[derive(Debug, Clone)]
pub struct NetOutcome {
    pub success: bool,
    pub stderr: String,
    /// Killed at the deadline rather than having failed on its own.
    pub timed_out: bool,
}

impl NetOutcome {
    /// One sentence naming what happened, for an activity event.
    pub fn describe(&self, what: &str) -> String {
        if self.timed_out {
            return format!("{what} timed out after {}s", NETWORK_TIMEOUT.as_secs());
        }
        if self.stderr.is_empty() {
            return format!("{what} failed");
        }
        format!("{what} failed: {}", self.stderr.lines().next().unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct Git {
    root: PathBuf,
}

impl Git {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        // `git rev-parse --show-toplevel` reports POSIX separators even on
        // Windows, and `--jotbay` accepts either form. Joining onto such a root
        // yields a mixed path like `C:/a/b\data`, which ShellExecute accepts and
        // then silently opens nothing - that is why the GUI's "Open Folder" and
        // the tray's "Open Jotbay Folder" were both dead on Windows while an
        // all-backslash or all-forward path opened Explorer fine. Normalise once,
        // at the only place a root is ever constructed.
        #[cfg(windows)]
        let root = PathBuf::from(root.to_string_lossy().replace('/', "\\"));
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Locate the repository containing `start`, walking upward.
    pub fn discover(start: &Path) -> Result<Self> {
        let out = git_command()
            .current_dir(start)
            .args(["rev-parse", "--show-toplevel"])
            .stderr(Stdio::null())
            .output()
            .map_err(Error::GitMissing)?;

        if !out.status.success() {
            return Err(Error::NotAJotbay(start.to_path_buf()));
        }
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(Self::new(path))
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = git_command();
        cmd.current_dir(&self.root);
        // Never let git open an editor or a credential prompt from a scheduled
        // run: both would block forever with no way to answer.
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        cmd.env("GIT_EDITOR", "true");
        cmd.args(args);
        cmd
    }

    /// Run git, returning trimmed stdout. Errors on a non-zero exit.
    pub fn run(&self, args: &[&str]) -> Result<String> {
        let out = self.command(args).output().map_err(Error::GitMissing)?;
        if !out.status.success() {
            return Err(Error::Git {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    }

    /// Run git, returning whether it succeeded. For probes where failure is a
    /// legitimate answer rather than an error (`rebase`, `push` races).
    pub fn try_run(&self, args: &[&str]) -> Result<bool> {
        let status = self
            .command(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(Error::GitMissing)?;
        Ok(status.success())
    }

    /// A git call that talks to the network, with a deadline.
    ///
    /// Everything else here can only block on the local filesystem. `fetch` and
    /// `push` can block on a remote that has stopped answering, and one did:
    /// four minutes at zero CPU inside a scheduled sync, the child parked in
    /// `poll_schedule_timeout`. Two things made that worse than it needed to be.
    /// The scheduler had no deadline, so a stall was indistinguishable from work.
    /// And the child ran with stderr on /dev/null, so whatever git was waiting
    /// on could not have been reported even if it had said so.
    ///
    /// This gives network calls a deadline and keeps what they said.
    pub fn run_networked(&self, args: &[&str], timeout: Duration) -> Result<NetOutcome> {
        self.networked(args, timeout, false).map(|(o, _)| o)
    }

    /// The same, keeping stdout, for a call whose answer *is* its output.
    ///
    /// `ls-remote` is the reason this exists: it is the cheapest question you
    /// can ask a remote, and the watcher asks it constantly to avoid asking
    /// expensive ones.
    pub fn run_networked_out(
        &self,
        args: &[&str],
        timeout: Duration,
    ) -> Result<(NetOutcome, String)> {
        self.networked(args, timeout, true)
    }

    fn networked(
        &self,
        args: &[&str],
        timeout: Duration,
        capture_stdout: bool,
    ) -> Result<(NetOutcome, String)> {
        let mut child = self
            .command(args)
            .stdin(Stdio::null())
            .stdout(if capture_stdout {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stderr(Stdio::piped())
            .spawn()
            .map_err(Error::GitMissing)?;

        // Drained on its own thread for the same reason stderr is: a full pipe
        // blocks the writer forever. `ls-remote` on a repository with many refs
        // produces more than enough to fill one.
        let mut out_pipe = child.stdout.take();
        let out_reader = std::thread::spawn(move || {
            let mut buf = String::new();
            if let Some(p) = out_pipe.as_mut() {
                use std::io::Read;
                let _ = p.read_to_string(&mut buf);
            }
            buf
        });

        // Drained on its own thread. A full pipe blocks the writer forever,
        // which would be the same hang this exists to prevent, arriving by a
        // different route.
        let mut pipe = child.stderr.take();
        let reader = std::thread::spawn(move || {
            let mut buf = String::new();
            if let Some(p) = pipe.as_mut() {
                use std::io::Read;
                let _ = p.read_to_string(&mut buf);
            }
            buf
        });

        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait().map_err(Error::GitMissing)? {
                Some(status) => {
                    return Ok((
                        NetOutcome {
                            success: status.success(),
                            stderr: reader.join().unwrap_or_default().trim().to_string(),
                            timed_out: false,
                        },
                        out_reader.join().unwrap_or_default(),
                    ))
                }
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok((
                        NetOutcome {
                            success: false,
                            stderr: reader.join().unwrap_or_default().trim().to_string(),
                            timed_out: true,
                        },
                        out_reader.join().unwrap_or_default(),
                    ));
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }

    /// Raw bytes of stdout, used for blob contents, which may not be UTF-8.
    pub fn run_bytes(&self, args: &[&str]) -> Result<Vec<u8>> {
        let out = self.command(args).output().map_err(Error::GitMissing)?;
        if !out.status.success() {
            return Err(Error::Git {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(out.stdout)
    }

    /// Feed `input` to git on stdin and return trimmed stdout.
    pub fn run_with_stdin(&self, args: &[&str], input: &[u8]) -> Result<String> {
        use std::io::Write;
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(Error::GitMissing)?;
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(input)?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            return Err(Error::Git {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    }

    // --- common queries ----------------------------------------------------

    pub fn current_branch(&self) -> Result<String> {
        self.run(&["rev-parse", "--abbrev-ref", "HEAD"])
    }

    pub fn head(&self) -> Result<String> {
        self.run(&["rev-parse", "HEAD"])
    }

    pub fn head_short(&self) -> Result<String> {
        self.run(&["rev-parse", "--short", "HEAD"])
    }

    pub fn has_upstream(&self) -> bool {
        self.run(&["rev-parse", "--abbrev-ref", "@{u}"]).is_ok()
    }

    /// Files with uncommitted changes, staged or not.
    pub fn dirty_files(&self) -> Result<Vec<String>> {
        let out = self.run(&["status", "--porcelain"])?;
        Ok(out
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l[3.min(l.len())..].to_string())
            .collect())
    }

    pub fn is_dirty(&self) -> Result<bool> {
        Ok(!self.run(&["status", "--porcelain"])?.trim().is_empty())
    }

    /// (ahead, behind) relative to upstream.
    pub fn ahead_behind(&self) -> Result<(u32, u32)> {
        if !self.has_upstream() {
            return Ok((0, 0));
        }
        let out = self.run(&["rev-list", "--left-right", "--count", "HEAD...@{u}"])?;
        let mut parts = out.split_whitespace();
        let ahead = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let behind = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        Ok((ahead, behind))
    }

    pub fn rebase_in_progress(&self) -> bool {
        let git_dir = self.root.join(".git");
        git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists()
    }

    /// Paths left unmerged by a stopped rebase.
    pub fn conflicted_paths(&self) -> Result<Vec<String>> {
        let out = self.run(&["diff", "--name-only", "--diff-filter=U"])?;
        Ok(out
            .lines()
            .map(str::to_string)
            .filter(|l| !l.is_empty())
            .collect())
    }

    /// Which merge stages exist for a conflicted path.
    /// Stage 2 is "ours", stage 3 is "theirs".
    pub fn conflict_stages(&self, path: &str) -> Result<(bool, bool)> {
        let out = self.run(&["ls-files", "-u", "--", path])?;
        let mut has_ours = false;
        let mut has_theirs = false;
        for line in out.lines() {
            // <mode> <sha> <stage>\t<path>
            if let Some(meta) = line.split('\t').next() {
                match meta.split_whitespace().nth(2) {
                    Some("2") => has_ours = true,
                    Some("3") => has_theirs = true,
                    _ => {}
                }
            }
        }
        Ok((has_ours, has_theirs))
    }

    pub fn stage_blob(&self, stage: u8, path: &str) -> Result<Vec<u8>> {
        self.run_bytes(&["show", &format!(":{stage}:{path}")])
    }
}
