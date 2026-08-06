# Security

## The trust model, stated plainly

Jotbay has no server. It never sends your notes anywhere except the git remote
you chose, and it does not phone home. There is no telemetry, no analytics, and
no account with the project.

The one outbound request the tool makes on its own is to
`api.github.com/repos/<tool repo>/releases/latest`, to notice when a newer
version exists. It sends nothing but the request; the answer is cached for six
hours. Set `JOTBAY_TOOL_REPO` to point it elsewhere, or block it, everything
except the update notice keeps working.

Credentials are never read, stored or transmitted by Jotbay. Pushing and pulling
go through your own `git` and your own credential helper, exactly as they would
if you ran the commands yourself. Setup uses `gh` only if you choose the
"create one for me" route, and only to create a repository under the account
`gh` is already signed in as.

## What Jotbay can do to your files

- It commits and pushes everything under `data/` in the vault it was pointed at.
- It never deletes a version. On a conflict both sides are kept, one under a
  suffixed name.
- Files at or over 100 MB are detected before staging and deliberately left
  uncommitted, because GitHub would reject the push.
- It does not touch anything outside the vault, except its own preferences file
  and, if you ask for one, a desktop shortcut.

## Reporting a vulnerability

Open a [security advisory](../../security/advisories/new) on this repository, or
email the address on the maintainer's GitHub profile. Please do not open a
public issue for anything exploitable.

Expect an acknowledgement within a week. This is a personal project maintained
in spare time; there is no paid security team and no bounty.

## Supported versions

The most recent release only. `jotbay upgrade` moves a machine to it in one
step.
