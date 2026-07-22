[CmdletBinding()]
param(
    [string]$Version,
    [string]$Repository = "madhavajay/alex",
    [string]$InstallDirectory = "$env:LOCALAPPDATA\Alex\bin",
    [switch]$NoService,
    [switch]$NoPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

throw "Windows support is not included in the Alex 0.1.29 stable release. Use the macOS or Linux installer."

function Write-Step([string]$Message) {
    Write-Host "◆ $Message" -ForegroundColor Cyan
}

if (-not [Environment]::Is64BitOperatingSystem) {
    throw "Alex V1 requires 64-bit Windows."
}

$headers = @{ "User-Agent" = "alex-windows-installer" }
if ([string]::IsNullOrWhiteSpace($Version)) {
    Write-Step "finding the latest stable Alex release"
    $release = Invoke-RestMethod -Headers $headers -Uri "https://api.github.com/repos/$Repository/releases/latest"
    $Version = [string]$release.tag_name
}
$Version = $Version.Trim().TrimStart("v")
if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') {
    throw "Invalid Alex version '$Version'."
}

$tag = "v$Version"
$asset = "alex-cli-$Version-windows-x86_64.zip"
$baseUrl = "https://github.com/$Repository/releases/download/$tag"
$temporary = Join-Path ([IO.Path]::GetTempPath()) ("alex-install-" + [Guid]::NewGuid())
$archive = Join-Path $temporary $asset
$checksum = "$archive.sha256"
$expanded = Join-Path $temporary "expanded"

try {
    New-Item -ItemType Directory -Path $temporary, $expanded -Force | Out-Null
    Write-Step "downloading Alex $Version for Windows x86-64"
    Invoke-WebRequest -Headers $headers -Uri "$baseUrl/$asset" -OutFile $archive
    Invoke-WebRequest -Headers $headers -Uri "$baseUrl/$asset.sha256" -OutFile $checksum

    $expected = ((Get-Content -Raw $checksum).Trim() -split '\s+')[0].ToLowerInvariant()
    $actual = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLowerInvariant()
    if ($expected -notmatch '^[a-f0-9]{64}$' -or $actual -ne $expected) {
        throw "SHA-256 verification failed for $asset."
    }

    Expand-Archive -Path $archive -DestinationPath $expanded -Force
    $alexSource = Get-ChildItem -Path $expanded -Filter "alex.exe" -File -Recurse | Select-Object -First 1
    if ($null -eq $alexSource) {
        throw "The release archive does not contain alex.exe."
    }

    $existing = Join-Path $InstallDirectory "alex.exe"
    if ((Test-Path $existing) -and -not $NoService) {
        & $existing service uninstall 2>$null | Out-Null
    }

    Write-Step "installing Alex to $InstallDirectory"
    New-Item -ItemType Directory -Path $InstallDirectory -Force | Out-Null
    Copy-Item -LiteralPath $alexSource.FullName -Destination (Join-Path $InstallDirectory "alex.exe") -Force

    if (-not $NoPath) {
        $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
        $entries = @($userPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
        if ($entries -notcontains $InstallDirectory) {
            [Environment]::SetEnvironmentVariable(
                "Path",
                ((@($InstallDirectory) + $entries) -join ';'),
                "User"
            )
        }
        $env:Path = "$InstallDirectory;$env:Path"
    }

    $alex = Join-Path $InstallDirectory "alex.exe"
    if (-not $NoService) {
        Write-Step "installing the per-user Alex Task Scheduler service"
        & $alex service install
        if ($LASTEXITCODE -ne 0) {
            throw "alex service install exited with status $LASTEXITCODE."
        }
    }

    & $alex --version
    Write-Host "Alex is installed. Open the local UI with:" -ForegroundColor Green
    Write-Host "  alex web"
    Write-Host "Then connect a provider in Onboarding, or run: alex doctor"
}
finally {
    if (Test-Path $temporary) {
        Remove-Item -LiteralPath $temporary -Recurse -Force
    }
}
