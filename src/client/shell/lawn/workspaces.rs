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
            .map(|pane| (pane.pane_id.clone(), pane.workspace_id.clone()))
            .collect();
        // Closing keeps parked terminals, so a workspace close leaves the workspace.
        let leave = match method {
            Method::WorkspaceClose(target) => {
                let focused_workspace = snapshot.focused_workspace_id.clone();
                // A group close may target the root while a linked member is focused.
                let left = focused_workspace
                    .clone()
                    .filter(|focused| panes.iter().any(|pane| &pane.workspace_id == focused))
                    .unwrap_or_else(|| target.workspace_id.clone());
                Some((left, focused_workspace))
            }
            _ => None,
        };
        let attempted = active.len();
        let mut closing = Vec::new();
        for (pane_id, workspace_id) in active {
            let kind = PendingEndpointKind::ParkedPaneClose {
                pane_id: pane_id.clone(),
            };
            let target = PaneTarget {
                pane_id: pane_id.clone(),
            };
            if self.push_endpoint_method_with_kind(Method::PaneClose(target), kind, outcome) {
                closing.push((pane_id, workspace_id));
            }
        }
        // Nothing could be sent, so the workspace stays as it was.
        let leave = leave.filter(|_| attempted == 0 || !closing.is_empty());
        if let Some((left, focused_workspace)) = leave {
            if let Some(parking) = self.active_parking_mut() {
                parking.leave_workspace(&left, focused_workspace, closing);
            }
            outcome.repaint = true;
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
        let mobile = self.mobile_layout_active();
        let flat = self.sidebar_collapsed && !mobile;
        let empty_groups = HashSet::new();
        let groups = if flat || mobile {
            &empty_groups
        } else {
            self.collapsed_groups_for_endpoint(&endpoint.endpoint_id)
                .unwrap_or(&empty_groups)
        };
        let mut entries =
            super::super::sidebar::workspace_entries_matching(source, groups, |workspace| {
                workspace.workspace_id == departed.workspace_id
                    || visible.contains(workspace.workspace_id.as_str())
            });
        if flat {
            entries.sort_by_key(|entry| entry.index);
        }
        let origin = entries
            .iter()
            .position(|entry| source.workspaces[entry.index].workspace_id == departed.workspace_id)
            .or_else(|| {
                let group = source
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.workspace_id == departed.workspace_id)?
                    .worktree
                    .as_ref()?;
                entries.iter().position(|entry| {
                    source.workspaces[entry.index]
                        .worktree
                        .as_ref()
                        .is_some_and(|candidate| {
                            candidate.key == group.key && !candidate.is_linked_worktree
                        })
                })
            })
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
