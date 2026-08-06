#!/usr/bin/env bash
#
# Build Jotbay.app.
#
# Resources/ is populated from lib/icons/generated rather than checked in, so
# there is exactly one source of truth for the icons.

set -euo pipefail
cd "$(dirname "$0")"

CONFIG="${1:-Release}"
ICONS="../icons/generated"
BUILD_DIR="build"

# Sign with the real Developer ID when this machine has it, ad hoc otherwise.
#
# Two reasons, and the second matters even without publishing: notarization
# accepts nothing but Developer ID, and TCC permissions bind to bundle ID *and*
# signing identity. An ad-hoc hash changes on every build, so macOS forgets
# every granted permission each time. CI runners have no certificate, so they
# fall through to ad hoc and keep working unchanged.
#
# Decided up here, before resources are staged, because the nested CLI must be
# signed as it is copied in, Xcode signs the bundle but not what is inside it.
#
# Who signs it lives in signing.env, which is gitignored: a certificate name,
# a Team ID and a bundle prefix all identify the publisher rather than the
# project, and hard-coding them means anyone who clones this builds an app
# claiming to be someone else's.
# shellcheck source=/dev/null
[ -f signing.env ] && . ./signing.env

IDENTITY="${JOTBAY_SIGN_IDENTITY:-}"
TEAM_ID="${JOTBAY_TEAM_ID:-}"
export JOTBAY_BUNDLE_PREFIX="${JOTBAY_BUNDLE_PREFIX:-com.example}"

HAVE_IDENTITY=no
if [ -n "$IDENTITY" ] && security find-identity -v -p codesigning 2>/dev/null | grep -qF "$IDENTITY"; then
  HAVE_IDENTITY=yes
fi

if [ ! -f "$ICONS/jotbay.icns" ]; then
  echo "error: $ICONS/jotbay.icns missing, run lib/icons/generate.sh first" >&2
  exit 1
fi

echo "==> staging resources"
rm -rf Resources
mkdir -p Resources
cp "$ICONS/jotbay.icns" Resources/jotbay.icns
cp "$ICONS"/menubar/*.png Resources/

# A universal app must carry a universal CLI. The plain release build is
# whatever this Mac is, so when a universal bundle is asked for, build the other
# target too and lipo them. Skipping this ships an Apple-silicon-only tool
# inside an app that claims to run on Intel.
if [ "${JOTBAY_ARCHS:-}" = "arm64 x86_64" ] || [ "${JOTBAY_ARCHS:-}" = "x86_64 arm64" ]; then
  # Always rebuilt, never reused. This used to skip the build when the binary
  # was already universal, which is a shape check, not a freshness check, and
  # a universal binary from the *previous* release satisfied it. The 1.5.0 DMG
  # shipped with 1.4.0 sealed inside its app that way. Cargo is incremental,
  # so an up-to-date rebuild costs seconds.
  echo "    building a universal CLI to bundle"
  ( cd .. && cargo build --release --quiet --target aarch64-apple-darwin \
              && cargo build --release --quiet --target x86_64-apple-darwin )
  lipo -create -output ../target/release/jotbay \
    ../target/aarch64-apple-darwin/release/jotbay \
    ../target/x86_64-apple-darwin/release/jotbay
fi

# Bundle the CLI so the app works regardless of the user's PATH. Prefer a
# release build; fall back to debug so a dev build is still runnable.
for candidate in ../target/release/jotbay ../target/debug/jotbay; do
  if [ -x "$candidate" ]; then
    cp "$candidate" Resources/jotbay
    chmod +x Resources/jotbay
    echo "    bundled CLI from $candidate"
    break
  fi
done
if [ ! -f Resources/jotbay ]; then
  echo "    warning: no jotbay binary found to bundle; the app will fall back to PATH" >&2
fi

# A nested executable must carry its own signature. Xcode signs the bundle but
# not arbitrary files copied into Resources, and notarization rejects the lot:
# "The binary is not signed", "no secure timestamp", "hardened runtime not
# enabled", all reported against Contents/Resources/jotbay, not the app.
# Signing it here means xcodebuild seals an already-valid binary inside.
if [ -f Resources/jotbay ] && [ "$HAVE_IDENTITY" = yes ]; then
  codesign --force --timestamp --options runtime --sign "$IDENTITY" Resources/jotbay
  echo "    signed the bundled CLI"
fi

echo "==> generating Xcode project"
# XcodeGen exits 0 even when spec validation fails, and --quiet hides the
# reason. Left alone that turns into a baffling "Unable to read project" from
# xcodebuild several steps later, so check for the output explicitly.
rm -rf Jotbay.xcodeproj
xcodegen generate
if [ ! -d Jotbay.xcodeproj ]; then
  echo "error: xcodegen produced no project, see its validation output above" >&2
  exit 1
fi

echo "==> building ($CONFIG)"
# xcodebuild's exit status is the only reliable success signal: it leaves a
# partial .app behind on failure, so testing for the bundle proves nothing.
# Piping to grep would report grep's status instead, hence PIPESTATUS.
set +e
# JOTBAY_ARCHS lets the release workflow ask for a universal build; a plain
# local build stays single-architecture so it stays fast.
ARCH_ARGS=()
if [ -n "${JOTBAY_ARCHS:-}" ]; then
  ARCH_ARGS=(ARCHS="$JOTBAY_ARCHS" ONLY_ACTIVE_ARCH=NO)
  echo "    building for: $JOTBAY_ARCHS"
fi

SIGN_ARGS=(CODE_SIGN_IDENTITY="-" CODE_SIGNING_REQUIRED=NO)
if [ "$HAVE_IDENTITY" = yes ]; then
  SIGN_ARGS=(
    CODE_SIGN_IDENTITY="$IDENTITY"
    DEVELOPMENT_TEAM="$TEAM_ID"
    CODE_SIGNING_REQUIRED=YES
    ENABLE_HARDENED_RUNTIME=YES
    # Apple rejects a signature with no secure timestamp, and rejects the
    # get-task-allow entitlement Xcode injects for debugging.
    OTHER_CODE_SIGN_FLAGS="--timestamp"
    CODE_SIGN_INJECT_BASE_ENTITLEMENTS=NO
  )
  echo "    signing with Developer ID"
else
  echo "    signing ad hoc (no Developer ID certificate on this machine)"
fi
echo "    bundle identifier: $JOTBAY_BUNDLE_PREFIX.jotbay"

xcodebuild \
  -project Jotbay.xcodeproj \
  -scheme Jotbay \
  -configuration "$CONFIG" \
  -derivedDataPath "$BUILD_DIR" \
  "${SIGN_ARGS[@]}" \
  ${ARCH_ARGS[@]+"${ARCH_ARGS[@]}"} \
  build 2>&1 | tee "$BUILD_DIR.log" | grep -E "error:|BUILD (SUCCEEDED|FAILED)"
status=${PIPESTATUS[0]}
set -e

if [ "$status" -ne 0 ]; then
  echo "error: xcodebuild failed (exit $status), full log in $BUILD_DIR.log" >&2
  exit "$status"
fi

APP="$BUILD_DIR/Build/Products/$CONFIG/Jotbay.app"
if [ ! -d "$APP" ]; then
  echo "error: build reported success but produced no app bundle" >&2
  exit 1
fi

echo
echo "built: $(cd "$(dirname "$APP")" && pwd)/$(basename "$APP")"
