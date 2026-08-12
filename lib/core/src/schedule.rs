//! Registering the background watcher with the operating system.
//!
//! This lived only in `install.sh` and `install.ps1`, which meant it happened
//! only for people who ran a script. Someone who downloaded the `.dmg`, dragged
//! it to Applications, opened the app and finished setup got no background sync
//! at all, their notes moved when they pressed a button and at no other time.
//! That is precisely backwards: the graphical route exists for the people least
//! likely to go and find a shell.
//!
//! So the app does it too. `ensure()` is idempotent and writes the same label,
//! unit and task name the installers use, so a machine that has run both ends
//! up with one scheduler rather than two.
//!
//! The watcher runs in the foreground and the OS supervises it, launchd
//! `KeepAlive`, systemd `Restart=always`, a Windows logon task with restart
//! counts. All three restart a process that dies and capture what it printed,
//! which is a supervisor worth more than anything this could hand-roll.

use crate::error::{Error, Result};
use std::path::PathBuf;

/// The one name this is registered under, on every platform that has a concept
/// of one. Matches `LAUNCH_LABEL` in install.sh and the task name in install.ps1.
pub const LABEL: &str = "com.jotbay.sync";

/// Whether a background watcher is registered on this machine.
pub fn is_installed() -> bool {
    if cfg!(target_os = "macos") {
        plist_path().exists()
    } else if cfg!(target_os = "windows") {
        crate::proc::quiet("schtasks")
            .args(["/query", "/tn", "jotbay-sync"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    } else {
        unit_path().exists()
    }
}

/// Restart the background sync so it runs the binaries that are on disk now.
///
/// Replacing the files does not replace the running process. Every operating
/// system here keeps a started process on the image it started with, so a
/// machine can be fully upgraded and still sync with the previous version
/// indefinitely, publishing that version as its own to every other machine.
/// Three upgrades in a row on this fleet needed this done by hand before the
/// node list stopped lying.
///
/// Never fatal: a machine whose scheduler will not restart is still upgraded,
/// and the caller says so rather than calling the whole upgrade a failure.
pub fn restart() -> bool {
    if !is_installed() {
        return false;
    }
    if cfg!(target_os = "macos") {
        // kickstart -k terminates the running job and starts it again. Unload
        // and load races with launchd's own restart of a KeepAlive job.
        let target = format!("gui/{}/{LABEL}", uid());
        crate::proc::quiet("launchctl")
            .args(["kickstart", "-k", &target])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    } else if cfg!(target_os = "windows") {
        // `schtasks /end` reports success and leaves the watcher running.
        //
        // The task's top-level process is conhost, which is what gets
        // terminated; the jotbay.exe it launched is orphaned and carries on.
        // Following it with /run then starts a second one. Measured on a real
        // machine after a few attempts: three watchers alive at once, two of
        // them orphans, all syncing the same vault.
        //
        // So end the task, then kill the watchers by what they are actually
        // running, and only then start it again.
        let _ = crate::proc::quiet("schtasks")
            .args(["/end", "/tn", "jotbay-sync"])
            .output();

        // Matched on the command line, and never this process: `jotbay upgrade`
        // is also jotbay.exe, and killing by image name would kill the upgrade
        // partway through.
        let script = format!(
            "Get-CimInstance Win32_Process -Filter \"Name='jotbay.exe'\" | \
             Where-Object {{ $_.CommandLine -like '* watch*' -and $_.ProcessId -ne {} }} | \
             ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}",
            std::process::id()
        );
        let _ = crate::proc::quiet("powershell")
            .args(["-NoProfile", "-Command", &script])
            .output();

        crate::proc::quiet("schtasks")
            .args(["/run", "/tn", "jotbay-sync"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    } else {
        crate::proc::quiet("systemctl")
            .args(["--user", "restart", "jotbay-sync.service"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

/// This account's numeric id, which is what launchd's domain target needs.
///
/// Read by asking `id`, rather than taking a dependency on libc for one call
/// that runs only when an upgrade finishes.
fn uid() -> String {
    crate::proc::quiet("id")
        .arg("-u")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Register it if it is not already there.
///
/// Never fatal to the caller: a machine where this fails still syncs whenever
/// somebody asks it to, and refusing to finish setup over a scheduler would be
/// a worse outcome than a missing one.
pub fn ensure() -> Result<bool> {
    if is_installed() {
        return Ok(false);
    }
    install()?;
    Ok(true)
}

/// The installed `jotbay` binary to schedule.
///
/// Not `current_exe()`: the caller may be the GUI, and scheduling *that* would
/// launch a window every ten minutes. The CLI is what runs headless.
fn cli_path() -> Result<PathBuf> {
    // Beside whatever is running, first. An app bundle carries its own copy,
    // and a cask links that same file onto PATH.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["jotbay", "jotbay.exe"] {
                let beside = dir.join(name);
                if beside.exists() && beside != exe {
                    return Ok(beside);
                }
            }
            // The macOS app keeps the CLI in Resources, next to MacOS/.
            let bundled = dir.join("../Resources/jotbay");
            if bundled.exists() {
                return Ok(bundled.canonicalize().unwrap_or(bundled));
            }
        }
        // A jotbay binary scheduling itself is the ordinary CLI case.
        if exe.file_stem().map(|s| s == "jotbay").unwrap_or(false) {
            return Ok(exe);
        }
    }

    for candidate in install_candidates() {
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(Error::Other(
        "could not find the jotbay command to schedule".into(),
    ))
}

fn install_candidates() -> Vec<PathBuf> {
    let home = crate::home();
    if cfg!(target_os = "windows") {
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Local"));
        vec![
            local.join(r"Jotbay\jotbay.exe"),
            local.join(r"Programs\jotbay\jotbay.exe"),
        ]
    } else {
        let mut candidates = vec![
            home.join(".local/bin/jotbay"),
            PathBuf::from("/usr/local/bin/jotbay"),
            PathBuf::from("/opt/homebrew/bin/jotbay"),
            PathBuf::from("/usr/bin/jotbay"),
        ];
        // The app bundle is an install location like any other, and it was the
        // one missing. A fresh macOS VM reached this list. Nothing else on it
        // had a jotbay, and `schedule::ensure` failed with "could not find the
        // jotbay command to schedule" while the CLI sat in Resources the whole
        // time. Harmless in the shipped layout, where the earlier
        // beside-current_exe branch matches first, and not harmless for
        // anything calling the core from elsewhere.
        if cfg!(target_os = "macos") {
            candidates.push(PathBuf::from(
                "/Applications/Jotbay.app/Contents/Resources/jotbay",
            ));
            candidates.push(home.join("Applications/Jotbay.app/Contents/Resources/jotbay"));
        }
        candidates
    }
}

fn plist_path() -> PathBuf {
    crate::home().join(format!("Library/LaunchAgents/{LABEL}.plist"))
}

fn unit_path() -> PathBuf {
    crate::home().join(".config/systemd/user/jotbay-sync.service")
}

fn install() -> Result<()> {
    let exe = cli_path()?;
    let home = crate::home();

    if cfg!(target_os = "macos") {
        let path = plist_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(
            &path,
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>watch</string>
  </array>
  <key>WorkingDirectory</key><string>{}</string>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>RunAtLoad</key><true/>
  <key>StandardOutPath</key><string>{}/Library/Logs/jotbay-sync.log</string>
  <key>StandardErrorPath</key><string>{}/Library/Logs/jotbay-sync.log</string>
</dict>
</plist>
"#,
                exe.display(),
                home.display(),
                home.display(),
                home.display()
            ),
        )?;
        // Unload first: re-registering over a loaded agent is otherwise a no-op.
        let _ = crate::proc::quiet("launchctl").args(["unload", &path.to_string_lossy()]).output();
        crate::proc::quiet("launchctl")
            .args(["load", &path.to_string_lossy()])
            .output()
            .map_err(|e| Error::Other(format!("launchctl: {e}")))?;
    } else if cfg!(target_os = "windows") {
        // Through PowerShell rather than schtasks.exe: the XML schtasks wants
        // for a logon trigger with restart counts is far more error-prone than
        // the cmdlets.
        //
        // -User is not optional. `New-ScheduledTrigger -AtLogOn` with no user
        // means *any* user's logon, which only an administrator may register
        // so the first version failed with "Access is denied" for exactly the
        // person it was meant to help, and did so silently, because the GUI
        // ignores the result. install.ps1 looked like precedent but used a time
        // trigger, which has no such rule. Proven unelevated on Windows 11.
        let script = format!(
            r#"$a = New-ScheduledTaskAction -Execute "$env:SystemRoot\System32\conhost.exe" `
  -Argument '--headless "{}" watch' -WorkingDirectory "$env:USERPROFILE"
$t = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$s = New-ScheduledTaskSettingsSet -StartWhenAvailable -DontStopIfGoingOnBatteries `
  -AllowStartIfOnBatteries -MultipleInstances IgnoreNew `
  -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
  -ExecutionTimeLimit ([TimeSpan]::Zero)
Register-ScheduledTask -TaskName 'jotbay-sync' -Action $a -Trigger $t -Settings $s `
  -Description 'Keep markdown notes in sync' -Force | Out-Null
Start-ScheduledTask -TaskName 'jotbay-sync' -ErrorAction SilentlyContinue
"#,
            exe.display()
        );
        let out = crate::proc::quiet("powershell")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
            .output()
            .map_err(|e| Error::Other(format!("powershell: {e}")))?;
        if !out.status.success() {
            return Err(Error::Other(format!(
                "could not register the scheduled task: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
    } else {
        let path = unit_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(
            &path,
            format!(
                "[Unit]\n\
                 Description=Keep markdown notes in sync\n\
                 After=network-online.target\n\
                 Wants=network-online.target\n\n\
                 [Service]\n\
                 Type=simple\n\
                 WorkingDirectory={}\n\
                 ExecStart={} watch\n\
                 Restart=always\n\
                 RestartSec=10\n\n\
                 [Install]\n\
                 WantedBy=default.target\n",
                home.display(),
                exe.display()
            ),
        )?;
        let _ = crate::proc::quiet("systemctl").args(["--user", "daemon-reload"]).output();
        crate::proc::quiet("systemctl")
            .args(["--user", "enable", "--now", "jotbay-sync.service"])
            .output()
            .map_err(|e| Error::Other(format!("systemctl: {e}")))?;
        // Without lingering the user manager is torn down at logout, and the
        // watcher stops on exactly the headless boxes that need it most.
        let user = std::env::var("USER").unwrap_or_default();
        if !user.is_empty() {
            let _ = crate::proc::quiet("loginctl").args(["enable-linger", &user]).output();
        }
    }

    Ok(())
}

/// Where the watcher's output goes, for a UI that wants to point at it.
pub fn log_hint() -> String {
    if cfg!(target_os = "macos") {
        "~/Library/Logs/jotbay-sync.log".into()
    } else if cfg!(target_os = "windows") {
        "Task Scheduler → jotbay-sync".into()
    } else {
        "journalctl --user -u jotbay-sync".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_matches_what_the_installers_write() {
        // A drifted label means a machine that ran both ends up with two
        // schedulers, each syncing on its own timetable.
        assert_eq!(LABEL, "com.jotbay.sync");
    }

    #[test]
    fn candidate_paths_are_platform_appropriate() {
        let candidates = install_candidates();
        assert!(!candidates.is_empty());
        let joined = candidates
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(" ");
        if cfg!(target_os = "windows") {
            assert!(joined.contains("jotbay.exe"));
        } else {
            assert!(joined.contains(".local/bin/jotbay"));
        }
    }
}
