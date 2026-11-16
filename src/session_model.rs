mod relocation;

pub(crate) use relocation::{relocate_tab, remove_empty_workspace, shown_workspace_from};

use crate::workspace::decode_public_number;

pub(crate) trait Workspace {
    fn id(&self) -> Option<&str>;
    fn parked(&self) -> bool;
    fn visible_tab_count(&self) -> usize;
    fn tab_position(&self, number: usize) -> Option<usize>;
    fn rename(&mut self, label: String);
    fn rename_tab(&mut self, index: usize, label: String) -> Option<()>;
}

pub(crate) fn workspace_index<W: Workspace>(workspaces: &[W], id: &str) -> Option<usize> {
    workspaces
        .iter()
        .position(|workspace| workspace.id() == Some(id))
        .or_else(|| id.strip_prefix("w_")?.parse::<usize>().ok()?.checked_sub(1))
        .or_else(|| id.parse::<usize>().ok()?.checked_sub(1))
        .filter(|&index| index < workspaces.len())
}

pub(crate) fn tab_index<W: Workspace>(workspaces: &[W], id: &str) -> Option<(usize, usize)> {
    let (workspace, tab) = if let Some(rest) = id.strip_prefix("t_") {
        let (workspace_raw, tab_raw) = rest.rsplit_once('_')?;
        (
            workspace_index(workspaces, workspace_raw)?,
            tab_raw.parse::<usize>().ok()?.checked_sub(1)?,
        )
    } else {
        let (workspace_raw, tab_raw) = id.rsplit_once(':')?;
        let workspace = workspace_index(workspaces, workspace_raw)?;
        let tab = match tab_raw.strip_prefix('t') {
            Some(encoded) => workspaces[workspace].tab_position(decode_public_number(encoded)?)?,
            None => tab_raw.parse::<usize>().ok()?.checked_sub(1)?,
        };
        (workspace, tab)
    };
    (tab < workspaces[workspace].visible_tab_count()).then_some((workspace, tab))
}

pub(crate) fn rename_workspace<W: Workspace>(
    workspaces: &mut [W],
    id: &str,
    label: &str,
) -> Option<usize> {
    let index = workspace_index(workspaces, id)?;
    workspaces[index].rename(label.to_owned());
    Some(index)
}

pub(crate) fn rename_tab<W: Workspace>(
    workspaces: &mut [W],
    id: &str,
    label: &str,
) -> Option<(usize, usize)> {
    let (workspace, tab) = tab_index(workspaces, id)?;
    workspaces[workspace].rename_tab(tab, label.to_owned())?;
    Some((workspace, tab))
}

pub(crate) enum LayoutMove {
    Workspace {
        source: usize,
        insert: usize,
    },
    Tab {
        workspace: usize,
        source: usize,
        insert: usize,
    },
}

pub(crate) fn move_item<T>(items: &mut [T], source: usize, insert: usize) -> Option<usize> {
    if source >= items.len() || insert > items.len() {
        return None;
    }
    let target = if source < insert { insert - 1 } else { insert };
    if source == target {
        return None;
    }
    if source < target {
        items[source..=target].rotate_left(1);
    } else {
        items[target..=source].rotate_right(1);
    }
    Some(target)
}

pub(crate) fn index_after_move(index: usize, source: usize, target: usize) -> usize {
    if index == source {
        target
    } else if source < index && index <= target {
        index - 1
    } else if target <= index && index < source {
        index + 1
    } else {
        index
    }
}

pub(crate) fn move_workspace<W>(
    workspaces: &mut [W],
    active: &mut Option<usize>,
    selected: &mut usize,
    source: usize,
    insert: usize,
) -> bool {
    let Some(target) = move_item(workspaces, source, insert) else {
        return false;
    };
    *active = active.map(|index| index_after_move(index, source, target));
    *selected = if *selected < workspaces.len() {
        index_after_move(*selected, source, target)
    } else {
        0
    };
    true
}

impl Workspace for crate::workspace::Workspace {
    fn id(&self) -> Option<&str> {
        Some(&self.id)
    }

    fn parked(&self) -> bool {
        self.parked
    }

    fn visible_tab_count(&self) -> usize {
        self.visible_tab_count()
    }

    fn tab_position(&self, number: usize) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.number == number)
    }

    fn rename(&mut self, label: String) {
        self.set_custom_name(label);
    }

    fn rename_tab(&mut self, index: usize, label: String) -> Option<()> {
        self.tabs.get_mut(index)?.set_custom_name(label);
        Some(())
    }
}

impl Workspace for crate::persist::WorkspaceSnapshot {
    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn parked(&self) -> bool {
        self.parked
    }

    fn visible_tab_count(&self) -> usize {
        self.visible_tab_count()
    }

    fn tab_position(&self, number: usize) -> Option<usize> {
        (0..self.tabs.len()).position(|tab| self.tab_number(tab) == number)
    }

    fn rename(&mut self, label: String) {
        self.custom_name = Some(label);
    }

