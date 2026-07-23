[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = "Continue"

Write-Host "=== winget present? ==="
$winget = Get-Command winget -ErrorAction SilentlyContinue
Write-Host "winget: $($winget.Source)"

Write-Host "=== winget list python ==="
if ($winget) { winget list --id Python.Python.3.12 2>&1 | Select-Object -Last 5 }

Write-Host "=== search filesystem for real python.exe ==="
foreach ($dir in @(
    (Join-Path $env:LOCALAPPDATA "Programs\Python"),
    "C:\Program Files\Python312",
    "C:\Program Files\Python313",
    "C:\Python312"
)) {
    if (Test-Path $dir) { Write-Host "exists: $dir"; Get-ChildItem $dir -Filter "python*.exe" -Recurse -ErrorAction SilentlyContinue | Select-Object -First 3 -ExpandProperty FullName }
}

Write-Host "=== git-bash python? ==="
& "C:\Program Files\Git\usr\bin\bash.exe" -c "command -v python3 python 2>/dev/null; python3 --version 2>&1 | head -1"
