use super::*;
use crate::api::schema::{Method, TabTarget, WorkspaceTarget};

impl ClientShellState {
    /// Closing a workspace on the server also closes its parked panes, so a
    /// workspace with parked panes closes only its visible tabs, and a parked
    /// pane or its hidden tab never closes.
    pub(in crate::client::shell) fn preserve_parked_close(
        &mut self,
        method: &Method,
        outcome: &mut ClientShellInput,
    ) -> Option<bool> {
        let parking = self.active_parking()?;
        match method {
            Method::PaneClose(target) => parking.hides_pane(&target.pane_id).then_some(false),
            // A peek or an empty parked view focuses a parked pane's hidden tab.
            Method::TabClose(target) => {
                let snapshot = self.snapshot.as_deref()?;
                (!snapshot.tabs.iter().any(|tab| tab.tab_id == target.tab_id)
                    && snapshot
                        .parked_panes
                        .iter()
                        .any(|pane| pane.tab_id == target.tab_id))
                .then_some(false)
            }
            Method::WorkspaceClose(target) => {
                let snapshot = self.snapshot.as_deref()?;
                // Includes a park still on its way to the server.
                if !snapshot.panes.iter().any(|pane| {
                    pane.workspace_id == target.workspace_id && parking.hides_pane(&pane.pane_id)
                }) {
                    return None;
                }
                let tabs: Vec<_> = snapshot
                    .tabs
                    .iter()
                    .filter(|tab| tab.workspace_id == target.workspace_id)
                    .map(|tab| tab.tab_id.clone())
                    .collect();
                for tab_id in tabs {
                    self.push_endpoint_method(Method::TabClose(TabTarget { tab_id }), outcome);
                }
                Some(true)
            }
            _ => None,
        }
    }

    pub(crate) fn focus_after_parked_close(&mut self, outcome: &mut ClientShellInput) {
        let Some(index) = self.endpoints.iter().position(|endpoint| {
            endpoint.endpoint_id == self.active_endpoint_id && endpoint.parking.available()
        }) else {
            return;
        };
        let Some(departed) = self.endpoints[index].parking.departed_workspace.take() else {
            return;
        };
        let endpoint = &self.endpoints[index];
        let Some(snapshot) = endpoint.snapshot.as_deref() else {
            return;
        };
        if snapshot.focused_workspace_id != departed.focused_workspace
            || snapshot
                .focused_workspace_id
                .as_deref()
                .is_some_and(|workspace| endpoint.parking.shows_workspace(workspace))
            || endpoint.parking.preview_pane().is_some()
        {
            return;
        }
        let visible: HashSet<_> = snapshot
            .workspaces
            .iter()
            .filter(|workspace| endpoint.parking.shows_workspace(&workspace.workspace_id))
            .map(|workspace| workspace.workspace_id.as_str())
            .collect();
        let source = departed.origin.as_deref().unwrap_or(snapshot);
        let entries = super::super::sidebar::workspace_entries_matching(source, |workspace| {
            workspace.workspace_id == departed.workspace_id
                || visible.contains(workspace.workspace_id.as_str())
        });
        let origin = entries
            .iter()
            .position(|entry| source.workspaces[entry.index].workspace_id == departed.workspace_id)
            .unwrap_or(0);
        let neighbor = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                visible.contains(source.workspaces[entry.index].workspace_id.as_str())
            })
            .min_by_key(|(index, _)| (index.abs_diff(origin), *index < origin))
            .map(|(_, entry)| source.workspaces[entry.index].workspace_id.clone());
        if let Some(workspace_id) = neighbor {
            self.reveal_workspace(&workspace_id);
            self.push_endpoint_method(
                Method::WorkspaceFocus(WorkspaceTarget { workspace_id }),
                outcome,
            );
        } else {
            self.selection = None;
            self.copy_mode = None;
            self.mode = ClientShellMode::Terminal;
            self.push_endpoint_method(
                Method::WorkspaceEnsureActive(crate::api::schema::WorkspaceEnsureActiveParams {
                    excluded_terminal_ids: Vec::new(),
                }),
                outcome,
            );
        }
    }
}
