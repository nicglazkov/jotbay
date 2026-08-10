//! Knowing when a machine is behind, and doing something about it.
//!
//! The awkward part of a self-syncing tool is that the repository updates
//! itself every ten minutes while the program doing the syncing never does.
//! Two machines silently sat a release behind for a day before anyone noticed,
//! and only then because someone compared version strings by hand.
//!
//! The fix needs no new network call. `lib/release.sh` writes the version it is
//! cutting into a marker file at the repository root, so the marker arrives on
//! every machine by the same sync that carries the notes. Reading it is a file
//! read; comparing it is string arithmetic.

use crate::error::{Error, Result};
use crate::git::Git;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Hidden, so the tidy repository root stays four visible entries.
pub const MARKER: &str = ".jotbay-release.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseMarker {
    pub version: String,
    #[serde(default)]
    pub tag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateStatus {
    /// The version of the binary asking the question.
    pub current: String,
    /// The newest release the repository knows about, if any.
    pub latest: Option<String>,
    pub available: bool,
}

pub fn read_marker(root: &Path) -> Option<ReleaseMarker> {
    std::fs::read(root.join(MARKER))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

pub fn write_marker(root: &Path, version: &str) -> Result<()> {
    let marker = ReleaseMarker {
        version: version.to_string(),
        tag: format!("v{version}"),
    };
    std::fs::write(root.join(MARKER), serde_json::to_vec_pretty(&marker)?)?;
    Ok(())
}

/// Where releases come from.
///
/// Deliberately not derived from the vault's `origin`: once the tool and the
/// notes live in separate repositories, a user's notes remote has no releases
/// on it, and may not even be on GitHub. A fork overrides this rather than
/// editing the source.
pub fn tool_repo() -> String {
    std::env::var("JOTBAY_TOOL_REPO").unwrap_or_else(|_| DEFAULT_TOOL_REPO.to_string())
}

const DEFAULT_TOOL_REPO: &str = "nicglazkov/jotbay";

/// How long a remote answer is trusted before asking again. Long enough that a
/// machine polling its status every twenty seconds does not hammer the API,
/// short enough that a release is noticed the same day.
const CACHE_SECS: u64 = 6 * 3600;

fn cache_path() -> std::path::PathBuf {
    crate::settings::config_dir().join("latest-release.json")
}

#[derive(Serialize, Deserialize)]
struct CachedRelease {
    version: String,
    checked_at: u64,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_cache() -> Option<String> {
    let cached: CachedRelease = std::fs::read(cache_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())?;
    Some(cached.version)
}

/// Ask GitHub what the newest release is, and remember the answer.
///
/// Called only on the paths that already touch the network. A status refresh
/// or an explicit upgrade, never from the local repaint loop, which runs every
/// twenty seconds and must not make an HTTP request to redraw a badge.
///
/// Honours the cache. `refresh_remote_now` is the one that does not.
pub fn refresh_remote() {
    refresh(false)
}

/// The same check, ignoring the cache.
///
/// For `jotbay upgrade`, where honouring a six-hour-old answer is exactly
/// wrong: a machine that cached "1.6.0" half an hour before 1.6.1 shipped
/// could not reach the new release at all, and the only way through was
/// deleting the cache file by hand. Hit on Linux during a rollout.
pub fn refresh_remote_now() {
    refresh(true)
}

fn refresh(force: bool) {
    if !force {
        if let Ok(bytes) = std::fs::read(cache_path()) {
            if let Ok(cached) = serde_json::from_slice::<CachedRelease>(&bytes) {
                if now_secs().saturating_sub(cached.checked_at) < CACHE_SECS {
                    return;
                }
            }
        }
    }

    let url = format!("https://api.github.com/repos/{}/releases/latest", tool_repo());
    let Ok(out) = crate::proc::quiet("curl")
        .args(["-fsSL", "--max-time", "10", "-H", "Accept: application/vnd.github+json", &url])
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }

    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return;
    };
    let Some(tag) = json.get("tag_name").and_then(|t| t.as_str()) else {
        return;
    };

    let record = CachedRelease {
        version: tag.trim_start_matches('v').to_string(),
        checked_at: now_secs(),
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&record) {
        let _ = std::fs::create_dir_all(crate::settings::config_dir());
        let _ = std::fs::write(cache_path(), bytes);
    }
}

