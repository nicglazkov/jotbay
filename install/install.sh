#!/usr/bin/env bash
#
# Jotbay installer — macOS and Linux.
#
#   ./install/install.sh              install, preferring a published release
#   ./install/install.sh --source     always build from source
#   ./install/install.sh --no-gui     CLI and scheduler only (headless servers)
#
# Handles everything: binaries, the background sync schedule, the launcher in
# the repo root, and a Desktop shortcut to the synced folder.

set -euo pipefail

JOTBAY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Where releases live. Deliberately not derived from the clone's origin any
# more: since the split, a clone of this script sits next to somebody's *notes*,
# and a notes repository has no releases on it. Deriving it meant every install
# silently fell through to a source build, which needs a Rust toolchain — the
# one thing the release assets exist to avoid.
#
# A fork overrides it rather than editing this file. Matches JOTBAY_TOOL_REPO in
# lib/core/src/update.rs, which `jotbay upgrade` uses for the same reason.
REPO="${JOTBAY_TOOL_REPO:-nicglazkov/jotbay}"
BIN_DIR="$HOME/.local/bin"
INTERVAL=600
LAUNCH_LABEL="com.jotbay.sync"

FROM_SOURCE=0
WANT_GUI=1
for arg in "$@"; do
  case "$arg" in
    --source)  FROM_SOURCE=1 ;;
    --no-gui)  WANT_GUI=0 ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

say()  { printf '\033[1m==>\033[0m %s\n' "$*"; }
info() { printf '    %s\n' "$*"; }
warn() { printf '\033[33m    warning:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# --- platform ---------------------------------------------------------------

case "$(uname -s)" in
  Darwin) OS=macos ;;
  Linux)  OS=linux ;;
  *) die "unsupported operating system: $(uname -s) — use install.ps1 on Windows" ;;
esac

case "$(uname -m)" in
  arm64|aarch64) ARCH=aarch64 ;;
  x86_64|amd64)  ARCH=x86_64 ;;
  *) die "unsupported architecture: $(uname -m)" ;;
esac

say "installing for $OS/$ARCH from $JOTBAY_DIR"

[ -d "$JOTBAY_DIR/.git" ] || die "$JOTBAY_DIR is not a git clone of Jotbay"
mkdir -p "$BIN_DIR"

# Binaries from before the rename. The installer replaced the *timer* but left
# these, so `inkway` stayed on PATH — and running one would republish to
# refs/inkway-status/ and recreate the orphan ref that was just deleted.
for STALE in inkway inkway-gui; do
  if [ -e "$BIN_DIR/$STALE" ]; then
    rm -f "$BIN_DIR/$STALE"
    info "removed the superseded $STALE binary"
  fi
done

# Two installs of the same tool is the quiet failure here. Ubuntu's default
# ~/.profile prepends ~/.local/bin, so this install shadows a packaged one and
# `jotbay` keeps running whichever is older — with nothing anywhere saying so.
# Observed on a machine that had both; the two binaries behaved differently.
if [ -x /usr/bin/jotbay ] && [ "$BIN_DIR/jotbay" != /usr/bin/jotbay ]; then
  warn "a packaged copy is already installed at /usr/bin/jotbay"
  info "$(/usr/bin/jotbay --version 2>/dev/null || echo 'version unknown')"
  info "this installs to $BIN_DIR, which comes first on PATH and will shadow it"
  info "remove one of them: sudo dpkg -r jotbay, or ./install/uninstall.sh"
fi

# --- obtain the binaries ----------------------------------------------------

