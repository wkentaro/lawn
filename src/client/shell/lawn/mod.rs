use super::*;
use crate::protocol::endpoint::pane_presentation::{
    PaneIdentities, PanePresentationError, SetPanePresentation,
};
use std::{path::PathBuf, time::Instant};

mod parked;
mod parking_store;
pub(super) use parked::{height as navigation_height, render as render_navigation, ParkedState};
mod parking;
pub(super) use parking::PaneParking;
mod workspaces;

impl ClientShellState {
    pub(crate) fn set_endpoint_parking_supported(
        &mut self,
        endpoint_id: &ClientEndpointId,
        supported: bool,
    ) {
        let previous_size = self.parking_size();
        if let Some(endpoint) = self
            .endpoints
            .iter_mut()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
        {
            endpoint.parking.set_supported(supported);
        }
        self.update_parking_size(previous_size);
    }

    pub(crate) fn set_endpoint_pane_identities(
        &mut self,
        endpoint_id: &ClientEndpointId,
        generation: u64,
        identities: PaneIdentities,
    ) {
        let previous_size = self.parking_size();
        if let Some(endpoint) = self
            .endpoints
            .iter_mut()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
        {
            endpoint
                .parking
                .receive_identities(Some(generation), identities);
            if let Some(snapshot) = endpoint.snapshot.as_deref() {
                endpoint
                    .parking
                    .observe_snapshot(endpoint.snapshot_generation, snapshot);
            }
        }
        self.update_parking_size(previous_size);
    }

    pub(crate) fn receive_endpoint_parking_error(
        &mut self,
        endpoint_id: &ClientEndpointId,
        generation: u64,
        error: PanePresentationError,
    ) {
        let previous_size = self.parking_size();
        if let Some(endpoint) = self
            .endpoints
            .iter_mut()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
        {
            endpoint.parking.receive_error(Some(generation), error);
        }
        self.update_parking_size(previous_size);
    }

    pub(crate) fn tick_parking(&mut self, now: Instant) -> ClientShellInput {
        let mut outcome = ClientShellInput::default();
        let previous_size = self.parking_size();
        let mut notice = None;
        for endpoint in &mut self.endpoints {
            if endpoint.status != ClientEndpointStatus::Online {
                continue;
            }
            if let Some(request) = endpoint.parking.poll(now) {
                outcome.actions.push(ClientShellAction::PanePresentation {
                    endpoint_id: endpoint.endpoint_id.clone(),
                    generation: endpoint.snapshot_generation,
                    request,
                    releases: Vec::new(),
                });
                outcome.repaint = true;
            }
            outcome.repaint |= endpoint.parking.take_repaint();
            if endpoint.endpoint_id == self.active_endpoint_id {
                notice = endpoint.parking.take_notice();
            }
        }
        for action in &mut outcome.actions {
            let ClientShellAction::PanePresentation {
                endpoint_id,
                releases,
                ..
            } = action
            else {
                continue;
            };
            if endpoint_id == &self.active_endpoint_id {
                let hidden = self
                    .active_parking()
                    .map(|parking| parking.hidden_panes.clone())
                    .unwrap_or_default();
                let mut release = ClientShellInput::default();
                let targets = (!self.has_parking_preview()).then_some(&hidden);
                self.release_input_leases(&mut release, targets);
                *releases = release.requests;
            }
        }
        self.update_parking_size(previous_size);
        outcome.resize = std::mem::take(&mut self.parked.resize_pending);
        outcome.repaint |= outcome.resize;
        if let Some(message) = notice {
            self.receive_endpoint_unavailable(message);
            outcome.repaint = true;
        }
        outcome
    }

    fn parking_size(&self) -> Option<ClientSurfaceSize> {
        self.last_composed_size
            .map(|(cols, rows)| self.surface_size(cols, rows))
    }

    fn update_parking_size(&mut self, previous: Option<ClientSurfaceSize>) {
        if previous != self.parking_size() {
            self.parked.resize_pending = true;
            self.invalidate_pane_surface();
        }
    }

    pub(super) fn active_parking(&self) -> Option<&PaneParking> {
        self.endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
            .map(|endpoint| &endpoint.parking)
    }

