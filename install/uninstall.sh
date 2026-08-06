#!/usr/bin/env bash
#
# Remove everything install.sh created. Leaves the Jotbay repository and your
# notes completely untouched — this uninstalls the tooling, not the data.
#
#   install/uninstall.sh          keep preferences (settings.json)
#   install/uninstall.sh --all    remove them as well

set -euo pipefail

JOTBAY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="$HOME/.local/bin"

say()  { printf '\033[1m==>\033[0m %s\n' "$*"; }
info() { printf '    %s\n' "$*"; }

case "$(uname -s)" in
  Darwin) OS=macos ;;
  Linux)  OS=linux ;;
  *) echo "use uninstall.ps1 on Windows" >&2; exit 2 ;;
esac

say "stopping the background sync"
if [ "$OS" = macos ]; then
  # Both labels: the publisher-specific one predates 1.4.0 and may still be
  # loaded on a machine installed before then.
  for PLIST in "$HOME/Library/LaunchAgents/com.jotbay.sync.plist" \
               "$HOME/Library/LaunchAgents/com.inkway.sync.plist" \
               "$HOME/Library/LaunchAgents/com.glazkov.inkway-sync.plist"; do
    launchctl unload "$PLIST" 2>/dev/null || true
    rm -f "$PLIST"
  done
  info "LaunchAgent removed"
else
  for UNIT in jotbay-sync inkway-sync; do
    # Both shapes: a .service since the watcher landed, a .timer before it.
    systemctl --user disable --now "$UNIT.service" 2>/dev/null || true
    systemctl --user disable --now "$UNIT.timer" 2>/dev/null || true
    rm -f "$HOME/.config/systemd/user/$UNIT.service" \
          "$HOME/.config/systemd/user/$UNIT.timer"
  done
  systemctl --user daemon-reload 2>/dev/null || true
  info "systemd timer removed"
fi

say "removing binaries and launchers"
# Both names: a machine installed before the rename still has the old pair.
rm -f "$BIN_DIR/jotbay" "$BIN_DIR/jotbay-gui" \
      "$BIN_DIR/inkway" "$BIN_DIR/inkway-gui"
rm -rf "$JOTBAY_DIR/Jotbay.app"
rm -f "$JOTBAY_DIR/jotbay.desktop" \
      "$HOME/.local/share/applications/jotbay.desktop"

say "removing shortcuts"
# Only remove the Desktop entry if it is our symlink, never a real directory
# someone happened to name Jotbay.
if [ -L "$HOME/Desktop/Jotbay" ]; then
  rm -f "$HOME/Desktop/Jotbay"
  # shellcheck disable=SC2088  # display text, not a path to be expanded
  info "~/Desktop/Jotbay removed"
fi
# The launcher `jotbay shortcut app` writes. It is a file rather than a symlink,
# so the check above never covered it, and it survived every uninstall as a
# desktop icon pointing at a binary that had just been deleted.
for LAUNCHER in "$HOME/Desktop/jotbay.desktop" "$HOME/Desktop/inkway.desktop"; do
  if [ -f "$LAUNCHER" ] && grep -q "^Exec=.*jotbay-gui\|^Exec=.*inkway-gui" "$LAUNCHER" 2>/dev/null; then
    rm -f "$LAUNCHER"
    info "$(basename "$LAUNCHER") removed"
  fi
done

# Preferences are kept unless asked. Reinstalling then remembers where the
# notes live, which is what somebody uninstalling to fix something wants — and
# it is exactly what makes a "clean reinstall" not clean, because setup finds a
# vault already configured and never shows the first-run screen. Both agent
# runs had to work this out and remove it by hand before their tests meant
# anything. Saying so is the least this can do.
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/jotbay"
if [ "${1:-}" = "--all" ]; then
  rm -rf "$CONFIG_DIR"
  say "removed your preferences too"
elif [ -d "$CONFIG_DIR" ]; then
  say "kept your preferences in $CONFIG_DIR"
  info "re-run with --all to remove them, which is what you want before"
  info "testing a genuinely fresh install"
fi

echo
say "done — your notes in $JOTBAY_DIR/data were not touched"
