# installed by herdr
# managed by herdr; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HERDR_INTEGRATION_ID=claude
# HERDR_INTEGRATION_VERSION=12

param([string]$Action = "")

if ($Action -cnotin @("session", "subagent-start", "subagent-stop", "stop", "bash-background")) { exit 0 }
if ($env:HERDR_ENV -ne "1") { exit 0 }
if ([string]::IsNullOrWhiteSpace($env:HERDR_PANE_ID)) { exit 0 }

# Stop after cap+1 bytes even if the producer leaves the pipe open.
$inputLimit = 1048576
$inputBytes = New-Object byte[] ($inputLimit + 1)
$stdinStream = [Console]::OpenStandardInput()
$inputLength = 0
$utf8 = New-Object System.Text.UTF8Encoding($false, $true)
try {
    while ($inputLength -lt $inputBytes.Length) {
        $read = $stdinStream.Read($inputBytes, $inputLength, $inputBytes.Length - $inputLength)
        if ($read -eq 0) { break }
        $inputLength += $read
    }
    if ($inputLength -gt $inputLimit) { exit 0 }
    $inputText = $utf8.GetString($inputBytes, 0, $inputLength)
    if (-not $inputText.TrimStart().StartsWith("{")) { exit 0 }
    $payload = if ([string]::IsNullOrWhiteSpace($inputText)) { $null } else { $inputText | ConvertFrom-Json -ErrorAction Stop }
} catch {
    exit 0
}

function Test-HookProperty($Value, [string]$Name) {
    return $Value -is [PSCustomObject] -and @($Value.PSObject.Properties.Name) -ccontains $Name
}

function Test-HookText($Value, [int]$Limit = 256) {
    if ($Value -isnot [string] -or [string]::IsNullOrWhiteSpace($Value)) { return $false }
    try {
        if ($utf8.GetByteCount($Value) -gt $Limit) { return $false }
    } catch { return $false }
    foreach ($character in $Value.ToCharArray()) {
        if ([char]::IsControl($character)) { return $false }
    }
    return $true
}

function Get-ClippedCommand([string]$Value) {
    $bytes = $utf8.GetBytes($Value)
    if ($bytes.Length -le 4096) { return @{ value = $Value; truncated = $false } }
    # Bash observations have no truncation flag, so disclose clipping in the text.
    $marker = "`n[truncated]"
    $length = 4096 - $utf8.GetByteCount($marker)
    while ($length -gt 0 -and ($bytes[$length] -band 0xC0) -eq 0x80) { $length-- }
    return @{ value = $utf8.GetString($bytes, 0, $length) + $marker; truncated = $true }
}

function Send-HookReport([string]$Method, $Params) {
    $request = @{
        id = "herdr:claude:$([Guid]::NewGuid())"
        method = $Method
        params = $Params
    }
    $pipe = $null
    try {
        while ($true) {
            $bytes = [System.Text.Encoding]::UTF8.GetBytes(($request | ConvertTo-Json -Depth 10 -Compress) + "`n")
            if ($bytes.Length -le 1048576 -or $Method -cne "background.shell.report") { break }
            $observation = $Params.observation
            if ($observation.event -cne "snapshot_observed" -or $observation.tasks.Count -eq 0) { return }
            $observation.tasks = @($observation.tasks | Select-Object -First ($observation.tasks.Count - 1))
            $observation.truncated = $true
        }
        $pipe = New-Object System.IO.Pipes.NamedPipeClientStream(".", $env:HERDR_SOCKET_PATH, [System.IO.Pipes.PipeDirection]::InOut, [System.IO.Pipes.PipeOptions]::Asynchronous)
        $pipe.Connect(500)
        $pipe.Write($bytes, 0, $bytes.Length)
        $pipe.Flush()
        $response = New-Object byte[] 4096
        $null = $pipe.ReadAsync($response, 0, $response.Length).Wait(500)
    } catch {
    } finally {
        if ($null -ne $pipe) { $pipe.Dispose() }
    }
}

if ($payload -isnot [PSCustomObject]) { exit 0 }
$propertyNames = @($payload.PSObject.Properties.Name)
if ((Test-Path Env:CURSOR_VERSION) -or $propertyNames -ccontains "cursor_version") { exit 0 }
if (-not ($propertyNames -ccontains "hook_event_name") -or $payload.hook_event_name -isnot [string]) { exit 0 }

$sessionId = $payload.session_id
if (-not ($propertyNames -ccontains "session_id") -or -not (Test-HookText $sessionId) -or -not (Test-HookText $env:HERDR_PANE_ID)) { exit 0 }

