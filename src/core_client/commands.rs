use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::core_lifecycle::Lifecycle;
use crate::hc::{located, qualified, Core};
use crate::session_doc::{Created, Destination, Direction, Layout, Placement, Store};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Program {
    pub(super) cwd: Option<PathBuf>,
    pub(super) argv: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Command {
    Ls {
        json: bool,
    },
    WorkspaceCreate {
        name: Option<String>,
        on: Option<String>,
        program: Program,
    },
    TabCreate {
        workspace: Option<String>,
        name: Option<String>,
        program: Program,
    },
    PaneSplit {
        beside: String,
        direction: Direction,
        ratio: f32,
        program: Program,
    },
    PaneClose {
        terminal: String,
    },
    Park {
        terminal: String,
    },
    Unpark {
        terminal: String,
        to: Option<Placement>,
    },
    StartDrop {
        reference: String,
    },
}

fn lifecycle(
    core: &Arc<Core>,
    session: &str,
    machine: Option<&str>,
    id: Option<&str>,
) -> Lifecycle {
    recovered(core, session, id, report_recovery(session, machine))
}

fn there(machine: Option<&str>) -> impl Fn(String) -> String + '_ {
    move |err| match machine {
        Some(machine) => format!("on {machine}: {err}"),
        None => err,
    }
}

fn report_recovery<'a>(session: &'a str, machine: Option<&'a str>) -> impl FnOnce(String) + 'a {
    move |err| {
        let err = there(machine)(err);
        eprintln!("warning: recovery in session {session}: {err}")
    }
}

fn recovered(
    core: &Arc<Core>,
    session: &str,
    id: Option<&str>,
    report: impl FnOnce(String),
) -> Lifecycle {
    let lifecycle = Lifecycle::on(core.clone(), session, id);
    if let Err(err) = lifecycle.recover() {
        report(err);
    }
    lifecycle
}

pub(super) fn creation_lifecycle(
    core: &Arc<Core>,
    session: &str,
    machine: Option<&str>,
    id: Option<&str>,
    report: impl FnOnce(String),
) -> Result<Lifecycle, String> {
    let pending = Store::new(core.clone(), session, id.map(str::to_owned))
        .load()?
        .layout
        .pending_starts;
    let lifecycle = recovered(core, session, id, report);
    if let Some(reference) = pending.keys().next() {
        let reference = qualified(machine, reference);
        return Err(format!("start {reference} was already pending; recovery was attempted, but no new program was started; inspect `lawn --session {session} ls`, or give it up with `lawn --session {session} start drop {reference}`"));
    }
    Ok(lifecycle)
}

pub(super) fn ls(core: &Arc<Core>, session: &str) -> Result<Layout, String> {
    let mut layout = lifecycle(core, session, None, None).layout()?;
    let cores = core.cores().unwrap_or_else(|error| {
        eprintln!("warning: linked machines are not listed: {error}");
        Vec::new()
    });
    let machines = cores.split_last().map_or(&[][..], |(_, machines)| machines);
    for machine in machines {
        let name = &machine.name;
        if machine.state != "connected" {
            let state = &machine.state;
            eprintln!("warning: {name} is {state}; its workspaces are not listed");
            continue;
        }
        let listed = crate::hc::core_id(&cores, name)
            .and_then(|id| lifecycle(core, session, Some(name), Some(id)).layout());
        match listed {
            Ok(far) => layout.join(far.qualified(name)),
            Err(error) => eprintln!("warning: the workspaces of {name} are not listed: {error}"),
        }
    }
    Ok(layout)
}

pub(super) fn ls_json(session: &str, layout: &Layout) -> Value {
    json!({"session": session, "layout": layout})
}

pub(super) fn ls_text(layout: &Layout) -> String {
    let mut text = String::new();
    let titled = |id: &str, name: &Option<String>| match name {
        Some(name) => format!("{id} {name}"),
        None => id.to_owned(),
    };
    for workspace in &layout.workspaces {
        text += &format!("{}\n", titled(&workspace.id, &workspace.name));
        for tab in &workspace.tabs {
            text += &format!("  {}\n", titled(&tab.id, &tab.name));
            for terminal in tab.root.terminals() {
                if layout.parked.contains(terminal) {
                    continue;
                }
                text += &format!("    {terminal}\n");
            }
        }
    }
    if !layout.parked.is_empty() {
        text += "parked:\n";
        for terminal in &layout.parked {
            text += &format!("  {terminal}\n");
        }
    }
    if !layout.pending_starts.is_empty() {
        text += "unconfirmed starts:\n";
        for reference in layout.pending_starts.keys() {
            text += &format!("  {reference}\n");
        }
    }
    if !layout.closing.is_empty() {
        text += "closing:\n";
        for terminal in &layout.closing {
            text += &format!("  {terminal}\n");
        }
    }
    text
}

type Reached<'a> = (Option<&'a str>, Option<String>, &'a str);

fn reached<'a>(core: &Core, typed: &'a str) -> Result<Reached<'a>, String> {
    let (machine, local) = located(typed)?;
    Ok((machine, core_id(core, machine)?, local))
}

pub(super) fn core_id(core: &Core, machine: Option<&str>) -> Result<Option<String>, String> {
    let Some(typed) = machine else {
        return Ok(None);
    };
    let cores = core.cores().map_err(|err| err.to_string())?;
    match crate::hc::core_id(&cores, typed) {
        Ok(id) => Ok(Some(id.to_owned())),
        Err(err) => Err(format!("{err}; nothing was changed")),
    }
}

#[cfg(unix)]
pub(super) fn core_address(core: &Core, typed: &str) -> Result<String, String> {
    let (_, id, local) = reached(core, typed)?;
    Ok(qualified(id.as_deref(), local))
}

