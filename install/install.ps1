<#
.SYNOPSIS
  Jotbay installer for Windows.

.DESCRIPTION
  Installs the Jotbay CLI and desktop app, schedules background sync, creates the
  launcher in the repository root, and puts a shortcut to the synced folder on the
  Desktop. Run from an ordinary PowerShell - no elevation needed, because
  everything lands under your own profile.

.PARAMETER Source
  Always build from source instead of downloading a published release.

.PARAMETER NoGui
  Install the CLI and scheduler only.
#>
[CmdletBinding()]
param([switch]$Source, [switch]$NoGui)

$ErrorActionPreference = 'Stop'

# Piped (`irm ... | iex`) there is no script file and no clone - only a
# published release can supply the binaries. From a clone, source builds are
# also possible.
$CloneDir = if ($PSScriptRoot) { Split-Path -Parent $PSScriptRoot } else { $null }

# Where releases live. Deliberately not derived from the clone's origin any
# more: since the split, a clone of this script sits next to somebody's *notes*,
# and a notes repository has no releases on it. Deriving it meant every install
# silently fell through to a source build, which needs a Rust toolchain - the
# one thing the release assets exist to avoid.
#
# A fork overrides it rather than editing this file. Matches JOTBAY_TOOL_REPO in
# lib/core/src/update.rs, which `jotbay upgrade` uses for the same reason.
$Repo = if ($env:JOTBAY_TOOL_REPO) { $env:JOTBAY_TOOL_REPO } else { 'nicglazkov/jotbay' }
$BinDir    = Join-Path $env:LOCALAPPDATA 'Programs\jotbay'

function Say  { param($m) Write-Host "==> $m" -ForegroundColor White }
function Info { param($m) Write-Host "    $m" }
function Warn { param($m) Write-Host "    warning: $m" -ForegroundColor Yellow }
function Die  { param($m) Write-Host "error: $m" -ForegroundColor Red; exit 1 }
function Have { param($c) [bool](Get-Command $c -ErrorAction SilentlyContinue) }

if ($CloneDir -and -not (Test-Path (Join-Path $CloneDir '.git'))) {
  Die "$CloneDir is not a git clone of Jotbay"
}
if ($Source -and -not $CloneDir) {
  Die "-Source needs a clone: git clone https://github.com/$Repo.git"
}

$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
if ($CloneDir) { Say "installing for windows/$arch from $CloneDir" }
else { Say "installing for windows/$arch from the published release" }
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null

# Everything the previous name left behind. The rename replaced the scheduled
# task but not the binaries, so inkway.exe stayed on PATH beside jotbay.exe -
# and running it would republish to refs/inkway-status/ and recreate the orphan
# ref the migration had just deleted. The directory has to come off PATH too,
# which is the half Unix does not have to deal with.
$LegacyBin = Join-Path $env:LOCALAPPDATA 'Programs\inkway'
if (Test-Path $LegacyBin) {
  Remove-Item -Recurse -Force $LegacyBin -ErrorAction SilentlyContinue
  Info "removed the superseded install at $LegacyBin"
}
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -and (($userPath -split ';') -contains $LegacyBin)) {
  $cleaned = ($userPath -split ';' | Where-Object { $_ -ne $LegacyBin -and $_ -ne '' }) -join ';'
  [Environment]::SetEnvironmentVariable('Path', $cleaned, 'User')
  Info 'removed the superseded install directory from PATH'
}

# --- binaries ---------------------------------------------------------------

