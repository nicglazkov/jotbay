#!/usr/bin/env bash
#
# Render every icon the project ships from the three source SVGs.
#
# Output lands in generated/ and IS committed: the release workflow builds on
# Linux and Windows runners, which have no `iconutil`, so the .icns must be
# produced here on a Mac rather than in CI.
#
# Requires: rsvg-convert (brew install librsvg), and on macOS iconutil.

set -euo pipefail

cd "$(dirname "$0")"
OUT="generated"

have() { command -v "$1" >/dev/null 2>&1; }

# Only a Mac can produce the .icns, and it is committed precisely so the Linux
# and Windows runners have one. Wiping generated/ on a non-Mac would delete it
# with no way to rebuild, the script would still exit 0, and Jotbay would
# faithfully sync the deletion to every machine. Preserve it across the wipe.
ICNS_KEEP=""
if [ -f "$OUT/jotbay.icns" ] && ! have iconutil; then
  ICNS_KEEP="$(mktemp -d)/jotbay.icns"
  cp "$OUT/jotbay.icns" "$ICNS_KEEP"
fi

rm -rf "$OUT"
mkdir -p "$OUT/linux" "$OUT/menubar" "$OUT/tauri" "$OUT/tray"

if [ -n "$ICNS_KEEP" ]; then
  cp "$ICNS_KEEP" "$OUT/jotbay.icns"
  rm -rf "$(dirname "$ICNS_KEEP")"
  echo "note: kept the existing jotbay.icns — only macOS can regenerate it" >&2
fi

if ! have rsvg-convert; then
  echo "error: rsvg-convert not found — brew install librsvg" >&2
  exit 1
fi

render() { # svg size out
  rsvg-convert -w "$2" -h "$2" "$1" -o "$3"
}

echo "==> app icon PNGs"
for size in 16 32 48 64 128 256 512 1024; do
  render jotbay.svg "$size" "$OUT/linux/${size}x${size}.png"
done
cp "$OUT/linux/512x512.png" "$OUT/icon.png"

echo "==> macOS .icns"
ICONSET="$OUT/jotbay.iconset"
mkdir -p "$ICONSET"
# Apple's required names; each @2x is the next size up rendered at that density.
render jotbay.svg 16   "$ICONSET/icon_16x16.png"
render jotbay.svg 32   "$ICONSET/icon_16x16@2x.png"
render jotbay.svg 32   "$ICONSET/icon_32x32.png"
render jotbay.svg 64   "$ICONSET/icon_32x32@2x.png"
render jotbay.svg 128  "$ICONSET/icon_128x128.png"
render jotbay.svg 256  "$ICONSET/icon_128x128@2x.png"
render jotbay.svg 256  "$ICONSET/icon_256x256.png"
render jotbay.svg 512  "$ICONSET/icon_256x256@2x.png"
render jotbay.svg 512  "$ICONSET/icon_512x512.png"
render jotbay.svg 1024 "$ICONSET/icon_512x512@2x.png"

if have iconutil; then
  iconutil -c icns "$ICONSET" -o "$OUT/jotbay.icns"
  rm -rf "$ICONSET"
  echo "    generated/jotbay.icns"
else
  echo "    skipped .icns (iconutil is macOS-only; run this script on a Mac)" >&2
fi

echo "==> Windows .ico"
if have magick; then
  magick \
    "$OUT/linux/16x16.png" "$OUT/linux/32x32.png" "$OUT/linux/48x48.png" \
    "$OUT/linux/64x64.png" "$OUT/linux/128x128.png" "$OUT/linux/256x256.png" \
    "$OUT/jotbay.ico"
  echo "    generated/jotbay.ico"
else
  echo "    skipped .ico (ImageMagick not found)" >&2
fi

echo "==> menu bar templates"
# Template images must be black-plus-alpha; the system tints them. The
# "Template" suffix is what tells AppKit to treat them that way.
for state in idle syncing attention; do
  render "menubar-$state.svg" 22 "$OUT/menubar/${state}Template.png"
  render "menubar-$state.svg" 44 "$OUT/menubar/${state}Template@2x.png"
done

echo "==> tray icons (Linux and Windows)"
# Full-colour, unlike the macOS templates: an AppIndicator draws the PNG as
# given, so a black-plus-alpha template would vanish into Ubuntu's dark panel.
# Rendered at 64 so the plate edge stays clean on a HiDPI panel.
for state in idle syncing attention; do
  render "tray-$state.svg" 32 "$OUT/tray/${state}.png"
  render "tray-$state.svg" 64 "$OUT/tray/${state}@2x.png"
done

echo "==> Tauri icon set"
# Exact filenames Tauri expects in its bundle config.
render jotbay.svg 32  "$OUT/tauri/32x32.png"
render jotbay.svg 128 "$OUT/tauri/128x128.png"
render jotbay.svg 256 "$OUT/tauri/128x128@2x.png"
cp "$OUT/icon.png" "$OUT/tauri/icon.png"
[ -f "$OUT/jotbay.icns" ] && cp "$OUT/jotbay.icns" "$OUT/tauri/icon.icns"
[ -f "$OUT/jotbay.ico" ]  && cp "$OUT/jotbay.ico"  "$OUT/tauri/icon.ico"

echo
echo "done — $(find "$OUT" -type f | wc -l | tr -d ' ') files in $OUT/"
