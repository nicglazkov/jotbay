# Publishing decisions

Supersedes `open-source-plan.md` (2026-08-01), which proposed a template
repository and argued for a rename. Both were rejected; this records what was
actually decided and why.

## Two repositories, not a template

The template-repository route — publish one repo, users click *Use this
template* and get their own private copy — was the cheaper option and is wrong
here. It puts a copy of the source inside every user's private notes repo, makes
every update a git merge instead of a command, and means a user's notes live in
a fork of a public repository, one setting away from being public.

So: **a public repo holding only the tool, and each user's own private notes
repo created by `jotbay init`.** The cost was real installer work — bootstrapping
without a clone, and guided first-run setup in both GUIs — and that work is done.

What it buys, all of which was blocked while one private repo held both:

| | |
|---|---|
| No `gh` needed to install or upgrade | public releases are plain HTTPS; `releases/latest/download/` 404s to *everyone* while a repo is private, including its owner |
| Homebrew cask, winget, Flathub become possible | all three need a public installer URL |
| Strangers can use it | onboarding stops being "clone my private repo" |
| Notes stop sharing history with the tool | they are unrelated things |

## The name changed, twice

The tool was originally `vault`, which collided with HashiCorp Vault — that
product puts a `vault` binary on PATH, which is a real conflict rather than a
branding preference. It became `inkway`, and `inkway` failed for a different and
more interesting reason: its author kept forgetting it. An abstract coinage with
no familiar root gives the memory nothing to hook onto.

It is now **jotbay** — *jot*, the verb for writing something down quickly, plus
*bay*, a sheltered place things return to.

Four things were checked before committing, and the last one is the one most
naming exercises skip:

1. **Registries.** crates.io, npm, PyPI and Homebrew are all free; there are zero
   GitHub repositories with the name.
2. **Live products, in any industry.** This is what registry checks cannot see,
   and it eliminated every other finalist: `synclave.app` is a shipping paid Mac
   sync product, `postrider.app` is a local-first macOS developer tool,
   `notecairn.com` is a personal-knowledge-management site ranking first for its
   own name, npm `homeport` is a two-week-old CLI for shipping binaries, and
   JETBAY is an air-charter company with an app in the US App Store.
3. **Phonetics.** The seam between the two halves of a compound decides whether
   it is pleasant to say. A sonorant running into a stop — the `/l/`+`/b/` of
   *mailbag* — flows; two stops colliding does not. `jotbay` pays a small tax
   here (`/t/`+`/b/`) and was chosen anyway, because connotation is forever and
   an awkward seam stops being noticed in a week.
4. **Whether the name survives as a search term.** Compounds of two very common
   words have empty registries *because* search engines decompose them back into
   parts: `NAME notes sync` returns nothing containing the name. `jot` is
   uncommon enough to hold together as one token, which is why it survived where
   `syncbay` and `stowbox` did not.

Trademark findings behind point 2 are a floor, not clearance — the authoritative
sources block automated access, so a real search is still owed before the name
goes on a business.

### What a rename costs, recorded so the next one is cheaper

A global find-and-replace is most of the work and all of the danger. The parts
that needed hand-holding, every one of which fails silently:

- **The settings file.** `vault_path` lives in the config directory, whose name
  contains the product name. Migrate it or every machine that upgrades opens onto
  first-run setup as though it had never been configured. `settings::load` now
  copies from the old location once, and copies rather than moves, because an
  older binary may still be on the machine and would find an empty directory.
- **Historical literals must not be renamed.** The launchd label, systemd unit
  and Windows scheduled-task name from previous releases have to survive
  verbatim in the uninstall paths, or a leftover scheduler keeps firing against
  a binary that no longer exists. A blind rewrite renamed them and quietly
  removed the ability to clean them up.
- **Bundle identifiers change app identity.** `com.glazkov.jotbay` is a new
  application to macOS, so TCC permissions reset and the old bundle stays
  installed until removed by hand.
- **Status refs are namespaced by product name.** `refs/inkway-status/*` become
  orphans that no version can read; they are deleted, not migrated.

## Releases come from the tool repo, not from `origin`

`update::install` used to derive the repo from the vault's `origin`, which only
worked while the tool and the notes shared one remote. It now targets a fixed
public repo, overridable with `JOTBAY_TOOL_REPO`.

Update *detection* had the same problem from the other side: it read a marker
file that `lib/release.sh` writes into the repository, so the answer arrived by
the same sync that carried the notes and cost no network call. A notes-only repo
carries no such marker. Detection now falls back to
`api.github.com/repos/<tool>/releases/latest`, cached for six hours in the config
directory, and — importantly — that request is made **only** on paths that
already accept network latency. The GUI repaints its state every twenty seconds;
an HTTP request on that path would be indefensible.

The marker is still read first when present, so a repository that holds both
still answers for free.

## Identity is configuration, not source

- **Signing** — certificate name, Team ID, bundle prefix and notary profile live
  in `lib/gui-macos/signing.env`, which is gitignored. A Team ID identifies an
  Apple developer account, not a project. Without the file the build signs ad
  hoc, which runs locally and cannot be notarized; CI has always taken that path.
- **Bundle identifiers** come from `JOTBAY_BUNDLE_PREFIX`, defaulting to
  `com.example` for macOS. It must stay stable across releases: an installer
  whose identifier changed reads as a different product and installs alongside
  the old one instead of upgrading it.
- **The scheduled sync's launchd label** is `com.jotbay.sync`. It ends up in
  every user's LaunchAgents directory, so it names the tool. `install.sh` and
  `uninstall.sh` also remove the publisher-specific label used before 1.4.0.

## History starts fresh

The existing history is not publishable and not worth laundering: 1,189 commits
mention the author by name, it has already been rewritten once to remove
accidentally-committed build output, and every automated sync commit is titled
with a machine's hostname. A `git checkout --orphan` start costs nothing that
matters — the tool's history is interesting to nobody but its author, and the
design notes that *are* interesting are checked in under `lib/docs/`.

A credential scan of the full history found nothing: no keys, no tokens, no
`.p8`/`.p12`, gitleaks clean. The problem was never secrets; it was hostnames and
a name in 1,189 commit trailers.

`install/agent/` is excluded from the public repo. It is deployment runbooks
written for three specific machines, naming them throughout, and generalising it
is a separate piece of work with no upside for anyone else.

## Windows signing

Unsigned until the LLC exists, then
[Azure Trusted Signing](https://azure.microsoft.com/products/trusted-signing) at
roughly $10/month — an order of magnitude cheaper than OV or EV, and unlike OV it
inherits Microsoft's SmartScreen reputation immediately rather than accruing its
own. Until then the Windows installers show "Windows protected your PC".

macOS is signed and notarized; `lib/gui-macos/package.sh` does it, and the order
that matters is *staple the app before packaging it*, because a DMG built first
seals in an app with no ticket and Gatekeeper hides the mistake by checking Apple
online. Linux has nothing to sign — distribution signing is per-repository, and
direct `.deb`/`.AppImage` downloads need none.
