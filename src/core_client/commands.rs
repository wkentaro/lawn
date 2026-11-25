use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::core_lifecycle::Lifecycle;
use crate::hc::Core;
use crate::session_doc::{Destination, Direction, Layout, Placement, Store};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Program {
    pub(super) cwd: PathBuf,
    pub(super) argv: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Command {
    Ls {
        json: bool,
    },
    WorkspaceCreate {
        name: Option<String>,
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

fn lifecycle(core: &Arc<Core>, session: &str) -> Lifecycle {
    recovered(core, session, report_recovery(session))
}

fn report_recovery(session: &str) -> impl FnOnce(String) + '_ {
    move |err| eprintln!("warning: recovery in session {session}: {err}")
}

fn recovered(core: &Arc<Core>, session: &str, report: impl FnOnce(String)) -> Lifecycle {
    let lifecycle = Lifecycle::new(core.clone(), session);
    if let Err(err) = lifecycle.recover() {
        report(err);
    }
    lifecycle
}

pub(super) fn creation_lifecycle(
    core: &Arc<Core>,
    session: &str,
    report: impl FnOnce(String),
) -> Result<Lifecycle, String> {
    let pending = Store::new(core.clone(), session, None)
        .load()?
        .layout
        .pending_starts;
    let lifecycle = recovered(core, session, report);
    if let Some(reference) = pending.keys().next() {
        return Err(format!("start {reference} was already pending; recovery was attempted, but no new program was started; inspect `lawn --session {session} ls`, or give it up with `lawn --session {session} start drop {reference}`"));
    }
    Ok(lifecycle)
}

pub(super) fn ls(core: &Arc<Core>, session: &str) -> Result<Layout, String> {
    lifecycle(core, session);
    Ok(Store::new(core.clone(), session, None).load()?.layout)
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

pub(super) fn workspace_create(
    core: &Arc<Core>,
    session: &str,
    name: Option<String>,
    program: &Program,
) -> Result<Value, String> {
    let created = creation_lifecycle(core, session, report_recovery(session))?.start(
        Destination::Workspace { name },
        &program.cwd,
        &program.argv,
    )?;
    serde_json::to_value(created).map_err(|err| err.to_string())
}

pub(super) fn tab_create(
    core: &Arc<Core>,
    session: &str,
    workspace: Option<&str>,
    name: Option<String>,
    program: &Program,
) -> Result<Value, String> {
    let created = creation_lifecycle(core, session, report_recovery(session))?.start(
        Destination::Tab {
            workspace: workspace.map(str::to_owned),
            name,
        },
        &program.cwd,
        &program.argv,
    )?;
    serde_json::to_value(created).map_err(|err| err.to_string())
}

pub(super) fn pane_split(
    core: &Arc<Core>,
    session: &str,
    beside: &str,
    direction: Direction,
    ratio: f32,
    program: &Program,
) -> Result<Value, String> {
    let created = creation_lifecycle(core, session, report_recovery(session))?.start(
        Destination::Split {
            beside: beside.to_owned(),
            direction,
            ratio,
        },
        &program.cwd,
        &program.argv,
    )?;
    serde_json::to_value(created).map_err(|err| err.to_string())
}

pub(super) fn pane_close(core: &Arc<Core>, session: &str, terminal: &str) -> Result<Value, String> {
    let lifecycle = Lifecycle::new(core.clone(), session);
    lifecycle.close(terminal)?;
    if let Err(err) = lifecycle.recover() {
        eprintln!("warning: recovery in session {session}: {err}");
    }
    Ok(json!({"terminal": terminal}))
}

pub(super) fn park(core: &Arc<Core>, session: &str, terminal: &str) -> Result<Value, String> {
    lifecycle(core, session).park(terminal)?;
    Ok(json!({"terminal": terminal}))
}

pub(super) fn unpark(
    core: &Arc<Core>,
    session: &str,
    terminal: &str,
    to: Option<Placement>,
) -> Result<Value, String> {
    if terminal.contains('/') {
        return Err("remote terminals are not supported by this rewrite build yet".to_owned());
    }
    let terminal = lifecycle(core, session).unpark(terminal, to)?;
    Ok(json!({"terminal": terminal}))
}

pub(super) fn start_drop(
    core: &Arc<Core>,
    session: &str,
    reference: &str,
) -> Result<Value, String> {
    Lifecycle::new(core.clone(), session).drop_start(reference)?;
    Ok(json!({"dropped": reference}))
}

pub(super) fn run(core: &Arc<Core>, session: &str, command: Command) -> Result<Value, String> {
    match command {
        Command::Ls { .. } => Ok(ls_json(session, &ls(core, session)?)),
        Command::WorkspaceCreate { name, program } => {
            workspace_create(core, session, name, &program)
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
