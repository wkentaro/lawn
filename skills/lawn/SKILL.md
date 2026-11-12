---
name: lawn
description: Inspect or control Lawn terminal panes, tabs, workspaces, and agents when the user asks to use Lawn. Use the inherited session context to target the correct runtime.
---

# Lawn

Lawn uses herdr's runtime protocol. Managed panes retain the `HERDR_ENV`,
`HERDR_SOCKET_PATH`, `HERDR_CLIENT_SOCKET_PATH`, and pane identity variables.

Before controlling a session, verify `HERDR_ENV=1` and inspect the inherited
session context. Keep that context when invoking `lawn`; clearing it can target
a different session. Outside a managed pane, ask the user to identify the target
session before issuing control commands.

Use `lawn --help` and the relevant group's `--help` as the syntax authority.
Bare `lawn` launches the TUI. A mutating command with omitted arguments may
execute with defaults, so use `--help` for discovery.

Read live IDs from `lawn workspace list`, `lawn tab list`, `lawn pane list`, or
`lawn agent list`. IDs are scoped to a server. Preserve the selected server and
session for subsequent commands, and prefer `--current` when targeting the
calling pane. Inspect a pane before sending input because its occupant may have
changed. Agent idle status does not prove that a task has completed.

Use pane commands for ordinary terminal processes and agent commands for a
recognized coding agent. Create, split, close, move, or send input only within
the user's requested scope. A failed request may already have taken effect;
inspect live state before retrying a mutation.

Parking is Lawn presentation state. Runtime pane lists may include panes hidden
by the TUI. Closing a pane terminates its terminal and is not a substitute for
parking it.
