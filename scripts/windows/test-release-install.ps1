[CmdletBinding()]
param(
    [string]$Version = "0.1.30"
)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Continue"

# Clean any prior install so this is a true fresh-user run.
cmd /c "schtasks /end /tn AlexDaemon >nul 2>&1"
cmd /c "schtasks /delete /tn AlexDaemon /f >nul 2>&1"
Get-Process alex -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
$bin = Join-Path $env:LOCALAPPDATA "Alex\bin"
if (Test-Path $bin) { Remove-Item -Recurse -Force $bin }

$setup = "Alex-Setup-$Version-windows-arm64.exe"
$base = "https://github.com/madhavajay/alex/releases/download/v$Version"
$tmp = Join-Path ([IO.Path]::GetTempPath()) "alex-release-test"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

Write-Host "==> downloading $setup from the real GitHub release"
Invoke-WebRequest -Uri "$base/$setup" -OutFile (Join-Path $tmp $setup)
Invoke-WebRequest -Uri "$base/$setup.sha256" -OutFile (Join-Path $tmp "$setup.sha256")

$expected = ((Get-Content -Raw (Join-Path $tmp "$setup.sha256")).Trim() -split '\s+')[0].ToLowerInvariant()
$actual = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $setup)).Hash.ToLowerInvariant()
if ($actual -ne $expected) { throw "sha256 mismatch: $actual vs $expected" }
Write-Host "==> sha256 verified: $actual"

Write-Host "==> running Setup silently"
$process = Start-Process -FilePath (Join-Path $tmp $setup) -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" -Wait -PassThru
Write-Host "setup exit: $($process.ExitCode)"

Start-Sleep -Seconds 5
$alex = Join-Path $bin "alex.exe"
Write-Host "installed: $((& $alex --version | Out-String).Trim())"
$task = Get-ScheduledTask -TaskName AlexDaemon -ErrorAction SilentlyContinue
Write-Host "task: $($task.State)"

$deadline = (Get-Date).AddSeconds(20); $health = 0
while ((Get-Date) -lt $deadline) {
    try { $health = (Invoke-WebRequest -UseBasicParsing -TimeoutSec 2 -Uri "http://127.0.0.1:4100/health").StatusCode; break }
    catch { Start-Sleep -Milliseconds 500 }
}
Write-Host "health: $health"

Write-Host "==> self-update check against the release manifest"
& $alex update --check 2>&1 | Select-Object -First 4
Write-Host "RELEASE-INSTALL-TEST-DONE"
