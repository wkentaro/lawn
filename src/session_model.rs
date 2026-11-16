use crate::workspace::decode_public_number;

pub(crate) trait Workspace {
    fn id(&self) -> Option<&str>;
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

impl Workspace for crate::workspace::Workspace {
    fn id(&self) -> Option<&str> {
        Some(&self.id)
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
