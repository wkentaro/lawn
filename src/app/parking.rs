use tracing::warn;

use super::state::AppState;
use super::App;
use crate::api::schema::{EventData, EventEnvelope, EventKind};
use crate::layout::PaneId;
use crate::pane::{CoreTerminal, PaneState};
use crate::terminal::{TerminalId, TerminalRuntime, TerminalState};
use crate::workspace::MovedPane;

impl App {
    /// Starts parking the core terminals no pane shows, when panes run in core.
    /// Call it once the session is restored, so its panes are known.
    pub(crate) fn watch_core_terminals(&self) {
        let Some(core) = crate::hc::backend() else {
            return;
        };
        let watching = core
            .map_err(std::io::Error::other)
            .and_then(|core| crate::pane::watch_core_terminals(&core, self.event_tx.clone()));
        if let Err(err) = watching {
            warn!(%err, "cannot watch herdr-core for terminals to park");
        }
    }

    /// Parks a core terminal that no pane shows. It joins the workspace of
    /// the pane whose program started it, to come back beside that pane, and
    /// otherwise the active workspace.
    pub(crate) fn park_core_terminal(&mut self, found: CoreTerminal) {
        let shows = |terminal_id: &TerminalId, address: &str| {
            self.terminal_runtimes
                .get(terminal_id)
                .is_some_and(|runtime| runtime.core_address() == Some(address))
        };
        let address = Some(found.address.as_str());
        if self
            .terminal_runtimes
            .values()
            .any(|runtime| runtime.core_address() == address)
            || self
                .state
                .terminals
                .values()
                .any(|terminal| terminal.unattached_core_terminal.as_deref() == address)
        {
            return;
        }
        let beside_parent = found.parent.as_deref().and_then(|parent| {
            self.state
                .workspaces
                .iter()
                .enumerate()
                .find_map(|(ws_idx, ws)| {
                    let tab = ws.tabs.iter().find(|tab| {
                        tab.panes
                            .values()
                            .any(|pane| shows(&pane.attached_terminal_id, parent))
                    })?;
                    Some((ws_idx, Some(tab.parked_from.unwrap_or(tab.number))))
                })
        });
        let place = beside_parent.or_else(|| {
            let ws_idx = self.state.active.unwrap_or(0);
            let ws = self.state.workspaces.get(ws_idx)?;
            Some((ws_idx, ws.active_tab().map(|tab| tab.number)))
        });
        let Some((ws_idx, origin)) = place else {
            self.core_terminals_to_park.push(found);
            return;
        };
        let pane_id = PaneId::alloc();
        // At the terminal's own size, so the attach resizes nothing. The
        // layout sizes it afterwards, like any pane.
        let runtime = match TerminalRuntime::reattach_core(
            pane_id,
            &found.address,
            found.rows,
            found.cols,
            self.state.pane_scrollback_limit_bytes,
            self.state.host_terminal_theme,
            self.pane_host_terminal_appearance(),
            self.event_tx.clone(),
            self.render_notify.clone(),
            self.render_dirty.clone(),
        ) {
            Ok(runtime) => runtime,
            Err(err) => {
                warn!(address = %found.address, %err, "cannot park the core terminal");
                return;
            }
        };
        let mut terminal = TerminalState::new(TerminalId::alloc(), found.cwd);
        if let Some(name) = found.name {
            terminal.set_manual_label(name);
        }
        let moved = MovedPane {
            pane_id,
            pane_state: PaneState::new(terminal.id.clone()),
        };
        self.terminal_runtimes.insert(terminal.id.clone(), runtime);
        self.state.terminals.insert(terminal.id.clone(), terminal);
        self.state.workspaces[ws_idx].add_parked_pane(moved, origin);
        self.state.remove_alias_shadowed_by_new_pane(pane_id);
        self.schedule_session_save();
        if let Some(pane) = self.pane_info(ws_idx, pane_id) {
            self.emit_event(EventEnvelope {
                event: EventKind::PaneParked,
                data: EventData::PaneParked { pane },
            });
        }
    }
}

impl AppState {
    /// Hides a pane in a parked tab. It stays in its workspace with its
    /// terminal running, and the workspace stays even without visible tabs.
    pub(crate) fn park_pane(&mut self, ws_idx: usize, pane_id: PaneId) -> Option<()> {
        self.workspaces.get_mut(ws_idx)?.park_pane(pane_id)?;
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
    pub(crate) fn park_workspace(&mut self, ws_idx: usize) -> Option<Vec<PaneId>> {
        let panes: Vec<PaneId> = self
            .workspaces
            .get(ws_idx)?
            .visible_tabs()
            .iter()
            .flat_map(|tab| tab.layout.pane_ids())
            .collect();
        for &pane_id in &panes {
            self.park_pane(ws_idx, pane_id)?;
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
