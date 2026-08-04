#!/usr/bin/env bash
#
# Regenerate the Windows/Linux screenshots for the README.
#
#   lib/gui-tauri/shots.sh
#
# Runs the real front end in a WKWebView with a stubbed Tauri bridge, so the
# page rendered is the one the app produces rather than a hand-edited DOM.
# macOS-only, because it needs WebKit — but the front end is identical to the
# one WebView2 and WebKitGTK render, so the result is representative.

set -euo pipefail
cd "$(dirname "$0")/../.."

echo "==> rendering"
swift lib/gui-tauri/screenshots.swift

echo
echo "docs/images:"
ls -1 docs/images
