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
head -c 1048577 >"$hook_input_file" 2>/dev/null || true

case "$action" in
  session|subagent-start|subagent-stop|stop|bash-background) ;;
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
import unicodedata

source = "herdr:claude"
action = os.environ.get("HERDR_ACTION", "")
pane_id = os.environ.get("HERDR_PANE_ID")
socket_path = os.environ.get("HERDR_SOCKET_PATH")
hook_input_file = os.environ.get("HERDR_HOOK_INPUT_FILE")

if not pane_id or not socket_path:
    raise SystemExit(0)

try:
    with open(hook_input_file, "rb") as handle:
        content = handle.read(1048577)
    if len(content) > 1048576:
        raise SystemExit(0)
    hook_input = json.loads(content.decode("utf-8"))
except (OSError, ValueError, RecursionError):
    raise SystemExit(0)


def valid_text(value, limit=256):
    if not isinstance(value, str) or not value.strip():
        return False
    try:
        return len(value.encode("utf-8")) <= limit and not any(
            unicodedata.category(char) == "Cc" for char in value
        )
    except UnicodeError:
        return False


def clipped_command(value):
    data = value.encode("utf-8")
    if len(data) <= 4096:
        return value, False
    # Bash observations have no truncation flag, so disclose clipping in the text.
    marker = "\n[truncated]"
    return data[:4096 - len(marker)].decode("utf-8", "ignore") + marker, True


def send(method, params):
    request = {
        "id": f"{source}:{int(time.time() * 1000)}:{random.randrange(1_000_000):06d}",
        "method": method,
        "params": params,
    }
    try:
        while True:
            line = (json.dumps(request, ensure_ascii=False, separators=(",", ":")) + "\n").encode("utf-8")
            if len(line) <= 1048576 or method != "background.shell.report":
                break
            observation = params["observation"]
            if observation["event"] != "snapshot_observed" or not observation["tasks"]:
                return
            observation["tasks"].pop()
            observation["truncated"] = True
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(0.5)
            client.connect(socket_path)
            client.sendall(line)
            try:
                client.recv(4096)
            except Exception:
                pass
    except Exception:
        pass

if not isinstance(hook_input, dict):
    raise SystemExit(0)
if "CURSOR_VERSION" in os.environ or "cursor_version" in hook_input:
    raise SystemExit(0)
hook_event_name = hook_input.get("hook_event_name")
session_id = hook_input.get("session_id")
if not valid_text(session_id) or not valid_text(pane_id):
    raise SystemExit(0)
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
    send("pane.report_agent_session", params)
elif action in ("subagent-start", "subagent-stop"):
    events = {
        "subagent-start": ("SubagentStart", "subagent_started"),
        "subagent-stop": ("SubagentStop", "subagent_stop_observed"),
    }
    expected_hook_event, event = events[action]
    task_id = hook_input.get("agent_id")
    if hook_event_name != expected_hook_event or not valid_text(task_id):
        raise SystemExit(0)
    params = {
        "pane_id": pane_id,
        "source": source,
        "session_id": session_id,
        "event": event,
        "task_id": task_id,
    }
    task_type = hook_input.get("agent_type")
    if valid_text(task_type):
        params["task_type"] = task_type
    send("background.report", params)

# Optional shell data must never suppress the lifecycle report above.
try:
    observation = None
    reporter = hook_input.get("agent_id")
    if "agent_id" in hook_input and not valid_text(reporter):
        raise SystemExit(0)
    if action in ("stop", "subagent-stop"):
        expected = "Stop" if action == "stop" else "SubagentStop"
        if hook_event_name != expected or "background_tasks" not in hook_input:
            raise SystemExit(0)
        native_tasks = hook_input["background_tasks"]
        if not isinstance(native_tasks, list):
            raise SystemExit(0)
        tasks = []
        truncated = False
        for task in native_tasks:
            if not isinstance(task, dict) or not isinstance(task.get("type"), str):
                raise SystemExit(0)
            if task["type"] != "shell":
                continue
            if not valid_text(task.get("id")) or not valid_text(task.get("status"), 128):
                raise SystemExit(0)
            if "command" in task and not isinstance(task["command"], str):
                raise SystemExit(0)
            if len(tasks) >= 128:
                if "command" in task:
                    task["command"].encode("utf-8")
                truncated = True
                continue
            entry = {"task_id": task["id"], "reported_status": task["status"]}
            if "command" in task:
                entry["command"], clipped = clipped_command(task["command"])
                truncated |= clipped
            tasks.append(entry)
        observation = {
            "event": "snapshot_observed",
            "hook_event": expected,
            "tasks": tasks,
            "truncated": truncated,
        }
    elif action == "bash-background":
        if hook_event_name != "PostToolUse" or hook_input.get("tool_name") != "Bash":
            raise SystemExit(0)
        response = hook_input.get("tool_response")
        if not isinstance(response, dict) or not valid_text(response.get("backgroundTaskId")):
            raise SystemExit(0)
        if not valid_text(hook_input.get("tool_use_id")):
            raise SystemExit(0)
        observation = {
            "event": "bash_background_observed",
            "task_id": response["backgroundTaskId"],
            "tool_use_id": hook_input["tool_use_id"],
        }
        tool_input = hook_input.get("tool_input")
        if isinstance(tool_input, dict) and "command" in tool_input:
            if not isinstance(tool_input["command"], str):
                raise SystemExit(0)
            observation["command"], _ = clipped_command(tool_input["command"])
    if observation is not None:
        if reporter is not None:
            observation["hook_agent_id"] = reporter
        send("background.shell.report", {
            "pane_id": pane_id,
            "source": source,
            "session_id": session_id,
            "observation": observation,
        })
except (UnicodeError, ValueError):
    pass
PY
