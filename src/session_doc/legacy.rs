use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::Value;

use super::edit::EVEN_RATIO;
use super::{Direction, Layout, Node, PendingStart, Sessions, Tab, Workspace};

#[derive(Deserialize)]
struct PruningLayout {
    #[serde(default)]
    workspaces: Vec<Workspace>,
    #[serde(default)]
    parked: BTreeMap<String, ReturnPosition>,
    #[serde(default)]
    next_id: u64,
    #[serde(default)]
    closing: BTreeSet<String>,
    #[serde(default)]
    pending_starts: BTreeMap<String, PendingStart>,
}

#[derive(Deserialize)]
struct ReturnPosition {
    workspace: String,
    tab: String,
    #[serde(default)]
    beside: Option<String>,
    direction: Direction,
}

pub(super) fn migrate(sessions: Value) -> Result<Sessions, String> {
    let sessions: BTreeMap<String, PruningLayout> =
        serde_json::from_value(sessions).map_err(|err| err.to_string())?;
    sessions
        .into_iter()
        .map(|(session, layout)| {
            let layout = restore(layout).map_err(|err| format!("session {session}: {err}"))?;
            Ok((session, layout))
        })
        .collect()
}

fn restore(pruning: PruningLayout) -> Result<Layout, String> {
    let mut layout = Layout {
        workspaces: pruning.workspaces,
        parked: BTreeSet::new(),
        next_id: pruning.next_id,
        closing: pruning.closing,
        pending_starts: pruning.pending_starts,
    };
    let mut records: Vec<_> = pruning.parked.into_iter().collect();
    while !records.is_empty() {
        let waiting = |beside: &str| records.iter().any(|(terminal, _)| terminal == beside);
        let next = records
            .iter()
            .position(|(_, position)| position.beside.as_deref().is_some_and(|b| layout.owns(b)))
            .or_else(|| {
                records
                    .iter()
                    .position(|(_, position)| !position.beside.as_deref().is_some_and(waiting))
            })
            .unwrap_or(0);
        let (terminal, position) = records.remove(next);
        put_back(&mut layout, terminal, position)?;
    }
    Ok(layout)
}

fn put_back(layout: &mut Layout, terminal: String, position: ReturnPosition) -> Result<(), String> {
    if layout.owns(&terminal) {
        return Err(format!("{terminal} is both shown and parked"));
    }
    if let Some(other) = layout.workspaces.iter().find(|workspace| {
        workspace.id != position.workspace && workspace.tabs.iter().any(|t| t.id == position.tab)
    }) {
        return Err(format!(
            "{terminal} returns to {} in {}, but {} holds it",
            position.tab, position.workspace, other.id
        ));
    }
    layout.parked.insert(terminal.clone());
    let index = match layout
        .workspaces
        .iter()
        .position(|workspace| workspace.id == position.workspace)
    {
        Some(index) => index,
        None => {
            layout.workspaces.push(Workspace {
                id: position.workspace,
                name: None,
                tabs: Vec::new(),
            });
            layout.workspaces.len() - 1
        }
    };
    let tabs = &mut layout.workspaces[index].tabs;
    let Some(tab) = tabs.iter().find(|tab| tab.id == position.tab) else {
        tabs.push(Tab {
            id: position.tab,
            name: None,
            root: Node::Pane { terminal },
        });
        return Ok(());
    };
    let beside = position
        .beside
        .filter(|beside| tab.root.contains(beside))
        .unwrap_or_else(|| tab.root.last_terminal().to_owned());
    layout.insert_beside(&beside, position.direction, EVEN_RATIO, terminal);
    Ok(())
}
