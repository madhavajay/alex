[CmdletBinding()]
param(
    [string]$OutputDirectory = "C:\crabbox\artifacts\alex-windows-arm64",
    [string]$CargoTargetDirectory = "C:\crabbox\cargo-target\alex",
    [string]$StampVersion = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Write-Step([string]$Message) {
    Write-Host "==> $Message"
}

if ($env:PROCESSOR_ARCHITECTURE -ne "ARM64") {
    throw "This build script targets Windows on ARM64; PROCESSOR_ARCHITECTURE is '$env:PROCESSOR_ARCHITECTURE'."
}

# Build state lives outside the synced repo directory so repository syncs
# (which mirror-delete unknown paths) cannot wipe the cargo cache or built
# assets between runs.
New-Item -ItemType Directory -Force -Path $CargoTargetDirectory | Out-Null
$env:CARGO_TARGET_DIR = $CargoTargetDirectory

$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (($env:Path -split ";") -notcontains $cargoBin) {
    $env:Path = "$cargoBin;$env:Path"
}

$llvmBin = "C:\Program Files\LLVM\bin"
if (-not (Test-Path (Join-Path $llvmBin "clang.exe"))) {
    throw "LLVM clang not found at $llvmBin; run scripts/windows/bootstrap-arm64-toolchain.ps1 first."
}
if (($env:Path -split ";") -notcontains $llvmBin) {
    $env:Path = "$llvmBin;$env:Path"
}

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))
Set-Location $repoRoot

$version = (Select-String -Path "Cargo.toml" -Pattern '^version = "([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
if ([string]::IsNullOrWhiteSpace($version)) {
    throw "Could not read workspace version from Cargo.toml."
}

$locked = "--locked"
if (-not [string]::IsNullOrWhiteSpace($StampVersion)) {
    if ($StampVersion -notmatch '^\d+\.\d+\.\d+(-beta\.\d+)?$') {
        throw "Invalid stamp version '$StampVersion' (expected X.Y.Z or X.Y.Z-beta.N)."
    }
    if ($StampVersion -ne $version) {
        Write-Step "stamping workspace version $version -> $StampVersion (VM copy only)"
        $toml = Get-Content -Raw "Cargo.toml"
        $toml = $toml -replace [regex]::Escape("version = `"$version`""), "version = `"$StampVersion`""
        Set-Content -NoNewline -Path "Cargo.toml" -Value $toml
        # cargo writes progress to stderr; under EAP=Stop that becomes a
        # terminating NativeCommandError unless stderr is merged via cmd.
        & cmd /c "cargo update --workspace 2>&1"
        if ($LASTEXITCODE -ne 0) { throw "cargo update after stamping failed with exit code $LASTEXITCODE" }
    }
    $version = $StampVersion
    $locked = $null
}

Write-Step "building alex $version for aarch64-pc-windows-msvc (release)"
if ($locked) {
    & cargo build --release $locked -p alex --bins
} else {
    & cargo build --release -p alex --bins
}
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

$exe = Join-Path $CargoTargetDirectory "release\alex.exe"
if (-not (Test-Path $exe)) { throw "Expected binary not found at $exe" }

$reported = (& $exe --version | Out-String).Trim()
Write-Step "built binary reports: $reported"
if ($reported -ne "alex $version") {
    throw "Binary version '$reported' does not match workspace version '$version'."
}

$outDir = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$asset = "alex-cli-$version-windows-arm64.zip"
$assetPath = Join-Path $outDir $asset

if (Test-Path $assetPath) { Remove-Item -LiteralPath $assetPath -Force }
Compress-Archive -Path $exe -DestinationPath $assetPath

$hash = (Get-FileHash -Algorithm SHA256 $assetPath).Hash.ToLowerInvariant()
"$hash  $asset" | Set-Content -NoNewline -Path "$assetPath.sha256"

Write-Step "packaged $assetPath"
Write-Host "version=$version"
Write-Host "asset=$assetPath"
Write-Host "sha256=$hash"

# Put the archive, checksum, and installer on the desktop so the release can
# be installed by hand exactly the way an end user would.
$desktop = [Environment]::GetFolderPath("Desktop")
if (-not [string]::IsNullOrWhiteSpace($desktop) -and (Test-Path $desktop)) {
    Copy-Item -LiteralPath $assetPath -Destination (Join-Path $desktop $asset) -Force
    Copy-Item -LiteralPath "$assetPath.sha256" -Destination (Join-Path $desktop "$asset.sha256") -Force
    Copy-Item -LiteralPath (Join-Path $repoRoot "install-release.ps1") -Destination (Join-Path $desktop "install-release.ps1") -Force
    $readme = @"
Alex $version (Windows ARM64) — manual install

Open PowerShell in this folder and run:

  powershell -NoProfile -ExecutionPolicy Bypass -File .\install-release.ps1 ``
    -Version $version -LocalArchive .\$asset

Then: alex web
Uninstall: alex service uninstall
"@
    Set-Content -Path (Join-Path $desktop "ALEX-INSTALL-README.txt") -Value $readme
    Write-Step "copied installer bundle to $desktop"
}
