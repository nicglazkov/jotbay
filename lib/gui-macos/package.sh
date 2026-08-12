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
# inside, and Gatekeeper hides the mistake by asking Apple online, so the gap
# is invisible on any connected machine and blocks a user whose first launch is
# offline. Only `stapler validate` on the app inside the mounted image proves it.
#
# Requires: the Developer ID certificate, a notarytool keychain profile named
# `notary`, and create-dmg. All three live on the build Mac; CI has none of them,
# which is why this is a local script and not a workflow.

set -euo pipefail

# Refuse to package a working tree that does not match its tag.
#
# This builds from whatever is checked out, and the 1.8.1 DMG therefore shipped
# the tag plus one later commit. A Mac running the cask had code the tag did
# not contain. Linux and Windows artifacts come from CI and can be checked with
# `gh attestation verify`; a locally built DMG can be checked against nothing,
# so the only thing standing between the label and the bytes is this.
#
# Override with JOTBAY_ALLOW_DIRTY=1 for a deliberate test build.
if [ "${JOTBAY_ALLOW_DIRTY:-0}" != "1" ]; then
  version=$(sed -n 's/^ *MARKETING_VERSION: *"\([^"]*\)".*/\1/p' lib/gui-macos/project.yml | head -1)
  if [ -n "$(git status --porcelain)" ]; then
    echo "error: the working tree is dirty. The DMG would not match any tag." >&2
    echo "       commit first, or set JOTBAY_ALLOW_DIRTY=1 for a test build." >&2
    exit 1
  fi
  if [ -n "$version" ] && git rev-parse "v$version" >/dev/null 2>&1; then
    if [ "$(git rev-parse HEAD)" != "$(git rev-parse "v$version^{commit}")" ]; then
      echo "error: HEAD is not v$version, so this DMG would be labelled with a" >&2
      echo "       version it does not contain. Check out the tag first:" >&2
      echo "         git checkout v$version" >&2
      echo "       or set JOTBAY_ALLOW_DIRTY=1 if that is deliberate." >&2
      exit 1
    fi
  fi
fi
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

# Drop Launch Services registrations for copies of the app that no longer
# exist. Nothing in macOS prunes these on its own, and Finder is free to launch
# any registered copy, so a ghost inside a long-gone DMG can win over the real
# install and serve a build from weeks ago.
#
# The dump prints "path: <path> (0xHEX)". Paths can contain spaces, as the
# volume name of every release DMG does, so match the whole path and read whole
# lines rather than splitting on whitespace.
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

prune_launch_services() {
  local lsr="$LSREGISTER"
  [ -x "$lsr" ] || return 0

  local total=0 gone stale path _pass
  # Loop rather than sweep once. Unregistering rewrites the database while the
  # sweep is reading it, and entries listed at the start can survive the pass:
  # a first run on this machine cleared 30 of 32 and needed a second for the
  # rest. Stop when a pass removes nothing, which is the only proof it is done.
  for _pass in 1 2 3; do
    gone=0
    stale=$("$lsr" -dump 2>/dev/null |
      sed -n 's/^[[:space:]]*path:[[:space:]]*\(.*Jotbay\.app\) (0x[0-9a-f]*)$/\1/p' |
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
    say "pruned $total stale Launch Services registration(s)"
  fi
  return 0
}

# `notarytool submit --wait` exits 0 even when the submission comes back
# Invalid, so the exit code proves nothing. Read the status, and on failure
# print the log. It names the exact file, reason and architecture, and is the
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
    # if-then-else. The `|| true` also swallows a failure of the *test*, which
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
  || die "no signing identity, copy signing.env.example to signing.env and fill it in"
security find-identity -v -p codesigning 2>/dev/null | grep -qF "$IDENTITY" \
  || die "no Developer ID certificate on this machine matching $IDENTITY"
# The credential lives in the data protection keychain, which the system makes
# unreadable while the screen is locked. notarytool cannot tell that apart from
# a credential that was never stored, so it says "No Keychain password item
# found" either way, and that sends you off to re-create credentials which are
# perfectly fine. Ask the screen first, and say the true thing.
screen_locked() {
  # True only when the key is present *and* set. Testing for the key alone was
  # not enough: it raced a lock that happened between the check and the call,
  # and reported the wrong cause for a build that had just failed.
  ioreg -n Root -d1 -a 2>/dev/null |
    plutil -extract IOConsoleLocked raw -o - - 2>/dev/null |
    grep -qx "true"
}

notary_ready() {
  xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null 2>&1
}

if ! notary_ready; then
  # Retried once. The credential lives in the data protection keychain, and a
  # single read has come back empty on a machine where the next one worked.
  sleep 3
  if ! notary_ready; then
    if screen_locked; then
      die "this Mac's screen is locked, so the notarization credentials cannot be
       read. Unlock it and run this again. Nothing is wrong with the profile."
    fi
    die "no notarytool profile named '$PROFILE', see the publishing guide"
  fi
fi
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

# The volume name carries the version, and that is load bearing rather than
# decorative. macOS App Management protects the path an installed app was
# distributed from, so creating /Volumes/Jotbay/Jotbay.app is refused with
# "Operation not permitted" on a machine that has ever installed Jotbay from a
# disk image, and no amount of resetting removable-volume consent clears it.
# Proven by elimination: Calculator.app on a volume named Jotbay works, and
# Jotbay.app on a volume named anything else works. Only the exact pair fails.
#
# Granting the terminal App Management would also fix it, but that is a
# permanent permission on the developer's machine to work around a naming
# collision, and a versioned volume name is what most applications ship anyway.
create-dmg \
  --volname "$APP_NAME $VERSION" \
  --window-pos 200 120 --window-size 560 380 \
  --icon-size 110 \
  --icon "$APP_NAME.app" 150 175 \
  --app-drop-link 410 175 \
  --hide-extension "$APP_NAME.app" \
  --no-internet-enable \
  "$DIST/$APP_NAME.dmg" "$DIST/dmgroot/" >/dev/null

rm -rf "$DIST/dmgroot"

# create-dmg mounts the image to lay out its window, and Launch Services
# registers the copy of the app it sees inside. Detaching the volume does not
# unregister it, so every build leaves another ghost behind: after twenty-odd
# builds this machine had twenty-five registrations for one installed app, and
# Finder is free to launch any of them. Prune the ones whose files are gone.
prune_launch_services

# Also drop the app we just built. It is a real file, so the prune above keeps
# it, but a registered build tree competes with /Applications for launches and
# has already won once: a DMG install was verified by opening what turned out
# to be the source tree's copy. Anyone who wants the dev build can open it, and
# opening it registers it again.
"$LSREGISTER" -u "$PWD/$BUILT" >/dev/null 2>&1 || true

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
echo "    Homebrew cask. Note the cask cannot work until the repo is public"
echo "    releases/latest/download/ 404s while it is private."