pub(super) fn reach<'a>(
    core: &Arc<Core>,
    session: &str,
    typed: &'a str,
) -> Result<(Lifecycle, Option<&'a str>, &'a str), String> {
    let (machine, id, local) = reached(core, typed)?;
    let lifecycle = Lifecycle::on(core.clone(), session, id.as_deref());
    Ok((lifecycle, machine, local))
}

pub(super) fn located_start(
    mut destination: Destination,
) -> Result<(Option<String>, Destination), String> {
    let anchor = match &mut destination {
        Destination::Tab {
            workspace: Some(anchor),
            ..
        }
        | Destination::Split { beside: anchor, .. } => anchor,
        _ => return Ok((None, destination)),
    };
    let (machine, local) = located(anchor)?;
    let machine = machine.map(str::to_owned);
    *anchor = local.to_owned();
    Ok((machine, destination))
}

fn start(
    core: &Arc<Core>,
    session: &str,
    on: Option<&str>,
    destination: Destination,
    program: &Program,
) -> Result<Value, String> {
    let (machine, destination) = located_start(destination)?;
    let machine = machine.as_deref().or(on);
    let id = core_id(core, machine)?;
    let report = report_recovery(session, machine);
    let lifecycle = creation_lifecycle(core, session, machine, id.as_deref(), report)?;
    let created = lifecycle
        .start(destination, program.cwd.as_deref(), &program.argv)
        .map_err(there(machine))?;
    let far = |local: String| qualified(machine, &local);
    serde_json::to_value(Created {
        terminal: far(created.terminal),
        workspace: created.workspace.map(far),
        tab: created.tab.map(far),
    })
    .map_err(|err| err.to_string())
}

pub(super) fn workspace_create(
    core: &Arc<Core>,
    session: &str,
    name: Option<String>,
    on: Option<&str>,
    program: &Program,
) -> Result<Value, String> {
    start(core, session, on, Destination::Workspace { name }, program)
}

pub(super) fn tab_create(
    core: &Arc<Core>,
    session: &str,
    workspace: Option<&str>,
    name: Option<String>,
    program: &Program,
) -> Result<Value, String> {
    let workspace = workspace.map(str::to_owned);
    let destination = Destination::Tab { workspace, name };
    start(core, session, None, destination, program)
}

pub(super) fn pane_split(
    core: &Arc<Core>,
    session: &str,
    beside: &str,
    direction: Direction,
    ratio: f32,
    program: &Program,
) -> Result<Value, String> {
    let destination = Destination::Split {
        beside: beside.to_owned(),
        direction,
        ratio,
    };
    start(core, session, None, destination, program)
}

pub(super) fn pane_close(core: &Arc<Core>, session: &str, terminal: &str) -> Result<Value, String> {
    let (lifecycle, machine, local) = reach(core, session, terminal)?;
    lifecycle.close(local).map_err(there(machine))?;
    if let Err(err) = lifecycle.recover() {
        report_recovery(session, machine)(err);
    }
    Ok(json!({"terminal": terminal}))
}

pub(super) fn park(core: &Arc<Core>, session: &str, terminal: &str) -> Result<Value, String> {
    let (machine, id, local) = reached(core, terminal)?;
    lifecycle(core, session, machine, id.as_deref())
        .park(local)
        .map_err(there(machine))?;
    Ok(json!({"terminal": terminal}))
}

fn placed(machine: Option<&str>, to: Option<Placement>) -> Result<Option<Placement>, String> {
    let near = |typed: &str| match located(typed)? {
        (on, local) if on == machine => Ok(local.to_owned()),
        _ => Err(format!("{typed} is on another machine than the terminal")),
    };
    Ok(match to {
        Some(Placement::Beside {
            terminal,
            direction,
        }) => Some(Placement::Beside {
            terminal: near(&terminal)?,
            direction,
        }),
        Some(Placement::NewTab {
            workspace: Some(workspace),
        }) => Some(Placement::NewTab {
            workspace: Some(near(&workspace)?),
        }),
        to => to,
    })
}

pub(super) fn unpark(
    core: &Arc<Core>,
    session: &str,
    terminal: &str,
    to: Option<Placement>,
) -> Result<Value, String> {
    let (machine, id, local) = reached(core, terminal)?;
    let to = placed(machine, to)?;
    let terminal = lifecycle(core, session, machine, id.as_deref())
        .unpark(local, to)
        .map_err(there(machine))?;
    Ok(json!({"terminal": qualified(machine, &terminal)}))
}

pub(super) fn start_drop(
    core: &Arc<Core>,
    session: &str,
    reference: &str,
) -> Result<Value, String> {
    let (machine, id, local) = reached(core, reference)?;
    Lifecycle::on(core.clone(), session, id.as_deref())
        .drop_start(local)
        .map_err(there(machine))?;
    Ok(json!({"dropped": reference}))
}

pub(super) fn run(core: &Arc<Core>, session: &str, command: Command) -> Result<Value, String> {
    match command {
        Command::Ls { .. } => Ok(ls_json(session, &ls(core, session)?)),
        Command::WorkspaceCreate { name, on, program } => {
            workspace_create(core, session, name, on.as_deref(), &program)
        }
        Command::TabCreate {
            workspace,
            name,
            program,
        } => tab_create(core, session, workspace.as_deref(), name, &program),
        Command::PaneSplit {
            beside,
            direction,
            ratio,
            program,
        } => pane_split(core, session, &beside, direction, ratio, &program),
        Command::PaneClose { terminal } => pane_close(core, session, &terminal),
        Command::Park { terminal } => park(core, session, &terminal),
        Command::Unpark { terminal, to } => unpark(core, session, &terminal, to),
        Command::StartDrop { reference } => start_drop(core, session, &reference),
    }
}
