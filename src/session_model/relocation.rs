use crate::layout::PaneId;
use crate::workspace::{public_pane_id_for_number, public_tab_id_for_number, Workspace};

pub(crate) struct TabRelocation {
    pub previous_tab_id: String,
    pub previous_workspace_id: String,
    pub previous_pane_ids: Vec<(String, PaneId)>,
    pub pane_ids: Vec<PaneId>,
    pub focused_pane: PaneId,
    pub workspace_index: usize,
    pub tab_index: usize,
    pub removed_workspace: bool,
    pub active: Option<usize>,
    pub selected: usize,
}

pub(crate) fn relocate_tab(
    workspaces: &mut Vec<Workspace>,
    source: (usize, usize),
    target: usize,
    insert: Option<usize>,
    active: Option<usize>,
    selected: usize,
) -> Option<TabRelocation> {
    let (source_workspace, source_tab) = source;
    if source_workspace == target || target >= workspaces.len() {
        return None;
    }
    let workspace = workspaces.get(source_workspace)?;
    if source_tab >= workspace.visible_tab_count() {
        return None;
    }
    let tab = workspace.tabs.get(source_tab)?;
    let previous_tab_id = public_tab_id_for_number(&workspace.id, tab.number);
    let previous_workspace_id = workspace.id.clone();
    let pane_ids = tab.layout.pane_ids();
    let previous_pane_ids = pane_ids
        .iter()
        .filter_map(|&pane| {
            workspace
                .public_pane_number(pane)
                .map(|number| (public_pane_id_for_number(&workspace.id, number), pane))
        })
        .collect();
    let focused_pane = tab.layout.focused();
    let tab = workspaces[source_workspace].take_tab(source_tab)?;
    let removed_workspace = workspaces[source_workspace].tabs.is_empty();
    let (active, selected) = if removed_workspace {
        remove_empty_workspace(workspaces, source_workspace, Some(target), active, selected)
    } else {
        (active, selected)
    };
    let workspace_index = target - usize::from(removed_workspace && target > source_workspace);
    let tab_index = workspaces[workspace_index].insert_moved_tab(tab, insert);
    Some(TabRelocation {
        previous_tab_id,
        previous_workspace_id,
        previous_pane_ids,
        pane_ids,
        focused_pane,
        workspace_index,
        tab_index,
        removed_workspace,
        active,
        selected,
    })
}

pub(crate) fn remove_empty_workspace(
    workspaces: &mut Vec<Workspace>,
    index: usize,
    follow: Option<usize>,
    active: Option<usize>,
    selected: usize,
) -> (Option<usize>, usize) {
    workspaces.remove(index);
    if workspaces.is_empty() {
        return (None, 0);
    }
    let shift = |i: usize| if i > index { i - 1 } else { i };
    let replacement = follow.map_or(index.min(workspaces.len() - 1), shift);
    let remap = |i: usize| if i == index { replacement } else { shift(i) };
    let shown = |i| shown_workspace_from(workspaces, i).unwrap_or(i);
    (active.map(|i| shown(remap(i))), shown(remap(selected)))
}

