[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$os = Get-CimInstance Win32_OperatingSystem
$cs = Get-CimInstance Win32_ComputerSystem

$link = Get-Command link.exe -ErrorAction SilentlyContinue
$rustc = Get-Command rustc -ErrorAction SilentlyContinue
$cargo = Get-Command cargo -ErrorAction SilentlyContinue
$rustup = Get-Command rustup -ErrorAction SilentlyContinue

$vswherePath = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
$hasVswhere = Test-Path $vswherePath

$vsInstall = $null
$vsArm64Tools = $false
if ($hasVswhere) {
    $vsInstall = & $vswherePath -latest -products * -property installationPath
    if ($vsInstall) {
        $arm64Dirs = Get-ChildItem -Path (Join-Path $vsInstall "VC\Tools\MSVC") -Directory -ErrorAction SilentlyContinue |
            ForEach-Object { Join-Path $_.FullName "bin\Hostarm64\arm64" } |
            Where-Object { Test-Path $_ }
        $vsArm64Tools = [bool]$arm64Dirs
    }
}

$rustupToolchains = $null
$rustHost = $null
if ($rustup) {
    $rustupToolchains = (& rustup show 2>&1) -join "`n"
}
if ($rustc) {
    $rustHost = ((& rustc -vV) | Select-String "^host:").ToString().Trim()
}

[pscustomobject]@{
    os_caption      = $os.Caption
    os_build        = $os.BuildNumber
    os_arch         = $os.OSArchitecture
    system_type     = $cs.SystemType
    processor_arch  = $env:PROCESSOR_ARCHITECTURE
    link_exe        = if ($link) { $link.Source } else { $null }
    rustc           = if ($rustc) { $rustc.Source } else { $null }
    cargo           = if ($cargo) { $cargo.Source } else { $null }
    rustup          = if ($rustup) { $rustup.Source } else { $null }
    rust_host       = $rustHost
    vswhere         = $hasVswhere
    vs_install      = $vsInstall
    vs_arm64_tools  = $vsArm64Tools
    free_disk_gb    = [math]::Round((Get-PSDrive C).Free / 1GB, 1)
} | ConvertTo-Json
