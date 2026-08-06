<#
.SYNOPSIS
  Remove everything install.ps1 created.

.DESCRIPTION
  Leaves the Jotbay repository and your notes untouched - this uninstalls the
  tooling, not the data.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Continue'

$JotbayDir = Split-Path -Parent $PSScriptRoot
$BinDir   = Join-Path $env:LOCALAPPDATA 'Programs\jotbay'

function Say  { param($m) Write-Host "==> $m" -ForegroundColor White }
function Info { param($m) Write-Host "    $m" }

Say 'stopping the scheduled sync'
foreach ($t in @('jotbay-sync','inkway-sync')) {
  Unregister-ScheduledTask -TaskName $t -Confirm:$false -ErrorAction SilentlyContinue
}
Info 'scheduled task removed'

Say 'removing binaries and launchers'
Remove-Item -Recurse -Force $BinDir -ErrorAction SilentlyContinue
# The directory the tool used before it was renamed, if a machine still has it.
Remove-Item -Recurse -Force (Join-Path $env:LOCALAPPDATA 'Programs\inkway') `
  -ErrorAction SilentlyContinue
Remove-Item -Force (Join-Path $JotbayDir 'Jotbay.lnk') -ErrorAction SilentlyContinue

Say 'removing shortcuts'
$desktop = [Environment]::GetFolderPath('Desktop')
Remove-Item -Force (Join-Path $desktop 'Jotbay.lnk') -ErrorAction SilentlyContinue

Say 'cleaning PATH'
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$LegacyBin = Join-Path $env:LOCALAPPDATA 'Programs\inkway'
if (($userPath -like "*$BinDir*") -or ($userPath -like "*$LegacyBin*")) {
  $cleaned = ($userPath -split ';' |
    Where-Object { $_ -and $_ -ne $BinDir -and $_ -ne $LegacyBin }) -join ';'
  [Environment]::SetEnvironmentVariable('Path', $cleaned, 'User')
  Info 'PATH entry removed'
}

Write-Host ''
Say "done - your notes in $JotbayDir\data were not touched"

# Preferences are kept. The NSIS uninstaller offers a "Delete the application
# data" checkbox for the same directory, unchecked by default and with nothing
# explaining what it covers; this script says it out loud instead. Keeping it
# means a reinstall remembers where the notes live, and also means a reinstall
# is not a clean one - setup finds a vault already configured and never shows
# the first-run screen.
$config = Join-Path $env:APPDATA 'Jotbay'
if (Test-Path $config) {
  Write-Host "    kept your preferences in $config"
  Write-Host "    remove that folder as well before testing a fresh install"
}
