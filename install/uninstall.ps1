<#
.SYNOPSIS
  Remove Jotbay from this machine, however it was installed.

.DESCRIPTION
  Your notes are never touched. This removes the program, not the data - the
  repository stays where it is, and a later install can adopt it.

  This used to undo only what install.ps1 had done, so it could not clean a
  machine installed from the -setup.exe, which is the route most people take.
  The fresh-install run had to finish the job by hand, and everything it found
  left behind is handled here: a scheduled task pointing at a deleted binary, a
  desktop shortcut for a program that was gone, and settings that made the next
  install silently not-fresh.

.PARAMETER All
  Remove preferences too, for a genuinely fresh install.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File install\uninstall.ps1 -All
#>
[CmdletBinding()]
param([switch]$All)

$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'

function Say  { param($m) Write-Host "==> $m" -ForegroundColor White }
function Info { param($m) Write-Host "    $m" }
function Warn { param($m) Write-Host "    $m" -ForegroundColor Yellow }

# Both install layouts. install.ps1 uses Programs\jotbay; the NSIS installer
# uses Jotbay, and for a long time nothing here knew about the second one.
$ScriptBin    = Join-Path $env:LOCALAPPDATA 'Programs\jotbay'
$InstallerBin = Join-Path $env:LOCALAPPDATA 'Jotbay'
$LegacyBin    = Join-Path $env:LOCALAPPDATA 'Programs\inkway'

# Asked before anything is removed, because afterwards there is no jotbay left
# to ask. Used only so the closing line can name the folder it did not touch.
$vault = ''
if (Get-Command jotbay -ErrorAction SilentlyContinue) {
  $data = (& jotbay path 2>$null)
  if ($data) { $vault = Split-Path -Parent $data }
}

# --- 1. stop the background sync -------------------------------------------
#
# First, before anything is deleted. A watcher left running holds the binaries
# about to be removed, and a task left registered fires at every logon against
# a file that is no longer there - silently, forever.
Say 'stopping background sync'
foreach ($t in @('jotbay-sync','inkway-sync')) {
  Stop-ScheduledTask -TaskName $t -ErrorAction SilentlyContinue
  Unregister-ScheduledTask -TaskName $t -Confirm:$false -ErrorAction SilentlyContinue
}
Get-Process jotbay,jotbay-gui -ErrorAction SilentlyContinue |
  Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
Info 'stopped'

# --- 2. the installed application ------------------------------------------
Say 'removing the program'
$uninstaller = Join-Path $InstallerBin 'uninstall.exe'
if (Test-Path $uninstaller) {
  # Let NSIS remove its own install, so its registry entry goes with it and
  # Windows stops listing Jotbay under Installed apps.
  Info 'running the installer''s own uninstaller'
  Start-Process -FilePath $uninstaller -ArgumentList '/S' -Wait -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 3
}
foreach ($d in @($ScriptBin, $InstallerBin, $LegacyBin)) {
  if (Test-Path $d) {
    Remove-Item -Recurse -Force $d -ErrorAction SilentlyContinue
    Info "removed $d"
  }
}

# --- 3. shortcuts we created -----------------------------------------------
Say 'removing shortcuts'
$desktop = [Environment]::GetFolderPath('Desktop')
$start   = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
foreach ($lnk in @(
    (Join-Path $desktop 'Jotbay.lnk'),
    (Join-Path $desktop 'Jotbay Notes.lnk'),
    (Join-Path $desktop 'Inkway.lnk'),
    (Join-Path $start   'Jotbay.lnk'))) {
  if (Test-Path $lnk) { Remove-Item -Force $lnk -EA SilentlyContinue; Info (Split-Path -Leaf $lnk) }
}

# --- 4. PATH ---------------------------------------------------------------
#
# Edited through .NET rather than the registry: a PATH longer than the NSIS
# 1024-character cap reads back truncated, and writing that value would delete
# the tail of it.
Say 'cleaning PATH'
$userPath = [Environment]::GetEnvironmentVariable('Path','User')
if ($userPath) {
  $keep = $userPath -split ';' | Where-Object {
    $_ -and $_ -ne $ScriptBin -and $_ -ne $InstallerBin -and $_ -ne $LegacyBin
  }
  $cleaned = $keep -join ';'
  if ($cleaned -ne $userPath) {
    [Environment]::SetEnvironmentVariable('Path', $cleaned, 'User')
    Info 'PATH entry removed'
  }
}

# --- 5. preferences --------------------------------------------------------
$config = Join-Path $env:APPDATA 'Jotbay'
$legacy = Join-Path $env:APPDATA 'Inkway'
if ($All) {
  Say 'removing preferences'
  Remove-Item -Recurse -Force $config,$legacy -ErrorAction SilentlyContinue
  Info 'removed - the next install starts from the first-run screen'
} elseif (Test-Path $config) {
  Say 'keeping your preferences'
  Info $config
  Info 'these record where your notes live, so a reinstall finds them again -'
  Info 'which also means a reinstall is NOT a fresh one. Use -All for that.'
}

# --- 6. prove it -----------------------------------------------------------
#
# An uninstaller that cannot say what it left behind is one nobody can trust to
# have finished. Every check here corresponds to something found by hand on a
# real machine.
Write-Host ''
Say 'checking'
$left = $false
foreach ($p in @($ScriptBin, $InstallerBin, (Join-Path $desktop 'Jotbay.lnk'))) {
  if (Test-Path $p) { Warn "still present: $p"; $left = $true }
}
if (Get-ScheduledTask -TaskName 'jotbay-sync' -ErrorAction SilentlyContinue) {
  Warn 'still present: the jotbay-sync scheduled task'; $left = $true
}
$stillOnPath = (Get-Command jotbay -ErrorAction SilentlyContinue)
if ($stillOnPath) {
  Warn "still on PATH: $($stillOnPath.Source)"
  Warn '(a terminal opened before this ran keeps the old PATH - check a new one)'
  $left = $true
}
if (-not $left) { Info 'nothing left behind' }

Write-Host ''
if ($vault -and (Test-Path $vault)) {
  Say "done - your notes in $vault are untouched"
} else {
  Say 'done - your notes were not touched'
}
