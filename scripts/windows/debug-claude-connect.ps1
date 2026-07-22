[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = "Continue"

$claudeDir = Join-Path $env:USERPROFILE ".claude"
Write-Host "=== alex-settings.json ==="
Get-Content -Raw (Join-Path $claudeDir "alex-settings.json")
Write-Host "=== claude on PATH ==="
$claude = Get-Command claude -ErrorAction SilentlyContinue
if ($claude) { Write-Host $claude.Source } else { Write-Host "not found" }
Write-Host "=== claude version ==="
if ($claude) { & $claude.Source --version 2>&1 | Select-Object -First 2 }
Write-Host "=== apiKeyHelper output test ==="
$settings = Get-Content -Raw (Join-Path $claudeDir "alex-settings.json") | ConvertFrom-Json
$helper = $settings.apiKeyHelper
Write-Host "helper command: $helper"
cmd /c "$helper" 2>&1 | Select-Object -First 3
Write-Host "=== gateway discovery env ==="
$settings.env | ConvertTo-Json
