[CmdletBinding()]
param(
    [string]$Version = "0.1.29",
    [string]$Archive = "C:\crabbox\artifacts\alex-windows-arm64\alex-cli-0.1.29-windows-arm64.zip"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))

# Remove any prior daemon/task regardless of state. cmd /c keeps native
# stderr away from PowerShell's terminating-error handling.
cmd /c "schtasks /end /tn AlexDaemon >nul 2>&1"
cmd /c "schtasks /delete /tn AlexDaemon /f >nul 2>&1"
Get-Process alex -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

& (Join-Path $repoRoot "install-release.ps1") -Version $Version -LocalArchive $Archive

Start-Sleep -Seconds 3
$health = (Invoke-WebRequest -UseBasicParsing -TimeoutSec 5 -Uri "http://127.0.0.1:4100/health").StatusCode
Write-Host "health: $health"

$desktop = [Environment]::GetFolderPath("Desktop")
Copy-Item (Join-Path $repoRoot "install-release.ps1") (Join-Path $desktop "install-release.ps1") -Force
Write-Host "desktop installer refreshed"
