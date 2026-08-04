//! Desktop shortcuts to the app and to the notes folder.
//!
//! Offered after setup rather than by the installer, because at install time
//! neither target exists yet: the app has not been asked where the notes go,
//! and on Windows the notes folder is not created until the first clone
//! finishes. A checkbox in an installer would have to point at a guess.
//!
//! Nothing here is required to use Jotbay. It exists because the folder is the
//! whole product for someone who never opens the window, and hunting for
//! `~/jotbay/data` in a file manager every time is a poor substitute for an
//! icon.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The application itself.
    App,
    /// The `data/` directory inside the vault — the notes, not the repository.
    Notes,
}

impl Target {
    pub fn parse(s: &str) -> Option<Target> {
        match s.trim().to_lowercase().as_str() {
            "app" | "application" | "jotbay" => Some(Target::App),
            "notes" | "folder" | "data" => Some(Target::Notes),
            _ => None,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Target::App => "Jotbay",
            Target::Notes => "Jotbay Notes",
        }
    }
}

/// Where a shortcut goes when the caller does not say.
///
/// Linux honours the XDG setting, because a localised desktop directory means
/// `~/Desktop` is an empty folder the user never sees.
pub fn default_location() -> PathBuf {
    let home = crate::home();
    if cfg!(target_os = "linux") {
        if let Some(dir) = xdg_desktop_dir() {
            return dir;
        }
    }
    home.join("Desktop")
}

fn xdg_desktop_dir() -> Option<PathBuf> {
    let config = crate::home().join(".config/user-dirs.dirs");
    let text = std::fs::read_to_string(config).ok()?;
    for line in text.lines() {
        let line = line.trim();
        let rest = line.strip_prefix("XDG_DESKTOP_DIR=")?;
        let value = rest.trim_matches('"');
        let expanded = value.strip_prefix("$HOME/").map_or_else(
            || PathBuf::from(value),
            |tail| crate::home().join(tail),
        );
        if expanded.is_dir() {
            return Some(expanded);
        }
    }
    None
}

/// Where the installed application lives on this machine, if it can be found.
///
/// Every install route puts it somewhere different — a `.dmg` dragged to
/// /Applications, an NSIS package under LocalAppData, a `.deb` in /usr/bin,
/// `install.sh` in ~/.local/bin — so this probes rather than assumes.
pub fn locate_app() -> Option<PathBuf> {
    let home = crate::home();
    let candidates: Vec<PathBuf> = if cfg!(target_os = "macos") {
        vec![
            PathBuf::from("/Applications/Jotbay.app"),
            home.join("Applications/Jotbay.app"),
        ]
    } else if cfg!(target_os = "windows") {
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Local"));
        let programs = std::env::var_os("PROGRAMFILES")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        vec![
            local.join("Programs/jotbay/jotbay-gui.exe"),
            local.join(r"Jotbay\jotbay-gui.exe"),
            programs.join(r"Jotbay\jotbay-gui.exe"),
        ]
    } else {
        vec![
            PathBuf::from("/usr/bin/jotbay-gui"),
            PathBuf::from("/usr/local/bin/jotbay-gui"),
            home.join(".local/bin/jotbay-gui"),
        ]
    };
    candidates.into_iter().find(|p| p.exists())
}

/// Create a shortcut in `location`, returning what was written.
pub fn create(target: Target, source: &Path, location: &Path) -> Result<PathBuf> {
    if !source.exists() {
        return Err(Error::Other(format!(
            "nothing to point at: {} does not exist",
            source.display()
        )));
    }
    std::fs::create_dir_all(location)?;

    if cfg!(target_os = "windows") {
        windows_link(target, source, location)
    } else if cfg!(target_os = "linux") && target == Target::App {
        linux_desktop_entry(source, location)
    } else {
        symlink(target, source, location)
    }
}

/// macOS, and folders everywhere but Windows.
///
/// A symlink is not a Finder alias, but Finder follows one to the same place
/// with the same icon, and it needs no Carbon call to make.
fn symlink(target: Target, source: &Path, location: &Path) -> Result<PathBuf> {
    let link = location.join(target.label());
    // Replacing is the expected behaviour: someone asking twice wants it to
    // work the second time, not to be told it already exists.
    if link.exists() || link.symlink_metadata().is_ok() {
        std::fs::remove_file(&link).or_else(|_| std::fs::remove_dir_all(&link))?;
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(source, &link)?;
    #[cfg(not(unix))]
    return Err(Error::Other("symlinks are not used on this platform".into()));

    #[cfg(unix)]
    Ok(link)
}

/// Linux wants a `.desktop` entry, which carries the icon and name a bare
/// symlink to a binary would not.
fn linux_desktop_entry(source: &Path, location: &Path) -> Result<PathBuf> {
    let file = location.join("jotbay.desktop");
    let contents = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Jotbay\n\
         Comment=Keep your notes in sync across machines\n\
         Exec={}\n\
         Icon=Jotbay\n\
         Terminal=false\n\
         Categories=Utility;\n",
        source.display()
    );
    std::fs::write(&file, contents)?;

    // Without the executable bit GNOME shows "Untrusted application launcher"
    // and refuses to run it, which looks like the shortcut simply not working.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(file)
}

/// Windows shortcuts are COM objects, not files with a format worth writing by
/// hand. WScript.Shell has made them since Windows 98 and is always present.
fn windows_link(target: Target, source: &Path, location: &Path) -> Result<PathBuf> {
    let link = location.join(format!("{}.lnk", target.label()));
    let script = format!(
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut('{}'); \
         $s.TargetPath = '{}'; $s.WorkingDirectory = '{}'; $s.Save()",
        escape_single(&link.to_string_lossy()),
        escape_single(&source.to_string_lossy()),
        escape_single(
            &source
                .parent()
                .unwrap_or(source)
                .to_string_lossy()
        ),
    );

    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| Error::Other(format!("could not run powershell: {e}")))?;
    if !output.status.success() {
        return Err(Error::Other(format!(
            "creating the shortcut failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(link)
}

/// PowerShell single-quoted strings escape a quote by doubling it. A path can
/// legally contain one, and without this the command silently becomes a
/// different command.
fn escape_single(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_parses_what_a_user_would_type() {
        assert_eq!(Target::parse("app"), Some(Target::App));
        assert_eq!(Target::parse(" Notes "), Some(Target::Notes));
        assert_eq!(Target::parse("folder"), Some(Target::Notes));
        assert_eq!(Target::parse("everything"), None);
    }

    #[test]
    fn escaping_survives_a_quote_in_the_path() {
        assert_eq!(escape_single(r"C:\Users\O'Brien"), r"C:\Users\O''Brien");
    }

    #[test]
    fn create_refuses_a_source_that_is_not_there() {
        let dir = std::env::temp_dir().join("jotbay-shortcut-missing");
        let err = create(Target::Notes, &dir.join("nope"), &dir).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_shortcut_resolves_back_to_the_folder() {
        let base = std::env::temp_dir().join(format!("jotbay-shortcut-{}", std::process::id()));
        let notes = base.join("data");
        let desktop = base.join("Desktop");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::create_dir_all(&desktop).unwrap();

        let link = create(Target::Notes, &notes, &desktop).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), notes);

        // Asking twice must succeed, not collide with the first answer.
        let again = create(Target::Notes, &notes, &desktop).unwrap();
        assert_eq!(again, link);

        std::fs::remove_dir_all(&base).ok();
    }
}
