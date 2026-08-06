#!/usr/bin/env bash
#
# Cut a release: bump every version, refresh every lockfile, run the tests.
#
#   lib/release.sh 1.2.0
#
# Exists because the version lives in four files across two independent cargo
# workspaces, and bumping them by hand has twice shipped a tag whose Tauri
# lockfile still named the old version. CI then fails deep inside the Linux and
# Windows builds with "cannot update the lock file because --locked was passed",
# which reads like a dependency problem rather than the bookkeeping slip it is.

set -euo pipefail

cd "$(dirname "$0")/.."
VERSION="${1:-}"

if ! printf '%s' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "usage: lib/release.sh <major.minor.patch>" >&2
  exit 2
fi

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }

say "setting version to $VERSION"
python3 - "$VERSION" <<'PY'
import pathlib, re, sys
version = sys.argv[1]
# (file, pattern, replacement). Every place the version is written.
targets = [
    ("lib/Cargo.toml", r'^version = "[^"]+"', f'version = "{version}"'),
    ("lib/gui-tauri/src-tauri/Cargo.toml", r'^version = "[^"]+"', f'version = "{version}"'),
    ("lib/gui-tauri/src-tauri/tauri.conf.json", r'"version": "[^"]+"', f'"version": "{version}"'),
    ("lib/gui-macos/project.yml", r'MARKETING_VERSION: "[^"]+"', f'MARKETING_VERSION: "{version}"'),
]
for path, pattern, replacement in targets:
    p = pathlib.Path(path)
    text = p.read_text()
    new, n = re.subn(pattern, replacement, text, count=1, flags=re.M)
    if n != 1:
        raise SystemExit(f"error: no version match in {path}")
    p.write_text(new)
    print(f"    {path}")
PY

# Written here rather than by CI so it lands in the release commit itself.
# Every machine then learns about the release through the sync it already does,
# with no extra network call anywhere.
say "writing the release marker"
printf '{\n  "version": "%s",\n  "tag": "v%s"\n}\n' "$VERSION" "$VERSION" > .jotbay-release.json
echo "    .jotbay-release.json -> $VERSION"

# Both workspaces, because they are genuinely separate: the Tauri crate carries
# its own [workspace] so the headless servers can build the CLI without pulling
# in WebKitGTK. That independence is why its lockfile is so easy to forget.
say "refreshing lockfiles"
cargo update --workspace --manifest-path lib/Cargo.toml --quiet
cargo update --workspace --manifest-path lib/gui-tauri/src-tauri/Cargo.toml --quiet

say "verifying both lockfiles are current (this is what CI enforces)"
cargo metadata --locked --format-version 1 --manifest-path lib/Cargo.toml >/dev/null
cargo metadata --locked --format-version 1 --manifest-path lib/gui-tauri/src-tauri/Cargo.toml >/dev/null

say "running tests"
cargo test --quiet --manifest-path lib/Cargo.toml

echo
say "ready, commit, then tag"
cat <<EOF

    git add -A && git commit -m "Release $VERSION"
    git tag -a v$VERSION -m "..." && git push origin main "v$VERSION"

EOF
