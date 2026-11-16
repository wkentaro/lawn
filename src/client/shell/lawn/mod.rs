use super::*;
use crate::protocol::endpoint::pane_presentation::{
    PaneIdentities, PanePresentationError, SetPanePresentation,
};
use std::path::PathBuf;

mod parked;
mod parking_store;
pub(super) use parked::{height as navigation_height, render as render_navigation, ParkedState};
mod parking;
pub(super) use parking::{PaneParking, RestorePreview};
mod workspaces;
pub(super) use workspaces::EmptyWorkspaceAction;

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
            // Peeking needs presentation; parking itself needs the methods.
            let methods = endpoint.methods.as_ref().is_none_or(|methods| {
                methods.contains("pane.park") && methods.contains("pane.unpark")
            });
            endpoint.parking.set_supported(supported && methods);
        }
        self.update_parking_size(previous_size);
    }

    pub(crate) fn set_endpoint_restore_preview_supported(
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
            let method = endpoint
                .methods
                .as_ref()
                .is_some_and(|methods| methods.contains("pane.unpark_to"));
            endpoint
                .parking
                .set_restore_preview_supported(supported && method);
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

    pub(crate) fn tick_parking(&mut self) -> ClientShellInput {
        let mut outcome = ClientShellInput::default();
        let previous_size = self.parking_size();
        if self.restore_controls_lost() {
            if let Some(endpoint) = self
                .endpoints
                .iter_mut()
                .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
            {
                endpoint.parking.leave_in_place();
            }
        }
        let mut notice = None;
        let mut migration = Vec::new();
        for endpoint in &mut self.endpoints {
            if endpoint.status != ClientEndpointStatus::Online {
                continue;
            }
            if let Some(request) = endpoint.parking.poll() {
                outcome.actions.push(ClientShellAction::PanePresentation {
                    endpoint_id: endpoint.endpoint_id.clone(),
                    generation: endpoint.snapshot_generation,
                    request,
                    restore_preview: endpoint.parking.restore_preview_request(),
                    releases: Vec::new(),
                });
                outcome.repaint = true;
            }
            outcome.repaint |= endpoint.parking.take_repaint();
            if endpoint.endpoint_id == self.active_endpoint_id {
                notice = endpoint.parking.take_notice();
                migration = endpoint.parking.take_migration();
            }
        }
        for pane_id in migration {
            self.push_park(pane_id, &mut outcome);
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
                releases.extend(release.requests);
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

    /// Every park hides its pane and releases its input before the request
    /// leaves, so nothing acts on the pane while the park is in flight.
    /// Endpoint methods dispatch ahead of plain requests, so the releases
    /// travel in a presentation action queued before the park.
    fn push_park(&mut self, pane_id: String, outcome: &mut ClientShellInput) {
        let mut release = ClientShellInput::default();
        self.release_input_leases(&mut release, Some(&HashSet::from([pane_id.clone()])));
        let presentation = self
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
            .and_then(|endpoint| {
                endpoint.parking.park(&pane_id);
                let request = endpoint.parking.poll()?;
                Some((
                    endpoint.snapshot_generation,
                    request,
                    endpoint.parking.restore_preview_request(),
                ))
            });
        if let Some((generation, request, restore_preview)) = presentation {
            outcome.actions.push(ClientShellAction::PanePresentation {
                endpoint_id: self.active_endpoint_id.clone(),
                generation,
                request,
                restore_preview,
                releases: release.requests,
            });
        } else {
            outcome.requests.extend(release.requests);
        }
        self.push_endpoint_method_with_kind(
            crate::api::schema::Method::PanePark(crate::api::schema::PaneTarget {
                pane_id: pane_id.clone(),
            }),
            PendingEndpointKind::PanePark { pane_id },
            outcome,
        );
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

const RESTORE_CONTROL: &str = " restore ";
const DISMISS_CONTROL: &str = " dismiss ";

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
        let same_endpoint = endpoint_id == self.active_endpoint_id;
        if matches!(action, ParkingAction::Restore)
            && same_endpoint
            && self.active_parking().is_some_and(|parking| {
                parking.preview_pane() == Some(pane.as_str()) && parking.restores_in_place()
            })
        {
            return self.planned_restore(&pane, outcome);
        }
        let in_place = matches!(action, ParkingAction::Preview)
            && self.restore_preview_fits(&endpoint_id, &pane);
        let preview_return = matches!(action, ParkingAction::Preview)
            .then(|| {
                self.active_parking()
                    .and_then(|parking| parking.preview_return.clone())
                    .or_else(|| self.focused_navigation_target())
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
                    ParkingAction::Park if endpoint.parking.hides_pane(&pane) => {
                        Err("This pane is already parked".into())
                    }
                    ParkingAction::Park => Ok(()),
                    ParkingAction::Preview => {
                        let continuing = same_endpoint && endpoint.parking.preview_pane().is_some();
                        endpoint.parking.preview(&terminal, in_place)?;
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
        match action {
            // Parking acts on the focused machine's own panes.
            ParkingAction::Park => {
                self.push_park(pane, outcome);
            }
            // Another machine's pane restores once that machine is active.
            ParkingAction::Restore => {
                self.focus_or_activate(
                    endpoint_id,
                    ClientEndpointFocusTarget::UnparkPane(pane),
                    outcome,
                );
            }
            ParkingAction::Preview => {
                self.activate_endpoint(endpoint_id, outcome);
            }
        }
        self.selection = None;
        self.copy_mode = None;
        self.mode = ClientShellMode::Terminal;
        self.update_parking_size(previous_size);
        outcome.resize |= previous_size.is_none();
        outcome.resize |= std::mem::take(&mut self.parked.resize_pending);
        true
    }

    fn planned_restore(&mut self, pane: &str, outcome: &mut ClientShellInput) -> bool {
        outcome.repaint = true;
        if !self.hits.panes.iter().any(|hit| hit.pane_id == pane) {
            return false;
        }
        let Some((params, attempt)) = self
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
            .and_then(|endpoint| endpoint.parking.begin_planned_restore(pane))
        else {
            return false;
        };
        let sent = self.push_endpoint_method_with_kind(
            crate::api::schema::Method::PaneUnparkTo(params),
            PendingEndpointKind::PaneUnparkTo { attempt },
            outcome,
        );
        if !sent {
            self.finish_planned_restore(attempt);
        }
        sent
    }

    pub(super) fn finish_planned_restore(&mut self, attempt: u64) {
        if let Some(endpoint) = self
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
        {
            endpoint.parking.finish_planned_restore(attempt);
        }
    }

    fn restore_preview_fits(&self, endpoint_id: &ClientEndpointId, pane: &str) -> bool {
        let Some(endpoint) = self
            .endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
        else {
            return false;
        };
        let (Some(target), Some(snapshot)) = (
            endpoint.parking.restore_target(pane),
            endpoint.snapshot.as_deref(),
        ) else {
            return false;
        };
        let Some(destination) = snapshot.tabs.iter().find(|tab| tab.tab_id == target.tab_id) else {
            return target.split.is_none();
        };
        let Some((cols, rows)) = self.last_composed_size else {
            return false;
        };
        let tabs = snapshot
            .tabs
            .iter()
            .filter(|tab| tab.workspace_id == destination.workspace_id)
            .count();
        cols <= self.config.mobile_width_threshold
            || (rows > 1 && !(self.config.hide_tab_bar_when_single_tab && tabs == 1))
            || (endpoint_id == &self.active_endpoint_id
                && self.pane_surface.as_ref().is_some_and(|surface| {
                    let top = surface.panes.iter().map(|pane| pane.rect.y).min();
                    surface
                        .panes
                        .iter()
                        .any(|pane| Some(pane.rect.y) == top && pane.inner_rect.y > pane.rect.y)
                }))
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
        let in_place = self
            .active_parking()
            .is_some_and(PaneParking::restores_in_place);
        let target = self
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
            .and_then(|endpoint| endpoint.parking.preview_return.take());
        self.leave_parking_preview();
        if let Some(target) = target.filter(|target| self.navigation_target_valid(target)) {
            if !in_place {
                self.focus_or_activate(
                    target.endpoint_id,
                    ClientEndpointFocusTarget::Workspace(target.workspace_id),
                    outcome,
                );
            } else if target.endpoint_id != self.active_endpoint_id {
                self.activate_endpoint(target.endpoint_id, outcome);
            }
        }
        outcome.repaint = true;
        outcome.resize = true;
    }

    pub(super) fn restore_preview(&self) -> Option<RestorePreview<'_>> {
        self.active_parking()?
            .restore_preview(self.snapshot.as_deref()?)
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
        let restoring = parking
            .restore_preview(snapshot)
            .is_some_and(|preview| preview.restoring);
        if let Some(pane_id) = parking
            .preview_pane()
            .filter(|pane| Some(*pane) == snapshot.focused_pane_id.as_deref())
        {
            let title = snapshot
                .parked_panes
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
                " restore   dismiss ",
                Style::default().fg(p.accent),
            );
            if restoring {
                render::put_text(
                    buffer,
                    button.x,
                    button.y,
                    button.width,
                    " restore",
                    Style::default().fg(p.overlay0),
                );
            } else {
                self.hits.restore_pane = Rect::new(button.x, button.y, button.width.min(10), 1);
            }
            self.hits.dismiss_parking_preview = Rect::new(
                button.x.saturating_add(10),
                button.y,
                button.width.saturating_sub(10),
                1,
            );
            return false;
        }
        self.render_empty_workspace(buffer, layout)
    }

    fn restore_controls_row(
        &self,
        layout: ClientShellLayout,
        pane_id: &str,
        mode_bar: Option<Rect>,
    ) -> Option<(Rect, bool)> {
        let border = self
            .pane_surface
            .as_ref()
            .and_then(|surface| surface.panes.iter().find(|pane| pane.pane_id == pane_id))
            .filter(|pane| pane.inner_rect.y > pane.rect.y && pane.rect.width > 2)
            .map(|pane| {
                Rect::new(
                    layout.pane_surface.x.saturating_add(pane.rect.x + 1),
                    layout.pane_surface.y.saturating_add(pane.rect.y),
                    pane.rect.width - 2,
                    1,
                )
            })
            .filter(|row| row.intersection(layout.pane_surface) == *row);
        let mobile = (!layout.mobile_header.is_empty()).then(|| {
            Rect::new(
                layout.mobile_header.x,
                layout.mobile_header.bottom().saturating_sub(1),
                layout
                    .mobile_header
                    .width
                    .saturating_sub(
                        super::mobile::MOBILE_BUTTON_WIDTH.min(layout.mobile_header.width),
                    )
                    .saturating_sub(1),
                1,
            )
        });
        let width = render::display_width(RESTORE_CONTROL) + render::display_width(DISMISS_CONTROL);
        [
            border.map(|row| (row, false)),
            Some((layout.tab_bar, true)),
            mobile.map(|row| (row, true)),
        ]
        .into_iter()
        .flatten()
        .find(|(row, _)| {
            row.height > 0
                && row.width >= width
                && mode_bar.is_none_or(|bar| bar.intersection(*row).is_empty())
        })
    }

    fn restore_controls_lost(&self) -> bool {
        let (Some(preview), Some((cols, rows)), Some(surface), Some(snapshot)) = (
            self.restore_preview()
                .filter(|preview| preview.target.split.is_some() && !preview.restoring),
            self.last_composed_size,
            self.pane_surface.as_ref(),
            self.snapshot.as_deref(),
        ) else {
            return false;
        };
        let layout = self.layout(cols, rows);
        surface.projection_revision == snapshot.revision
            && surface.frame.width == layout.pane_surface.width
            && surface.frame.height == layout.pane_surface.height
            && self
                .restore_controls_row(layout, &preview.pane.pane_id, None)
                .is_none()
    }

    pub(super) fn render_restore_controls(
        &self,
        frame: &mut FrameData,
        layout: ClientShellLayout,
        mode_bar: Option<Rect>,
    ) -> Option<(Option<Rect>, Rect, Rect)> {
        let preview = self
            .restore_preview()
            .filter(|preview| preview.target.split.is_some())?;
        let label = format!(
            "({})",
            preview.pane.label.as_deref().unwrap_or("Parked pane")
        );
        let restoring = preview.restoring;
        let p = &self.config.palette;
        let (row, chrome) = self.restore_controls_row(layout, &preview.pane.pane_id, mode_bar)?;
        let width = usize::from(frame.width);
        let first = usize::from(row.y) * width;
        let mut buffer = FrameData {
            cells: frame.cells.get(first..first + width)?.to_vec(),
            width: frame.width,
            height: 1,
            cursor: None,
            hyperlinks: Vec::new(),
            graphics: Vec::new(),
        }
        .to_ratatui_buffer()?;
        buffer.area.y = row.y;
        let controls_width =
            render::display_width(RESTORE_CONTROL) + render::display_width(DISMISS_CONTROL);
        let restore = Rect::new(
            row.right() - controls_width,
            row.y,
            render::display_width(RESTORE_CONTROL),
            1,
        );
        let dismiss = Rect::new(
            restore.right(),
            row.y,
            render::display_width(DISMISS_CONTROL),
            1,
        );
        let (start, control_style) = if chrome {
            let label = format!("{label} ");
            let label_width = render::display_width(&label).min(restore.x - row.x);
            let start = restore.x - label_width;
            buffer.set_style(
                Rect::new(start, row.y, row.right() - start, 1),
                Style::default().bg(p.panel_bg),
            );
            render::put_text(
                &mut buffer,
                start,
                row.y,
                label_width,
                &label,
                Style::default().fg(p.overlay0),
            );
            (start, Style::default())
        } else {
            let line = buffer[(row.right() - 1, row.y)].clone();
            for x in row.x..row.right() {
                buffer[(x, row.y)] = line.clone();
            }
            let style = line.style();
            render::put_text(
                &mut buffer,
                row.x,
                row.y,
                restore.x - row.x,
                &format!(" {label} "),
                style.add_modifier(Modifier::BOLD),
            );
            (row.x, style)
        };
        render::put_text(
            &mut buffer,
            restore.x,
            row.y,
            restore.width,
            RESTORE_CONTROL,
            control_style.fg(if restoring { p.overlay0 } else { p.accent }),
        );
        render::put_text(
            &mut buffer,
            dismiss.x,
            row.y,
            dismiss.width,
            DISMISS_CONTROL,
            control_style.fg(p.accent),
        );
        let drawn = Rect::new(start, row.y, row.right() - start, 1);
        buffer.area.y = 0;
        let line = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
        let (from, to) = (usize::from(drawn.x), usize::from(drawn.right()));
        for (target, cell) in frame.cells[first + from..first + to]
            .iter_mut()
            .zip(&line.cells[from..to])
        {
            *target = cell.clone();
        }
        Some(((!restoring).then_some(restore), dismiss, drawn))
    }
}
