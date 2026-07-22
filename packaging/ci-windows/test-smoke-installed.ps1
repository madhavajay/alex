[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))
$Installer = Join-Path $RepoRoot "install-release.ps1"
$Smoke = Join-Path $PSScriptRoot "smoke-installed.ps1"
$ExpectedChecks = @(
    "windows-11-x86_64",
    "release-installer-sha256",
    "packaged-alex-binary",
    "task-scheduler-action-and-health",
    "web-ui-and-loopback-bootstrap",
    "loopback-exo-route",
    "trace-and-response-body",
    "task-scheduler-pid-replacement",
    "trace-and-body-after-restart",
    "service-state-install-and-path-cleanup"
)

foreach ($path in @($Installer, $Smoke, $PSCommandPath)) {
    $tokens = $null
    $errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile(
        $path,
        [ref]$tokens,
        [ref]$errors
    ) | Out-Null
    if ($errors.Count -gt 0) {
        $details = $errors | Format-List | Out-String
        throw "PowerShell parser errors in ${path}:`n$details"
    }
}

$Plan = (& $Smoke -Version "0.0.0" -PlanOnly | Out-String) | ConvertFrom-Json
if ($Plan.schema_version -ne 1 -or
    $Plan.kind -ne "alex-windows-installed-smoke-plan" -or
    $Plan.platform -ne "windows-11-x86_64") {
    throw "Smoke plan identity or schema changed unexpectedly."
}
if ([bool]$Plan.external_provider_network -or [bool]$Plan.provider_secrets) {
    throw "Windows smoke plan must remain provider-network- and secret-free."
}
$ActualChecks = @($Plan.checks | ForEach-Object { [string]$_ })
if ($ActualChecks.Count -ne $ExpectedChecks.Count) {
    throw "Smoke plan check count changed: expected $($ExpectedChecks.Count), got $($ActualChecks.Count)."
}
foreach ($check in $ExpectedChecks) {
    if ($ActualChecks -notcontains $check) {
        throw "Smoke plan is missing required check '$check'."
    }
}

$InstallerText = Get-Content -LiteralPath $Installer -Raw
foreach ($contract in @(
    "Get-FileHash -Algorithm SHA256",
    'alex-cli-$Version-windows-x86_64.zip',
    "service install"
)) {
    if (-not $InstallerText.Contains($contract)) {
        throw "Windows installer no longer contains required contract '$contract'."
    }
}

$SmokeText = Get-Content -LiteralPath $Smoke -Raw
foreach ($contract in @(
    '"$BaseUrl/ui/"',
    '"$BaseUrl/connect"',
    '"$BaseUrl/admin/exo"',
    '"$BaseUrl/v1/chat/completions"',
    '"$BaseUrl/traces/$TraceId/body/response"',
    "Get-ScheduledTaskPid",
    "service restart",
    "service uninstall"
)) {
    if (-not $SmokeText.Contains($contract)) {
        throw "Windows smoke no longer contains required contract '$contract'."
    }
}
foreach ($providerHost in @(
    "api.anthropic.com",
    "api.openai.com",
    "generativelanguage.googleapis.com",
    "api.x.ai",
    "openrouter.ai"
)) {
    if ($SmokeText.Contains($providerHost)) {
        throw "Windows smoke must not contact provider host '$providerHost'."
    }
}

Write-Host "Windows installer/smoke parser and static contracts passed."
