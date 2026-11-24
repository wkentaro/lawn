---
name: lawn
description: Manage Lawn workspaces, tabs, pane placement, and parking over herdr-core. Use hc for terminal content and input.
---

# Lawn

Lawn owns layouts and parking. herdr-core owns terminals. Lawn commands return hc terminal addresses such as `t3`, not the old Lawn pane IDs.

## Select the layout

The rewrite is opt-in. Prefix the Lawn commands below with `LAWN_REWRITE=1`.
Use `lawn --session NAME ...` for a named layout session. Without it, Lawn uses the inherited layout session or `default`.

Keep the inherited `HC_SESSION` and `HC_SOCKET`. A Lawn layout session does not select a different core session.

## Inspect first

```sh
LAWN_REWRITE=1 lawn ls --json
```

The result contains the session name and layout, including workspace IDs, tab IDs, terminal addresses, the parked terminals, and any pending operations. A parked terminal keeps its pane in the tab tree, so tree walks must skip addresses listed under `parked`. Listing also attempts recovery of recorded starts and closes.

For terminal output, input, process status, or child terminals, read `hc --skill` and use hc. Lawn's listing describes its layout, not every terminal in the core.

## Change the layout

These examples use addresses and IDs from the listing. Replace them with the actual values.

```sh
LAWN_REWRITE=1 lawn workspace create --name work --cwd /path/to/repo -- sh
LAWN_REWRITE=1 lawn tab create --workspace ws0 --name logs -- tail -f app.log
LAWN_REWRITE=1 lawn pane split t3 --down --ratio 0.4 -- sh
LAWN_REWRITE=1 lawn park t3
LAWN_REWRITE=1 lawn unpark t3
LAWN_REWRITE=1 lawn unpark t3 --beside t1 --down
LAWN_REWRITE=1 lawn unpark t3 --tab --workspace ws0
LAWN_REWRITE=1 lawn pane close t3
```

Creation without a program starts the core's default shell. `--cwd` defaults to the command's current directory. Tab creation without `--workspace` uses the first workspace.

Parking hides the pane and clears the terminal's keep flag. Its workspace and tab stay, even when it was their last visible pane. The program continues to run until it exits, but a parked job does not survive a machine restart. Unparking without a destination shows the pane where it was; a destination moves it there. A terminal shown in another layout session must be parked there first.

Closing ends the terminal's program. Use it only when the user wants that program stopped.

## Record a resume command

Hook integrations can pipe a JSON report to `LAWN_REWRITE=1 lawn record-resume TERMINAL`. Read `LAWN_REWRITE=1 lawn record-resume --help` for its fields and limits.

Reports belong to the core terminal, not a layout session. Each agent invocation uses a fresh `reporter_id` and increasing `seq`. Records preserve argv and cwd as data. The argv must start with a plain command name and contain no apostrophes. Run the recorder inside the terminal it reports on: Lawn stores the report with that terminal's root program only then, and otherwise answers `ignored` without storing it. Only Linux and macOS record reports; elsewhere the recorder answers `ignored`. A program sent in the report is ignored. Recording does not execute the reported command. Once that root program is gone and the core has given the terminal a new shell, the Lawn TUI types the last stored such report into it once. A report sent by a job of the root shell is forgotten when a Lawn TUI sees the root shell back in the foreground, because that agent quit: at once while a TUI is attached, otherwise when one next starts. An agent quit while no TUI is attached is still resumed if the root program is lost before a TUI starts. Lawn's Claude and Codex hooks record their sessions this way on their own.

## Draw one terminal

On Unix, `LAWN_REWRITE=1 lawn attach t3` draws that terminal without a Lawn server. Ctrl+\ detaches and leaves the program running. This command requires a terminal.

## Handle uncertain results

A timed-out or disconnected start may have created a terminal. Run `lawn ls --json` with the same rewrite flag and layout session. If the start remains pending, stop and report it. Repeated creation is not a safe way to retry the same start.

A pending close means termination is not yet confirmed. Listing retries it. Check the terminal's current state through hc rather than assuming that a request to stop has finished.

Layout commands do not yet support remote-qualified terminal addresses.
