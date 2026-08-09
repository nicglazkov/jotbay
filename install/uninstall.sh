#!/usr/bin/env bash
#
# Remove Jotbay from this machine, however it was installed.
#
#   install/uninstall.sh          keep your preferences
#   install/uninstall.sh --all    remove them too, for a genuinely fresh install
#   curl -fsSL https://raw.githubusercontent.com/nicglazkov/jotbay/main/install/uninstall.sh | bash -s -- --all
#
# Your notes are never touched. This removes the program, not the data. The
# repository stays where it is, and a later install can adopt it.
#
# This used to undo only what install.sh had done, so it could not clean a
# machine installed from the .deb or the .dmg. The two routes most people take.
# Both fresh-install runs had to finish the job by hand, and everything they
# found left behind is handled here: a scheduled task pointing at a deleted
# binary, a desktop launcher for a program that was gone, and settings that made
# the next install silently not-fresh.

set -uo pipefail

say()  { printf '\033[1m==>\033[0m %s\n' "$*"; }
info() { printf '    %s\n' "$*"; }
warn() { printf '\033[33m    %s\033[0m\n' "$*"; }

# Drop Launch Services registrations for copies of the app that are no longer
# on disk. macOS never prunes these, so a removed app keeps appearing in Open
# With menus, and Finder can launch a ghost inside a DMG that was mounted once.
#
# The dump prints "path: <path> (0xHEX)". Paths can contain spaces, as the
# volume name of every release DMG does, so match the whole path and read whole
# lines rather than splitting on whitespace.
prune_launch_services() {
  lsr=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
  [ -x "$lsr" ] || return 0

  total=0
  # Loop rather than sweep once. Unregistering rewrites the database while the
  # sweep is reading it, and entries listed at the start can survive the pass:
  # a first run on the author's machine cleared 30 of 32 and needed a second
  # for the rest. Stop when a pass removes nothing.
  for _pass in 1 2 3; do
    gone=0
    stale=$("$lsr" -dump 2>/dev/null |
      sed -n 's/^[[:space:]]*path:[[:space:]]*\(.*\.app\) (0x[0-9a-f]*)$/\1/p' |
      grep -E '/(Jotbay|Inkway)\.app$' |
      sort -u) || true

    while IFS= read -r path; do
      [ -n "$path" ] || continue
      [ -e "$path" ] && continue
      "$lsr" -u "$path" >/dev/null 2>&1 && gone=$((gone + 1))
    done <<EOF
$stale
EOF

    total=$((total + gone))
    [ "$gone" -eq 0 ] && break
  done

  if [ "$total" -gt 0 ]; then
    info "removed $total stale Launch Services registration(s)"
  fi
  return 0
}

ALL=0
[ "${1:-}" = "--all" ] && ALL=1

case "$(uname -s)" in
  Darwin) OS=macos ;;
  Linux)  OS=linux ;;
  *) echo "use uninstall.ps1 on Windows" >&2; exit 2 ;;
esac

# Asked before anything is removed, because afterwards there is no `jotbay` left
# to ask. Used only so the closing line can name the folder it did not touch.
# Read from the recorded setting, not from `jotbay path`. That command answers
# for the current directory, so running this uninstaller from inside any git
# repository made it announce that repository as "your notes", which is the
# same cwd-versus-settings confusion as issue #2, in a script rather than the
# app. Harmless in what it deleted, wrong in what it told the user.
VAULT=""
SETTINGS="${XDG_CONFIG_HOME:-$HOME/.config}/jotbay/settings.json"
if [ -f "$SETTINGS" ]; then
  VAULT="$(sed -n 's/.*"vault_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$SETTINGS" | head -1)"
fi

# --- 1. stop the background sync -------------------------------------------
#
# First, before anything is deleted. A watcher left running holds the binaries
# about to be removed, and a unit left enabled keeps trying to start a file that
# is no longer there.
say "stopping background sync"
if [ "$OS" = macos ]; then
  for LABEL in com.jotbay.sync com.inkway.sync com.glazkov.inkway-sync; do
    launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null
    launchctl unload "$HOME/Library/LaunchAgents/$LABEL.plist" 2>/dev/null
    rm -f "$HOME/Library/LaunchAgents/$LABEL.plist"
  done
