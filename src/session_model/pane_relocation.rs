use std::path::PathBuf;

use ratatui::layout::Direction;

use crate::layout::PaneId;
use crate::workspace::{MovedPane, Workspace};

pub(crate) enum PaneDestination {
    ExistingTab {
        tab_id: String,
        target_pane_id: PaneId,
        direction: Direction,
        ratio: f32,
    },
    NewTab {
        workspace_id: String,
        label: Option<String>,
    },
    NewWorkspace {
        label: Option<String>,
        tab_label: Option<String>,
        cwd: PathBuf,
    },
}

pub(crate) struct PaneRelocation {
    pub workspace_index: usize,
    pub tab_index: usize,
    pub pane_id: PaneId,
    pub removed_tab: bool,
    pub removed_workspace: bool,
    pub created_workspace: bool,
    pub created_tab: bool,
    pub cross_workspace: bool,
    pub active: Option<usize>,
    pub selected: usize,
}

#[derive(Debug)]
pub(crate) struct PaneRelocationError {
    pub message: &'static str,
    pub active: Option<usize>,
    pub selected: usize,
    pub preserve_previous_alias: bool,
    pub recovered: bool,
}

struct RecoveryContext {
    source_workspace: usize,
    workspace_id: String,
    workspace_label: Option<String>,
    tab_label: Option<String>,
    cwd: PathBuf,
}

pub(crate) fn relocate_pane(
    workspaces: &mut Vec<Workspace>,
    source: (usize, PaneId),
    destination: PaneDestination,
    focus: bool,
    mut active: Option<usize>,
    mut selected: usize,
) -> Result<PaneRelocation, PaneRelocationError> {
    let (source_workspace, pane_id) = source;
    let rejected = || PaneRelocationError {
        message: "source pane could not be moved",
        active,
        selected,
        preserve_previous_alias: false,
        recovered: false,
    };
    let workspace = workspaces.get(source_workspace).ok_or_else(rejected)?;
    let source_tab = workspace
        .find_tab_index_for_pane(pane_id)
        .ok_or_else(rejected)?;
    let recovery = RecoveryContext {
        source_workspace,
        workspace_id: workspace.id.clone(),
        workspace_label: workspace.custom_name.clone(),
        tab_label: workspace.tabs[source_tab].custom_name.clone(),
        cwd: workspace.identity_cwd.clone(),
    };
    let cross_workspace = match &destination {
        PaneDestination::ExistingTab { tab_id, .. } => super::tab_index(workspaces, tab_id)
            .is_none_or(|(workspace, _)| workspace != source_workspace),
        PaneDestination::NewTab { workspace_id, .. } => workspace_id != &workspace.id,
        PaneDestination::NewWorkspace { .. } => true,
    };
    let taken = workspaces[source_workspace]
        .take_pane_for_move(pane_id)
        .ok_or_else(rejected)?;
    let removed_tab = taken.removed_tab_idx.is_some();
    let removed_workspace = taken.workspace_empty && cross_workspace;
    if cross_workspace {
        workspaces[source_workspace].unregister_moved_pane(pane_id);
    }
    if removed_workspace {
        (active, selected) =
            super::remove_empty_workspace(workspaces, source_workspace, None, active, selected);
    }
    let mut created_workspace = false;
    let mut created_tab = false;
    let target = match destination {
        PaneDestination::ExistingTab {
            tab_id,
            target_pane_id,
            direction,
            ratio,
        } => match super::tab_index(workspaces, &tab_id) {
            Some((workspace, tab)) => {
                match workspaces[workspace].insert_moved_pane_into_tab(
                    tab,
                    target_pane_id,
                    taken.moved,
                    direction,
                    ratio,
                    focus,
                ) {
                    Ok(pane) => Ok((workspace, tab, pane)),
                    Err(moved) => Err(("target pane could not be split", moved)),
                }
            }
            None => Err(("target tab disappeared", taken.moved)),
        },
        PaneDestination::NewTab {
            workspace_id,
            label,
        } => match super::workspace_index(workspaces, &workspace_id) {
            Some(workspace) => {
                let tab = workspaces[workspace].create_tab_from_existing_pane(taken.moved, label);
                created_tab = true;
                Ok((workspace, tab, pane_id))
            }
            None => Err(("target workspace disappeared", taken.moved)),
        },
        PaneDestination::NewWorkspace {
            label,
            tab_label,
            cwd,
        } => {
            let workspace = Workspace::from_existing_pane(label, tab_label, cwd, taken.moved);
            workspaces.push(workspace);
            created_workspace = true;
            created_tab = true;
            Ok((workspaces.len() - 1, 0, pane_id))
        }
    };
    match target {
        Ok((workspace_index, tab_index, pane_id)) => Ok(PaneRelocation {
            workspace_index,
            tab_index,
            pane_id,
            removed_tab,
            removed_workspace,
            created_workspace,
            created_tab,
            cross_workspace,
            active,
            selected,
        }),
        Err((message, moved)) => {
            (active, selected) = recover(workspaces, recovery, moved, active, selected);
            Err(PaneRelocationError {
                message,
                active,
                selected,
                preserve_previous_alias: cross_workspace,
                recovered: true,
            })
        }
    }
}

