[CmdletBinding()]
param(
    [string]$Tier = "mock"
)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Continue"
$ProgressPreference = "SilentlyContinue"

$env:Path = (Join-Path $env:USERPROFILE ".cargo\bin") + ";C:\Program Files\LLVM\bin;C:\Program Files\Git\usr\bin;" + $env:Path
$env:CARGO_TARGET_DIR = "C:\crabbox\cargo-target\alex"

# The Microsoft Store python stub prints an install hint instead of running;
# detect real Python by asking for its version.
function Find-RealPython {
    $roots = @(Join-Path $env:LOCALAPPDATA "Programs\Python")
    foreach ($root in $roots) {
        if (-not (Test-Path $root)) { continue }
        $exe = Get-ChildItem $root -Directory |
            Sort-Object Name -Descending |
            ForEach-Object { Join-Path $_.FullName "python.exe" } |
            Where-Object { Test-Path $_ } |
            Select-Object -First 1
        if ($exe) { return $exe }
    }
    return $null
}

$pythonExe = Find-RealPython
if (-not $pythonExe) {
    Write-Host "==> installing Python 3.12 (winget, silent)"
    winget install --id Python.Python.3.12 --silent --accept-package-agreements --accept-source-agreements | Out-Null
    $pythonExe = Find-RealPython
}
if (-not $pythonExe) { throw "No real Python installation found after winget install." }

$pyDir = Split-Path $pythonExe
# test.sh calls python3; CPython installs ship python.exe only. The shim must
# outrank the WindowsApps Store stub on PATH.
if (-not (Test-Path (Join-Path $pyDir "python3.exe"))) {
    Copy-Item $pythonExe (Join-Path $pyDir "python3.exe")
}
$env:Path = "$pyDir;$(Join-Path $pyDir 'Scripts');" + $env:Path
& (Join-Path $pyDir "python3.exe") --version

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))
Set-Location $repoRoot

# cmd /c keeps bash's stderr chatter from becoming PowerShell errors.
cmd /c "bash ./test.sh $Tier 2>&1" | Select-Object -Last 30