else
  # One unit per command. Naming several and letting a missing one fail aborts
  # the whole call, so the service is never disabled and its enable symlink
  # survives, which then makes the next install's scheduler check meaningless,
  # because an inherited unit looks exactly like a freshly created one. Found by
  # an agent who proved it with a dummy unit rather than assuming.
  for UNIT in jotbay-sync.service jotbay-sync.timer inkway-sync.service inkway-sync.timer; do
    systemctl --user disable --now "$UNIT" 2>/dev/null
  done
  rm -f "$HOME/.config/systemd/user/jotbay-sync.service" \
        "$HOME/.config/systemd/user/jotbay-sync.timer" \
        "$HOME/.config/systemd/user/inkway-sync.service" \
        "$HOME/.config/systemd/user/inkway-sync.timer"
  systemctl --user daemon-reload 2>/dev/null
fi
pkill -x jotbay 2>/dev/null
pkill -x jotbay-gui 2>/dev/null
info "stopped"

# --- 2. the package, if this machine has one -------------------------------
if [ "$OS" = linux ] && command -v dpkg >/dev/null 2>&1; then
  # `dpkg -s`, not `dpkg -l | grep -q`. With `pipefail` set, grep exits the
  # moment it matches, dpkg dies of SIGPIPE, and the pipeline reports failure
  # so the package was silently never removed on a machine that definitely had
  # it. The uninstaller's own closing check is what caught that.
  if dpkg -s jotbay >/dev/null 2>&1; then
    say "removing the jotbay package"
    if sudo -n true 2>/dev/null; then
      sudo apt-get remove -y jotbay >/dev/null 2>&1 && info "removed"
    else
      warn "needs sudo, finish with: sudo apt-get remove -y jotbay"
    fi
  fi
fi

# --- 3. binaries, bundles and launchers ------------------------------------
say "removing the program"
rm -f "$HOME/.local/bin/jotbay" "$HOME/.local/bin/jotbay-gui" \
      "$HOME/.local/bin/inkway" "$HOME/.local/bin/inkway-gui"

if [ "$OS" = macos ]; then
  for APP in "/Applications/Jotbay.app" "$HOME/Applications/Jotbay.app" \
             "/Applications/Inkway.app" "$HOME/Applications/Inkway.app"; do
    [ -d "$APP" ] && rm -rf "$APP" && info "$(basename "$APP") removed"
  done
  # install.sh kept a copy in the vault before the app and the notes came apart.
  [ -n "$VAULT" ] && rm -rf "$VAULT/Jotbay.app" "$VAULT/Inkway.app"
  # Deleting a bundle leaves its Launch Services registration behind, so an
  # uninstalled app stays in Open With menus and can still be launched by name.
  # Prune every registration whose bundle is gone, which is all of ours by now,
  # plus any left by a DMG that was mounted once and detached.
  prune_launch_services
  # Homebrew owns its own symlink; say so rather than leaving a dangling link
  # or fighting brew over a file it will put back.
  if [ -L /opt/homebrew/bin/jotbay ] || [ -L /usr/local/bin/jotbay ]; then
    warn "installed with Homebrew, finish with: brew uninstall --cask jotbay"
  fi
else
  rm -f "$HOME/.local/share/applications/jotbay.desktop" \
        "$HOME/.local/share/applications/inkway.desktop"
  update-desktop-database "$HOME/.local/share/applications" 2>/dev/null
fi

# --- 4. shortcuts we created -----------------------------------------------
say "removing shortcuts"
# Only ours. A symlink named Jotbay is one we made; a real folder somebody
# happened to name Jotbay is not, and has to survive.
[ -L "$HOME/Desktop/Jotbay" ] && rm -f "$HOME/Desktop/Jotbay" && info "Desktop/Jotbay"
[ -L "$HOME/Desktop/Jotbay Notes" ] && rm -f "$HOME/Desktop/Jotbay Notes" && info "Desktop/Jotbay Notes"
for LAUNCHER in "$HOME/Desktop/jotbay.desktop" "$HOME/Desktop/inkway.desktop" \
                "$HOME/Desktop/Jotbay.command"; do
  # Content-checked, not just name-matched: this deletes files off a desktop.
  if [ -f "$LAUNCHER" ] && grep -qi "jotbay\|inkway" "$LAUNCHER" 2>/dev/null; then
    rm -f "$LAUNCHER" && info "$(basename "$LAUNCHER")"
  fi
