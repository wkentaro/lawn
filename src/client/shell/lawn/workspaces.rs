use super::*;
use crate::api::schema::{Method, PaneTarget, WorkspaceTarget};

impl ClientShellState {
    pub(in crate::client::shell) fn preserve_parked_close(
        &mut self,
        method: &Method,
        outcome: &mut ClientShellInput,
    ) -> Option<bool> {
        if !matches!(
            method,
            Method::WorkspaceClose(_) | Method::TabClose(_) | Method::PaneClose(_)
        ) {
            return None;
        }
        let parking = self.active_parking()?;
        if !parking.supported {
            return None;
        }
        if !parking.ready() {
            self.receive_endpoint_unavailable(
                "Wait for Parked to finish syncing before closing panes".into(),
            );
            outcome.repaint = true;
            return Some(false);
        }
        if let Method::PaneClose(target) = method {
            return parking.hides_pane(&target.pane_id).then_some(false);
        }
        let snapshot = self.snapshot.as_deref()?;
        let panes: Vec<_> = match method {
            Method::TabClose(target) => snapshot
                .panes
                .iter()
                .filter(|pane| pane.tab_id == target.tab_id)
                .collect(),
            Method::WorkspaceClose(target) => snapshot
                .panes
                .iter()
                .filter(|pane| pane.workspace_id == target.workspace_id)
                .collect(),
            _ => return None,
        };
        if !panes.iter().any(|pane| parking.hides_pane(&pane.pane_id)) {
            return None;
        }
        let active: Vec<_> = panes
            .iter()
            .filter(|pane| !parking.hides_pane(&pane.pane_id))
            .map(|pane| pane.pane_id.clone())
            .collect();
        for pane_id in active {
            self.push_endpoint_method(Method::PaneClose(PaneTarget { pane_id }), outcome);
        }
        Some(true)
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
            self.handle_endpoint_navigation(crate::input::KeybindAction::NextWorkspace, outcome);
        }
    }
}
