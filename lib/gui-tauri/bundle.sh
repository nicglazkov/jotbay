#!/usr/bin/env bash
#
# Build the native installers for this platform.
#
#   lib/gui-tauri/bundle.sh [target-triple]
#
# Produces -setup.exe on Windows, .deb and .AppImage on Linux. macOS is not
# built here: its GUI is the native Swift app, packaged by lib/gui-macos/package.sh.
#
# NSIS only on Windows, deliberately. The WiX/MSI bundle installs per-machine
# (ALLUSERS=1). It prompts for UAC, registers under HKLM, and an unelevated
# uninstall fails with error 1730, while the NSIS installer is currentUser and
# is the only one that puts the CLI on PATH. Two installers with different
# elevation semantics and different outcomes is worse than one that works.
#
# The CLI is staged into src-tauri/staged first, because the bundler can only
# package files that sit inside the crate. Both installers place it where a
# terminal can reach it, /usr/bin on Linux, the install directory plus a PATH
# entry on Windows, so installing the app does not leave a CLI user stranded.

set -euo pipefail
cd "$(dirname "$0")"

TARGET="${1:-}"
STAGED="src-tauri/staged"

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

case "$(uname -s)" in
  Darwin) die "macOS ships the Swift app; run lib/gui-macos/package.sh instead" ;;
esac

command -v cargo >/dev/null || die "cargo not found"
command -v tauri >/dev/null || die "tauri CLI not found (npm i -g @tauri-apps/cli@^2)"

# Strip absolute build paths, matching what release.yml does, so a binary built
# here and one built by CI can be compared at all. `file!()` puts the path of
# every panic site into the binary, 128 of them in the 1.7.1 CLI, which
# otherwise embeds this machine's home directory in anything shipped from it.
#
# Exported rather than passed per-command so the tauri bundler's own cargo
# invocation picks it up too. Set RUSTFLAGS yourself to override.
if [ -z "${RUSTFLAGS:-}" ]; then
  _ws="$(cd .. && pwd)"
  _registry="${CARGO_HOME:-$HOME/.cargo}/registry"
  if command -v cygpath >/dev/null 2>&1; then
    _ws="$(cygpath -w "$_ws")"
    _registry="$(cygpath -w "$_registry")"
  fi
  export RUSTFLAGS="--remap-path-prefix=$_ws=/jotbay --remap-path-prefix=$_registry=/cargo-registry"
fi

# --- stage the CLI ----------------------------------------------------------

say "building the CLI"
if [ -n "$TARGET" ]; then
  ( cd .. && cargo build --release --locked --target "$TARGET" )
  BUILT_DIR="../target/$TARGET/release"
else
  ( cd .. && cargo build --release --locked )
  BUILT_DIR="../target/release"
fi

rm -rf "$STAGED"
mkdir -p "$STAGED"
if [ -f "$BUILT_DIR/jotbay.exe" ]; then
  cp "$BUILT_DIR/jotbay.exe" "$STAGED/jotbay.exe"
else
  cp "$BUILT_DIR/jotbay" "$STAGED/jotbay"
fi
say "staged $(ls "$STAGED")"

# --- bundle -----------------------------------------------------------------

# A bundle identifier belongs to whoever publishes the app. The committed value
# is this project's; a fork sets JOTBAY_BUNDLE_PREFIX and gets its own without
# editing tracked files. It must stay stable across releases. An installer
# whose identifier changed reads as a different product and installs alongside
# the old one instead of upgrading it.
# Wipe previous output first. The listing at the end prints whatever is in the
# bundle directory, so a leftover from an earlier build, including one under
# the tool's previous name, was reported as if it had just been produced.
rm -rf "src-tauri/target/${TARGET:+$TARGET/}release/bundle"

say "bundling"
ARGS=()
[ -n "$TARGET" ] && ARGS+=(--target "$TARGET")
if [ -n "${JOTBAY_BUNDLE_PREFIX:-}" ]; then
  say "identifier: $JOTBAY_BUNDLE_PREFIX.jotbay"
  ARGS+=(--config "{\"identifier\":\"$JOTBAY_BUNDLE_PREFIX.jotbay\"}")
fi
( cd src-tauri && tauri build ${ARGS[@]+"${ARGS[@]}"} )

BUNDLE_ROOT="src-tauri/target/${TARGET:+$TARGET/}release/bundle"
[ -d "$BUNDLE_ROOT" ] || die "no bundle directory at $BUNDLE_ROOT"

echo
say "built"
find "$BUNDLE_ROOT" -type f \( -name '*.deb' -o -name '*.AppImage' -o -name '*-setup.exe' \) \
  -exec printf '    %s\n' {} \;
