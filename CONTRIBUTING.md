# Contributing

## The shape of the thing

One engine, three front ends. `lib/core` owns every decision about git — what
gets committed, how a conflict is resolved, when a node counts as stale — and
the CLI, the Tauri app and the macOS app all drive it. If you find yourself
writing git logic in a UI, that is the bug.

```
lib/core/          the engine: sync, conflicts, status refs, limits, updates
lib/cli/           the `jotbay` command, including the terminal dashboard
lib/gui-tauri/     the Windows and Linux app (its own cargo workspace)
lib/gui-macos/     the native SwiftUI app; shells out to `jotbay --json`
install/           install and uninstall scripts for people who prefer them
```

`lib/gui-tauri` is a **separate workspace** on purpose: linking it needs
WebKitGTK, and a headless server should be able to build the CLI without
installing a browser engine.

## Building

```bash
cd lib && cargo test           # engine and CLI
cargo build --release          # produces target/release/jotbay

lib/gui-macos/build.sh         # the macOS app (needs Xcode and XcodeGen)
lib/gui-tauri/bundle.sh        # the Windows/Linux app and its installers
lib/icons/generate.sh          # regenerate every icon from the source SVG
```

Signing is optional and local. Copy `lib/gui-macos/signing.env.example` to
`signing.env` and fill it in if you have a Developer ID; without it the build
signs ad hoc, which runs fine and cannot be notarized.

## Testing

`cargo test` drives real git repositories in temporary directories — there are
no mocks, because the behaviour worth testing is what git actually does. CI runs
the suite on Linux **and** Windows: line endings, path separators and process
spawning differ enough that a Unix-only run has already missed a real bug.

If you change conflict resolution, add a test that asserts on bytes. The policy
is that no version is ever lost, and that is only meaningful if it holds for
binaries too.

## Style

Match the surrounding code. A few things that are load-bearing here:

- **Comments explain why, not what.** Most of the comments in this codebase
  record something that went wrong and the reason the code now looks strange.
  Deleting one of those loses the reason.
- **Errors are sentences a user can act on.** `summarise_error` in `sync.rs`
  exists because five lines of raw `remote: error:` told a first-time user
  nothing. Raw output belongs behind the verbose setting.
- **One source of truth.** File-size advice lives in `limits.rs` and is
  serialized to the front ends; when each UI had its own copy, they drifted, and
  one of them was still recommending something the engine had stopped
  supporting.

## Pull requests

Keep them focused, and say what problem the change solves in the description.
The test suite must pass on both runners. There is no CLA.
