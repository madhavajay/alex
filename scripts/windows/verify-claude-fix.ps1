[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = "Continue"

$alex = Join-Path $env:LOCALAPPDATA "Alex\bin\alex.exe"

Write-Host "=== reconnect claude (picks provider-routable default) ==="
& $alex connect claude 2>&1 | Select-Object -First 12

Write-Host "=== selected model in alex-settings.json ==="
$settings = Get-Content -Raw (Join-Path $env:USERPROFILE ".claude\alex-settings.json") | ConvertFrom-Json
Write-Host "model: $($settings.model)"

Write-Host "=== non-interactive claude run ==="
$claude = (Get-Command claude -ErrorAction SilentlyContinue).Source
& $claude --settings (Join-Path $env:USERPROFILE ".claude\alex-settings.json") -p "Reply with the single word: ok" 2>&1 | Select-Object -First 10