/// Purely local. The marker wins when the vault carries one. A repository that
/// holds both the tool and the notes still delivers the answer by sync, with no
/// request at all, and the cached remote answer covers the case where the
/// notes repository is just notes.
pub fn check(root: &Path) -> UpdateStatus {
    decide(read_marker(root).map(|m| m.version), read_cache())
}

/// The decision alone, with both inputs handed in.
///
/// `check` reads the user's real config directory, which made its tests
/// environment-dependent: they passed until the day this machine genuinely
/// upgraded, at which point the cached "1.4.0" satisfied the fallback and a
/// test named `a_missing_marker_never_claims_an_update` started failing on
/// true behaviour.
fn decide(marker: Option<String>, cached: Option<String>) -> UpdateStatus {
    let current = crate::VERSION.to_string();
    let latest = marker.or(cached);
    let available = latest
        .as_deref()
        .map(|l| is_newer(l, &current))
        .unwrap_or(false);
    UpdateStatus {
        current,
        latest,
        available,
    }
}

/// Semantic ordering on `major.minor.patch`.
///
/// Compared numerically rather than lexically because string comparison puts
/// "1.10.0" before "1.9.0", which would leave a machine sitting on an older
/// build convinced it was current.
fn is_newer(candidate: &str, current: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        v.trim_start_matches('v')
            .split('.')
            .map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>())
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    }
    let (a, b) = (parts(candidate), parts(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// The release asset this platform needs.
pub fn asset_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "jotbay-macos-universal.tar.gz"
    } else if cfg!(target_os = "windows") {
        "jotbay-windows-x86_64.zip"
    } else {
        "jotbay-linux-x86_64.tar.gz"
    }
}

/// Where a script install puts the binaries. The only layout `upgrade` can
/// safely write to without being told.
///
/// Kept as the fallback for when the running executable cannot be located at
/// all. It is *not* the answer on its own: see `install_target`.
pub fn bin_dir() -> std::path::PathBuf {
    if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::home().join("AppData/Local"))
            .join("Programs/jotbay")
    } else {
        crate::home().join(".local/bin")
    }
}

/// Where to write the new binaries: beside the ones that are running.
///
/// This used to be `bin_dir()` unconditionally, which is right for a script
/// install and wrong for every native installer we ship. On a `.deb` machine
/// the binaries live in `/usr/bin`, so `upgrade` wrote a *second* copy to
/// `~/.local/bin`, reported success, and left the watcher, started from an
/// absolute `/usr/bin/jotbay` in its unit file. Running the old version
/// forever. `jotbay --version` then answered for PATH, which preferred the new
/// copy, so every check a human could run said the upgrade had worked. The
/// component with the bug was the one thing never touched.
///
/// The same mismatch existed on Windows (`\Jotbay` from the installer versus
/// `\Programs\jotbay` here) and on macOS, where the CLI lives inside
/// `Jotbay.app`.
///
/// So: resolve from the running executable, and refuse when that location is
/// not ours to replace. Refusing is the honest outcome. A package manager
/// owns those files, and writing a shadow copy somewhere else only recreates
/// the split.
pub fn install_target() -> Result<std::path::PathBuf> {
    let exe = match std::env::current_exe() {
        Ok(p) => p.canonicalize().unwrap_or(p),
        // Nothing to reason from; the script layout is the best guess left.
        Err(_) => return Ok(bin_dir()),
    };

    // Inside a macOS app bundle. Replacing a binary in there breaks the code
    // signature, and Gatekeeper then refuses to launch the app at all. A far
    // worse outcome than not upgrading.
    if exe.components().any(|c| c.as_os_str().to_string_lossy().ends_with(".app")) {
        return Err(Error::Other(
            "this copy lives inside Jotbay.app, which is managed by its installer. \
             Upgrade with `brew upgrade --cask jotbay`, or download the latest .dmg."
                .into(),
        ));
    }

    let dir = exe
        .parent()
        .ok_or_else(|| Error::Other("could not tell where jotbay is installed".into()))?
        .to_path_buf();

    if !is_writable(&dir) {
        return Err(Error::Other(format!(
            "jotbay is installed at {}, which this account cannot write to, \
             it came from a system package. Upgrade it the way it was installed: \
             download the latest .deb from the releases page and open it, or run \
             `sudo apt install ./Jotbay_<version>_amd64.deb`.",
            dir.display()
        )));
    }

    Ok(dir)
}

