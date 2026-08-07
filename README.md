# Jotbay

Keep a folder of markdown notes in sync across every machine you use, through a
private git repository that you own.

There's no server, no account, and no telemetry. Your notes go to your remote and
nowhere else. **Conflicts keep both versions.** Jotbay never discards anything
you wrote in order to resolve one.

Jotbay is built for handing context to an AI assistant from whichever machine
you're at, which is a use case that punishes silent data loss more than most.

![The Jotbay window: four machines, one behind, one failing](docs/images/macos-main.png)

*Every machine reports in. The one that has been failing for six hours says so
here, and in no commit log anywhere.*

## Install

### Use an installer

Download an installer from the [releases page](../../releases/latest):

| File | Platform | What it does |
|---|---|---|
| `Jotbay.dmg` | macOS | Signed and notarized. Drag the app to Applications. |
| `Jotbay_<version>_x64-setup.exe` | Windows | Installs for your user only. No administrator rights needed. |
| `Jotbay_<version>_amd64.deb` | Debian, Ubuntu | Double-click, or run `sudo apt install ./Jotbay_<version>_amd64.deb`. |
| `Jotbay_<version>_amd64.AppImage` | Other Linux | Make it executable and run it. |

Every installer includes the `jotbay` command, so choosing the graphical route
doesn't cost you the command line.

Open Jotbay. The first screen asks where your notes should live. Jotbay can
create the private repository for you, clone one that you already have, or adopt
a folder that's already a clone.

The Windows installer isn't code-signed yet, so SmartScreen blocks it with
*"Windows protected your PC"*. Click **More info**, then **Run anyway**.

### Use Homebrew

```bash
brew trust nicglazkov/tap
brew install --cask nicglazkov/tap/jotbay
```

Homebrew 6 refuses to load a cask from a third-party tap until you trust it, so
the first line is required once.

### Use the terminal

```bash
curl -fsSL https://raw.githubusercontent.com/nicglazkov/jotbay/main/install/install.sh | bash
jotbay init
```

```powershell
irm https://raw.githubusercontent.com/nicglazkov/jotbay/main/install/install.ps1 | iex
jotbay init
```

Both routes end in the same place. `jotbay init` offers the same three choices as
the first-run screen.

<p align="center">
  <img src="docs/images/macos-first-run.png" width="49%" alt="First run: create, clone, or adopt">
  <img src="docs/images/macos-ready.png" width="49%" alt="Setup complete, offering desktop shortcuts">
</p>

On a headless server, skip the GUI:

```bash
./install/install.sh --no-gui
```