    pub(super) fn parking_ready(&self) -> bool {
        self.active_parking().is_none_or(PaneParking::ready)
    }

    pub(super) fn pane_accepts_input(&self, pane: &str) -> bool {
        self.active_parking()
            .is_none_or(|parking| parking.accepts_input(pane))
    }
}

#[derive(Clone, Copy)]
pub(super) enum ParkingAction {
    Park,
    // Exercise retained full-pane presentation policy in compatibility tests.
    #[cfg(test)]
    Preview,
    Restore,
}

impl ClientShellState {
    pub(super) fn parking_action(
        &mut self,
        endpoint_id: ClientEndpointId,
        pane: String,
        action: ParkingAction,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let previous_size = self.parking_size();
        #[cfg(test)]
        let same_endpoint = endpoint_id == self.active_endpoint_id;
        #[cfg(test)]
        let preview_return = matches!(action, ParkingAction::Preview)
            .then(|| {
                self.active_parking()
                    .and_then(|parking| parking.preview_return.clone())
                    .or_else(|| self.focused_navigation_target())
            })
            .flatten();
        let replacement = matches!(action, ParkingAction::Park)
            .then(|| {
                let endpoint = self
                    .endpoints
                    .iter()
                    .find(|endpoint| endpoint.endpoint_id == endpoint_id)?;
                let snapshot = endpoint.snapshot.as_deref()?;
                let source = snapshot.panes.iter().find(|item| item.pane_id == pane)?;
                (!endpoint.parking.hides_pane(&pane)
                    && snapshot.panes.iter().all(|item| {
                        item.workspace_id != source.workspace_id
                            || item.pane_id == pane
                            || endpoint.parking.hides_pane(&item.pane_id)
                    }))
                .then(|| crate::api::schema::TabCreateParams {
                    workspace_id: Some(source.workspace_id.clone()),
                    cwd: source.foreground_cwd.clone().or_else(|| source.cwd.clone()),
                    focus: true,
                    label: None,
                    env: Default::default(),
                })
            })
            .flatten();
        let result = self
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.endpoint_id == endpoint_id)
            .ok_or_else(|| "This machine is no longer available".to_owned())
            .and_then(|endpoint| {
                if endpoint.status != ClientEndpointStatus::Online {
                    return Err("This machine is not connected".into());
                }
                if !endpoint.parking.available() {
                    return Err("Parking is unavailable on this server".into());
                }
                let terminal = endpoint
                    .parking
                    .terminal_for_pane(&pane)
                    .ok_or("This pane is no longer available")?
                    .to_owned();
                match action {
                    ParkingAction::Park => endpoint.parking.park(&pane),
                    #[cfg(test)]
                    ParkingAction::Preview => {
                        let continuing = same_endpoint && endpoint.parking.preview_pane().is_some();
                        endpoint.parking.preview(&terminal)?;
                        if !continuing {
                            endpoint.parking.preview_return = preview_return.clone();
                        }
                        Ok(())
                    }
                    ParkingAction::Restore => endpoint.parking.restore(&terminal),
                }
            });
        outcome.repaint = true;
        if let Err(message) = result {
            self.receive_endpoint_unavailable(message);
            return false;
        }
        if !matches!(action, ParkingAction::Park) {
            self.activate_endpoint(endpoint_id, outcome);
        }
        if let Some(replacement) = replacement {
            self.push_endpoint_method(crate::api::schema::Method::TabCreate(replacement), outcome);
        }
        self.selection = None;
        self.copy_mode = None;
        self.mode = ClientShellMode::Terminal;
        self.update_parking_size(previous_size);
        outcome.resize |= previous_size.is_none();
        outcome.resize |= std::mem::take(&mut self.parked.resize_pending);
        true
    }

    pub(super) fn focused_parking_action(
        &mut self,
        action: ParkingAction,
        outcome: &mut ClientShellInput,
    ) {
        if self.popup_terminal_id.is_some() && matches!(action, ParkingAction::Park) {
            self.receive_endpoint_unavailable("Select a workspace pane to park it".into());
            outcome.repaint = true;
            return;
        }
        let pane = match action {
            ParkingAction::Restore => self
                .active_parking()
                .and_then(PaneParking::preview_pane)
                .map(str::to_owned),
            _ => self
                .snapshot
                .as_deref()
                .and_then(|snapshot| snapshot.focused_pane_id.clone()),
        };
        if let Some(pane) = pane {
            self.parking_action(self.active_endpoint_id.clone(), pane, action, outcome);
        }
    }

    pub(super) fn open_parked(&mut self) {
        self.open_navigator_overlay();
        if let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() {
            navigator.parked = true;
            navigator.selected = None;
        }
    }

    pub(super) fn leave_parking_preview(&mut self) {
        let previous_size = self.parking_size();
        if let Some(endpoint) = self
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
        {
            endpoint.parking.leave_preview();
        }
        self.update_parking_size(previous_size);
    }

    pub(super) fn dismiss_parking_preview(&mut self, outcome: &mut ClientShellInput) {
        let target = self
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
            .and_then(|endpoint| endpoint.parking.preview_return.take());
        self.leave_parking_preview();
        if let Some(target) = target.filter(|target| self.navigation_target_valid(target)) {
            self.focus_or_activate(
                target.endpoint_id,
                ClientEndpointFocusTarget::Workspace(target.workspace_id),
                outcome,
            );
        }
        outcome.repaint = true;
        outcome.resize = true;
    }

    pub(super) fn has_parking_preview(&self) -> bool {
        self.active_parking()
            .is_some_and(|parking| parking.preview_pane().is_some())
    }

    pub(super) fn render_parking_chrome(
        &mut self,
        buffer: &mut Buffer,
        layout: ClientShellLayout,
    ) -> bool {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return false;
        };
        let Some(parking) = self.active_parking() else {
            return false;
        };
        if !parking.available() {
            return false;
        }
        let p = &self.config.palette;
        if let Some(pane_id) = parking
            .preview_pane()
            .filter(|pane| Some(*pane) == snapshot.focused_pane_id.as_deref())
        {
            let title = snapshot
                .panes
                .iter()
                .find(|pane| pane.pane_id == pane_id)
                .and_then(|pane| pane.label.as_deref())
                .unwrap_or("Parked pane");
            let area = layout.parking_header;
            if area.is_empty() {
                return false;
            }
            buffer.set_style(area, Style::default().bg(p.panel_bg));
            let button_width = 20.min(area.width);
            render::put_text(
                buffer,
                area.x,
                area.y,
                area.width.saturating_sub(button_width),
                &format!(" {title}"),
                Style::default().fg(p.overlay0),
            );
            let button = Rect::new(
                area.right().saturating_sub(button_width),
                area.y,
                button_width,
                1,
            );
            render::put_text(
                buffer,
                button.x,
                button.y,
                button.width,
                " unpark    dismiss ",
                Style::default().fg(p.accent),
            );
            self.hits.restore_pane = Rect::new(button.x, button.y, button.width.min(10), 1);
            self.hits.dismiss_parking_preview = Rect::new(
                button.x.saturating_add(10),
                button.y,
                button.width.saturating_sub(10),
                1,
            );
            return false;
        }
        let empty = snapshot
            .focused_workspace_id
            .as_deref()
            .is_some_and(|workspace| !parking.shows_workspace(workspace));
        if empty && !layout.pane_surface.is_empty() {
            let area = layout.pane_surface;
            if area.height >= 3 {
                let new_workspace = Rect::new(
                    area.x + area.width.saturating_sub(17) / 2,
                    (area.y + area.height / 2).saturating_sub(1).max(area.y),
                    area.width.min(17),
                    1,
                );
                render::put_text(
                    buffer,
                    new_workspace.x,
                    new_workspace.y,
                    new_workspace.width,
                    " New workspace ",
                    Style::default().fg(p.accent).bg(p.surface0),
                );
                self.hits.new_workspace_empty = new_workspace;
            }
            let button = Rect::new(
                area.x + area.width.saturating_sub(14) / 2,
                (area.y + area.height / 2)
                    .saturating_add(1)
                    .min(area.bottom().saturating_sub(1)),
                area.width.min(14),
                1,
            );
            render::put_text(
                buffer,
                button.x,
                button.y,
                button.width,
                " open parked ",
                Style::default().fg(p.accent).bg(p.surface0),
            );
            self.hits.open_parked = button;
        }
        empty
    }
}