    fn rename_tab(&mut self, index: usize, label: String) -> Option<()> {
        self.tabs.get_mut(index)?.custom_name = Some(label);
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn captured(state: &crate::app::AppState) -> crate::persist::SessionSnapshot {
        crate::persist::capture(
            &state.workspaces,
            &state.terminals,
            &crate::terminal::TerminalRuntimeRegistry::new(),
            state.active,
            state.selected,
        )
    }

    #[test]
    fn workspace_moves_preserve_the_same_objects_and_selection_in_both_models() {
        for source in 0..4 {
            for insert in 0..5 {
                for active in 0..3 {
                    let mut state = crate::app::AppState::test_new();
                    state.workspaces = ["one", "two", "three"]
                        .map(crate::workspace::Workspace::test_new)
                        .into();
                    state.active = Some(active);
                    state.selected = 2;
                    state.ensure_test_terminals();
                    let mut saved = captured(&state);
                    let live_moved = state.move_workspace(source, insert);
                    let saved_moved = move_workspace(
                        &mut saved.workspaces,
                        &mut saved.active,
                        &mut saved.selected,
                        source,
                        insert,
                    );
                    assert_eq!(live_moved, saved_moved);
                    assert_eq!(
                        serde_json::to_value(captured(&state)).unwrap(),
                        serde_json::to_value(saved).unwrap()
                    );
                    state.assert_invariants_for_test();
                }
            }
        }
    }

    #[test]
    fn tab_moves_preserve_public_identity_focus_and_parked_suffix_in_both_models() {
        for parked in [false, true] {
            for source in 0..4 {
                for insert in 0..5 {
                    let mut state = crate::app::AppState::test_with_adversarial_identity_state();
                    state.workspaces[0].tabs.last_mut().unwrap().parked_from = parked.then_some(1);
                    for (index, tab) in state.workspaces[0].tabs.iter().enumerate() {
                        state
                            .terminals
                            .get_mut(tab.terminal_id(tab.root_pane).unwrap())
                            .unwrap()
                            .cwd = std::path::PathBuf::from(format!("/projects/{index}"));
                    }
                    let mut saved = captured(&state);
                    let live_moved = state.workspaces[0].move_tab(source, insert);
                    let saved_moved = saved.workspaces[0].move_tab(source, insert);
                    assert_eq!(live_moved, saved_moved);
                    assert_eq!(
                        serde_json::to_value(captured(&state)).unwrap(),
                        serde_json::to_value(saved).unwrap()
                    );
                    state.assert_invariants_for_test();
                }
            }
        }
    }

    #[test]
    fn tab_moves_materialize_missing_public_numbers_without_reusing_identity() {
        for numbers in [vec![], vec![8], vec![8, 2, 3, 99]] {
            let state = crate::app::AppState::test_with_adversarial_identity_state();
            let mut saved = captured(&state);
            let workspace = &mut saved.workspaces[0];
            workspace.public_tab_numbers = numbers;
            workspace.tabs.last_mut().unwrap().parked_from = Some(1);
            let before = (0..workspace.tabs.len())
                .map(|index| workspace.tab_number(index))
                .collect::<Vec<_>>();
            let next = workspace.next_tab_number();
            assert!(workspace.move_tab(0, 2));
            assert_eq!(workspace.tab_number(0), before[1]);
            assert_eq!(workspace.tab_number(1), before[0]);
            assert_eq!(workspace.tab_number(2), before[2]);
            assert_eq!(workspace.next_tab_number(), next);
        }
    }

    #[test]
    fn live_and_saved_renames_preserve_identity_and_apply_empty_or_repeated_labels() {
        let mut state = crate::app::AppState::test_with_adversarial_identity_state();
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let mut saved = crate::persist::capture(
            &state.workspaces,
            &state.terminals,
            &runtimes,
            state.active,
            state.selected,
        );
        let workspace_id = state.workspaces[0].id.clone();
        let tab_id = crate::workspace::public_tab_id_for_number(
            &workspace_id,
            state.workspaces[0].tabs[0].number,
        );
        for label in ["", "docs", "docs"] {
            assert_eq!(
                rename_workspace(&mut state.workspaces, &workspace_id, label),
                Some(0)
            );
            assert_eq!(
                rename_workspace(&mut saved.workspaces, &workspace_id, label),
                Some(0)
            );
            assert_eq!(
                rename_tab(&mut state.workspaces, &tab_id, label),
                Some((0, 0))
            );
            assert_eq!(
                rename_tab(&mut saved.workspaces, &tab_id, label),
                Some((0, 0))
            );
            assert_eq!(state.workspaces[0].custom_name.as_deref(), Some(label));
            assert_eq!(saved.workspaces[0].custom_name.as_deref(), Some(label));
            assert_eq!(
                state.workspaces[0].tabs[0].custom_name.as_deref(),
                Some(label)
            );
            assert_eq!(
                saved.workspaces[0].tabs[0].custom_name.as_deref(),
                Some(label)
            );
            state.assert_invariants_for_test();
        }
        for id in ["missing", "0", "999"] {
            assert_eq!(rename_workspace(&mut state.workspaces, id, "bad"), None);
            assert_eq!(rename_workspace(&mut saved.workspaces, id, "bad"), None);
        }
        let parked = state.workspaces[0].tabs.last_mut().unwrap();
        let parked_id = crate::workspace::public_tab_id_for_number(&workspace_id, parked.number);
        parked.parked_from = Some(1);
        saved.workspaces[0].tabs.last_mut().unwrap().parked_from = Some(1);
        for id in [parked_id.as_str(), "missing:t1", "w_1:999", "t_1_0"] {
            assert_eq!(rename_tab(&mut state.workspaces, id, "bad"), None);
            assert_eq!(rename_tab(&mut saved.workspaces, id, "bad"), None);
        }
        assert_eq!(state.workspaces[0].custom_name.as_deref(), Some("docs"));
        assert_eq!(saved.workspaces[0].custom_name.as_deref(), Some("docs"));
        assert_eq!(state.workspaces[0].tabs.last().unwrap().custom_name, None);
        assert_eq!(saved.workspaces[0].tabs.last().unwrap().custom_name, None);
        state.assert_invariants_for_test();
    }
}
