#!/usr/bin/env bash
#
# Remove everything install.sh created. Leaves the Jotbay repository and your
# notes completely untouched — this uninstalls the tooling, not the data.

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
    systemctl --user disable --now "$UNIT.timer" 2>/dev/null || true
    rm -f "$HOME/.config/systemd/user/$UNIT.service" \
          "$HOME/.config/systemd/user/$UNIT.timer"
  done
  systemctl --user daemon-reload 2>/dev/null || true
  info "systemd timer removed"
fi

say "removing binaries and launchers"
rm -f "$BIN_DIR/jotbay" "$BIN_DIR/jotbay-gui"
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

echo
say "done — your notes in $JOTBAY_DIR/data were not touched"
