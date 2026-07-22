[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Continue"

$env:Path = (Join-Path $env:USERPROFILE ".cargo\bin") + ";C:\Program Files\LLVM\bin;" + $env:Path
$env:CARGO_TARGET_DIR = "C:\crabbox\cargo-target\alex"

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))
Set-Location $repoRoot

cmd /c "cargo check -p alex --bins 2>&1" | Select-String -Pattern "error\[", "^error", "-->" -Context 0, 8 | ForEach-Object { $_.Line; $_.Context.PostContext }
