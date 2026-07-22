[CmdletBinding()]
param(
    [string]$AlexExe = "C:\crabbox\cargo-target\alex\release\alex.exe",
    [string]$OutputDirectory = "C:\crabbox\artifacts\alex-windows-arm64"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Write-Step([string]$Message) {
    Write-Host "==> $Message"
}

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))

if (-not (Test-Path $AlexExe)) {
    throw "alex.exe not found at $AlexExe; run scripts\windows\build-arm64.ps1 first."
}

$version = ((& $AlexExe --version | Out-String).Trim() -split '\s+')[-1]
if ($version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') {
    throw "Could not parse version from alex.exe --version output."
}

$iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
if (-not (Test-Path $iscc)) {
    Write-Step "installing Inno Setup 6 (silent)"
    $temporary = Join-Path ([IO.Path]::GetTempPath()) "alex-innosetup"
    New-Item -ItemType Directory -Force -Path $temporary | Out-Null
    $installer = Join-Path $temporary "innosetup-installer.exe"
    Invoke-WebRequest -Uri "https://github.com/jrsoftware/issrc/releases/download/is-6_7_3/innosetup-6.7.3.exe" -OutFile $installer
    $process = Start-Process -FilePath $installer `
        -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        throw "Inno Setup installer failed with exit code $($process.ExitCode)"
    }
    if (-not (Test-Path $iscc)) {
        throw "Inno Setup installed but ISCC.exe was not found."
    }
}

New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null

Write-Step "compiling Alex-Setup-$version-windows-arm64.exe"
& $iscc `
    "/DAppVersion=$version" `
    "/DAlexExe=$AlexExe" `
    "/DOutputDir=$OutputDirectory" `
    "/DArch=arm64" `
    (Join-Path $repoRoot "packaging\windows-installer\alex.iss")
if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }

$setup = Join-Path $OutputDirectory "Alex-Setup-$version-windows-arm64.exe"
if (-not (Test-Path $setup)) { throw "Expected installer not found at $setup" }
$hash = (Get-FileHash -Algorithm SHA256 $setup).Hash.ToLowerInvariant()
"$hash  $(Split-Path -Leaf $setup)" | Set-Content -NoNewline -Path "$setup.sha256"

$desktop = [Environment]::GetFolderPath("Desktop")
Copy-Item -LiteralPath $setup -Destination (Join-Path $desktop (Split-Path -Leaf $setup)) -Force
Copy-Item -LiteralPath "$setup.sha256" -Destination (Join-Path $desktop "$(Split-Path -Leaf $setup).sha256") -Force

Write-Step "installer ready"
Write-Host "setup=$setup"
Write-Host "sha256=$hash"
Write-Host "desktop copy refreshed"