pub(crate) fn shown_workspace_from(workspaces: &[Workspace], start: usize) -> Option<usize> {
    let count = workspaces.len();
    (0..count)
        .map(|offset| (start + offset) % count)
        .find(|&index| !workspaces[index].parked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relocation_preserves_tab_state_and_allocates_destination_public_numbers() {
        for source_index in [0, 1] {
            for insert in [None, Some(0), Some(1), Some(usize::MAX)] {
                let mut state = crate::app::AppState::test_with_adversarial_identity_state();
                let mut destination = Workspace::test_adversarial_identity_state();
                destination.tabs.last_mut().unwrap().parked_from = Some(1);
                state.workspaces.insert(1 - source_index, destination);
                state.ensure_test_terminals();
                state.active = Some(source_index);
                state.selected = source_index;
                let source = &mut state.workspaces[source_index];
                let moving = source
                    .tabs
                    .iter()
                    .position(|tab| tab.panes.len() > 1)
                    .unwrap();
                source.tabs[moving].zoomed = true;
                source.tabs[moving].custom_name = Some("moving".into());
                let old_workspace = source.id.clone();
                let old_tab = public_tab_id_for_number(&source.id, source.tabs[moving].number);
                let pane_ids = source.tabs[moving].layout.pane_ids();
                let old_panes: Vec<_> = pane_ids
                    .iter()
                    .map(|&pane| {
                        (
                            public_pane_id_for_number(
                                &source.id,
                                source.public_pane_number(pane).unwrap(),
                            ),
                            pane,
                        )
                    })
                    .collect();
                let focused = source.tabs[moving].layout.focused();
                let attachments: Vec<_> = pane_ids
                    .iter()
                    .map(|pane| source.tabs[moving].panes[pane].attached_terminal_id.clone())
                    .collect();
                let captured = crate::session_model::tests::captured(&state);
                let before =
                    serde_json::to_value(&captured.workspaces[source_index].tabs[moving]).unwrap();
                let destination = &state.workspaces[1 - source_index];
                let next_tab = destination.next_public_tab_number;
                let next_pane = destination.next_public_pane_number;
                let destination_focus = destination.tabs[destination.active_tab].root_pane;
                let target_index = insert
                    .unwrap_or(usize::MAX)
                    .min(destination.visible_tab_count());

                let moved = relocate_tab(
                    &mut state.workspaces,
                    (source_index, moving),
                    1 - source_index,
                    insert,
                    state.active,
                    state.selected,
                )
                .unwrap();

                assert!(!moved.removed_workspace);
                assert_eq!(moved.workspace_index, 1 - source_index);
                assert_eq!(moved.tab_index, target_index);
                assert_eq!(moved.previous_workspace_id, old_workspace);
                assert_eq!(moved.previous_tab_id, old_tab);
                assert_eq!(moved.previous_pane_ids, old_panes);
                assert_eq!(moved.pane_ids, pane_ids);
                assert_eq!(moved.focused_pane, focused);
                assert_eq!(
                    (moved.active, moved.selected),
                    (Some(source_index), source_index)
                );
                let destination = &state.workspaces[moved.workspace_index];
                assert_eq!(destination.tabs[moved.tab_index].number, next_tab);
                assert_eq!(
                    destination.tabs[destination.active_tab].root_pane,
                    destination_focus
                );
                for (offset, pane) in pane_ids.iter().enumerate() {
                    assert_eq!(
                        destination.public_pane_number(*pane),
                        Some(next_pane + offset)
                    );
                    assert_eq!(
                        destination.tabs[moved.tab_index].panes[pane].attached_terminal_id,
                        attachments[offset]
                    );
                }
                let after = crate::session_model::tests::captured(&state);
                assert_eq!(
                    before,
                    serde_json::to_value(
                        &after.workspaces[moved.workspace_index].tabs[moved.tab_index]
                    )
                    .unwrap()
                );
                state.assert_invariants_for_test();
            }
        }
    }

    #[test]
    fn relocation_rejects_invalid_or_parked_sources_without_mutation() {
        for (source, target) in [
            ((0, 0), 0),
            ((0, 0), 9),
            ((9, 0), 0),
            ((0, 9), 1),
            ((0, 2), 1),
        ] {
            let mut state = crate::app::AppState::test_with_adversarial_identity_state();
            state.workspaces[0].tabs[2].parked_from = Some(1);
            state.workspaces.push(Workspace::test_new("destination"));
            state.ensure_test_terminals();
            let before =
                serde_json::to_value(crate::session_model::tests::captured(&state)).unwrap();
            assert!(relocate_tab(
                &mut state.workspaces,
                source,
                target,
                None,
                state.active,
                state.selected
            )
            .is_none());
            assert_eq!(
                before,
                serde_json::to_value(crate::session_model::tests::captured(&state)).unwrap()
            );
            state.assert_invariants_for_test();
        }
    }

    #[test]
    fn relocation_keeps_a_source_workspace_with_parked_tabs() {
        let mut workspaces = vec![Workspace::test_new("source"), Workspace::test_new("target")];
        let parked = workspaces[0].test_add_tab(Some("parked"));
        workspaces[0].tabs[parked].parked_from = Some(1);
        let old_id = workspaces[0].id.clone();
        let moved = relocate_tab(&mut workspaces, (0, 0), 1, None, Some(1), 1).unwrap();
        assert!(!moved.removed_workspace);
        assert_eq!(workspaces.len(), 2);
        assert_eq!(workspaces[0].id, old_id);
        assert_eq!(workspaces[0].visible_tab_count(), 0);
        assert_eq!(workspaces[0].parked_tabs().len(), 1);
        for workspace in &workspaces {
            workspace.assert_invariants_for_test();
        }
    }
}
