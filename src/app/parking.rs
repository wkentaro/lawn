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
