#!/usr/bin/env bash
#
# Build, notarize and package Jotbay.app for distribution outside the App Store.
#
#   lib/gui-macos/package.sh
#
# Produces a notarized, stapled Jotbay.dmg and Jotbay.zip in dist/.
#
# ORDER IS THE WHOLE GAME. The app is notarized and stapled BEFORE anything is
# packaged around it. Stapling a finished DMG does not reach the app sealed
# inside, and Gatekeeper hides the mistake by asking Apple online — so the gap
# is invisible on any connected machine and blocks a user whose first launch is
# offline. Only `stapler validate` on the app inside the mounted image proves it.
#
# Requires: the Developer ID certificate, a notarytool keychain profile named
# `notary`, and create-dmg. All three live on the build Mac; CI has none of them,
# which is why this is a local script and not a workflow.

set -euo pipefail
cd "$(dirname "$0")"

APP_NAME="Jotbay"
# See signing.env.example. Gitignored, because a certificate name and a Team ID
# say who is publishing, not what is being published.
# shellcheck source=/dev/null
[ -f signing.env ] && . ./signing.env
IDENTITY="${JOTBAY_SIGN_IDENTITY:-}"
PROFILE="${JOTBAY_NOTARY_PROFILE:-notary}"
DIST="dist"
BUILT="build/Build/Products/Release/$APP_NAME.app"

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# `notarytool submit --wait` exits 0 even when the submission comes back
# Invalid, so the exit code proves nothing. Read the status, and on failure
# print the log — it names the exact file, reason and architecture, and is the
# only thing worth reading when Apple rejects something.
notarize() {
  local target="$1" out id status
  out=$(xcrun notarytool submit "$target" --keychain-profile "$PROFILE" --wait 2>&1)
  printf '%s\n' "$out"
  status=$(printf '%s' "$out" | awk '/^ *status:/ {s=$2} END {print s}')
  if [ "$status" != "Accepted" ]; then
    id=$(printf '%s' "$out" | awk '/^ *id:/ {print $2; exit}')
    printf '\033[31mnotarization %s\033[0m\n' "${status:-failed}" >&2
    # Written as an `if` rather than `A && B || true`: that form is not
    # if-then-else — the `|| true` also swallows a failure of the *test*, which
    # is the one case worth seeing. shellcheck 0.8 flags it (SC2015); 0.11 does
    # not, so CI caught what the local tool missed.
    if [ -n "$id" ]; then
      xcrun notarytool log "$id" --keychain-profile "$PROFILE" >&2 || true
    fi
    die "notarization did not succeed for $target"
  fi
}

# --- preflight --------------------------------------------------------------

[ -n "$IDENTITY" ] \
  || die "no signing identity — copy signing.env.example to signing.env and fill it in"
security find-identity -v -p codesigning 2>/dev/null | grep -qF "$IDENTITY" \
  || die "no Developer ID certificate on this machine matching $IDENTITY"
xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null 2>&1 \
  || die "no notarytool profile named '$PROFILE' — see the publishing guide"
command -v create-dmg >/dev/null || die "create-dmg not found (brew install create-dmg)"

VERSION=$(grep -m1 'MARKETING_VERSION' project.yml | cut -d'"' -f2)
[ -n "$VERSION" ] || die "could not read MARKETING_VERSION from project.yml"

say "packaging $APP_NAME $VERSION"
rm -rf "$DIST"
mkdir -p "$DIST"

# --- build ------------------------------------------------------------------

say "building universal and signing with Developer ID"
JOTBAY_ARCHS="arm64 x86_64" ./build.sh Release >/dev/null
[ -d "$BUILT" ] || die "build produced no app"

archs=$(lipo -archs "$BUILT/Contents/MacOS/$APP_NAME")
case "$archs" in
  *x86_64*arm64*|*arm64*x86_64*) ;;
  *) die "app is not universal: $archs" ;;
esac

# Piping codesign into grep -q under pipefail makes grep exit on the first match,
# codesign take SIGPIPE, and the pipeline report failure on a passing check.
# Capture, then test.
sig=$(codesign -dv --verbose=4 "$BUILT" 2>&1)
case "$sig" in
  *"Authority=Developer ID Application"*) ;;
  *) die "app is not signed with Developer ID" ;;
esac
case "$sig" in
  *"flags=0x10000(runtime)"*) ;;
  *) die "hardened runtime is not enabled" ;;
esac
case "$sig" in
  *Timestamp=*) ;;
  *) die "signature carries no secure timestamp" ;;
esac

ent=$(codesign -d --entitlements - "$BUILT" 2>/dev/null || true)
case "$ent" in
  *get-task-allow*) die "get-task-allow entitlement present; Apple will reject" ;;
esac

# --- notarize the APP, before any packaging ---------------------------------

say "notarizing the app (2-15 minutes, sometimes longer)"
ditto -c -k --keepParent "$BUILT" "$DIST/notarize-app.zip"
notarize "$DIST/notarize-app.zip"
rm -f "$DIST/notarize-app.zip"

say "stapling the app"
xcrun stapler staple "$BUILT"

# --- package around the stapled app -----------------------------------------

say "building the disk image"
rm -rf "$DIST/dmgroot"
mkdir -p "$DIST/dmgroot"
cp -R "$BUILT" "$DIST/dmgroot/"

create-dmg \
  --volname "$APP_NAME" \
  --window-pos 200 120 --window-size 560 380 \
  --icon-size 110 \
  --icon "$APP_NAME.app" 150 175 \
  --app-drop-link 410 175 \
  --hide-extension "$APP_NAME.app" \
  --no-internet-enable \
  "$DIST/$APP_NAME.dmg" "$DIST/dmgroot/" >/dev/null

rm -rf "$DIST/dmgroot"

say "notarizing the disk image"
notarize "$DIST/$APP_NAME.dmg"
xcrun stapler staple "$DIST/$APP_NAME.dmg"

# A zip cannot carry a ticket of its own; it inherits the app's, which is
# exactly why the app was stapled before anything was built around it.
ditto -c -k --keepParent "$BUILT" "$DIST/$APP_NAME.zip"

# --- verify the way an offline stranger's Mac would -------------------------

say "verifying"
xcrun stapler validate "$BUILT"
xcrun stapler validate "$DIST/$APP_NAME.dmg"
spctl -a -vvv -t install "$BUILT" 2>&1 | tail -2

echo
say "done"
printf '    %s\n' "$DIST/$APP_NAME.dmg" "$DIST/$APP_NAME.zip"
printf '    sha256: %s\n' "$(shasum -a 256 "$DIST/$APP_NAME.dmg" | cut -d' ' -f1)"
echo
echo "    Next: attach both to the release, then bump version and sha256 in the"
echo "    Homebrew cask. Note the cask cannot work until the repo is public —"
echo "    releases/latest/download/ 404s while it is private."
