[CmdletBinding()]
param(
    [switch]$SkipVsBuildTools
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Write-Step([string]$Message) {
    Write-Host "==> $Message"
}

if ($env:PROCESSOR_ARCHITECTURE -ne "ARM64") {
    throw "This bootstrap targets Windows on ARM64; PROCESSOR_ARCHITECTURE is '$env:PROCESSOR_ARCHITECTURE'."
}

$temporary = Join-Path ([IO.Path]::GetTempPath()) "alex-arm64-bootstrap"
New-Item -ItemType Directory -Force -Path $temporary | Out-Null

$vswherePath = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"

function Get-MsvcArm64ToolsPresent {
    if (-not (Test-Path $vswherePath)) { return $false }
    $installPath = & $vswherePath -latest -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.ARM64 `
        -property installationPath
    if (-not $installPath) { return $false }
    $candidates = Get-ChildItem -Path (Join-Path $installPath "VC\Tools\MSVC") -Directory -ErrorAction SilentlyContinue |
        ForEach-Object { Join-Path $_.FullName "bin\Hostarm64\arm64\link.exe" } |
        Where-Object { Test-Path $_ }
    return [bool]$candidates
}

if (-not $SkipVsBuildTools) {
    if (Get-MsvcArm64ToolsPresent) {
        Write-Step "MSVC ARM64 build tools already present; skipping VS Build Tools install"
    } else {
        Write-Step "downloading Visual Studio 2022 Build Tools bootstrapper"
        $vsBootstrapper = Join-Path $temporary "vs_buildtools.exe"
        Invoke-WebRequest -Uri "https://aka.ms/vs/17/release/vs_buildtools.exe" -OutFile $vsBootstrapper

        Write-Step "installing MSVC v143 ARM64 build tools and Windows 11 SDK (quiet, no restart)"
        $vsArgs = @(
            "--quiet", "--wait", "--norestart", "--nocache",
            "--add", "Microsoft.VisualStudio.Component.VC.Tools.ARM64",
            "--add", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "--add", "Microsoft.VisualStudio.Component.Windows11SDK.22621"
        )
        $process = Start-Process -FilePath $vsBootstrapper -ArgumentList $vsArgs -Wait -PassThru
        if ($process.ExitCode -notin @(0, 3010)) {
            throw "VS Build Tools installer failed with exit code $($process.ExitCode)"
        }
        if ($process.ExitCode -eq 3010) {
            Write-Warning "VS Build Tools requested a reboot (3010); continuing without one."
        }
        if (-not (Get-MsvcArm64ToolsPresent)) {
            throw "VS Build Tools installer completed but MSVC ARM64 tools were not found."
        }
        Write-Step "MSVC ARM64 build tools installed"
    }
}

$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
$rustupExe = Join-Path $cargoBin "rustup.exe"

$llvmBin = "C:\Program Files\LLVM\bin"
$clangExe = Join-Path $llvmBin "clang.exe"
if (Test-Path $clangExe) {
    Write-Step "LLVM clang already present; skipping LLVM install"
} else {
    Write-Step "downloading LLVM 20.1.8 Windows-on-ARM installer"
    $llvmInstaller = Join-Path $temporary "llvm-woa64.exe"
    Invoke-WebRequest -Uri "https://github.com/llvm/llvm-project/releases/download/llvmorg-20.1.8/LLVM-20.1.8-woa64.exe" -OutFile $llvmInstaller

    Write-Step "installing LLVM silently (NSIS /S)"
    $process = Start-Process -FilePath $llvmInstaller -ArgumentList "/S" -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        throw "LLVM installer failed with exit code $($process.ExitCode)"
    }
    if (-not (Test-Path $clangExe)) {
        throw "LLVM installer completed but clang.exe was not found at $clangExe."
    }
    Write-Step "LLVM clang installed"
}

if (Test-Path $rustupExe) {
    Write-Step "rustup already installed; ensuring stable toolchain"
    & $rustupExe update stable
    if ($LASTEXITCODE -ne 0) { throw "rustup update failed with exit code $LASTEXITCODE" }
} else {
    Write-Step "downloading rustup-init for aarch64-pc-windows-msvc"
    $rustupInit = Join-Path $temporary "rustup-init.exe"
    Invoke-WebRequest -Uri "https://static.rust-lang.org/rustup/dist/aarch64-pc-windows-msvc/rustup-init.exe" -OutFile $rustupInit

    Write-Step "installing stable Rust toolchain (default host aarch64-pc-windows-msvc)"
    & $rustupInit -y --default-toolchain stable --profile minimal
    if ($LASTEXITCODE -ne 0) { throw "rustup-init failed with exit code $LASTEXITCODE" }
}

if (($env:Path -split ";") -notcontains $cargoBin) {
    $env:Path = "$cargoBin;$env:Path"
}

Write-Step "toolchain summary"
& (Join-Path $cargoBin "rustc.exe") -vV
if ($LASTEXITCODE -ne 0) { throw "rustc verification failed with exit code $LASTEXITCODE" }
& (Join-Path $cargoBin "cargo.exe") --version
if ($LASTEXITCODE -ne 0) { throw "cargo verification failed with exit code $LASTEXITCODE" }

$hostLine = (& (Join-Path $cargoBin "rustc.exe") -vV | Select-String "^host:").ToString().Trim()
if ($hostLine -ne "host: aarch64-pc-windows-msvc") {
    throw "Unexpected rust host '$hostLine'; expected aarch64-pc-windows-msvc."
}

Write-Step "bootstrap complete"