/// Whether we could actually replace a file in this directory.
///
/// Probes by creating a file rather than reading permission bits: the bits lie
/// under root-squashed mounts, immutable flags, and read-only filesystems, and
/// the only answer that matters is whether the write will succeed.
fn is_writable(dir: &std::path::Path) -> bool {
    let probe = dir.join(".jotbay-write-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Other `jotbay` executables on PATH, which are what a split install looks
/// like from the outside.
///
/// Reported after an upgrade because a second copy is invisible until it
/// disagrees: PATH answers with one, the scheduler runs the other, and the
/// version you are shown is not the version doing the work.
pub fn other_copies_on_path(installed: &std::path::Path) -> Vec<std::path::PathBuf> {
    let name = if cfg!(target_os = "windows") { "jotbay.exe" } else { "jotbay" };
    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if !candidate.is_file() {
            continue;
        }
        let real = candidate.canonicalize().unwrap_or(candidate);
        if real.parent() != Some(installed) && !found.contains(&real) {
            found.push(real);
        }
    }
    found
}

/// Fetch the current release and replace the installed binaries.
///
/// Downloads through `gh` when it is available and authenticated, because the
/// repository may be private, `releases/latest/download/` returns 404 to
/// everyone, including the owner, while it is. Falls back to a plain HTTPS
/// download so this keeps working unchanged once the repository is public.
pub fn install(root: &Path, version: &str) -> Result<Vec<String>> {
    // Before the download, not after it.
    //
    // This used to be resolved once the tarball was already unpacked, so a
    // Homebrew or .deb install paid for several megabytes it could never use,
    // and, worse, any download problem answered first: the honest "this copy
    // belongs to its installer" was replaced by "couldn't download, check that
    // gh is signed in", which sends someone to fix an authentication problem
    // they do not have.
    let dir = install_target()?;

    let repo = tool_repo();
    let asset = asset_name();
    let tmp = std::env::temp_dir().join(format!("jotbay-upgrade-{version}"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;

    // Every `?` below used to skip the cleanup at the end, so each failed
    // upgrade stranded ~8 MB of downloaded tarball in the temp directory.
    // A guard runs on the error paths too.
    struct Sweep(std::path::PathBuf);
    impl Drop for Sweep {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _sweep = Sweep(tmp.clone());

    let downloaded = tmp.join(asset);
    let fetched_with_gh = crate::proc::quiet("gh")
        .args([
            "release", "download", &format!("v{version}"),
            "--repo", &repo,
            "--pattern", asset,
            "--dir", &tmp.to_string_lossy(),
            "--clobber",
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !fetched_with_gh {
        let url = format!(
            "https://github.com/{repo}/releases/download/v{version}/{asset}"
        );
        let ok = crate::proc::quiet("curl")
            .args(["-fsSL", "-o", &downloaded.to_string_lossy(), &url])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            return Err(Error::Other(format!(
                "Couldn't download {asset}. Check that gh is signed in, or that the repository is public."
            )));
        }
    }

    if !downloaded.exists() {
        return Err(Error::Other(format!("{asset} did not download")));
    }

    // Unpack.
    let status = if asset.ends_with(".zip") {
        crate::proc::quiet("tar")
            .args(["-xf", &downloaded.to_string_lossy(), "-C", &tmp.to_string_lossy()])
            .status()
    } else {
        crate::proc::quiet("tar")
            .args(["-xzf", &downloaded.to_string_lossy(), "-C", &tmp.to_string_lossy()])
            .status()
    };
    if !status.map(|s| s.success()).unwrap_or(false) {
        return Err(Error::Other("could not unpack the release".into()));
    }


    std::fs::create_dir_all(&dir)?;
    let mut replaced = Vec::new();

    for name in binaries() {
        let from = tmp.join(name);
        if !from.exists() {
            continue;
        }
        let to = dir.join(name);
        replace_binary(&from, &to).map_err(|e| explain_busy(e, name))?;
        replaced.push(name.to_string());
    }

    // macOS ships the GUI as a bundle in the repository root rather than a
    // binary in bin_dir, so the loop above never sees it. Without this the CLI
    // upgrades and the app someone actually looks at stays on the old version.
    if cfg!(target_os = "macos") {
        let from = tmp.join("Jotbay.app");
        // Wherever it actually is: /Applications for anyone who used the .dmg,
        // the vault root for a machine set up by install.sh before the two came
        // apart. Upgrading the copy nobody launches is the same as not
        // upgrading at all.
        let to = crate::shortcut::locate_app()
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| root.join("Jotbay.app"));
        if from.is_dir() && to.parent().map(|p| p.is_dir()).unwrap_or(false) {
            let staging = to.with_file_name(".Jotbay.app.incoming");
            let _ = std::fs::remove_dir_all(&staging);
            // Copy beside the live bundle, then swap, so a failed copy cannot
            // leave a half-written app where the working one used to be.
            let copied = crate::proc::quiet("cp")
                .args(["-R", &from.to_string_lossy(), &staging.to_string_lossy()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if copied {
                let _ = std::fs::remove_dir_all(&to);
                if std::fs::rename(&staging, &to).is_ok() {
                    replaced.push("Jotbay.app".to_string());
                }
            }
            let _ = std::fs::remove_dir_all(&staging);
        }
    }

    if replaced.is_empty() {
        return Err(Error::Other("the release contained no binaries".into()));
    }
    Ok(replaced)
}

/// Turn the kernel's refusal into something a person can act on.
///
/// Linux returns ETXTBSY when anything tries to open a running executable for
/// writing. Versions up to 1.3.2 replaced binaries by truncating them in place,
/// so upgrading *to* 1.3.3, which is the release that stops doing that, fails
/// with a bare "Text file busy (os error 26)". The upgrader doing the work is
/// always the old one, so this specific failure cannot be fixed by the upgrade
/// that fixes it; the least it can do is say what to run instead.
fn explain_busy(e: Error, name: &str) -> Error {
    let text = e.to_string();
    if text.contains("Text file busy") || text.contains("os error 26") {
        return Error::Other(format!(
            "cannot replace {name} while it is running. This version of the \
upgrader writes over the live file; the fix ships in a later one. Install it \
directly instead, ./install/install.sh, or install.ps1 on Windows, and \
`jotbay upgrade` will work from then on."
        ));
    }
    e
}

fn binaries() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["jotbay.exe", "jotbay-gui.exe"]
    } else {
        &["jotbay", "jotbay-gui"]
    }
}

/// Replace a binary that may be the one currently executing.
///
/// Unix lets a running executable's path be replaced. The old inode stays
/// alive for the running process. Windows refuses to overwrite a running image
/// but does allow renaming it, so the outgoing binary is moved aside first and
/// swept up on the next upgrade.
fn replace_binary(from: &Path, to: &Path) -> Result<()> {
    if to.exists() && cfg!(target_os = "windows") {
        let parked = to.with_extension("old");
        let _ = std::fs::remove_file(&parked);
        std::fs::rename(to, &parked)?;
    }

    // Stage beside the target, then rename over it. Copying onto the live path
    // would rewrite the bytes of a file that is very likely mapped right now
    // this function usually runs from the binary it is replacing. Doing that
    // corrupts the running image and invalidates its signature; macOS then
    // SIGKILLs it, leaving an executable that is a valid Mach-O and dies at
    // exit 137. rename() swaps the directory entry instead, so the new file
    // arrives whole and the old inode stays alive for the running process.
    let staging = to.with_extension("incoming");
    let _ = std::fs::remove_file(&staging);
    std::fs::copy(from, &staging)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))?;
    }

    std::fs::rename(&staging, to)?;
    Ok(())
}

/// `owner/name`, from whatever form the remote takes.
pub fn remote_slug(git: &Git) -> Result<String> {
    let url = git.run(&["remote", "get-url", "origin"])?;
    let slug = url
        .trim()
        .rsplit_once(':')
        .map(|(_, s)| s.to_string())
        .filter(|_| url.starts_with("git@"))
        .unwrap_or_else(|| {
            url.trim()
                .rsplitn(3, '/')
                .take(2)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("/")
        });
    Ok(slug.trim_end_matches(".git").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically_not_lexically() {
        assert!(is_newer("1.10.0", "1.9.0"), "1.10 is newer than 1.9");
        assert!(is_newer("1.2.0", "1.1.9"));
        assert!(is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("1.2.0", "1.2.0"));
        assert!(!is_newer("1.1.0", "1.2.0"));
        assert!(is_newer("v1.3.0", "1.2.0"), "a leading v is tolerated");
    }

    #[test]
    fn a_binary_is_replaced_by_rename_not_by_overwrite() {
        // Copying onto a live path rewrites bytes that may be mapped by the
        // running process; on macOS that yields a valid Mach-O the kernel then
        // SIGKILLs. The replacement must arrive as a fresh inode.
        let dir = std::env::temp_dir().join("jotbay-replace-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let target = dir.join("thing");
        std::fs::write(&target, b"old").unwrap();
        let before = file_id(&target);

        let source = dir.join("new-thing");
        std::fs::write(&source, b"new").unwrap();
        replace_binary(&source, &target).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        // Inodes are the direct evidence, and only Unix has them. Windows takes
        // a different path anyway. It parks the old file as .old first, because
        // it will not overwrite a running image at all.
        #[cfg(unix)]
        assert_ne!(before, file_id(&target), "must arrive as a different inode");
        let _ = before;
        assert!(!dir.join("thing.incoming").exists(), "staging file cleaned up");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    fn file_id(p: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(p).unwrap().ino()
    }
    #[cfg(not(unix))]
    fn file_id(_p: &Path) -> u64 {
        0
    }

    #[test]
    fn no_marker_and_no_cache_never_claims_an_update() {
        let status = decide(None, None);
        assert!(!status.available);
        assert_eq!(status.latest, None);
    }

    #[test]
    fn the_cached_remote_answer_covers_a_markerless_vault() {
        // The post-split case: a notes-only repository carries no marker.
        let status = decide(None, Some("999.0.0".into()));
        assert!(status.available);
        assert_eq!(status.latest.as_deref(), Some("999.0.0"));
    }

    #[test]
    fn the_marker_wins_over_the_cache() {
        let status = decide(Some("999.0.0".into()), Some("1.0.0".into()));
        assert_eq!(status.latest.as_deref(), Some("999.0.0"));
    }

    #[test]
    fn a_marker_newer_than_the_binary_reports_an_update() {
        let tmp = std::env::temp_dir().join("jotbay-marker-test");
        let _ = std::fs::create_dir_all(&tmp);
        write_marker(&tmp, "999.0.0").unwrap();
        let status = check(&tmp);
        assert!(status.available);
        assert_eq!(status.latest.as_deref(), Some("999.0.0"));

        write_marker(&tmp, "0.0.1").unwrap();
        assert!(!check(&tmp).available);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_writable_directory_is_writable_and_a_missing_one_is_not() {
        let dir = std::env::temp_dir().join("jotbay-writable-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(is_writable(&dir));
        // The probe must clean up after itself, or every upgrade leaves litter
        // in the install directory.
        assert!(
            !dir.join(".jotbay-write-probe").exists(),
            "the write probe left its file behind"
        );

        assert!(
            !is_writable(&dir.join("does-not-exist")),
            "a directory that is not there cannot be written to"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_copy_in_the_install_directory_is_not_reported_as_a_rival() {
        // other_copies_on_path exists to surface split installs. Reporting the
        // binary we just upgraded would make the warning fire on every healthy
        // machine, and a warning that always fires is one nobody reads.
        let dir = std::env::temp_dir().join("jotbay-copies-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let name = if cfg!(target_os = "windows") { "jotbay.exe" } else { "jotbay" };
        std::fs::write(dir.join(name), b"").unwrap();

        let canonical = dir.canonicalize().unwrap_or(dir.clone());
        let saved = std::env::var_os("PATH");
        std::env::set_var("PATH", &canonical);
        let found = other_copies_on_path(&canonical);
        if let Some(p) = saved {
            std::env::set_var("PATH", p);
        }

        assert!(
            found.is_empty(),
            "the installed copy was reported as a rival: {found:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
