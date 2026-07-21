[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [string]$Repository = "madhavajay/alex",
    [string]$InstallerPath = "",
    [string]$InstallDirectory = "$env:LOCALAPPDATA\Alex\bin",
    [string]$EvidencePath = "",
    [string]$SmokeRoot = "",
    [switch]$KeepArtifacts,
    [switch]$PlanOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$TaskName = "AlexDaemon"
$BaseUrl = "http://127.0.0.1:4100"
$SessionId = "manual-windows-installed-smoke"
$Model = "ci-smoke-model"
$Harness = "clean-machine-ci-windows"
$ExpectedResponse = "installed route ok"
$PlanChecks = @(
    "windows-11-x86_64",
    "release-installer-sha256",
    "both-packaged-binaries",
    "task-scheduler-action-and-health",
    "web-ui-and-loopback-bootstrap",
    "loopback-exo-route",
    "trace-and-response-body",
    "task-scheduler-pid-replacement",
    "trace-and-body-after-restart",
    "service-state-install-and-path-cleanup"
)

if ($PlanOnly) {
    [ordered]@{
        schema_version = 1
        kind = "alex-windows-installed-smoke-plan"
        platform = "windows-11-x86_64"
        checks = $PlanChecks
        external_provider_network = $false
        provider_secrets = $false
    } | ConvertTo-Json -Depth 4
    return
}

function Write-Step([string]$Message) {
    Write-Host "◆ $Message" -ForegroundColor Cyan
}

function Assert-Condition([bool]$Condition, [string]$Message) {
    if (-not $Condition) {
        throw $Message
    }
}

function Test-Health {
    try {
        $response = Invoke-WebRequest -UseBasicParsing -TimeoutSec 1 -Uri "$BaseUrl/health"
        return $response.StatusCode -eq 200
    }
    catch {
        return $false
    }
}

function Wait-ForCondition(
    [scriptblock]$Condition,
    [string]$FailureMessage,
    [int]$Attempts = 150
) {
    for ($attempt = 0; $attempt -lt $Attempts; $attempt++) {
        if (& $Condition) {
            return
        }
        Start-Sleep -Milliseconds 100
    }
    throw $FailureMessage
}

function Invoke-JsonRequest(
    [string]$Method,
    [string]$Uri,
    [hashtable]$Headers = @{},
    [AllowNull()][string]$Body = $null
) {
    $arguments = @{
        Method = $Method
        Uri = $Uri
        Headers = $Headers
        TimeoutSec = 10
        UseBasicParsing = $true
    }
    if ($null -ne $Body) {
        $arguments["Body"] = $Body
        $arguments["ContentType"] = "application/json"
    }
    $response = Invoke-WebRequest @arguments
    return ($response.Content | ConvertFrom-Json)
}

function Get-ScheduledTaskPid {
    $scheduler = New-Object -ComObject "Schedule.Service"
    $scheduler.Connect()
    $matches = @($scheduler.GetRunningTasks(0) | Where-Object { $_.Name -eq $TaskName })
    if ($matches.Count -ne 1) {
        return $null
    }
    return [int]$matches[0].EnginePID
}

function Get-CanonicalTrace($Detail) {
    return ([ordered]@{
        id = [string]$Detail.trace.id
        session_id = [string]$Detail.trace.session_id
        status = [int]$Detail.trace.status
        provider = [string]$Detail.trace.upstream_provider
        requested_model = [string]$Detail.trace.requested_model
        routed_model = [string]$Detail.trace.routed_model
        harness = [string]$Detail.trace.harness
    } | ConvertTo-Json -Compress)
}

function Start-LoopbackMock(
    [string]$ReadyFile,
    [string]$LogFile,
    [string]$MockModel,
    [string]$ResponseText
) {
    $server = {
        param($ReadyFile, $LogFile, $MockModel, $ResponseText)
        $ErrorActionPreference = "Stop"
        $listener = [System.Net.Sockets.TcpListener]::new(
            [System.Net.IPAddress]::Loopback,
            0
        )
        $listener.Start()
        $port = ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
        Set-Content -LiteralPath $ReadyFile -Value $port -Encoding ASCII
        New-Item -ItemType File -Path $LogFile -Force | Out-Null

        try {
            while ($true) {
                $client = $listener.AcceptTcpClient()
                try {
                    $stream = $client.GetStream()
                    $reader = [System.IO.StreamReader]::new(
                        $stream,
                        [System.Text.UTF8Encoding]::new($false),
                        $false,
                        4096,
                        $true
                    )
                    $requestLine = $reader.ReadLine()
                    if ([string]::IsNullOrWhiteSpace($requestLine)) {
                        continue
                    }
                    $parts = $requestLine.Split(" ")
                    $method = $parts[0]
                    $path = $parts[1]
                    $headers = @{}
                    while ($true) {
                        $line = $reader.ReadLine()
                        if ($null -eq $line -or $line.Length -eq 0) {
                            break
                        }
                        $separator = $line.IndexOf(":")
                        if ($separator -gt 0) {
                            $name = $line.Substring(0, $separator).Trim().ToLowerInvariant()
                            $headers[$name] = $line.Substring($separator + 1).Trim()
                        }
                    }

                    $rawBody = ""
                    $contentLength = 0
                    if ($headers.ContainsKey("content-length")) {
                        $contentLength = [int]$headers["content-length"]
                    }
                    if ($contentLength -gt 0) {
                        $buffer = [char[]]::new($contentLength)
                        $offset = 0
                        while ($offset -lt $contentLength) {
                            $read = $reader.Read($buffer, $offset, $contentLength - $offset)
                            if ($read -le 0) {
                                break
                            }
                            $offset += $read
                        }
                        $rawBody = [string]::new($buffer, 0, $offset)
                    }

                    $status = 404
                    $payload = [ordered]@{
                        error = [ordered]@{ message = "not found"; type = "mock_error" }
                    }
                    if ($method -eq "GET" -and $path -eq "/v1/models") {
                        $status = 200
                        $payload = [ordered]@{
                            object = "list"
                            data = @([ordered]@{
                                id = $MockModel
                                object = "model"
                                owned_by = "ci-loopback"
                            })
                        }
                        [ordered]@{ event = "models"; path = $path } |
                            ConvertTo-Json -Compress |
                            Add-Content -LiteralPath $LogFile -Encoding UTF8
                    }
                    elseif ($method -eq "POST" -and $path -eq "/v1/chat/completions") {
                        $request = $rawBody | ConvertFrom-Json
                        $authorized = $headers.ContainsKey("authorization") -and
                            $headers["authorization"] -eq "Bearer x"
                        [ordered]@{
                            authorized = $authorized
                            event = "chat"
                            model = [string]$request.model
                            path = $path
                            stream = [bool]$request.stream
                        } | ConvertTo-Json -Compress |
                            Add-Content -LiteralPath $LogFile -Encoding UTF8
                        if ($authorized -and $request.model -eq $MockModel -and
                            -not [bool]$request.stream) {
                            $status = 200
                            $payload = [ordered]@{
                                id = "chatcmpl-ci-installed-smoke"
                                object = "chat.completion"
                                created = 946684800
                                model = $MockModel
                                choices = @([ordered]@{
                                    index = 0
                                    message = [ordered]@{
                                        role = "assistant"
                                        content = $ResponseText
                                    }
                                    finish_reason = "stop"
                                })
                                usage = [ordered]@{
                                    prompt_tokens = 1
                                    completion_tokens = 3
                                    total_tokens = 4
                                }
                            }
                        }
                        else {
                            $status = 400
                            $payload = [ordered]@{
                                error = [ordered]@{
                                    message = "unexpected routed request"
                                    type = "mock_error"
                                }
                            }
                        }
                    }

                    $json = $payload | ConvertTo-Json -Compress -Depth 10
                    $bodyBytes = [System.Text.Encoding]::UTF8.GetBytes($json)
                    $reason = if ($status -eq 200) { "OK" } elseif ($status -eq 400) {
                        "Bad Request"
                    } else { "Not Found" }
                    $head = "HTTP/1.1 $status $reason`r`n" +
                        "Content-Type: application/json`r`n" +
                        "Content-Length: $($bodyBytes.Length)`r`n" +
                        "Connection: close`r`n`r`n"
                    $headBytes = [System.Text.Encoding]::ASCII.GetBytes($head)
                    $stream.Write($headBytes, 0, $headBytes.Length)
                    $stream.Write($bodyBytes, 0, $bodyBytes.Length)
                    $stream.Flush()
                }
                finally {
                    $client.Dispose()
                }
            }
        }
        finally {
            $listener.Stop()
        }
    }
    return (Start-Job -ScriptBlock $server -ArgumentList @(
        $ReadyFile, $LogFile, $MockModel, $ResponseText
    ))
}

$Version = $Version.Trim().TrimStart("v")
Assert-Condition ($Version -match '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') `
    "Invalid Alex version '$Version'."

if ([string]::IsNullOrWhiteSpace($InstallerPath)) {
    $InstallerPath = Join-Path (Join-Path $PSScriptRoot "..\..") "install-release.ps1"
}
$InstallerPath = [IO.Path]::GetFullPath($InstallerPath)
$InstallDirectory = [IO.Path]::GetFullPath($InstallDirectory)
if ([string]::IsNullOrWhiteSpace($EvidencePath)) {
    $EvidencePath = Join-Path (Get-Location) "windows-smoke-evidence.json"
}
$EvidencePath = [IO.Path]::GetFullPath($EvidencePath)
if ([string]::IsNullOrWhiteSpace($SmokeRoot)) {
    $SmokeRoot = Join-Path ([IO.Path]::GetTempPath()) (
        "alex-installed-windows-" + [Guid]::NewGuid().ToString("N")
    )
}
$SmokeRoot = [IO.Path]::GetFullPath($SmokeRoot)
$StateDirectory = Join-Path $env:USERPROFILE ".alex"
$AlexBin = Join-Path $InstallDirectory "alex.exe"
$LegacyBin = Join-Path $InstallDirectory "alex.exe"
$ReadyFile = Join-Path $SmokeRoot "mock.port"
$MockLog = Join-Path $SmokeRoot "mock.ndjson"
$OriginalUserPath = [Environment]::GetEnvironmentVariable("Path", "User")
$OriginalProcessPath = $env:Path
$MockJob = $null
$CleanupAllowed = $false
$SmokeRootOwned = $false
$RunSucceeded = $false
$Failure = $null
$CleanupFailure = $null

$Checks = [ordered]@{}
foreach ($check in $PlanChecks) {
    $Checks[$check] = $false
}
$Result = [ordered]@{
    schema_version = 1
    passed = $false
    platform = [ordered]@{
        os = "windows-11"
        arch = "x86_64"
        build = $null
    }
    package = [ordered]@{
        version = $Version
        repository = $Repository
        checksum_verified_by_installer = $false
        alex = $false
        alex = $false
    }
    service = [ordered]@{
        manager = "task-scheduler-user"
        task = $TaskName
        action = $null
        trigger = $null
        run_level = $null
        pid_before = $null
        pid_after = $null
        replaced = $false
    }
    web_ui = [ordered]@{
        url = "$BaseUrl/ui/"
        index = $false
        javascript = $false
        stylesheet = $false
        loopback_bootstrap = $false
    }
    route = [ordered]@{
        provider = "exo"
        model = "exo/$Model"
        loopback_mock = $true
        response = $null
    }
    trace = [ordered]@{
        id = $null
        session_id = $SessionId
        response_body = $false
        persisted_across_restart = $false
    }
    cleanup = [ordered]@{
        task_removed = $false
        daemon_stopped = $false
        state_removed = $false
        install_removed = $false
        user_path_restored = $false
        artifacts_retained = [bool]$KeepArtifacts
    }
    checks = $Checks
    external_provider_network = $false
    provider_secrets = $false
    error = $null
}

try {
    Write-Step "checking for a clean Windows 11 x86-64 user environment"
    Assert-Condition ($env:OS -eq "Windows_NT") "This smoke requires Windows."
    Assert-Condition ([Environment]::Is64BitOperatingSystem) `
        "This smoke requires 64-bit Windows."
    $architecture = if ([string]::IsNullOrWhiteSpace($env:PROCESSOR_ARCHITEW6432)) {
        $env:PROCESSOR_ARCHITECTURE
    } else {
        $env:PROCESSOR_ARCHITEW6432
    }
    Assert-Condition ($architecture -eq "AMD64") `
        "This smoke requires Windows x86-64; found '$architecture'."
    $operatingSystem = Get-CimInstance Win32_OperatingSystem
    $build = [int]$operatingSystem.BuildNumber
    Assert-Condition ($build -ge 22000) `
        "This smoke requires Windows 11 (build 22000 or newer); found $build."
    $Result.platform.build = $build
    Assert-Condition (Test-Path -LiteralPath $InstallerPath -PathType Leaf) `
        "Release installer not found at $InstallerPath."
    Assert-Condition (-not (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue)) `
        "Task Scheduler entry '$TaskName' already exists. Use a clean VM."
    Assert-Condition (-not (Test-Health)) `
        "A daemon is already listening at $BaseUrl. Use a clean VM."
    Assert-Condition (-not (Test-Path -LiteralPath $InstallDirectory)) `
        "Install directory already exists at $InstallDirectory. Use a clean VM."
    Assert-Condition (-not (Test-Path -LiteralPath $StateDirectory)) `
        "State directory already exists at $StateDirectory. Use a clean VM."
    Assert-Condition ([string]::IsNullOrWhiteSpace($env:ALEX_HOME)) `
        "ALEX_HOME is already set. Use a clean VM user environment."
    $EvidenceInsideSmokeRoot = $EvidencePath.Equals(
        $SmokeRoot,
        [StringComparison]::OrdinalIgnoreCase
    ) -or $EvidencePath.StartsWith(
        $SmokeRoot.TrimEnd("\") + "\",
        [StringComparison]::OrdinalIgnoreCase
    )
    Assert-Condition (-not $EvidenceInsideSmokeRoot) `
        "EvidencePath must be outside SmokeRoot so cleanup cannot delete it."
    Assert-Condition (-not (Test-Path -LiteralPath $SmokeRoot)) `
        "SmokeRoot already exists at $SmokeRoot; choose a new disposable path."
    $CleanupAllowed = $true
    New-Item -ItemType Directory -Path $SmokeRoot -Force | Out-Null
    $SmokeRootOwned = $true
    $Checks["windows-11-x86_64"] = $true

    Write-Step "starting the deterministic loopback OpenAI-compatible mock"
    $MockJob = Start-LoopbackMock $ReadyFile $MockLog $Model $ExpectedResponse
    Wait-ForCondition {
        (Test-Path -LiteralPath $ReadyFile) -and
            ((Get-Job -Id $MockJob.Id).State -eq "Running")
    } "Loopback mock did not become ready."
    $MockPort = [int](Get-Content -LiteralPath $ReadyFile -Raw)
    Assert-Condition ($MockPort -gt 0) "Loopback mock returned an invalid port."
    $MockUrl = "http://127.0.0.1:$MockPort"

    Write-Step "installing and checksum-verifying Alex $Version"
    & $InstallerPath -Version $Version -Repository $Repository `
        -InstallDirectory $InstallDirectory
    Assert-Condition (Test-Path -LiteralPath $AlexBin -PathType Leaf) `
        "Installer did not install alex.exe."
    Assert-Condition (Test-Path -LiteralPath $LegacyBin -PathType Leaf) `
        "Installer did not install alex.exe."
    $AlexVersion = (& $AlexBin --version | Out-String).Trim()
    $LegacyVersion = (& $LegacyBin --version | Out-String).Trim()
    Assert-Condition ($AlexVersion -eq "alex $Version") `
        "Unexpected alex.exe version '$AlexVersion'."
    Assert-Condition ($LegacyVersion -eq $AlexVersion) `
        "Compatibility executable reports '$LegacyVersion', expected '$AlexVersion'."
    $Result.package.checksum_verified_by_installer = $true
    $Result.package.alex = $true
    $Result.package.alex = $true
    $Checks["release-installer-sha256"] = $true
    $Checks["both-packaged-binaries"] = $true

    $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction Stop
    Assert-Condition ($task.State.ToString() -eq "Running") `
        "Installed Task Scheduler entry is not running."
    Assert-Condition ($task.Actions.Count -eq 1) `
        "Installed task should have exactly one executable action."
    $TaskAction = [IO.Path]::GetFullPath([string]$task.Actions[0].Execute)
    Assert-Condition ([string]::Equals(
        $TaskAction,
        $AlexBin,
        [StringComparison]::OrdinalIgnoreCase
    )) `
        "Installed task action '$TaskAction' is not pinned to '$AlexBin'."
    Assert-Condition ([string]$task.Actions[0].Arguments -eq "daemon") `
        "Installed task arguments are not exactly 'daemon'."
    Assert-Condition ($task.Triggers.Count -eq 1 -and
        $task.Triggers[0].CimClass.CimClassName -eq "MSFT_TaskLogonTrigger") `
        "Installed task does not have exactly one per-user logon trigger."
    $RunLevel = $task.Principal.RunLevel.ToString()
    Assert-Condition ($RunLevel -eq "Limited") `
        "Installed task run level is '$RunLevel', expected 'Limited'."
    Wait-ForCondition { Test-Health } `
        "Installed Task Scheduler daemon did not become healthy."
    $status = (& $AlexBin status --json | Out-String) | ConvertFrom-Json
    Assert-Condition ([bool]$status.daemon.running) `
        "Public status JSON does not report a running daemon."
    Assert-Condition ([bool]$status.daemon.service.managed) `
        "Public status JSON does not report a managed Task Scheduler service."
    $Result.service.action = "$TaskAction daemon"
    $Result.service.trigger = "at-user-logon"
    $Result.service.run_level = $RunLevel
    $Checks["task-scheduler-action-and-health"] = $true

    Write-Step "checking the installed shared web UI and loopback bootstrap"
    $WebOutput = (& $AlexBin web --no-open | Out-String)
    Assert-Condition ($WebOutput.Contains("$BaseUrl/ui/")) `
        "alex web --no-open did not print the loopback UI URL."
    $IndexResponse = Invoke-WebRequest -UseBasicParsing -TimeoutSec 5 -Uri "$BaseUrl/ui/"
    $ScriptResponse = Invoke-WebRequest -UseBasicParsing -TimeoutSec 5 -Uri "$BaseUrl/ui/app.js"
    $StyleResponse = Invoke-WebRequest -UseBasicParsing -TimeoutSec 5 -Uri "$BaseUrl/ui/styles.css"
    Assert-Condition ($IndexResponse.StatusCode -eq 200 -and $IndexResponse.Content.Length -gt 0) `
        "Shared web UI index is unavailable."
    Assert-Condition ($ScriptResponse.StatusCode -eq 200 -and $ScriptResponse.Content.Length -gt 0) `
        "Shared web UI JavaScript is unavailable."
    Assert-Condition ($StyleResponse.StatusCode -eq 200 -and $StyleResponse.Content.Length -gt 0) `
        "Shared web UI stylesheet is unavailable."
    $Connect = Invoke-JsonRequest "GET" "$BaseUrl/connect"
    Assert-Condition ([string]$Connect.base_url -eq $BaseUrl) `
        "Loopback /connect returned an unexpected base URL."
    $LocalKey = [string]$Connect.api_key
    Assert-Condition ($LocalKey.StartsWith("alx-")) `
        "Loopback /connect did not return a local Alex key."
    $Result.web_ui.index = $true
    $Result.web_ui.javascript = $true
    $Result.web_ui.stylesheet = $true
    $Result.web_ui.loopback_bootstrap = $true
    $Checks["web-ui-and-loopback-bootstrap"] = $true

    Write-Step "routing one request through loopback Exo"
    $AdminHeaders = @{ "x-api-key" = $LocalKey }
    $ExoBody = [ordered]@{
        url = $MockUrl
        enabled_models = @($Model)
    } | ConvertTo-Json -Compress
    $Exo = Invoke-JsonRequest "PUT" "$BaseUrl/admin/exo" $AdminHeaders $ExoBody
    Assert-Condition ([string]$Exo.url -eq $MockUrl) `
        "Daemon did not retain the loopback Exo endpoint."
    Assert-Condition (@($Exo.enabled_models).Count -eq 1 -and
        [string]$Exo.enabled_models[0] -eq $Model) `
        "Daemon did not retain the loopback Exo model."
    $Models = Invoke-JsonRequest "GET" "$BaseUrl/v1/models" @{
        Authorization = "Bearer $LocalKey"
    }
    $PublishedModels = @($Models.data | ForEach-Object { [string]$_.id })
    Assert-Condition ($PublishedModels -contains "alex/$Model") `
        "Enabled loopback model was not published by /v1/models."
    $RequestBody = [ordered]@{
        model = "exo/$Model"
        stream = $false
        messages = @([ordered]@{
            role = "user"
            content = "installed Windows smoke"
        })
    } | ConvertTo-Json -Compress -Depth 5
    $Response = Invoke-JsonRequest "POST" "$BaseUrl/v1/chat/completions" @{
        Authorization = "Bearer $LocalKey"
        "x-alex-harness" = $Harness
        "x-session-id" = $SessionId
    } $RequestBody
    Assert-Condition ([string]$Response.id -eq "chatcmpl-ci-installed-smoke" -and
        [string]$Response.choices[0].message.content -eq $ExpectedResponse) `
        "Routed response did not come from the deterministic loopback mock."
    $MockEvents = @(Get-Content -LiteralPath $MockLog | ForEach-Object {
        $_ | ConvertFrom-Json
    })
    $Observed = @($MockEvents | Where-Object {
        $_.event -eq "chat" -and $_.path -eq "/v1/chat/completions" -and
        $_.model -eq $Model -and [bool]$_.authorized -and -not [bool]$_.stream
    })
    Assert-Condition ($Observed.Count -ge 1) `
        "Loopback mock did not observe the normalized Exo request."
    $Result.route.response = $ExpectedResponse
    $Checks["loopback-exo-route"] = $true

    Write-Step "locating the routed trace and persisted response body"
    $TraceId = $null
    for ($attempt = 0; $attempt -lt 100; $attempt++) {
        $Traces = Invoke-JsonRequest "GET" `
            "$BaseUrl/admin/traces?session=$SessionId&limit=10" $AdminHeaders
        $Match = @($Traces.traces | Where-Object {
            $_.session_id -eq $SessionId -and [int]$_.status -eq 200 -and
            $_.upstream_provider -eq "exo"
        } | Select-Object -First 1)
        if ($Match.Count -eq 1) {
            $TraceId = [string]$Match[0].id
            break
        }
        Start-Sleep -Milliseconds 100
    }
    Assert-Condition (-not [string]::IsNullOrWhiteSpace($TraceId)) `
        "Routed request was not written to the trace API."
    $TraceBefore = Invoke-JsonRequest "GET" "$BaseUrl/traces/$TraceId" $AdminHeaders
    Assert-Condition ([string]$TraceBefore.trace.id -eq $TraceId -and
        [string]$TraceBefore.trace.session_id -eq $SessionId -and
        [int]$TraceBefore.trace.status -eq 200 -and
        [string]$TraceBefore.trace.upstream_provider -eq "exo" -and
        [string]$TraceBefore.trace.requested_model -eq "exo/$Model" -and
        [string]$TraceBefore.trace.routed_model -eq $Model -and
        [string]$TraceBefore.trace.harness -eq $Harness) `
        "Trace detail does not describe the installed Windows route."
    $CanonicalBefore = Get-CanonicalTrace $TraceBefore
    $BodyBeforeResponse = Invoke-WebRequest -UseBasicParsing -TimeoutSec 5 `
        -Headers $AdminHeaders -Uri "$BaseUrl/traces/$TraceId/body/response"
    $BodyBefore = $BodyBeforeResponse.Content
    $BodyBeforeJson = $BodyBefore | ConvertFrom-Json
    Assert-Condition ([string]$BodyBeforeJson.choices[0].message.content -eq $ExpectedResponse) `
        "Persisted trace response body is not readable before restart."
    $Result.trace.id = $TraceId
    $Result.trace.response_body = $true
    $Checks["trace-and-response-body"] = $true

    $PidBefore = Get-ScheduledTaskPid
    Assert-Condition ($null -ne $PidBefore -and $PidBefore -gt 0) `
        "Task Scheduler did not report the daemon PID."
    $Result.service.pid_before = $PidBefore
    Write-Step "restarting Task Scheduler daemon PID $PidBefore"
    & $AlexBin service restart
    $PidAfter = $null
    Wait-ForCondition {
        $candidate = Get-ScheduledTaskPid
        if ($null -ne $candidate -and $candidate -gt 0 -and
            $candidate -ne $PidBefore -and (Test-Health)) {
            $script:PidAfter = $candidate
            return $true
        }
        return $false
    } "Task Scheduler restart did not replace daemon PID $PidBefore."
    $Result.service.pid_after = $PidAfter
    $Result.service.replaced = $true
    $Checks["task-scheduler-pid-replacement"] = $true

    $TraceAfter = Invoke-JsonRequest "GET" "$BaseUrl/traces/$TraceId" $AdminHeaders
    $CanonicalAfter = Get-CanonicalTrace $TraceAfter
    Assert-Condition ($CanonicalAfter -eq $CanonicalBefore) `
        "Trace metadata changed or disappeared across Task Scheduler restart."
    $BodyAfterResponse = Invoke-WebRequest -UseBasicParsing -TimeoutSec 5 `
        -Headers $AdminHeaders -Uri "$BaseUrl/traces/$TraceId/body/response"
    Assert-Condition ($BodyAfterResponse.Content -eq $BodyBefore) `
        "Trace response body changed or disappeared across restart."
    $Result.trace.persisted_across_restart = $true
    $Checks["trace-and-body-after-restart"] = $true
    $RunSucceeded = $true
}
catch {
    $Failure = $_.Exception.Message
}
finally {
    Write-Step "cleaning smoke-owned task, state, install, and PATH changes"
    try {
        if ($CleanupAllowed) {
            if (Test-Path -LiteralPath $AlexBin -PathType Leaf) {
                & $AlexBin service uninstall 2>$null | Out-Null
            }
            elseif (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
                $task = Get-ScheduledTask -TaskName $TaskName
                if ($task.State.ToString() -eq "Running") {
                    Stop-ScheduledTask -TaskName $TaskName
                }
                Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
            }
            Wait-ForCondition {
                -not (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue)
            } "Task Scheduler entry remained after cleanup." 50
            Wait-ForCondition { -not (Test-Health) } `
                "Daemon remained healthy after task cleanup." 50
            $Result.cleanup.task_removed = $true
            $Result.cleanup.daemon_stopped = $true

            if ($null -ne $MockJob) {
                Stop-Job -Id $MockJob.Id -ErrorAction SilentlyContinue
                Remove-Job -Id $MockJob.Id -Force -ErrorAction SilentlyContinue
            }
            if (Test-Path -LiteralPath $StateDirectory) {
                Remove-Item -LiteralPath $StateDirectory -Recurse -Force
            }
            if (Test-Path -LiteralPath $InstallDirectory) {
                Remove-Item -LiteralPath $InstallDirectory -Recurse -Force
            }
            [Environment]::SetEnvironmentVariable("Path", $OriginalUserPath, "User")
            $env:Path = $OriginalProcessPath
            $Result.cleanup.state_removed = -not (Test-Path -LiteralPath $StateDirectory)
            $Result.cleanup.install_removed = -not (Test-Path -LiteralPath $InstallDirectory)
            $RestoredUserPath = [Environment]::GetEnvironmentVariable("Path", "User")
            $Result.cleanup.user_path_restored = $RestoredUserPath -eq $OriginalUserPath
            Assert-Condition ([bool]$Result.cleanup.state_removed) `
                "State directory remained after cleanup."
            Assert-Condition ([bool]$Result.cleanup.install_removed) `
                "Install directory remained after cleanup."
            Assert-Condition ([bool]$Result.cleanup.user_path_restored) `
                "User PATH was not restored after cleanup."
            $Checks["service-state-install-and-path-cleanup"] = $true
        }
    }
    catch {
        $CleanupFailure = $_.Exception.Message
    }
    finally {
        if ($null -ne $MockJob) {
            Stop-Job -Id $MockJob.Id -ErrorAction SilentlyContinue
            Remove-Job -Id $MockJob.Id -Force -ErrorAction SilentlyContinue
        }
        if (-not $KeepArtifacts -and $SmokeRootOwned -and
            (Test-Path -LiteralPath $SmokeRoot)) {
            Remove-Item -LiteralPath $SmokeRoot -Recurse -Force
        }
    }

    if ($null -ne $CleanupFailure) {
        if ($null -eq $Failure) {
            $Failure = "cleanup failed: $CleanupFailure"
        }
        else {
            $Failure = "$Failure; cleanup failed: $CleanupFailure"
        }
    }
    $Result.passed = $RunSucceeded -and ($null -eq $Failure)
    $Result.error = $Failure
    New-Item -ItemType Directory -Path (Split-Path -Parent $EvidencePath) -Force |
        Out-Null
    $Result | ConvertTo-Json -Depth 8 |
        Set-Content -LiteralPath $EvidencePath -Encoding UTF8
}

if (-not [bool]$Result.passed) {
    Write-Host "Windows installed smoke failed: $Failure" -ForegroundColor Red
    Write-Host "Evidence: $EvidencePath"
    exit 1
}

Write-Host (
    "Windows installed smoke passed: trace {0} survived Task Scheduler PID {1} -> {2}" -f
    $Result.trace.id, $Result.service.pid_before, $Result.service.pid_after
) -ForegroundColor Green
Write-Host "Evidence: $EvidencePath"
