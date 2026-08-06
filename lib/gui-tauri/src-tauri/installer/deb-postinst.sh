#!/bin/sh
#
# Point the installed desktop entry at the binary this package shipped.
#
# Tauri generates `Exec=jotbay-gui`. A bare command, so the application menu
# resolves it through PATH. On a machine that also ran install.sh, Ubuntu's
# default ~/.profile puts ~/.local/bin first, and the menu launches that copy
# instead of the one just installed. Observed: the two were different versions
# and behaved differently.
#
# This runs at .deb install time, which is exactly why the fix lives here
# rather than in a desktopTemplate. The AppImage symlinks the *same* generated
# desktop file into its AppDir, and its AppRun passes Exec= straight to
# execvp(), which only consults PATH when the argument has no slash. An
# absolute Exec would therefore make the AppImage try to run the host's
# /usr/bin/jotbay-gui and fail on any machine without this package installed.
# Verified against the AppRun binary Tauri actually embeds.
#
# A maintainer script only ever runs for the package, so the AppImage keeps the
# relative Exec it needs.

set -e

desktop=/usr/share/applications/Jotbay.desktop
[ -f "$desktop" ] || exit 0

# Anchored, and only when still relative, so re-running is harmless.
sed -i 's|^Exec=jotbay-gui\([[:space:]]*\)$|Exec=/usr/bin/jotbay-gui\1|' "$desktop"

exit 0