download_release() {
  have gh || return 1
  gh auth status >/dev/null 2>&1 || return 1

  # macOS ships one universal asset covering Apple Silicon and Intel; the
  # per-arch name is still accepted so older releases keep installing.
  local candidates=()
  [ "$OS" = macos ] && candidates+=("jotbay-macos-universal.tar.gz")
  candidates+=("jotbay-$OS-$ARCH.tar.gz")

  local available asset=""
  available=$(gh release view --repo "$REPO" --json assets --jq '.assets[].name' 2>/dev/null || true)
  [ -n "$available" ] || return 1

  local c
  for c in "${candidates[@]}"; do
    if printf '%s\n' "$available" | grep -qx "$c"; then asset="$c"; break; fi
  done
  [ -n "$asset" ] || return 1

  local tmp
  tmp=$(mktemp -d)
  info "downloading $asset"
  gh release download --repo "$REPO" --pattern "$asset" --dir "$tmp" --clobber >/dev/null 2>&1 || return 1
  tar -xzf "$tmp/$asset" -C "$tmp" || return 1

  install -m 755 "$tmp/jotbay" "$BIN_DIR/jotbay"
  if [ "$WANT_GUI" -eq 1 ] && [ -d "$tmp/Jotbay.app" ]; then
    rm -rf "$JOTBAY_DIR/Jotbay.app"
    cp -R "$tmp/Jotbay.app" "$JOTBAY_DIR/Jotbay.app"
  elif [ "$WANT_GUI" -eq 1 ] && [ -f "$tmp/jotbay-gui" ]; then
    install -m 755 "$tmp/jotbay-gui" "$BIN_DIR/jotbay-gui"
  fi

  rm -rf "$tmp"
  return 0
}

build_from_source() {
  have cargo || die "cargo not found — install Rust (https://rustup.rs) or wait for a published release"

  info "building the CLI (this takes a minute)"
  ( cd "$JOTBAY_DIR/lib" && cargo build --release --quiet )
  install -m 755 "$JOTBAY_DIR/lib/target/release/jotbay" "$BIN_DIR/jotbay"

  if [ "$WANT_GUI" -eq 1 ]; then
    if [ "$OS" = macos ]; then
      if have xcodebuild && have xcodegen; then
        info "building the macOS app"
        ( cd "$JOTBAY_DIR/lib/gui-macos" && ./build.sh Release >/dev/null )
        rm -rf "$JOTBAY_DIR/Jotbay.app"
        cp -R "$JOTBAY_DIR/lib/gui-macos/build/Build/Products/Release/Jotbay.app" \
              "$JOTBAY_DIR/Jotbay.app"
      else
        warn "skipping the GUI — needs Xcode and xcodegen (brew install xcodegen)"
      fi
    else
      info "building the desktop app"
      if ( cd "$JOTBAY_DIR/lib/gui-tauri/src-tauri" && cargo build --release --quiet ); then
        install -m 755 "$JOTBAY_DIR/lib/gui-tauri/src-tauri/target/release/jotbay-gui" "$BIN_DIR/jotbay-gui"
      else
        warn "skipping the GUI — Tauri needs libwebkit2gtk-4.1-dev and libgtk-3-dev"
      fi
    fi
  fi
}

say "installing binaries"
if [ "$FROM_SOURCE" -eq 1 ]; then
  build_from_source
elif download_release; then
  info "installed from the latest release"
else
  info "no published release available, building from source"
  build_from_source
fi

have "$BIN_DIR/jotbay" || [ -x "$BIN_DIR/jotbay" ] || die "installation produced no jotbay binary"
info "jotbay $("$BIN_DIR/jotbay" --version | awk '{print $2}') → $BIN_DIR/jotbay"

# --- git identity -----------------------------------------------------------
#
# `gh auth setup-git` configures credentials but NOT user.name/user.email, so a
# machine can be perfectly authenticated to GitHub and still be incapable of
# creating a commit. Nothing catches that until the first sync that actually has
# something to commit: every sync before it reports success, because a pull and
# push with no local work never needs an identity. On a fresh desktop that gap
# can be days — the user writes notes the whole time and nothing leaves the
# machine. Checking here costs nothing and closes it.

IDENTITY_OK=1

