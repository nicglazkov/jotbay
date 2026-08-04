//! Per-machine preferences.
//!
//! Deliberately **not** stored in the vault. Everything under the repository
//! syncs, and these are exactly the settings that should not: a laptop that
//! follows the system theme and a desktop pinned to dark are both correct at
//! once, and one machine's verbose mode is nobody else's business.
//!
//! One file, read by the CLI and both GUIs, so a preference set in the window
//! is honoured by the scheduled sync and vice versa.

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the operating system.
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub fn parse(s: &str) -> Option<Theme> {
        match s.trim().to_lowercase().as_str() {
            "system" | "auto" => Some(Theme::System),
            "light" => Some(Theme::Light),
            "dark" => Some(Theme::Dark),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    /// Where this machine's vault lives.
    ///
    /// An installed app is launched from /Applications or Program Files, so the
    /// working directory says nothing about where the notes are. First-run
    /// setup records the answer here; the CLI still prefers whatever repository
    /// it is invoked inside, so `cd`-ing into a different vault keeps working.
    #[serde(default)]
    pub vault_path: Option<String>,
    pub theme: Theme,
    /// Show raw underlying detail — full git stderr, unabridged errors.
    ///
    /// Off by default because the common failures are far more useful as one
    /// sentence: a push rejected for a private email address rendered five
    /// lines of `remote: error: …` with URLs in it, which told a first-time
    /// user nothing and swamped the pane it appeared in.
    pub verbose: bool,
}

/// `~/Library/Application Support/Jotbay` · `~/.config/jotbay` ·
/// `%APPDATA%\Jotbay`, following each platform's convention.
pub fn config_dir() -> PathBuf {
    let home = crate::home();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Jotbay")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Roaming"))
            .join("Jotbay")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("jotbay")
    }
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

/// Where the settings lived before the tool was renamed in 1.4.0.
///
/// This matters more than a preferences file usually would: `vault_path` lives
/// here, and it is the only thing an installed app has to find the notes with.
/// Lose it and every machine that upgrades opens onto the first-run screen
/// asking where its notes are, as though it had never been set up.
fn legacy_config_dir() -> Option<PathBuf> {
    let home = crate::home();
    let dir = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Inkway")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Roaming"))
            .join("Inkway")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("inkway")
    };
    dir.is_dir().then_some(dir)
}

/// Copy the old settings across, once, if the new location has none.
///
/// Copies rather than moves: an older binary may still be on the machine — the
/// upgrade replaces one copy, not every copy — and it would find an empty
/// directory and re-run setup. Leaving the original costs a few hundred bytes.
fn migrate_settings() {
    let target = settings_path();
    if target.exists() {
        return;
    }
    let Some(legacy) = legacy_config_dir() else {
        return;
    };
    let source = legacy.join("settings.json");
    if !source.exists() {
        return;
    }
    if std::fs::create_dir_all(config_dir()).is_ok() {
        let _ = std::fs::copy(&source, &target);
    }
}

impl Settings {
    /// Never fails: a missing or unreadable file means defaults. Preferences
    /// are not worth aborting a sync over.
    pub fn load() -> Settings {
        migrate_settings();
        std::fs::read(settings_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(settings_path(), serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_parses_the_forms_a_user_might_type() {
        assert_eq!(Theme::parse("Dark"), Some(Theme::Dark));
        assert_eq!(Theme::parse(" system "), Some(Theme::System));
        assert_eq!(Theme::parse("auto"), Some(Theme::System));
        assert_eq!(Theme::parse("purple"), None);
    }

    #[test]
    fn defaults_are_system_theme_and_quiet() {
        let s = Settings::default();
        assert_eq!(s.theme, Theme::System);
        assert!(!s.verbose);
    }

    #[test]
    fn config_dir_is_outside_the_vault() {
        // The whole point: these must not sync.
        let dir = config_dir();
        assert!(!dir.to_string_lossy().contains("/jotbay/data"));
    }
}
