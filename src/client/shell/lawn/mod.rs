use super::*;
use crate::protocol::endpoint::pane_presentation::{
    PaneIdentities, PanePresentationError, SetPanePresentation,
};
use std::{path::PathBuf, time::Instant};

mod later;
mod parking_store;
pub(super) use later::{
    height as later_height, render as render_later, rows as later_rows, LaterState,
};
mod parking;
pub(super) use parking::PaneParking;

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
        outcome.resize = std::mem::take(&mut self.later.resize_pending);
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
            self.later.resize_pending = true;
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
                    ParkingAction::Preview => endpoint.parking.preview(&terminal),
                    ParkingAction::Restore => endpoint.parking.restore(&terminal),
                }
            });
        outcome.repaint = true;
        if let Err(message) = result {
            self.receive_endpoint_unavailable(message);
            return false;
        }
        if matches!(action, ParkingAction::Preview | ParkingAction::Restore) {
            self.activate_endpoint(endpoint_id, outcome);
        }
        self.selection = None;
        self.copy_mode = None;
        self.mode = ClientShellMode::Terminal;
        self.update_parking_size(previous_size);
        outcome.resize |= previous_size.is_none();
        outcome.resize |= std::mem::take(&mut self.later.resize_pending);
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

    pub(super) fn open_later(&mut self) {
        self.open_navigator_overlay();
        if let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() {
            navigator.filter = Some(ClientNavigatorFilter::Later);
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
            let button_width = 11.min(area.width);
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
                " Restore ",
                Style::default().fg(p.accent),
            );
            self.hits.restore_pane = button;
            return false;
        }
        let empty = snapshot
            .panes
            .iter()
            .filter(|pane| {
                Some(pane.workspace_id.as_str()) == snapshot.focused_workspace_id.as_deref()
            })
            .all(|pane| parking.hides_pane(&pane.pane_id))
            && snapshot.panes.iter().any(|pane| {
                Some(pane.workspace_id.as_str()) == snapshot.focused_workspace_id.as_deref()
            });
        if empty && !layout.pane_surface.is_empty() {
            let area = layout.pane_surface;
            let button = Rect::new(
                area.x + area.width.saturating_sub(14) / 2,
                area.y + area.height / 2,
                area.width.min(14),
                1,
            );
            render::put_text(
                buffer,
                button.x,
                button.y,
                button.width,
                " Open Later ",
                Style::default().fg(p.accent).bg(p.surface0),
            );
            self.hits.open_later = button;
        }
        empty
    }
}