Jotbay requires `git`. The **Create one for me** route also requires
[`gh`](https://cli.github.com) to be signed in. The other two routes don't, and
nothing after setup requires it.

### Other release files

The releases page also contains `jotbay-<platform>.tar.gz` and
`jotbay-windows-x86_64.zip`. These contain the binaries without an installer.
The install scripts and `jotbay upgrade` download them for you, so you don't
normally need to download them yourself.

## Commands

| Command | What it does |
|---|---|
| `jotbay` | Shows the current state, locally and across every machine |
| `jotbay sync` | Commits, integrates the remote, and pushes |
| `jotbay dash` | Opens a live dashboard in the terminal |
| `jotbay nodes` | Lists every machine that has reported in |
| `jotbay activity` | Shows what every machine has done: syncs, conflicts, failures |
| `jotbay log` | Shows the commit history of what changed |
| `jotbay path` | Prints the path of your notes folder |
| `jotbay shortcut` | Creates desktop icons for the app and for your notes folder |
| `jotbay upgrade` | Fetches the current release and replaces this machine's binaries |
| `jotbay init` | Sets up a vault on a new machine |

Jotbay watches the folder and syncs a couple of seconds after you stop typing, so
notes reach your other machines in well under a minute without you pressing
anything.

The desktop app shows the same information with per-machine detail and history.
It lives in the menu bar on macOS and in the system tray on Windows and Linux.
The icon changes with state: idle, syncing, or needs attention. That icon is your
only health signal after you close the window. On Windows, drag it out of the
overflow flyout onto the taskbar so that you can see it.

macOS gets a native SwiftUI app. Windows and Linux share one that's built on the
system webview. Both show the same information, and each follows its own
platform's conventions:

![The Windows and Linux window](docs/images/tauri-main.png)

## How it works

Sync is git against a private repository. GitHub is the default because `gh`
makes **Create one for me** a single click, but nothing depends on it. GitLab,
Codeberg, a self-hosted Forgejo, or a bare SSH remote all work. There's no server
and nothing to run.

**Conflicts keep both versions.** If you edit the same file on two machines
between syncs, the incoming version keeps the filename, and your version is saved
beside it as `notes.conflict-<machine>-<timestamp>.md`. Sync never stalls, and no
version is ever discarded.

**Machine status rides on its own git refs.** Each machine publishes to
`refs/jotbay-status/<hostname>`, so no two machines can conflict, and none of it
lands on `main`. The history stays pure content.

**Activity is a real feed, not a commit log.** Alongside its status, each machine
keeps a bounded buffer of what it actually did: content moved, conflicts
resolved, and syncs that failed. Jotbay deliberately doesn't record syncs that
changed nothing, because six machines checking in every ten minutes would bury
everything worth seeing. A machine that has been failing for three days shows up
here, and in no commit log anywhere.

## What your notes folder can hold

Your notes folder takes any file type. Sync and conflict handling are byte-exact
on binaries, which is verified by test. The limits come from git and from the
host, not from Jotbay:

| Limit | Value |
|---|---|
| Single file | **100 MB hard ceiling.** GitHub rejects the push. Jotbay warns above 50 MB. |
| Repository | Stay under 1 GB if you can, and under 5 GB at most. |
| Practical file size | Keep files under about 25 MB. |

The constraint that bites isn't size, it's **churn**. Git stores a near-complete
copy of every version of a binary, and deleting the file doesn't reclaim the
space. In one measurement, a 100 MB file that was pushed and then deleted left
the remote at 178 MB permanently. A 10 MB image that you edit twenty times costs
about 200 MB forever, on every machine that clones the repository.

Markdown, notes, and the occasional screenshot or PDF are all comfortable. Video,
disk images, datasets, and anything large that you rewrite often belong somewhere
else.

**You don't have to remember any of this.** Jotbay detects anything at or over
100 MB before staging it and leaves it uncommitted, so it never creates a commit
that could never be pushed. The file stays where you put it, everything else
syncs normally, and Jotbay lists the file in `jotbay status`, in `jotbay
activity`, and in both GUIs until you deal with it. Jotbay flags files over 25 MB
and 50 MB the same way, but still syncs them.

**Jotbay doesn't support Git LFS.** LFS would lift the 100 MB ceiling, but
conflict resolution reads merge stages with `git show :2:`, which returns an LFS
pointer rather than the file. A conflict on an LFS-tracked file would therefore
write a 130-byte stub over real content. Supporting LFS is a small change once
the resolver pipes stages through `git lfs smudge`, and until then Jotbay
deliberately doesn't recommend it.

Two implementation notes. Conflict resolution holds both versions of the
conflicted file in memory at once, so peak usage is roughly twice that file's
size. Jotbay also normalizes line endings to LF for text, which is what keeps
markdown from churning between Windows and Linux. File types where CRLF is
significant, such as `.bat`, `.pem`, and `.reg`, are exempted in
`.gitattributes`.

## Update Jotbay

Run `jotbay upgrade` to fetch the current release and replace this machine's
binaries. Both GUIs offer the upgrade as a notice when one is available.

If you installed Jotbay from a package, such as the `.deb` or the Homebrew cask,
`jotbay upgrade` tells you to use that package manager instead. It doesn't write
a second copy elsewhere.

The replacement path is worth knowing about. On Unix, Jotbay stages the new
binary beside the old one and renames it over the old one, so the running process
keeps its own inode. On Windows, Jotbay renames the outgoing file to `.old`
first, because Windows won't overwrite a running image at all. Writing over the
live file, which is what releases up to 1.3.2 did, corrupts the running program
on macOS and fails outright on Linux.

A self-updater's replacement path is always executed by the *old* version, so an
upgrade can never deliver a fix to it. A machine on 1.3.2 or earlier needs one
manual install first.

## Build from source

See [CONTRIBUTING.md](CONTRIBUTING.md). The short version:

```bash
cd lib && cargo test           # engine and CLI
lib/gui-macos/build.sh         # the macOS app
lib/gui-tauri/bundle.sh        # the Windows and Linux app, plus installers
```

GitHub Actions builds releases for every platform on a `v*` tag, so no machine
needs a Rust toolchain to install one.

## License

[MIT](LICENSE). For the security policy and trust model, see
[SECURITY.md](SECURITY.md).