ensure_git_identity() {
  local name email login id
  name=$(git -C "$JOTBAY_DIR" config --get user.name 2>/dev/null || true)
  email=$(git -C "$JOTBAY_DIR" config --get user.email 2>/dev/null || true)
  [ -n "$name" ] && [ -n "$email" ] && return 0

  # gh already knows who you are, so borrow it rather than asking. The
  # noreply address is the one GitHub itself hands out, and it keeps a private
  # email private.
  if have gh && gh auth status >/dev/null 2>&1; then
    login=$(gh api user --jq '.login' 2>/dev/null || true)
    id=$(gh api user --jq '.id' 2>/dev/null || true)
    if [ -n "$login" ] && [ -n "$id" ]; then
      [ -n "$name" ]  || name=$(gh api user --jq '.name // .login' 2>/dev/null || echo "$login")
      [ -n "$email" ] || email="${id}+${login}@users.noreply.github.com"
      # Set on this clone only. An installer has no business rewriting the
      # identity every other repository on the machine commits under.
      git -C "$JOTBAY_DIR" config user.name  "$name"
      git -C "$JOTBAY_DIR" config user.email "$email"
      info "commits from this machine will be authored as $name <$email>"
      return 0
    fi
  fi

  IDENTITY_OK=0
  return 0
}

say "checking git can commit"
ensure_git_identity

# --- background sync --------------------------------------------------------

say "scheduling background sync every $((INTERVAL / 60)) minutes"

if [ "$OS" = macos ]; then
  # A neutral label: this one ends up in every user's LaunchAgents directory,
  # so it should name the tool rather than whoever built it.
  PLIST="$HOME/Library/LaunchAgents/$LAUNCH_LABEL.plist"
  mkdir -p "$HOME/Library/LaunchAgents"

  # Every label this tool has ever used. These are historical literals and must
  # not be renamed with the rest of the codebase: left loaded, each one keeps
  # firing its own sync alongside the new one, against a binary that may no
  # longer exist.
  for LEGACY in "$HOME/Library/LaunchAgents/com.glazkov.inkway-sync.plist" \
                "$HOME/Library/LaunchAgents/com.inkway.sync.plist"; do
    if [ -f "$LEGACY" ] && [ "$LEGACY" != "$PLIST" ]; then
      launchctl unload "$LEGACY" 2>/dev/null || true
      rm -f "$LEGACY"
      info "removed a superseded LaunchAgent: $(basename "$LEGACY")"
    fi
  done
  cat > "$PLIST" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LAUNCH_LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>$BIN_DIR/jotbay</string>
    <string>sync</string>
    <string>--jotbay</string>
    <string>$JOTBAY_DIR</string>
  </array>
  <key>StartInterval</key><integer>$INTERVAL</integer>
  <key>RunAtLoad</key><true/>
  <key>StandardOutPath</key><string>$HOME/Library/Logs/jotbay-sync.log</string>
  <key>StandardErrorPath</key><string>$HOME/Library/Logs/jotbay-sync.log</string>
</dict>
</plist>
PLIST_EOF
  launchctl unload "$PLIST" 2>/dev/null || true
  launchctl load "$PLIST"
  info "LaunchAgent loaded · logs in ~/Library/Logs/jotbay-sync.log"

else
  UNITS="$HOME/.config/systemd/user"
  mkdir -p "$UNITS"

  # Historical unit name, not to be renamed with the rest of the codebase.
  # A leftover timer keeps calling a binary that no longer exists, and the
  # failure only ever surfaces in the journal.
  if [ -f "$UNITS/inkway-sync.timer" ]; then
    systemctl --user disable --now inkway-sync.timer 2>/dev/null || true
    rm -f "$UNITS/inkway-sync.service" "$UNITS/inkway-sync.timer"
    info "removed the superseded systemd timer"
  fi

  cat > "$UNITS/jotbay-sync.service" <<UNIT_EOF
[Unit]
Description=Keep markdown notes in sync
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=$BIN_DIR/jotbay sync --jotbay $JOTBAY_DIR
UNIT_EOF

  cat > "$UNITS/jotbay-sync.timer" <<UNIT_EOF
[Unit]
Description=Run jotbay sync every $((INTERVAL / 60)) minutes

[Timer]
OnBootSec=2min
OnUnitActiveSec=${INTERVAL}s
# Catch up after suspend or downtime instead of waiting a whole interval —
# this matters for laptops that move between networks.
Persistent=true