if ($Action -cin @("subagent-start", "subagent-stop")) {
    $expectedEvent = if ($Action -ceq "subagent-start") { "SubagentStart" } else { "SubagentStop" }
    if ($payload.hook_event_name -cne $expectedEvent) { exit 0 }
    if (-not ($propertyNames -ccontains "agent_id") -or -not (Test-HookText $payload.agent_id)) { exit 0 }
    if ([string]::IsNullOrWhiteSpace($env:HERDR_SOCKET_PATH)) { exit 0 }
    $params = @{
        pane_id = $env:HERDR_PANE_ID
        source = "herdr:claude"
        session_id = $sessionId
        event = $(if ($Action -ceq "subagent-start") { "subagent_started" } else { "subagent_stop_observed" })
        task_id = $payload.agent_id
    }
    if (($propertyNames -ccontains "agent_type") -and (Test-HookText $payload.agent_type)) {
        $params.task_type = $payload.agent_type
    }
    Send-HookReport "background.report" $params
}

# Optional shell data must never suppress the lifecycle report above.
if ($Action -cin @("stop", "subagent-stop", "bash-background")) {
    if ([string]::IsNullOrWhiteSpace($env:HERDR_SOCKET_PATH)) { exit 0 }
    if (($propertyNames -ccontains "agent_id") -and -not (Test-HookText $payload.agent_id)) { exit 0 }
    try {
        if ($Action -cin @("stop", "subagent-stop")) {
            $expectedEvent = if ($Action -ceq "stop") { "Stop" } else { "SubagentStop" }
            if ($payload.hook_event_name -cne $expectedEvent) { exit 0 }
            if (-not ($propertyNames -ccontains "background_tasks") -or $payload.background_tasks -isnot [array]) { exit 0 }
            $tasks = New-Object System.Collections.Generic.List[object]
            $truncated = $false
            foreach ($task in $payload.background_tasks) {
                if (-not (Test-HookProperty $task "type") -or $task.type -isnot [string]) { exit 0 }
                if ($task.type -cne "shell") { continue }
                if (-not (Test-HookProperty $task "id") -or -not (Test-HookText $task.id)) { exit 0 }
                if (-not (Test-HookProperty $task "status") -or -not (Test-HookText $task.status 128)) { exit 0 }
                $hasCommand = Test-HookProperty $task "command"
                if ($hasCommand -and $task.command -isnot [string]) { exit 0 }
                if ($tasks.Count -ge 128) {
                    if ($hasCommand) { $null = $utf8.GetByteCount($task.command) }
                    $truncated = $true
                    continue
                }
                $entry = @{ task_id = $task.id; reported_status = $task.status }
                if ($hasCommand) {
                    $command = Get-ClippedCommand $task.command
                    $entry.command = $command.value
                    $truncated = $truncated -or $command.truncated
                }
                $tasks.Add($entry)
            }
            $observation = @{
                event = "snapshot_observed"
                hook_event = $expectedEvent
                tasks = @($tasks.ToArray())
                truncated = $truncated
            }
        } else {
            if ($payload.hook_event_name -cne "PostToolUse" -or -not ($propertyNames -ccontains "tool_name") -or $payload.tool_name -isnot [string] -or $payload.tool_name -cne "Bash") { exit 0 }
            if (-not ($propertyNames -ccontains "tool_response") -or -not (Test-HookProperty $payload.tool_response "backgroundTaskId")) { exit 0 }
            if (-not (Test-HookText $payload.tool_response.backgroundTaskId)) { exit 0 }
            if (-not ($propertyNames -ccontains "tool_use_id") -or -not (Test-HookText $payload.tool_use_id)) { exit 0 }
            $observation = @{
                event = "bash_background_observed"
                task_id = $payload.tool_response.backgroundTaskId
                tool_use_id = $payload.tool_use_id
            }
            if (($propertyNames -ccontains "tool_input") -and (Test-HookProperty $payload.tool_input "command")) {
                if ($payload.tool_input.command -isnot [string]) { exit 0 }
                $command = Get-ClippedCommand $payload.tool_input.command
                $observation.command = $command.value
            }
        }
        if ($propertyNames -ccontains "agent_id") { $observation.hook_agent_id = $payload.agent_id }
        # ConvertFrom-Json can replace lone surrogate escapes before validation.
        if ([regex]::IsMatch($inputText, '\\u[dD][89a-fA-F]')) {
            $unicodeProbe = [regex]::Replace(
                $inputText, '\\(?:u([0-9a-fA-F]{4})|.)',
                [System.Text.RegularExpressions.MatchEvaluator]{
                    param($escape)
                    if ($escape.Groups[1].Success) {
                        return [string][char][Convert]::ToInt32($escape.Groups[1].Value, 16)
                    }
                    return ' '
                })
            $null = $utf8.GetByteCount($unicodeProbe)
        }
        Send-HookReport "background.shell.report" @{
            pane_id = $env:HERDR_PANE_ID
            source = "herdr:claude"
            session_id = $sessionId
            observation = $observation
        }
    } catch { }
    exit 0
}
if ($Action -cne "session") {
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
