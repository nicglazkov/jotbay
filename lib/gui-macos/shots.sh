#!/usr/bin/env bash
#
# Regenerate the README screenshots.
#
#   lib/gui-macos/shots.sh
#
# Compiles the real view files against fixed demo data and renders them
# offscreen, so a screenshot cannot drift from the UI. Nothing is captured from
# a running app and no screen-recording permission is involved.
#
# JotbayApp.swift is excluded because it owns @main; screenshots.swift provides
# its own entry point.

set -euo pipefail
cd "$(dirname "$0")/../.."   # repository root, so output lands in docs/images

SRC=lib/gui-macos/Sources
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT

sources=()
for f in "$SRC"/*.swift; do
  case "$(basename "$f")" in
    JotbayApp.swift) continue ;;   # its @main would collide
  esac
  sources+=("$f")
done

echo "==> compiling ${#sources[@]} view files plus the demo data"
swiftc -O -parse-as-library "${sources[@]}" lib/gui-macos/screenshots.swift -o "$OUT/shots"

# The icon is looked up in the executable's directory when it is not in a real
# bundle, so put it beside the binary or every screenshot shows the generic
# document icon instead of the app's.
cp lib/icons/generated/jotbay.icns "$OUT/jotbay.icns"

echo "==> rendering"
( cd "$(pwd)" && "$OUT/shots" )

echo
echo "docs/images:"
ls -1 docs/images
