use super::state::AppState;
use crate::layout::PaneId;
use crate::workspace::TabChannels;

impl AppState {
    /// Hides a pane in a parked tab. It stays in its workspace with its
    /// terminal running, and the workspace stays even without visible tabs.
    pub(crate) fn park_pane(
        &mut self,
        ws_idx: usize,
        pane_id: PaneId,
        channels: TabChannels,
    ) -> Option<()> {
        self.workspaces
            .get_mut(ws_idx)?
            .park_pane(pane_id, channels)?;
        if self
            .previous_pane_focus
            .as_ref()
            .is_some_and(|focus| focus.pane_id == pane_id)
        {
            self.previous_pane_focus = None;
        }
        self.mark_session_dirty();
        Some(())
    }

    /// Parks every pane of a workspace and hides the workspace itself until a
    /// pane comes back. Focus moves to the next workspace still shown.
    /// Returns the panes it parked.
    pub(crate) fn park_workspace(
        &mut self,
        ws_idx: usize,
        channels: TabChannels,
    ) -> Option<Vec<PaneId>> {
        let panes: Vec<PaneId> = self
            .workspaces
            .get(ws_idx)?
            .visible_tabs()
            .iter()
            .flat_map(|tab| tab.layout.pane_ids())
            .collect();
        for &pane_id in &panes {
            self.park_pane(ws_idx, pane_id, channels.clone())?;
        }
        let workspace = &mut self.workspaces[ws_idx];
        if workspace.parked_tabs().is_empty() {
            return None;
        }
        workspace.parked = true;
        self.mark_session_dirty();
        if self.active == Some(ws_idx) {
            // With none left, the server opens a home workspace.
            if let Some(next) = self.shown_workspace_from(ws_idx) {
                self.switch_workspace(next);
            }
        }
        self.keep_focus_on_shown_workspaces();
        Some(panes)
    }

    /// Moves focus and selection off parked workspaces, after a workspace
    /// closed or was parked, to the next one shown. With none shown, the server
    /// opens a home workspace.
    pub(crate) fn keep_focus_on_shown_workspaces(&mut self) {
        self.active = self
            .active
            .map(|idx| self.shown_workspace_from(idx).unwrap_or(idx));
        self.selected = self
            .shown_workspace_from(self.selected)
            .unwrap_or(self.selected);
    }

    /// The first workspace that is not parked, from `start` on, wrapping around.
    pub(crate) fn shown_workspace_from(&self, start: usize) -> Option<usize> {
        let count = self.workspaces.len();
        (0..count)
            .map(|offset| (start + offset) % count)
            .find(|&idx| !self.workspaces[idx].parked)
    }

    pub(crate) fn unpark_pane(
        &mut self,
        ws_idx: usize,
        pane_id: PaneId,
        focus: bool,
    ) -> Option<()> {
        self.workspaces
            .get_mut(ws_idx)?
            .unpark_pane(pane_id, focus)?;
        self.mark_session_dirty();
        if focus {
            self.focus_pane_in_workspace(ws_idx, pane_id);
            self.mode = super::Mode::Terminal;
        }
        Some(())
    }
}
