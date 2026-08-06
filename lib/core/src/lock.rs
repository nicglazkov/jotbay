use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

/// A whole-jotbay mutex, so a scheduled sync cannot land on top of a manual one.
///
/// `create_dir` is atomic on every filesystem we target, which `File::create`
/// is not, and unlike `flock` it exists on macOS, Linux and Windows alike.
pub struct SyncLock {
    path: PathBuf,
}

impl SyncLock {
    /// Where the lock lives: inside `.git`, never in the working tree.
    ///
    /// It used to sit at `<root>/.jotbay-lock`, which was invisible to git only
    /// by accident. Git does not track empty directories, and the lock was
    /// always empty. The moment it gained a `pid` file the vault became
    /// permanently dirty, so every sync committed the lock, which dirtied it
    /// again. Caught by the integration suite before it shipped.
    ///
    /// `.git` is the right home regardless: this is coordination state, not
    /// notes, and it is exactly where git keeps its own `index.lock`. It is on
    /// the same filesystem, so the atomicity argument below still holds.
    fn path_for(root: &Path) -> PathBuf {
        let git = root.join(".git");
        if git.is_dir() {
            git.join("jotbay-lock")
        } else {
            // A worktree or submodule keeps a `.git` *file*. Falling back to
            // the old location keeps those working rather than failing.
            root.join(".jotbay-lock")
        }
    }

    pub fn acquire(root: &Path) -> Result<Self> {
        let path = Self::path_for(root);
        match std::fs::create_dir(&path) {
            Ok(()) => {
                Self::write_owner(&path);
                Ok(Self { path })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // A lock left behind by a killed process would block every
                // future sync forever, so treat a clearly stale one as free.
                if Self::is_stale(&path) {
                    let _ = std::fs::remove_dir_all(&path);
                    std::fs::create_dir(&path)?;
                    Self::write_owner(&path);
                    Ok(Self { path })
                } else {
                    Err(Error::Locked)
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Record which process holds this, so a lock nobody owns can be spotted
    /// immediately instead of waited out.
    fn write_owner(path: &Path) {
        let _ = std::fs::write(path.join("pid"), std::process::id().to_string());
    }

    /// Whether this lock can be taken even though the directory exists.
    ///
    /// Two tests, cheapest first.
    ///
    /// **Nobody owns it.** `Drop` removes the lock on a clean exit, but a
    /// SIGTERM does not unwind, so any hard stop of the watcher strands one
    /// and *every upgrade stops the watcher*. That left a fifteen-minute window
    /// after each one where nothing synced and the only symptom was `jotbay
    /// sync` saying "another sync is already running" about a process that no
    /// longer existed. Checking whether the recorded pid is still alive
    /// collapses that to nothing.
    ///
    /// **Or it is simply ancient.** Kept as the fallback for a lock written
    /// before this pid file existed, one whose pid is unreadable, and the case
    /// the pid check cannot decide: a pid may have been recycled by an
    /// unrelated process, which only makes us wait, never take a live lock.
    fn is_stale(path: &Path) -> bool {
        if let Ok(text) = std::fs::read_to_string(path.join("pid")) {
            if let Ok(pid) = text.trim().parse::<u32>() {
                if pid != std::process::id() && !process_alive(pid) {
                    return true;
                }
            }
        }

        const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(15 * 60);
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().unwrap_or_default() > STALE_AFTER)
            .unwrap_or(false)
    }
}

/// Whether a process with this id currently exists.
///
/// Deliberately conservative: anything ambiguous answers "alive", so an
/// uncertain reading makes a caller wait rather than seize a lock somebody is
/// holding. Waiting is recoverable; two syncs interleaving their git
/// operations is not.
fn process_alive(pid: u32) -> bool {
    // Never a real process id, and on unix `kill(0, )` means "my whole process
    // group", which succeeds, and would report a nonexistent owner as alive.
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        // Signal 0 performs the permission and existence checks without
        // delivering anything. EPERM (1) means it exists but belongs to
        // somebody else, which is still very much alive.
        unsafe { libc_kill(pid as i32, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(1) }
    }
    #[cfg(windows)]
    {
        crate::proc::quiet("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(true)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

impl Drop for SyncLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jotbay-lock-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        // A real .git directory, because that is where the lock now lives. A
        // fixture without one silently exercises the worktree fallback and
        // proves nothing about the path everybody actually uses.
        std::fs::create_dir_all(d.join(".git")).unwrap();
        d
    }

    #[test]
    fn a_second_acquire_is_refused_while_the_first_is_held() {
        let root = scratch("held");
        let first = SyncLock::acquire(&root).expect("first acquire");
        assert!(
            matches!(SyncLock::acquire(&root), Err(Error::Locked)),
            "two syncs could interleave their git operations"
        );
        drop(first);
        // And releasing it makes the lock available again.
        assert!(SyncLock::acquire(&root).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_lock_whose_owner_is_gone_is_taken_immediately() {
        // The upgrade case. SIGTERM does not unwind, so a stopped watcher
        // strands its lock, and every upgrade stops the watcher. Before the
        // pid check this blocked all syncing for fifteen minutes, and the only
        // symptom was "another sync is already running" about a process that
        // no longer existed.
        let root = scratch("dead-owner");
        let path = SyncLock::path_for(&root);
        std::fs::create_dir(&path).unwrap();

        // A pid that is genuinely gone, obtained by watching one exit rather
        // than by picking a number and hoping. The first attempt used 0, which
        // looks dead and is not: `kill(0, 0)` signals the caller's own process
        // group and succeeds.
        let mut child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) { vec!["/C", "exit"] } else { vec![] })
            .spawn()
            .expect("spawn a process that exits immediately");
        let dead_pid = child.id();
        child.wait().expect("reap it");
        std::fs::write(path.join("pid"), dead_pid.to_string()).unwrap();

        assert!(
            SyncLock::acquire(&root).is_ok(),
            "a lock owned by a dead process must not block a sync"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_lock_held_by_a_live_process_is_respected() {
        // Our own pid is unambiguously alive, so this must be refused rather
        // than seized. Waiting is recoverable; interleaving is not.
        let root = scratch("live-owner");
        let path = SyncLock::path_for(&root);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("pid"), std::process::id().to_string()).unwrap();

        assert!(
            matches!(SyncLock::acquire(&root), Err(Error::Locked)),
            "a lock held by a running process was taken from under it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_lock_with_no_pid_file_still_falls_back_to_age() {
        // Written by a version before the pid file existed. Fresh, so it must
        // be respected; the age rule is what eventually frees it.
        let root = scratch("no-pid");
        let path = SyncLock::path_for(&root);
        std::fs::create_dir(&path).unwrap();
        assert!(matches!(SyncLock::acquire(&root), Err(Error::Locked)));
        let _ = std::fs::remove_dir_all(&root);
    }
}
