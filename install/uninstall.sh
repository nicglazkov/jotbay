#!/usr/bin/env bash
#
# Remove Jotbay from this machine, however it was installed.
#
#   install/uninstall.sh          keep your preferences
#   install/uninstall.sh --all    remove them too, for a genuinely fresh install
#   curl -fsSL https://raw.githubusercontent.com/nicglazkov/jotbay/main/install/uninstall.sh | bash -s -- --all
#
# Your notes are never touched. This removes the program, not the data — the
# repository stays where it is, and a later install can adopt it.
#
# This used to undo only what install.sh had done, so it could not clean a
# machine installed from the .deb or the .dmg — the two routes most people take.
# Both fresh-install runs had to finish the job by hand, and everything they
# found left behind is handled here: a scheduled task pointing at a deleted
# binary, a desktop launcher for a program that was gone, and settings that made
# the next install silently not-fresh.

set -uo pipefail

say()  { printf '\033[1m==>\033[0m %s\n' "$*"; }
info() { printf '    %s\n' "$*"; }
warn() { printf '\033[33m    %s\033[0m\n' "$*"; }

ALL=0
[ "${1:-}" = "--all" ] && ALL=1

case "$(uname -s)" in
  Darwin) OS=macos ;;
  Linux)  OS=linux ;;
  *) echo "use uninstall.ps1 on Windows" >&2; exit 2 ;;
esac

# Asked before anything is removed, because afterwards there is no `jotbay` left
# to ask. Used only so the closing line can name the folder it did not touch.
VAULT=""
if command -v jotbay >/dev/null 2>&1; then
  VAULT="$(jotbay path 2>/dev/null | sed 's|/data$||')"
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
  # survives — which then makes the next install's scheduler check meaningless,
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
  # moment it matches, dpkg dies of SIGPIPE, and the pipeline reports failure —
  # so the package was silently never removed on a machine that definitely had
  # it. The uninstaller's own closing check is what caught that.
  if dpkg -s jotbay >/dev/null 2>&1; then
    say "removing the jotbay package"
    if sudo -n true 2>/dev/null; then
      sudo apt-get remove -y jotbay >/dev/null 2>&1 && info "removed"
    else
      warn "needs sudo — finish with: sudo apt-get remove -y jotbay"
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
  # Homebrew owns its own symlink; say so rather than leaving a dangling link
  # or fighting brew over a file it will put back.
  if [ -L /opt/homebrew/bin/jotbay ] || [ -L /usr/local/bin/jotbay ]; then
    warn "installed with Homebrew — finish with: brew uninstall --cask jotbay"
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
CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/jotbay"
[ "$OS" = macos ] && CONFIG="$HOME/.config/jotbay"
LEGACY="${XDG_CONFIG_HOME:-$HOME/.config}/inkway"

if [ "$ALL" = 1 ]; then
  say "removing preferences"
  rm -rf "$CONFIG" "$LEGACY"
  rm -f "$HOME/Library/Logs/jotbay-sync.log" 2>/dev/null
  info "removed — the next install starts from the first-run screen"
elif [ -d "$CONFIG" ]; then
  say "keeping your preferences"
  info "$CONFIG"
  info "these record where your notes live, so a reinstall finds them again —"
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
         "$HOME/.config/systemd/user/jotbay-sync.service"; do
  if [ -e "$P" ]; then warn "still present: $P"; LEFT=1; fi
done
if command -v jotbay >/dev/null 2>&1; then
  warn "still on PATH: $(command -v jotbay)"
  LEFT=1
fi
[ "$LEFT" = 0 ] && info "nothing left behind"

echo
if [ -n "$VAULT" ] && [ -d "$VAULT" ]; then
  say "done — your notes in $VAULT are untouched"
else
  say "done — your notes were not touched"
fi