fn recover(
    workspaces: &mut Vec<Workspace>,
    context: RecoveryContext,
    moved: MovedPane,
    mut active: Option<usize>,
    mut selected: usize,
) -> (Option<usize>, usize) {
    if let Some(workspace) = super::workspace_index(workspaces, &context.workspace_id) {
        workspaces[workspace].create_tab_from_existing_pane(moved, context.tab_label);
    } else {
        let mut workspace = Workspace::from_existing_pane(
            context.workspace_label,
            context.tab_label,
            context.cwd,
            moved,
        );
        workspace.id = context.workspace_id;
        let insert = context.source_workspace.min(workspaces.len());
        if let Some(index) = active.as_mut() {
            if *index >= insert {
                *index += 1;
            }
        }
        if selected >= insert && !workspaces.is_empty() {
            selected += 1;
        }
        workspaces.insert(insert, workspace);
    }
    (active, selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn pane_relocation_preserves_every_attachment_for_each_destination() {
        for source_index in [0, 1] {
            for kind in [
                "existing-same",
                "existing-other",
                "new-same",
                "new-other",
                "workspace",
            ] {
                for focus in [false, true] {
                    let mut state = crate::app::AppState::test_with_adversarial_identity_state();
                    state.workspaces.insert(
                        1 - source_index,
                        Workspace::test_adversarial_identity_state(),
                    );
                    state.ensure_test_terminals();
                    state.active = Some(source_index);
                    state.selected = source_index;
                    let source_tab = state.workspaces[source_index]
                        .tabs
                        .iter()
                        .position(|tab| tab.panes.len() > 1)
                        .unwrap();
                    let source = state.workspaces[source_index].tabs[source_tab].root_pane;
                    let old_number = state.workspaces[source_index]
                        .public_pane_number(source)
                        .unwrap();
                    let attachments = |workspaces: &[Workspace]| {
                        workspaces
                            .iter()
                            .flat_map(|workspace| &workspace.tabs)
                            .flat_map(|tab| &tab.panes)
                            .map(|(&pane, state)| (pane, state.attached_terminal_id.clone()))
                            .collect::<HashMap<_, _>>()
                    };
                    let before = attachments(&state.workspaces);
                    let target_workspace = if kind.ends_with("same") {
                        source_index
                    } else {
                        1 - source_index
                    };
                    let target = state.workspaces[target_workspace].tabs[0].root_pane;
                    let target_focus = state.workspaces[target_workspace].tabs[0].layout.focused();
                    let destination = if kind.starts_with("existing") {
                        PaneDestination::ExistingTab {
                            tab_id: crate::workspace::public_tab_id_for_number(
                                &state.workspaces[target_workspace].id,
                                state.workspaces[target_workspace].tabs[0].number,
                            ),
                            target_pane_id: target,
                            direction: Direction::Vertical,
                            ratio: 0.3,
                        }
                    } else if kind.starts_with("new") {
                        PaneDestination::NewTab {
                            workspace_id: state.workspaces[target_workspace].id.clone(),
                            label: Some("moved".into()),
                        }
                    } else {
                        PaneDestination::NewWorkspace {
                            label: Some("destination".into()),
                            tab_label: Some("moved".into()),
                            cwd: PathBuf::from("/destination"),
                        }
                    };

                    let moved = relocate_pane(
                        &mut state.workspaces,
                        (source_index, source),
                        destination,
                        focus,
                        state.active,
                        state.selected,
                    )
                    .unwrap();

                    state.active = moved.active;
                    state.selected = moved.selected;
                    assert_eq!(before, attachments(&state.workspaces));
                    assert_eq!(moved.pane_id, source);
                    assert!(!moved.removed_tab);
                    assert!(!moved.removed_workspace);
                    assert_eq!(moved.cross_workspace, !kind.ends_with("same"));
                    let destination = &state.workspaces[moved.workspace_index];
                    if kind.ends_with("same") {
                        assert_eq!(destination.public_pane_number(source), Some(old_number));
                    } else {
                        assert!(state.workspaces[source_index]
                            .public_pane_number(source)
                            .is_none());
                        assert!(destination.public_pane_number(source).is_some());
                    }
                    assert_eq!(moved.created_workspace, kind == "workspace");
                    assert_eq!(moved.created_tab, !kind.starts_with("existing"));
                    if kind.starts_with("existing") {
                        assert_eq!(
                            destination.tabs[moved.tab_index].layout.focused(),
                            if focus { source } else { target_focus }
                        );
                    } else {
                        assert_eq!(
                            destination.tabs[moved.tab_index].custom_name.as_deref(),
                            Some("moved")
                        );
                    }
                    if kind == "workspace" {
                        assert_eq!(destination.custom_name.as_deref(), Some("destination"));
                        assert_eq!(destination.identity_cwd, PathBuf::from("/destination"));
                    }
                    state.assert_invariants_for_test();
                }
            }
        }
    }

    #[test]
    fn rejected_insertion_recovers_source_identity_and_reports_caller_effects() {
        for source_index in [0, 1] {
            let mut state = crate::app::AppState::test_new();
            let mut source = Workspace::test_new("source");
            source.custom_name = Some("source label".into());
            source.tabs[0].custom_name = Some("tab label".into());
            let old_id = source.id.clone();
            let cwd = source.identity_cwd.clone();
            let pane = source.tabs[0].root_pane;
            let terminal = source.terminal_id(pane).unwrap().clone();
            state.workspaces = vec![Workspace::test_new("target")];
            state.workspaces.insert(source_index, source);
            state.ensure_test_terminals();
            state.active = Some(source_index);
            state.selected = source_index;
            let target = 1 - source_index;
            let tab_id = crate::workspace::public_tab_id_for_number(
                &state.workspaces[target].id,
                state.workspaces[target].tabs[0].number,
            );
            let error = relocate_pane(
                &mut state.workspaces,
                (source_index, pane),
                PaneDestination::ExistingTab {
                    tab_id,
                    target_pane_id: PaneId::alloc(),
                    direction: Direction::Horizontal,
                    ratio: 0.5,
                },
                false,
                state.active,
                state.selected,
            )
            .err()
            .unwrap();

            assert_eq!(error.message, "target pane could not be split");
            assert!(error.recovered);
            assert!(error.preserve_previous_alias);
            state.active = error.active;
            state.selected = error.selected;
            let restored = &state.workspaces[source_index];
            assert_eq!(restored.id, old_id);
            assert_eq!(restored.custom_name.as_deref(), Some("source label"));
            assert_eq!(restored.tabs[0].custom_name.as_deref(), Some("tab label"));
            assert_eq!(restored.identity_cwd, cwd);
            assert_eq!(restored.terminal_id(pane), Some(&terminal));
            assert_eq!(restored.public_pane_number(pane), Some(1));
            state.assert_invariants_for_test();
        }
    }

    #[test]
    fn missing_source_is_rejected_without_mutation_or_recovery() {
        let mut state = crate::app::AppState::test_with_adversarial_identity_state();
        let before = serde_json::to_value(crate::session_model::tests::captured(&state)).unwrap();
        let error = relocate_pane(
            &mut state.workspaces,
            (0, PaneId::alloc()),
            PaneDestination::NewWorkspace {
                label: None,
                tab_label: None,
                cwd: PathBuf::from("/unused"),
            },
            false,
            state.active,
            state.selected,
        )
        .err()
        .unwrap();
        assert!(!error.recovered);
        assert!(!error.preserve_previous_alias);
        assert_eq!(
            (error.active, error.selected),
            (state.active, state.selected)
        );
        assert_eq!(
            before,
            serde_json::to_value(crate::session_model::tests::captured(&state)).unwrap()
        );
        state.assert_invariants_for_test();
    }
}
