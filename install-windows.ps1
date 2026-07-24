[CmdletBinding()]
param(
    [string]$Repository = "madhavajay/alex",
    [string]$OutputPath = (Join-Path (Get-Location) "Alex-Setup.exe"),
    [switch]$NoLaunch
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Write-Step([string]$Message) {
    Write-Host "◆ $Message" -ForegroundColor Cyan
}

if (-not [Environment]::Is64BitOperatingSystem) {
    throw "Alex requires 64-bit Windows."
}

$machineArch = $env:PROCESSOR_ARCHITEW6432
if ([string]::IsNullOrWhiteSpace($machineArch)) {
    $machineArch = $env:PROCESSOR_ARCHITECTURE
}
switch ($machineArch) {
    "AMD64" { $assetArch = "x86_64" }
    "ARM64" { $assetArch = "arm64" }
    default { throw "Unsupported Windows architecture '$machineArch'. Alex supports x86-64 and ARM64." }
}

$headers = @{ "User-Agent" = "alex-windows-installer" }
Write-Step "finding the latest Alex release"
$release = Invoke-RestMethod -Headers $headers -Uri "https://api.github.com/repos/$Repository/releases/latest"
$version = ([string]$release.tag_name).Trim().TrimStart("v")
$assetName = "Alex-Setup-$version-windows-$assetArch.exe"
$checksumName = "$assetName.sha256"
$asset = $release.assets | Where-Object { $_.name -eq $assetName } | Select-Object -First 1
$checksumAsset = $release.assets | Where-Object { $_.name -eq $checksumName } | Select-Object -First 1

if ($null -eq $asset -or $null -eq $checksumAsset) {
    throw "Release $version does not include the expected $assetArch Windows installer and checksum."
}

$resolvedOutput = [IO.Path]::GetFullPath($OutputPath)
$outputDirectory = Split-Path -Parent $resolvedOutput
$temporary = Join-Path ([IO.Path]::GetTempPath()) ("alex-windows-installer-" + [Guid]::NewGuid())
$temporaryInstaller = Join-Path $temporary $assetName
$temporaryChecksum = Join-Path $temporary $checksumName

try {
    New-Item -ItemType Directory -Path $temporary, $outputDirectory -Force | Out-Null
    Write-Step "downloading Alex $version for Windows $assetArch"
    Invoke-WebRequest -Headers $headers -Uri $asset.browser_download_url -OutFile $temporaryInstaller
    Invoke-WebRequest -Headers $headers -Uri $checksumAsset.browser_download_url -OutFile $temporaryChecksum

    $expected = ((Get-Content -Raw $temporaryChecksum).Trim() -split '\s+')[0].ToLowerInvariant()
    $actual = (Get-FileHash -Algorithm SHA256 $temporaryInstaller).Hash.ToLowerInvariant()
    if ($expected -notmatch '^[a-f0-9]{64}$' -or $actual -ne $expected) {
        throw "SHA-256 verification failed for $assetName."
    }

    Copy-Item -LiteralPath $temporaryInstaller -Destination $resolvedOutput -Force
}
finally {
    if (Test-Path $temporary) {
        Remove-Item -LiteralPath $temporary -Recurse -Force
    }
}

Write-Host "Downloaded the verified installer to $resolvedOutput" -ForegroundColor Green
if (-not $NoLaunch) {
    Write-Step "opening the Alex installer"
    Start-Process -FilePath $resolvedOutput
}
