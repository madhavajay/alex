[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = "Continue"

$env:Path = (Join-Path $env:USERPROFILE ".cargo\bin") + ";C:\Program Files\LLVM\bin;" + $env:Path
$env:CARGO_TARGET_DIR = "C:\crabbox\cargo-target\alex"

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))
Set-Location $repoRoot

Write-Host "=== cargo test -p alex-core ==="
cmd /c "cargo test -p alex-core 2>&1" | Select-String -Pattern "test result|error\[|FAILED" | ForEach-Object { $_.Line }
Write-Host "=== cargo test -p alex-auth ==="
cmd /c "cargo test -p alex-auth 2>&1" | Select-String -Pattern "test result|error\[|FAILED" | ForEach-Object { $_.Line }
Write-Host "=== cargo test -p alex ==="
cmd /c "cargo test -p alex 2>&1" | Select-String -Pattern "test result|error\[|FAILED|failures:" | ForEach-Object { $_.Line }
Write-Host "=== done ==="