[Install]
WantedBy=timers.target
UNIT_EOF

  systemctl --user daemon-reload
  # Enabled but not started yet. `--now` starts the timer immediately, and
  # because it is Persistent with an elapsed OnBootSec systemd fires a sync
  # right then — which races the first sync below, takes the lock, and makes
  # the installer sign off with "another sync is already running". Started
  # after that first sync instead.
  systemctl --user enable jotbay-sync.timer >/dev/null
  # Without lingering, the user manager is torn down at logout and the timer
  # stops firing on exactly the headless boxes that need it most.
  loginctl enable-linger "$USER" 2>/dev/null || \
    warn "could not enable lingering — run: sudo loginctl enable-linger $USER"
  info "systemd timer enabled · logs: journalctl --user -u jotbay-sync"
fi

# --- launcher and shortcuts -------------------------------------------------

if [ "$WANT_GUI" -eq 1 ]; then
  say "creating the launcher"

  if [ "$OS" = macos ]; then
    if [ -d "$JOTBAY_DIR/Jotbay.app" ]; then
      info "Jotbay.app is in the repository root — double-click it"
    fi
  else
    if [ -x "$BIN_DIR/jotbay-gui" ]; then
      DESKTOP_FILE="$JOTBAY_DIR/jotbay.desktop"
      ICON="$JOTBAY_DIR/lib/icons/generated/linux/256x256.png"
      cat > "$DESKTOP_FILE" <<DESKTOP_EOF
[Desktop Entry]
Type=Application
Name=Jotbay
Comment=Manage the synced markdown jotbay
Exec=$BIN_DIR/jotbay-gui
Icon=$ICON
Terminal=false
Categories=Utility;
DESKTOP_EOF
      chmod +x "$DESKTOP_FILE"
      mkdir -p "$HOME/.local/share/applications"
      cp "$DESKTOP_FILE" "$HOME/.local/share/applications/jotbay.desktop"
      update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
      info "jotbay.desktop is in the repository root and the application menu"
    fi
  fi
fi

say "linking the synced folder to your Desktop"
if [ -d "$HOME/Desktop" ]; then
  ln -sfn "$JOTBAY_DIR/data" "$HOME/Desktop/Jotbay"
  # shellcheck disable=SC2088  # display text, not a path to be expanded
  info "~/Desktop/Jotbay -> $JOTBAY_DIR/data"
else
  info "no Desktop directory, skipping"
fi

# --- PATH -------------------------------------------------------------------

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) warn "$BIN_DIR is not on your PATH — add this to your shell profile:"
     # shellcheck disable=SC2016  # $PATH is meant to stay literal for copy-paste
     printf '\n        export PATH="%s:$PATH"\n\n' "$BIN_DIR" ;;
esac

say "running the first sync"

# On a reinstall the schedule is already live, so its ten-minute run can land on
# top of this one. The lock then does exactly its job and this sync exits having
# done nothing — correct, but "another sync is already running" is the last
# thing the installer says, which reads like a failure. Name it for what it is.
sync_log=$(mktemp)
set +e
"$BIN_DIR/jotbay" sync --jotbay "$JOTBAY_DIR" 2>&1 | tee "$sync_log"
sync_status=${PIPESTATUS[0]}
set -e

if grep -q "another sync is already running" "$sync_log"; then
  info "that was the scheduled sync already in flight; it finishes on its own"
elif [ "$sync_status" -ne 0 ]; then
  warn "first sync did not complete — run 'jotbay status' to see why"
fi
rm -f "$sync_log"

# Now that the first sync has finished and released the lock, hand over to the
# schedule. On macOS launchctl already did this via RunAtLoad.
if [ "$OS" = linux ]; then
  systemctl --user start jotbay-sync.timer
fi

echo
say "done"
info "jotbay status   — see every machine"
info "jotbay dash     — live dashboard"
info "jotbay sync     — sync right now"

if [ "$IDENTITY_OK" -eq 0 ]; then
  echo
  warn "git has no user.name or user.email on this machine, so Jotbay cannot commit."
  warn "Sync will look healthy until the first time you change a file, and then fail."
  # shellcheck disable=SC2016  # display text for the user to copy verbatim
  printf '\n        git config --global user.name  "Your Name"\n'
  # shellcheck disable=SC2016
  printf '        git config --global user.email "you@example.com"\n\n'
fi
