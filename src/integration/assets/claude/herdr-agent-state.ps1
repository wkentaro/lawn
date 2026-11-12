# installed by herdr
# managed by herdr; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HERDR_INTEGRATION_ID=claude
# HERDR_INTEGRATION_VERSION=12

param([string]$Action = "")

if ($Action -cnotin @("session", "subagent-start", "subagent-stop")) { exit 0 }
if ($env:HERDR_ENV -ne "1") { exit 0 }
if ([string]::IsNullOrWhiteSpace($env:HERDR_PANE_ID)) { exit 0 }

# Read hook input as raw bytes so UTF-8 payloads decode correctly even when the
# console input code page is not UTF-8 (for example CP949 on Korean Windows).
$stdinReader = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), [System.Text.Encoding]::UTF8)
$inputText = $stdinReader.ReadToEnd()
try {
    $payload = if ([string]::IsNullOrWhiteSpace($inputText)) { $null } else { $inputText | ConvertFrom-Json -ErrorAction Stop }
} catch {
    exit 0
}

$propertyNames = @($payload.PSObject.Properties.Name)
if ((Test-Path Env:CURSOR_VERSION) -or $propertyNames -ccontains "cursor_version") { exit 0 }
if (-not ($propertyNames -ccontains "hook_event_name") -or $payload.hook_event_name -isnot [string]) { exit 0 }

$sessionId = $payload.session_id
if ($sessionId -isnot [string] -or [string]::IsNullOrWhiteSpace($sessionId)) { exit 0 }

if ($Action -cne "session") {
    $expectedEvent = if ($Action -ceq "subagent-start") { "SubagentStart" } else { "SubagentStop" }
    if ($payload.hook_event_name -cne $expectedEvent) { exit 0 }
    if ($payload.agent_id -isnot [string] -or [string]::IsNullOrWhiteSpace($payload.agent_id)) { exit 0 }
    if ([string]::IsNullOrWhiteSpace($env:HERDR_SOCKET_PATH)) { exit 0 }
    $params = @{
        pane_id = $env:HERDR_PANE_ID
        source = "herdr:claude"
        session_id = $sessionId
        event = $(if ($Action -ceq "subagent-start") { "subagent_started" } else { "subagent_stop_observed" })
        task_id = $payload.agent_id
    }
    if ($payload.agent_type -is [string] -and -not [string]::IsNullOrWhiteSpace($payload.agent_type)) {
        $params.task_type = $payload.agent_type
    }
    $request = @{
        id = "herdr:claude:$([Guid]::NewGuid())"
        method = "background.report"
        params = $params
    }
    $pipe = $null
    try {
        $pipe = New-Object System.IO.Pipes.NamedPipeClientStream(".", $env:HERDR_SOCKET_PATH, [System.IO.Pipes.PipeDirection]::InOut, [System.IO.Pipes.PipeOptions]::Asynchronous)
        $pipe.Connect(500)
        $bytes = [System.Text.Encoding]::UTF8.GetBytes(($request | ConvertTo-Json -Depth 10 -Compress) + "`n")
        $pipe.Write($bytes, 0, $bytes.Length)
        $pipe.Flush()
        $response = New-Object byte[] 4096
        $null = $pipe.ReadAsync($response, 0, $response.Length).Wait(500)
    } catch {
    } finally {
        if ($null -ne $pipe) { $pipe.Dispose() }
    }
    exit 0
}

if ($payload.hook_event_name -cne "SessionStart") { exit 0 }
if (-not [string]::IsNullOrWhiteSpace($payload.agent_id)) { exit 0 }

$seq = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$herdr = if ([string]::IsNullOrWhiteSpace($env:HERDR_BIN_PATH)) { "herdr" } else { $env:HERDR_BIN_PATH }
try {
    $args = @(
        "pane",
        "report-agent-session",
        $env:HERDR_PANE_ID,
        "--source",
        "herdr:claude",
        "--agent",
        "claude",
        "--seq",
        "$seq",
        "--agent-session-id",
        "$sessionId"
    )
    if ($payload.transcript_path -is [string] -and -not [string]::IsNullOrWhiteSpace($payload.transcript_path)) {
        $args += @("--agent-session-path", "$($payload.transcript_path)")
    }
    if ($payload.hook_event_name -eq "SessionStart" -and $payload.source -is [string] -and -not [string]::IsNullOrWhiteSpace($payload.source)) {
        $args += @("--session-start-source", "$($payload.source)")
    }
    & $herdr @args 2>$null | Out-Null
} catch {
}
