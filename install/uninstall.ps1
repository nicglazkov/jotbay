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
Remove-Item -Force (Join-Path $JotbayDir 'Jotbay.lnk') -ErrorAction SilentlyContinue

Say 'removing shortcuts'
$desktop = [Environment]::GetFolderPath('Desktop')
Remove-Item -Force (Join-Path $desktop 'Jotbay.lnk') -ErrorAction SilentlyContinue

Say 'cleaning PATH'
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -like "*$BinDir*") {
  $cleaned = ($userPath -split ';' | Where-Object { $_ -and $_ -ne $BinDir }) -join ';'
  [Environment]::SetEnvironmentVariable('Path', $cleaned, 'User')
  Info 'PATH entry removed'
}

Write-Host ''
Say "done - your notes in $JotbayDir\data were not touched"
