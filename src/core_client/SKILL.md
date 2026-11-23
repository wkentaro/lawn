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

The listing includes the workspaces of every connected machine linked to the core. Their IDs, addresses and pending starts carry the machine's name, such as `boxb/ws0` and `boxb/t3`. A machine that is not connected, or whose name is another machine's core id, is left out, and a warning on standard error names it. Every command refuses that name or id until `hc machine rename` gives one of the two machines another name.

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

Creation without a program starts the core's default shell. `--cwd` defaults to the command's current directory. Tab creation without `--workspace` uses the first workspace on this machine.

`workspace create --on MACHINE` starts the program on a machine linked to the core; `hc machine ls` lists them. A workspace belongs to one machine. A tab or split in it starts on that machine, and its layout is saved there, so every core linked to that machine lists the same workspaces. With `--on`, or a workspace or pane on another machine, `--cwd` names a folder on that machine; without `--cwd` the program starts in the folder that machine's core runs in. An address more than one machine away, such as `boxb/boxc/t3`, is refused.

Parking hides the pane and clears the terminal's keep flag. Its workspace and tab stay, even when it was their last visible pane. The program continues to run until it exits, but a parked job does not survive a machine restart. Unparking without a destination shows the pane where it was; a destination moves it there, and must be on the terminal's own machine. A terminal shown in another layout session must be parked there first.

Closing ends the terminal's program. Use it only when the user wants that program stopped.

## Record a resume command

Hook integrations can pipe a JSON report to `LAWN_REWRITE=1 lawn record-resume TERMINAL`. Read `LAWN_REWRITE=1 lawn record-resume --help` for its fields and limits.

Reports belong to the core terminal, not a layout session. Each agent invocation uses a fresh `reporter_id` and increasing `seq`. Records preserve argv and cwd as data. The argv must start with a plain command name and contain no apostrophes. Run the recorder inside the terminal it reports on: Lawn stores the report with that terminal's root program only then, and otherwise answers `ignored` without storing it. Only Linux and macOS record reports; elsewhere the recorder answers `ignored`. A program sent in the report is ignored. Recording does not execute the reported command. Existing hooks are not connected to this recorder yet.

## Draw one terminal

On Unix, `LAWN_REWRITE=1 lawn attach t3` draws that terminal without a Lawn server. Ctrl+\ detaches and leaves the program running. This command requires a terminal.

## Handle uncertain results

A timed-out or disconnected start may have created a terminal. Run `lawn ls --json` with the same rewrite flag and layout session. If the start remains pending, stop and report it. Repeated creation is not a safe way to retry the same start. A pending start refuses every later start in its layout session on its machine. Once the user confirms that nothing runs under it, `lawn start drop <reference>` gives it up, with the reference `ls` prints under `unconfirmed starts`. The drop looks the start up first. A terminal that already exists is handled as `ls` would handle it and the drop is refused: it is placed, or queued for closing when its place is gone, because the pane it was to split is no longer shown or the workspace its tab was for is closed. A lookup that fails leaves the start pending. A terminal that appears after the drop stays outside the layout until it is unparked. A start on a linked machine is pending in that machine's layout, so the machine must be connected before the start can be listed, recovered or dropped.

A pending close means termination is not yet confirmed. Listing retries it. Check the terminal's current state through hc rather than assuming that a request to stop has finished.
