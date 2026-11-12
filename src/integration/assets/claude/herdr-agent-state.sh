#!/bin/sh
# installed by herdr
# managed by herdr; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HERDR_INTEGRATION_ID=claude
# HERDR_INTEGRATION_VERSION=12

set -eu

action="${1:-}"
hook_input_file="$(mktemp "${TMPDIR:-/tmp}/herdr-claude-hook.XXXXXX")" || exit 0
trap 'rm -f "$hook_input_file"' EXIT HUP INT TERM
cat >"$hook_input_file" 2>/dev/null || true

case "$action" in
  session|subagent-start|subagent-stop) ;;
  *) exit 0 ;;
esac

[ "${HERDR_ENV:-}" = "1" ] || exit 0
[ -n "${HERDR_SOCKET_PATH:-}" ] || exit 0
[ -n "${HERDR_PANE_ID:-}" ] || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

HERDR_ACTION="$action" HERDR_HOOK_INPUT_FILE="$hook_input_file" python3 - <<'PY'
import json
import os
import random
import socket
import time

source = "herdr:claude"
action = os.environ.get("HERDR_ACTION", "")
pane_id = os.environ.get("HERDR_PANE_ID")
socket_path = os.environ.get("HERDR_SOCKET_PATH")
hook_input_file = os.environ.get("HERDR_HOOK_INPUT_FILE")

if not pane_id or not socket_path:
    raise SystemExit(0)

hook_input = {}
if hook_input_file:
    try:
        with open(hook_input_file, encoding="utf-8") as handle:
            content = handle.read()
        if content.strip():
            hook_input = json.loads(content)
    except Exception:
        hook_input = {}

if not isinstance(hook_input, dict):
    raise SystemExit(0)
if "CURSOR_VERSION" in os.environ or "cursor_version" in hook_input:
    raise SystemExit(0)
hook_event_name = hook_input.get("hook_event_name")
session_id = hook_input.get("session_id")
if not isinstance(session_id, str) or not session_id:
    raise SystemExit(0)
request_id = f"{source}:{int(time.time() * 1000)}:{random.randrange(1_000_000):06d}"
if action == "session":
    if hook_event_name != "SessionStart" or hook_input.get("agent_id"):
        raise SystemExit(0)
    params = {
        "pane_id": pane_id,
        "source": source,
        "agent": "claude",
        "seq": time.time_ns(),
        "agent_session_id": session_id,
    }
    transcript_path = hook_input.get("transcript_path")
    if isinstance(transcript_path, str) and transcript_path:
        params["agent_session_path"] = transcript_path
    session_start_source = hook_input.get("source")
    if isinstance(session_start_source, str) and session_start_source:
        params["session_start_source"] = session_start_source
    method = "pane.report_agent_session"
else:
    events = {
        "subagent-start": ("SubagentStart", "subagent_started"),
        "subagent-stop": ("SubagentStop", "subagent_stop_observed"),
    }
    expected_hook_event, event = events[action]
    task_id = hook_input.get("agent_id")
    if hook_event_name != expected_hook_event or not session_id.strip() or not isinstance(task_id, str) or not task_id.strip():
        raise SystemExit(0)
    params = {
        "pane_id": pane_id,
        "source": source,
        "session_id": session_id,
        "event": event,
        "task_id": task_id,
    }
    task_type = hook_input.get("agent_type")
    if isinstance(task_type, str) and task_type.strip():
        params["task_type"] = task_type
    method = "background.report"
request = {"id": request_id, "method": method, "params": params}

try:
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(0.5)
    client.connect(socket_path)
    client.sendall((json.dumps(request) + "\n").encode())
    try:
        client.recv(4096)
    except Exception:
        pass
    client.close()
except Exception:
    pass
PY