function Install-FromRelease {
  $asset = "jotbay-windows-$arch.zip"
  if (-not (Have gh)) { return $false }
  gh auth status 2>&1 | Out-Null
  if ($LASTEXITCODE -ne 0) { return $false }

  $tmp = Join-Path $env:TEMP ("jotbay-" + [guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Force -Path $tmp | Out-Null

  Info "downloading $asset"
  gh release download --repo $Repo --pattern $asset --dir $tmp --clobber 2>&1 | Out-Null
  if ($LASTEXITCODE -ne 0) { Remove-Item -Recurse -Force $tmp; return $false }

  Expand-Archive -Path (Join-Path $tmp $asset) -DestinationPath $tmp -Force
  Copy-Item (Join-Path $tmp 'jotbay.exe') $BinDir -Force
  $gui = Join-Path $tmp 'jotbay-gui.exe'
  if ((-not $NoGui) -and (Test-Path $gui)) { Copy-Item $gui $BinDir -Force }

  Remove-Item -Recurse -Force $tmp
  return $true
}

function Install-FromSource {
  if (-not $CloneDir) {
    Die 'no release could be downloaded, and a source build needs a clone'
  }
  if (-not (Have cargo)) {
    Die 'cargo not found - install Rust from https://rustup.rs, or wait for a published release'
  }

  Info 'building the CLI (this takes a minute)'
  Push-Location (Join-Path $CloneDir 'lib')
  cargo build --release --quiet
  Pop-Location
  Copy-Item (Join-Path $CloneDir 'lib\target\release\jotbay.exe') $BinDir -Force

  if (-not $NoGui) {
    # The GUI crate's bundle config packages the CLI from src-tauri/staged,
    # and tauri-build validates that resources exist even for a plain
    # `cargo build` - so without this staging step (normally bundle.sh's job)
    # the GUI can never build from a fresh clone: the build script dies with
    # "resource path `staged\jotbay.exe` doesn't exist". Found during the
    # jotbay migration, masked before it by a stale staged/ dir left behind
    # by earlier bundle.sh runs.
    $staged = Join-Path $CloneDir 'lib\gui-tauri\src-tauri\staged'
    New-Item -ItemType Directory -Force -Path $staged | Out-Null
    Copy-Item (Join-Path $CloneDir 'lib\target\release\jotbay.exe') $staged -Force

    Info 'building the desktop app'
    Push-Location (Join-Path $CloneDir 'lib\gui-tauri\src-tauri')
    cargo build --release --quiet
    $ok = ($LASTEXITCODE -eq 0)
    Pop-Location
    if ($ok) {
      Copy-Item (Join-Path $CloneDir 'lib\gui-tauri\src-tauri\target\release\jotbay-gui.exe') $BinDir -Force
    } else {
      Warn 'skipping the GUI - Tauri needs the WebView2 runtime and MSVC build tools'
    }
  }
}

Say 'installing binaries'
if ($Source) { Install-FromSource }
elseif (Install-FromRelease) { Info 'installed from the latest release' }
else { Info 'no published release available, building from source'; Install-FromSource }

$JotbayExe = Join-Path $BinDir 'jotbay.exe'
if (-not (Test-Path $JotbayExe)) { Die 'installation produced no jotbay.exe' }
Info "jotbay -> $JotbayExe"

# --- git identity -----------------------------------------------------------
#
# Without user.name/user.email git cannot commit, and the failure is invisible
# until the first time the user actually changes a file: installing, scheduling
# and even a pull/push all succeed with no identity, so everything reports
# healthy right up to the moment their data starts mattering. `gh auth
# setup-git` configures credentials but NOT identity, so authenticating with gh
# is not enough. Found during a Linux deployment.

# The installer used to assume it was running inside the vault, which stopped
# being true the day the tool and the notes split. Ask instead: resolved from
# recorded settings (or the default location), from the profile directory so a
# clone this script happens to sit in is never mistaken for the notes.
Push-Location $env:USERPROFILE
$VaultData = (& $JotbayExe path 2>$null)
Pop-Location
$VaultDir = if ($LASTEXITCODE -eq 0 -and $VaultData) { Split-Path -Parent $VaultData } else { $null }
if ($VaultDir) { Info "notes found at $VaultDir" }

$IdentityOk = $true

function Initialize-GitIdentity {
  # No vault yet means no repository to configure; `jotbay init` runs its own
  # identity check when it creates one.
  if (-not $VaultDir) { return $true }
  $name  = (& git -C $VaultDir config --get user.name  2>$null)
  $email = (& git -C $VaultDir config --get user.email 2>$null)
  if ($name -and $email) { return $true }

  # gh already knows who you are, so borrow it rather than asking. The noreply
  # address is the one GitHub itself hands out, and keeps a private email private.
  if (Have gh) {
    gh auth status 2>&1 | Out-Null
    if ($LASTEXITCODE -eq 0) {
      $login = (& gh api user --jq '.login' 2>$null)
      $id    = (& gh api user --jq '.id'    2>$null)
      if ($login -and $id) {
        if (-not $name)  { $name  = (& gh api user --jq '.name // .login' 2>$null); if (-not $name) { $name = $login } }
        if (-not $email) { $email = "$id+$login@users.noreply.github.com" }
        # This clone only. An installer has no business rewriting the identity
        # every other repository on the machine commits under.
        & git -C $VaultDir config user.name  $name
        & git -C $VaultDir config user.email $email
        Info "commits from this machine will be authored as $name <$email>"
        return $true
      }
    }
  }
  return $false
}

Say 'checking git can commit'
$IdentityOk = Initialize-GitIdentity

# --- scheduled sync ---------------------------------------------------------

Say 'starting the background sync'

# Launched through `conhost --headless`: a console binary started directly by
# an interactive scheduled task flashes a console window at every run - here,
# every ten minutes, stealing focus each time. Headless conhost gives the
# process its console without ever creating a window, and unlike a hidden
# PowerShell wrapper nothing flashes first. S4U ("run whether user is logged
# on or not") would also be windowless but moves the sync into a session where
# the credential manager is not guaranteed to open.
$action = New-ScheduledTaskAction -Execute "$env:SystemRoot\System32\conhost.exe" `
  -Argument "--headless `"$JotbayExe`" watch" -WorkingDirectory $env:USERPROFILE
# A watcher that runs for the whole session, not a repeating alarm: it starts
# at logon and the scheduler restarts it if it dies. RestartCount/-Interval are
# the Windows equivalent of launchd's KeepAlive and systemd's Restart=always,
# which is why the watcher stays in the foreground on every platform.
#
# ExecutionTimeLimit must be zero. The default is three days, after which the
# scheduler kills the task - which for a repeating one-shot never mattered and
# for a long-lived watcher means sync silently stopping after 72 hours.
$trigger  = New-ScheduledTaskTrigger -AtLogOn
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable `
  -DontStopIfGoingOnBatteries -AllowStartIfOnBatteries -MultipleInstances IgnoreNew `
  -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
  -ExecutionTimeLimit ([TimeSpan]::Zero)

# Historical task name; a leftover one keeps invoking a binary that is gone.
Unregister-ScheduledTask -TaskName 'inkway-sync' -Confirm:$false -ErrorAction SilentlyContinue

Register-ScheduledTask -TaskName 'jotbay-sync' -Action $action -Trigger $trigger `
  -Settings $settings -Description 'Keep markdown notes in sync' -Force | Out-Null
Info 'watching for changes - scheduled task "jotbay-sync" runs at logon'

# Registering does not start it; without this the watcher would not run until
# the next logon, and nothing would sync in the meantime.
Start-ScheduledTask -TaskName 'jotbay-sync' -ErrorAction SilentlyContinue

# --- launcher and shortcuts -------------------------------------------------

$shell = New-Object -ComObject WScript.Shell

if (-not $NoGui) {
  $guiExe = Join-Path $BinDir 'jotbay-gui.exe'
  if (Test-Path $guiExe) {
    # Start Menu, not "the repository root": since the split there may be no
    # clone at all, and a launcher inside a throwaway clone dies with it. The
    # exe carries its own icon, so nothing here points into a directory that
    # can disappear.
    Say 'creating the launcher'
    $programs = [Environment]::GetFolderPath('Programs')
    $lnk = $shell.CreateShortcut((Join-Path $programs 'Jotbay.lnk'))
    $lnk.TargetPath       = $guiExe
    $lnk.WorkingDirectory = $env:USERPROFILE
    $lnk.IconLocation     = "$guiExe,0"
    $lnk.Description      = 'Keep your markdown notes in sync'
    $lnk.Save()
    Info 'Jotbay is in the Start Menu'
  }
}

$desktop = [Environment]::GetFolderPath('Desktop')
if ($VaultData -and $desktop) {
  Say 'putting a shortcut to the synced folder on your Desktop'
  $s = $shell.CreateShortcut((Join-Path $desktop 'Jotbay.lnk'))
  $s.TargetPath = $VaultData
  $s.Save()
  Info "Desktop\Jotbay -> $VaultData"
}

# --- PATH -------------------------------------------------------------------

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -notlike "*$BinDir*") {
  [Environment]::SetEnvironmentVariable('Path', "$userPath;$BinDir", 'User')
  Info "added $BinDir to your PATH (restart your terminal to pick it up)"
}

$syncOut = @()
$syncOk = $true
if ($VaultDir) {
  Say 'running the first sync'

  # Echo the sync live but keep a copy, so the specific failures worth
  # explaining can be recognised instead of leaving the user with raw git
  # stderr. EAP is relaxed for the call because native stderr merged with 2>&1
  # under 'Stop' terminates the script.
  $prevEap = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  Push-Location $env:USERPROFILE
  $syncOut = & $JotbayExe sync 2>&1 |
    ForEach-Object { $line = "$_"; Write-Host $line; $line }
  $syncOk = ($LASTEXITCODE -eq 0)
  Pop-Location
  $ErrorActionPreference = $prevEap
}

# GitHub refuses a push carrying any commit whose author email is private, when
# "Block command line pushes that expose my email" is on - error GH007. The
# identity check above only proves user.name/user.email are *set*, so an
# ordinary global identity sails through it and then cannot push: the same
# silent wall that check exists to prevent, moved one step later. The address
# gh hands out is public by construction, which is why it is the one to use.
$EmailBlocked = (-not $syncOk) -and (($syncOut -join "`n") -match 'GH007|publish a private email')

$NoReply = $null
if ($EmailBlocked -and (Have gh)) {
  $login = (& gh api user --jq '.login' 2>$null)
  $id    = (& gh api user --jq '.id'    2>$null)
  if ($login -and $id) {
    # This clone only, as above.
    $NoReply = "$id+$login@users.noreply.github.com"
    & git -C $VaultDir config user.email $NoReply
  }
}

# On a reinstall the scheduled task is already live, so its ten-minute run can
# land on top of this one. The lock then does exactly its job and this sync
# exits having done nothing - correct, but "another sync is already running" is
# the last thing the installer says, which reads like a failure.
$LockHit = ($syncOut -join "`n") -match 'another sync is already running'

if ($LockHit) {
  Info 'that was the scheduled sync already in flight; it finishes on its own'
} elseif ((-not $syncOk) -and (-not $EmailBlocked)) {
  Warn "first sync did not complete - run 'jotbay status' to see why"
}

Write-Host ''
Say 'done'
if (-not $VaultDir) {
  Info 'no notes on this machine yet - one more step:'
  Info '  jotbay init          create, clone, or adopt your notes repository'
  if (-not $NoGui) { Info '  ...or open Jotbay from the Start Menu - the first screen asks the same question' }
} else {
  Info 'jotbay status   - see every machine'
  Info 'jotbay dash     - live dashboard'
  Info 'jotbay sync     - sync right now'
}

if (-not $IdentityOk) {
  Write-Host ''
  Warn 'git has no user.name or user.email on this machine, so Jotbay cannot commit.'
  Warn 'Sync will look healthy until the first time you change a file, and then fail.'
  Write-Host ''
  Write-Host '        git config --global user.name  "Your Name"'
  Write-Host '        git config --global user.email "you@example.com"'
  Write-Host ''
}

if ($EmailBlocked) {
  Write-Host ''
  Warn 'GitHub refused the push: your commits carry a private email address.'
  Warn 'Until that is fixed, nothing you write will ever leave this machine.'
  Write-Host ''
  if ($NoReply) {
    Info "this clone will now commit as $NoReply"
    Write-Host ''
    Write-Host '        The commit already made still carries the old address.'
    Write-Host '        Re-author it and sync again:'
    Write-Host ''
    Write-Host "        git -C `"$VaultDir`" commit --amend --reset-author --no-edit"
    Write-Host '        jotbay sync'
  } else {
    Write-Host '        Find your noreply address at https://github.com/settings/emails, then:'
    Write-Host ''
    Write-Host "        git -C `"$VaultDir`" config user.email `"ID+USER@users.noreply.github.com`""
    Write-Host "        git -C `"$VaultDir`" commit --amend --reset-author --no-edit"
    Write-Host '        jotbay sync'
  }
  Write-Host ''
}