done

# --- 5. preferences --------------------------------------------------------
# Mirrors settings::config_dir() exactly. This used to say ~/.config/jotbay on
# macOS, which is not where the app stores anything, so --all removed nothing
# and every "fresh" install on a Mac silently started with the previous vault
# path already recorded. That is the same failure that made both agent runs'
# first-run tests meaningless, in the tool that exists to prevent it.
if [ "$OS" = macos ]; then
  CONFIG="$HOME/Library/Application Support/Jotbay"
  LEGACY="$HOME/Library/Application Support/Inkway"
else
  CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/jotbay"
  LEGACY="${XDG_CONFIG_HOME:-$HOME/.config}/inkway"
fi

if [ "$ALL" = 1 ]; then
  say "removing preferences"
  rm -rf "$CONFIG" "$LEGACY"
  if [ "$OS" = macos ]; then
    # State the apps create for themselves, which no installer ever wrote and
    # so nothing was ever removing: window positions, the webview's storage,
    # caches, and crash reports. Harmless to keep, but "wipe this machine"
    # should mean it.
    rm -f "$HOME/Library/Logs/jotbay-sync.log" "$HOME/Library/Logs/inkway-sync.log"
    rm -rf "$HOME/Library/Caches/jotbay-gui" "$HOME/Library/Caches/inkway-gui" \
           "$HOME/Library/WebKit/jotbay-gui" "$HOME/Library/WebKit/inkway-gui" \
           "$HOME/Library/HTTPStorages/com.glazkov.jotbay" \
           "$HOME/Library/Saved Application State/com.glazkov.jotbay.savedState" \
           "$HOME/Library/Saved Application State/com.example.jotbay.savedState"
    # com.example is the Tauri default identifier, which shipped before the
    # real one was set. A machine from that era still carries it.
    for ID in com.glazkov.jotbay com.glazkov.inkway com.example.jotbay; do
      rm -f "$HOME/Library/Preferences/$ID.plist"
      defaults delete "$ID" 2>/dev/null
    done
    rm -f "$HOME"/Library/Application\ Support/CrashReporter/jotbay_*.plist \
          "$HOME"/Library/Application\ Support/CrashReporter/inkway_*.plist
  fi
  info "removed. The next install starts from the first-run screen"
elif [ -d "$CONFIG" ]; then
  say "keeping your preferences"
  info "$CONFIG"
  info "these record where your notes live, so a reinstall finds them again"
  info "which also means a reinstall is NOT a fresh one. Use --all for that."
fi

# --- 6. prove it ------------------------------------------------------------
#
# An uninstaller that cannot say what it left behind is one nobody can trust to
# have finished. Every path named here was found by hand on a real machine.
echo
say "checking"
LEFT=0
for P in "$HOME/.local/bin/jotbay" "/usr/bin/jotbay" "/usr/local/bin/jotbay" \
         "/Applications/Jotbay.app" \
         "$HOME/Library/LaunchAgents/com.jotbay.sync.plist" \
         "$HOME/.config/systemd/user/jotbay-sync.service" \
         "$CONFIG"; do
  if [ -e "$P" ]; then warn "still present: $P"; LEFT=1; fi
done
if command -v jotbay >/dev/null 2>&1; then
  warn "still on PATH: $(command -v jotbay)"
  LEFT=1
fi
# Homebrew's copy is deliberately not removed here, but it is still a copy
# and reporting "nothing left behind" beside a warning to run brew was a
# contradiction that only one of the two could be true about.
if [ -L /opt/homebrew/bin/jotbay ] || [ -L /usr/local/bin/jotbay ]; then
  warn "still installed by Homebrew, run: brew uninstall --cask jotbay"
  LEFT=1
fi
[ "$LEFT" = 0 ] && info "nothing left behind"

echo
if [ -n "$VAULT" ] && [ -d "$VAULT" ]; then
  say "done, your notes in $VAULT are untouched"
else
  say "done, your notes were not touched"
fi
