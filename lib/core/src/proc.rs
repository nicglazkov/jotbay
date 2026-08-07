//! Spawning child processes without a console window appearing.
//!
//! On Windows a console subprocess started by a windowed parent allocates its
//! own console, so `git`, `gh` and `powershell` each flash a black window and
//! steal focus. First run is the worst case, capabilities, clone, identity,
//! shortcuts and scheduling in a row, each with its own flash.
//!
//! `git.rs` had this from the start; nothing else did, because nothing else was
//! called from a GUI until first-run setup arrived and then started registering
//! schedulers and making shortcuts.

use std::process::Command;

/// A `Command` that never allocates a console on Windows.
///
/// Use this for every child process in the core. The flag is inert everywhere
/// else, so there is no reason to spawn any other way.
pub fn quiet(program: &str) -> Command {
    // Resolved rather than taken bare, so a windowed process finds tools that
    // only a shell profile would normally put on PATH.
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = Command::new(resolve(program));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Resolve a command to an absolute path, searching beyond `PATH`.
///
/// A GUI application launched from Finder or the Dock inherits
/// `/usr/bin:/bin:/usr/sbin:/sbin` and nothing else. `git` lives in `/usr/bin`
/// and is found; `gh` is installed by Homebrew into `/opt/homebrew/bin`, which
/// is not on that list. So the app reported "Needs the GitHub CLI (gh)
/// installed and signed in" on a machine where gh was installed and signed in,
/// and "Create one for me" was permanently disabled for anybody who had
/// installed gh the usual way.
///
/// Invisible from a terminal, where the shell profile has already added those
/// directories, which is why it survived until somebody launched the app from
/// the Dock on a clean machine.
///
/// Returns the bare name when nothing is found, so callers still produce the
/// normal "command not found" rather than a path that does not exist.
pub fn resolve(program: &str) -> String {
    // Windows needs none of this and must not have it. A GUI application there
    // inherits the full machine and user PATH from the registry, so installed
    // tools are already reachable, and CreateProcess does its own search
    // including the executable extensions this function knows nothing about.
    // Returning a bare name lets Windows do the job it already does correctly.
    if cfg!(target_os = "windows") {
        return program.to_string();
    }

    // Trust PATH first: a user who put a specific build ahead of everything
    // else meant it.
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(program);
            if candidate.is_file() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    let home = crate::home();
    let mut extra: Vec<std::path::PathBuf> = vec![
        // Apple Silicon Homebrew, then Intel, then the usual system locations.
        std::path::PathBuf::from("/opt/homebrew/bin"),
        std::path::PathBuf::from("/usr/local/bin"),
        std::path::PathBuf::from("/usr/bin"),
        std::path::PathBuf::from("/bin"),
        home.join(".local/bin"),
    ];
    if cfg!(target_os = "linux") {
        // Where a Flatpak, Snap or Nix install puts things.
        extra.push(std::path::PathBuf::from("/var/lib/flatpak/exports/bin"));
        extra.push(std::path::PathBuf::from("/snap/bin"));
        extra.push(home.join(".nix-profile/bin"));
    }
    for dir in extra {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    program.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Unix only: the fallback list is Unix paths, and Windows deliberately
    // short-circuits because its PATH is already complete for a windowed app.
    #[cfg(not(windows))]
    #[test]
    fn a_tool_only_homebrew_has_is_still_found_with_a_bare_path() {
        // The bug this exists for: a GUI app launched from Finder gets
        // /usr/bin:/bin:/usr/sbin:/sbin, so a Homebrew-installed gh is
        // invisible and "Create one for me" greys itself out on a machine
        // where gh is installed and signed in.
        let real = resolve("git");
        assert!(
            std::path::Path::new(&real).is_file(),
            "git did not resolve to a file: {real}"
        );

        let saved = std::env::var_os("PATH");
        std::env::set_var("PATH", "/usr/sbin:/sbin");
        let found = resolve("git");
        if let Some(p) = saved {
            std::env::set_var("PATH", p);
        }
        assert!(
            std::path::Path::new(&found).is_file(),
            "with a minimal PATH, git resolved to {found}, which is not a file"
        );
    }

    #[test]
    fn something_that_does_not_exist_comes_back_unchanged() {
        // So the caller still gets the ordinary "command not found" rather than
        // a fabricated path that fails in a more confusing way.
        assert_eq!(resolve("definitely-not-a-real-tool-xyz"), "definitely-not-a-real-tool-xyz");
    }
}
