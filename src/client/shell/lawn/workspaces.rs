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
        if self.closes_root_of_parked_group(method) {
            self.receive_endpoint_unavailable(
                "Unpark sessions before closing this worktree group's root workspace".into(),
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
            Method::WorkspaceClose(target) => {
                let workspace = snapshot
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.workspace_id == target.workspace_id)?;
                let group = target
                    .close_group
                    .then_some(workspace.worktree.as_ref())
                    .flatten();
                let workspace_ids: HashSet<_> = snapshot
                    .workspaces
                    .iter()
                    .filter(|candidate| {
                        candidate.workspace_id == target.workspace_id
                            || group.is_some_and(|group| {
                                candidate
                                    .worktree
                                    .as_ref()
                                    .is_some_and(|member| member.key == group.key)
                            })
                    })
                    .map(|workspace| workspace.workspace_id.as_str())
                    .collect();
                snapshot
                    .panes
                    .iter()
                    .filter(|pane| workspace_ids.contains(pane.workspace_id.as_str()))
                    .collect()
            }
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

    fn closes_root_of_parked_group(&self, method: &Method) -> bool {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return false;
        };
        let Some(parking) = self.active_parking() else {
            return false;
        };
        let workspace_id = match method {
            Method::WorkspaceClose(target) => Some(target.workspace_id.as_str()),
            Method::TabClose(target) => snapshot
                .tabs
                .iter()
                .find(|tab| tab.tab_id == target.tab_id)
                .map(|tab| tab.workspace_id.as_str()),
            Method::PaneClose(target) => snapshot
                .panes
                .iter()
                .find(|pane| pane.pane_id == target.pane_id)
                .map(|pane| pane.workspace_id.as_str()),
            _ => None,
        };
        let Some(workspace) = snapshot
            .workspaces
            .iter()
            .find(|workspace| Some(workspace.workspace_id.as_str()) == workspace_id)
        else {
            return false;
        };
        let root = if matches!(method, Method::WorkspaceClose(target) if target.close_group) {
            snapshot
                .workspaces
                .iter()
                .find(|candidate| {
                    candidate.worktree.as_ref().is_some_and(|candidate| {
                        !candidate.is_linked_worktree
                            && workspace
                                .worktree
                                .as_ref()
                                .is_some_and(|source| source.key == candidate.key)
                    })
                })
                .unwrap_or(workspace)
        } else {
            workspace
        };
        if !super::super::sidebar::workspace_close_is_group(snapshot, root) {
            return false;
        }
        let root_panes: Vec<_> = snapshot
            .panes
            .iter()
            .filter(|pane| pane.workspace_id == root.workspace_id)
            .collect();
        if root_panes
            .iter()
            .any(|pane| parking.hides_pane(&pane.pane_id))
        {
            return false;
        }
        let removes_root = match method {
            Method::WorkspaceClose(_) => true,
            Method::TabClose(target) => root_panes.iter().all(|pane| pane.tab_id == target.tab_id),
            Method::PaneClose(target) => {
                root_panes.iter().all(|pane| pane.pane_id == target.pane_id)
            }
            _ => false,
        };
        removes_root
            && snapshot
                .workspaces
                .iter()
                .filter(|candidate| {
                    candidate.worktree.as_ref().is_some_and(|candidate| {
                        root.worktree
                            .as_ref()
                            .is_some_and(|root| root.key == candidate.key)
                    })
                })
                .any(|workspace| {
                    snapshot.panes.iter().any(|pane| {
                        pane.workspace_id == workspace.workspace_id
                            && parking.hides_pane(&pane.pane_id)
                    })
                })
    }

    pub(super) fn focus_after_parked_close(&mut self, outcome: &mut ClientShellInput) {
        let Some(endpoint) = self
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
            .filter(|endpoint| endpoint.parking.available())
        else {
            return;
        };
        let Some(departed) = endpoint.parking.departed_workspace.take() else {
            return;
        };
        let Some(snapshot) = endpoint.snapshot.as_deref() else {
            return;
        };
        if snapshot.focused_workspace_id.as_deref() != Some(&departed)
            || endpoint.parking.shows_workspace(&departed)
            || endpoint.parking.preview_pane().is_some()
        {
            return;
        }
        let Some(workspace_id) = snapshot
            .workspaces
            .iter()
            .find(|workspace| endpoint.parking.shows_workspace(&workspace.workspace_id))
            .map(|workspace| workspace.workspace_id.clone())
        else {
            return;
        };
        self.push_endpoint_method(
            Method::WorkspaceFocus(WorkspaceTarget { workspace_id }),
            outcome,
        );
    }
}
