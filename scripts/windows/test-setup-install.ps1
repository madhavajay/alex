[CmdletBinding()]
param(
    [string]$Setup = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($Setup)) {
    $desktop = [Environment]::GetFolderPath("Desktop")
    $Setup = Get-ChildItem -Path $desktop -Filter "Alex-Setup-*.exe" |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
}
if (-not $Setup -or -not (Test-Path $Setup)) {
    throw "No Alex-Setup exe found."
}

# Remove the script-installed copy so Setup.exe starts clean.
cmd /c "schtasks /end /tn AlexDaemon >nul 2>&1"
cmd /c "schtasks /delete /tn AlexDaemon /f >nul 2>&1"
Get-Process alex -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
$oldBin = Join-Path $env:LOCALAPPDATA "Alex\bin"
if (Test-Path $oldBin) { Remove-Item -Recurse -Force $oldBin }

Write-Host "==> running $Setup silently"
$process = Start-Process -FilePath $Setup -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" -Wait -PassThru
if ($process.ExitCode -ne 0) {
    throw "Setup exited with code $($process.ExitCode)"
}

Start-Sleep -Seconds 5
$alex = Join-Path $env:LOCALAPPDATA "Alex\bin\alex.exe"
if (-not (Test-Path $alex)) { throw "Setup did not install $alex" }
Write-Host "installed: $((& $alex --version | Out-String).Trim())"

$task = Get-ScheduledTask -TaskName AlexDaemon -ErrorAction Stop
Write-Host "task state: $($task.State)"

$deadline = (Get-Date).AddSeconds(20)
$health = 0
while ((Get-Date) -lt $deadline) {
    try {
        $health = (Invoke-WebRequest -UseBasicParsing -TimeoutSec 2 -Uri "http://127.0.0.1:4100/health").StatusCode
        break
    } catch { Start-Sleep -Milliseconds 500 }
}
Write-Host "health: $health"
if ($health -ne 200) { throw "daemon did not become healthy after Setup install" }

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*Alex\bin*") { throw "user PATH does not contain Alex\bin" }
Write-Host "user PATH contains Alex\bin"
Write-Host "SETUP-INSTALL-OK"
