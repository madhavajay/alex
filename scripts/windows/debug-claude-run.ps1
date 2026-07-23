[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = "Continue"

$claudeDir = Join-Path $env:USERPROFILE ".claude"
$settings = Join-Path $claudeDir "alex-settings.json"

Write-Host "=== non-interactive claude run through Alex ==="
$claude = (Get-Command claude -ErrorAction SilentlyContinue).Source
if (-not $claude) { throw "claude not on PATH" }
& $claude --settings $settings -p "Reply with the single word: ok" --model "claude-alex/claude-sonnet-5" 2>&1 | Select-Object -First 25

Write-Host "=== recent daemon requests to /v1/models (if logged) ==="
$log = Join-Path $env:USERPROFILE ".alex\logs"
if (Test-Path $log) {
    Get-ChildItem $log -Filter "*.log" | Sort-Object LastWriteTime -Descending |
        Select-Object -First 1 | ForEach-Object {
            Get-Content $_.FullName -Tail 40 | Select-String -Pattern "models|401|403|error" | Select-Object -First 10
        }
}
