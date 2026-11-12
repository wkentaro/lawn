use super::*;
use std::time::{Duration, Instant};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreviewTarget {
    endpoint_id: ClientEndpointId,
    pane_id: String,
    boot_id: String,
    generation: Option<u64>,
    max_cols: u16,
    max_rows: u16,
}

struct PendingPreview {
    target: PreviewTarget,
    request_id: String,
    epoch: u64,
    styled: bool,
}

enum PreviewContent {
    Text(String),
    Cells(crate::terminal::TerminalCellSnapshot),
}

#[derive(Default)]
pub(super) struct NavigatorPreview {
    pub(super) area: Rect,
    selected: Option<PreviewTarget>,
    pending: Option<PendingPreview>,
    content: Option<PreviewContent>,
    error: Option<String>,
    last_requested: Option<Instant>,
    epoch: u64,
}

pub(crate) enum PreviewDisplay<'a> {
    None,
    Loading,
    Text(&'a str),
    Cells(&'a crate::terminal::TerminalCellSnapshot),
    Message(&'a str),
}

impl ClientShellState {
    pub(super) fn clear_navigator_preview(&mut self) {
        self.navigator_preview.epoch = self.navigator_preview.epoch.wrapping_add(1);
        self.navigator_preview.selected = None;
        self.navigator_preview.area = Rect::default();
        self.navigator_preview.content = None;
        self.navigator_preview.error = None;
        self.navigator_preview.last_requested = None;
    }

    fn selected_preview_target(&self, area: Rect) -> Option<PreviewTarget> {
        if area.is_empty() {
            return None;
        }
        let ClientShellOverlay::Navigator(navigator) = self.overlay.as_ref()? else {
            return None;
        };
        let rows = aggregate_navigation::navigator_rows(
            &self.endpoints,
            &self.active_endpoint_id,
            navigator,
        );
        let ClientNavigatorTarget::Pane {
            endpoint_id,
            pane_id,
        } = aggregate_navigation::selected_navigator_target(&rows, navigator)?
        else {
            return None;
        };
        let endpoint = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == endpoint_id)?;
        Some(PreviewTarget {
            endpoint_id,
            pane_id,
            boot_id: endpoint.snapshot.as_ref()?.boot_id.clone(),
            generation: endpoint.snapshot_generation,
            max_cols: area.width,
            max_rows: area.height,
        })
    }

    pub(crate) fn navigator_preview_display(&self, area: Rect) -> PreviewDisplay<'_> {
        let Some(target) = self.selected_preview_target(area) else {
            return PreviewDisplay::None;
        };
        if self.navigator_preview.selected.as_ref() != Some(&target) {
            return PreviewDisplay::Loading;
        }
        if let Some(message) = self.navigator_preview.error.as_deref() {
            PreviewDisplay::Message(message)
        } else if let Some(content) = self.navigator_preview.content.as_ref() {
            match content {
                PreviewContent::Text(text) => PreviewDisplay::Text(text),
                PreviewContent::Cells(snapshot) => PreviewDisplay::Cells(snapshot),
            }
        } else {
            PreviewDisplay::Loading
        }
    }

    pub(crate) fn tick_navigator_preview(&mut self, now: Instant) -> ClientShellInput {
        let target = self.selected_preview_target(self.navigator_preview.area);
        let preview = &mut self.navigator_preview;
        let changed = preview.selected != target;
        let mut outcome = ClientShellInput::default();
        if changed {
            preview.epoch = preview.epoch.wrapping_add(1);
            preview.selected = target.clone();
            preview.content = None;
            preview.error = None;
            preview.last_requested = None;
            outcome.repaint = true;
        }
        let Some(target) = target else {
            return outcome;
        };
        // Keep the wire request in its endpoint lane until completion/timeout. Movement only
        // replaces the desired target, so even fast navigation has one global preview request.
        if preview.pending.is_some()
            || preview.error.is_some()
            || preview
                .last_requested
                .is_some_and(|last| now.saturating_duration_since(last) < REFRESH_INTERVAL)
        {
            return outcome;
        }
        let endpoint = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == target.endpoint_id);
        let message = match endpoint {
            None => Some("Server unavailable"),
            Some(endpoint) if endpoint.status != ClientEndpointStatus::Online => {
                Some("Server offline")
            }
            Some(endpoint)
                if endpoint.methods.as_ref().is_none_or(|methods| {
                    !methods.contains("pane.snapshot") && !methods.contains("pane.read")
                }) =>
            {
                Some("Preview unavailable on this server")
            }
            _ => None,
        };
        if let Some(message) = message {
            preview.error = Some(message.into());
            outcome.repaint = true;
            return outcome;
        }
        let method = if endpoint
            .and_then(|endpoint| endpoint.methods.as_ref())
            .is_some_and(|methods| methods.contains("pane.snapshot"))
        {
            crate::api::schema::Method::PaneSnapshot(crate::api::schema::PaneSnapshotParams {
                pane_id: target.pane_id.clone(),
                max_cols: target.max_cols,
                max_rows: target.max_rows,
            })
        } else {
            crate::api::schema::Method::PaneRead(crate::api::schema::PaneReadParams {
                pane_id: target.pane_id.clone(),
                source: crate::api::schema::ReadSource::Detection,
                lines: Some(40),
                format: crate::api::schema::ReadFormat::Text,
                strip_ansi: true,
                intent: crate::api::schema::ReadIntent::Passive,
            })
        };
        let request_id = format!("client-shell-preview:{}", self.next_request_id);
        self.next_request_id = self.next_request_id.saturating_add(1);
        preview.pending = Some(PendingPreview {
            target: target.clone(),
            request_id: request_id.clone(),
            epoch: preview.epoch,
            styled: matches!(method, crate::api::schema::Method::PaneSnapshot(_)),
        });
        preview.last_requested = Some(now);
        outcome.actions.push(ClientShellAction::NavigatorPreview {
            endpoint_id: target.endpoint_id,
            boot_id: target.boot_id,
            request: Box::new(crate::api::schema::Request {
                id: request_id,
                method,
            }),
        });
        outcome
    }

    pub(crate) fn handle_navigator_preview_result(
        &mut self,
        endpoint_id: &ClientEndpointId,
        generation: u64,
        boot_id: &str,
        request_id: &str,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> bool {
        let Some(pending) = self.navigator_preview.pending.as_ref() else {
            return false;
        };
        if pending.request_id != request_id || pending.target.endpoint_id != *endpoint_id {
            return false;
        }
        let pending = self
            .navigator_preview
            .pending
            .take()
            .expect("checked preview request");
        if pending.target.boot_id != boot_id
            || pending
                .target
                .generation
                .is_some_and(|expected| expected != generation)
            || pending.epoch != self.navigator_preview.epoch
            || self
                .selected_preview_target(self.navigator_preview.area)
                .as_ref()
                != Some(&pending.target)
        {
            return false;
        }
        match result {
            Ok(crate::api::schema::ResponseResult::PaneRead { read })
                if !pending.styled && read.pane_id == pending.target.pane_id =>
            {
                self.navigator_preview.content = Some(PreviewContent::Text(read.text));
                self.navigator_preview.error = None;
            }
            Ok(crate::api::schema::ResponseResult::PaneSnapshot { snapshot })
                if pending.styled
                    && snapshot.pane_id == pending.target.pane_id
                    && valid_snapshot(&snapshot.snapshot, &pending.target) =>
            {
                self.navigator_preview.content = Some(PreviewContent::Cells(snapshot.snapshot));
                self.navigator_preview.error = None;
            }
            Err(error) if error.code.as_deref() == Some("endpoint_busy") => return false,
            Ok(_) => self.navigator_preview.error = Some("Preview response unavailable".into()),
            Err(error) => self.navigator_preview.error = Some(error.message),
        }
        true
    }

    pub(crate) fn defer_navigator_preview(&mut self, request_id: &str) {
        if self
            .navigator_preview
            .pending
            .as_ref()
            .is_some_and(|pending| pending.request_id == request_id)
        {
            self.navigator_preview.pending = None;
        }
    }

    pub(crate) fn cancel_navigator_preview(&mut self, request_id: &str) -> bool {
        let Some(pending) = self.navigator_preview.pending.as_ref() else {
            return false;
        };
        if pending.request_id != request_id {
            return false;
        }
        let current = pending.epoch == self.navigator_preview.epoch
            && self
                .selected_preview_target(self.navigator_preview.area)
                .as_ref()
                == Some(&pending.target);
        self.navigator_preview.pending = None;
        if current {
            self.navigator_preview.error = Some("Preview unavailable on this server".into());
        }
        current
    }
}

fn valid_snapshot(
    snapshot: &crate::terminal::TerminalCellSnapshot,
    target: &PreviewTarget,
) -> bool {
    snapshot.width > 0
        && snapshot.height > 0
        && snapshot.width <= target.max_cols.min(snapshot.source_cols)
        && snapshot.height <= target.max_rows.min(snapshot.source_rows)
        && snapshot
            .start_row
            .checked_add(u32::from(snapshot.height))
            .is_some()
        && snapshot.cells.len() == usize::from(snapshot.width) * usize::from(snapshot.height)
        && snapshot.cells.iter().all(|cell| {
            !cell.symbol.chars().any(char::is_control)
                && unicode_width::UnicodeWidthStr::width(cell.symbol.as_str()) <= 2
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ClientShellState {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(
            &crate::config::Config::default(),
        ));
        state.set_snapshot(Box::new(super::super::tests::snapshot()));
        state.set_endpoint_methods(Some(vec!["pane.read".into()]));
        state.open_navigator_overlay();
        state.compose(120, 36);
        state
    }

    fn request(outcome: ClientShellInput) -> (ClientEndpointId, String, String) {
        let mut actions = outcome.actions;
        let Some(ClientShellAction::NavigatorPreview {
            endpoint_id,
            boot_id,
            request,
        }) = actions.pop()
        else {
            panic!("expected preview request");
        };
        assert!(actions.is_empty());
        (endpoint_id, boot_id, request.id)
    }

    fn read(pane_id: &str, text: &str) -> crate::api::schema::ResponseResult {
        crate::api::schema::ResponseResult::PaneRead {
            read: crate::api::schema::PaneReadResult {
                pane_id: pane_id.into(),
                workspace_id: "ws_1".into(),
                tab_id: "tab_1".into(),
                source: crate::api::schema::ReadSource::Detection,
                format: crate::api::schema::ReadFormat::Text,
                text: text.into(),
                revision: 0,
                truncated: false,
            },
        }
    }

    #[test]
    fn request_coalesces_and_late_response_cannot_replace_selection() {
        let mut state = state();
        let now = Instant::now();
        let (endpoint, boot, first) = request(state.tick_navigator_preview(now));
        assert!(state
            .tick_navigator_preview(now + Duration::from_secs(2))
            .actions
            .is_empty());
        let ClientShellOverlay::Navigator(navigator) = state.overlay.as_mut().unwrap() else {
            panic!("navigator");
        };
        navigator.selected = Some(ClientNavigatorTarget::Workspace {
            endpoint_id: ClientEndpointId::Local,
            workspace_id: "ws_1".into(),
        });
        assert!(state
            .tick_navigator_preview(now + Duration::from_secs(3))
            .actions
            .is_empty());
        assert!(!state.handle_navigator_preview_result(
            &endpoint,
            0,
            &boot,
            &first,
            Ok(read("pane_1", "stale"))
        ));
        assert!(matches!(
            state.navigator_preview_display(state.navigator_preview.area),
            PreviewDisplay::None
        ));
        navigator_selected_pane(&mut state);
        let (_, _, second) = request(state.tick_navigator_preview(now + Duration::from_secs(4)));
        assert_ne!(first, second);
        assert!(!state.handle_navigator_preview_result(
            &endpoint,
            0,
            &boot,
            &first,
            Ok(read("pane_1", "stale"))
        ));
        assert!(state.handle_navigator_preview_result(
            &endpoint,
            0,
            &boot,
            &second,
            Ok(read("pane_1", "fresh"))
        ));
        assert!(matches!(
            state.navigator_preview_display(state.navigator_preview.area),
            PreviewDisplay::Text("fresh")
        ));
    }

    fn navigator_selected_pane(state: &mut ClientShellState) {
        let ClientShellOverlay::Navigator(navigator) = state.overlay.as_mut().unwrap() else {
            panic!("navigator");
        };
        navigator.selected = Some(ClientNavigatorTarget::Pane {
            endpoint_id: ClientEndpointId::Local,
            pane_id: "pane_1".into(),
        });
    }

    #[test]
    fn close_and_reopen_rejects_old_response_without_changing_focus() {
        let mut state = state();
        let now = Instant::now();
        let (endpoint, boot, first) = request(state.tick_navigator_preview(now));
        let focus = state.snapshot.as_ref().unwrap().focused_pane_id.clone();
        state.overlay = None;
        assert!(state.tick_navigator_preview(now).actions.is_empty());
        state.open_navigator_overlay();
        state.compose(120, 36);
        assert!(state.tick_navigator_preview(now).actions.is_empty());
        assert!(!state.handle_navigator_preview_result(
            &endpoint,
            0,
            &boot,
            &first,
            Ok(read("pane_1", "old"))
        ));
        assert!(matches!(
            state.navigator_preview_display(state.navigator_preview.area),
            PreviewDisplay::Loading
        ));
        assert_eq!(state.snapshot.as_ref().unwrap().focused_pane_id, focus);
    }

    #[test]
    fn overlapping_pane_ids_on_inactive_endpoint_do_not_cross_contaminate() {
        let mut state = state();
        let profile = SavedSshEndpoint {
            id: crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef")
                .unwrap(),
            label: "Build".into(),
            target: "dev@build.example".into(),
            session: "agents".into(),
            enabled: true,
        };
        let remote = ClientEndpointId::Ssh(profile.id.clone());
        state.set_endpoint_catalog(&[profile]);
        state.set_endpoint_status(&remote, ClientEndpointStatus::Online);
        let mut snapshot = super::super::tests::snapshot();
        snapshot.boot_id = "remote-boot".into();
        state.set_endpoint_snapshot(&remote, Box::new(snapshot));
        state.set_endpoint_methods_for(&remote, Some(vec!["pane.read".into()]));
        let ClientShellOverlay::Navigator(navigator) = state.overlay.as_mut().unwrap() else {
            panic!("navigator");
        };
        navigator.selected = Some(ClientNavigatorTarget::Pane {
            endpoint_id: remote.clone(),
            pane_id: "pane_1".into(),
        });
        let (endpoint, boot, id) = request(state.tick_navigator_preview(Instant::now()));
        assert_eq!(endpoint, remote);
        assert_eq!(boot, "remote-boot");
        assert_eq!(state.active_endpoint_id, ClientEndpointId::Local);
        assert!(!state.handle_navigator_preview_result(
            &ClientEndpointId::Local,
            0,
            "boot-1",
            &id,
            Ok(read("pane_1", "wrong"))
        ));
        assert!(matches!(
            state.navigator_preview_display(state.navigator_preview.area),
            PreviewDisplay::Loading
        ));
        assert!(state.handle_navigator_preview_result(
            &remote,
            0,
            &boot,
            &id,
            Ok(read("pane_1", "right"))
        ));
        assert!(matches!(
            state.navigator_preview_display(state.navigator_preview.area),
            PreviewDisplay::Text("right")
        ));
        assert_eq!(
            state.snapshot.as_ref().unwrap().focused_pane_id.as_deref(),
            Some("pane_1")
        );
    }

    #[test]
    fn obsolete_preview_cancellation_does_not_disable_the_current_preview() {
        for reopen in [false, true] {
            let mut state = state();
            let mut snapshot = super::super::tests::snapshot();
            let mut second = snapshot.panes[0].clone();
            second.pane_id = "pane_2".into();
            snapshot.panes.push(second);
            state.set_snapshot(Box::new(snapshot));
            let now = Instant::now();
            let (_, _, old) = request(state.tick_navigator_preview(now));
            if reopen {
                state.overlay = None;
                state.open_navigator_overlay();
                state.compose(120, 36);
            } else {
                let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
                    panic!("navigator");
                };
                navigator.selected = Some(ClientNavigatorTarget::Pane {
                    endpoint_id: ClientEndpointId::Local,
                    pane_id: "pane_2".into(),
                });
            }
            assert!(state.tick_navigator_preview(now).actions.is_empty());
            assert!(!state.cancel_navigator_preview(&old));
            let (_, _, current) = request(state.tick_navigator_preview(now));
            assert!(state.cancel_navigator_preview(&current));
            assert!(matches!(
                state.navigator_preview_display(state.navigator_preview.area),
                PreviewDisplay::Message(_)
            ));
        }
    }

    #[test]
    fn missing_advertisement_disables_only_preview() {
        let mut state = state();
        state.set_endpoint_methods(None);
        assert!(state
            .tick_navigator_preview(Instant::now())
            .actions
            .is_empty());
        assert!(matches!(
            state.navigator_preview_display(state.navigator_preview.area),
            PreviewDisplay::Message(_)
        ));
        assert!(state.visible_endpoint_notice.is_none());
        assert_eq!(state.active_endpoint_id, ClientEndpointId::Local);
    }

    #[test]
    fn wide_and_stacked_previews_clip_text_without_touching_the_tree() {
        for (width, height) in [(120, 36), (70, 24)] {
            let state = state();
            let ClientShellOverlay::Navigator(navigator) = state.overlay.as_ref().unwrap() else {
                panic!("navigator");
            };
            let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
            let rendered = render::render_navigator_overlay(
                &mut buffer,
                navigator,
                &state.endpoints,
                &state.active_endpoint_id,
                &state.config.palette,
            )
            .unwrap();
            let area = rendered.navigator_content_preview;
            assert!(!area.is_empty());
            assert_eq!(area.intersection(rendered.area), area);
            let close = rendered.navigator_close;
            assert_eq!(
                (close.x..close.right())
                    .map(|x| buffer[(x, close.y)].symbol())
                    .collect::<String>(),
                "Esc close"
            );
            assert!(close.intersection(rendered.navigator_preview).is_empty());
            for (row, _) in &rendered.navigator_rows {
                assert!(row.intersection(area).is_empty());
            }
            let before = buffer.clone();
            let text = (0..area.height + 2)
                .map(|index| format!("{index} {}", "界".repeat(100)))
                .collect::<Vec<_>>()
                .join("\n");
            render::render_navigator_preview(
                &mut buffer,
                area,
                PreviewDisplay::Text(&text),
                &state.config.palette,
            );
            assert_eq!(buffer[(area.x, area.y)].symbol(), "2");
            for y in 0..height {
                for x in 0..width {
                    if !area.contains((x, y).into()) {
                        assert_eq!(buffer[(x, y)], before[(x, y)]);
                    }
                }
            }
        }
    }

    #[test]
    fn unsupported_and_failure_are_preview_local() {
        let mut state = state();
        state.set_endpoint_methods(Some(vec![]));
        let now = Instant::now();
        assert!(state.tick_navigator_preview(now).actions.is_empty());
        assert!(matches!(
            state.navigator_preview_display(state.navigator_preview.area),
            PreviewDisplay::Message(_)
        ));
        assert!(state.visible_endpoint_notice.is_none());
        state.set_endpoint_methods(Some(vec!["pane.read".into()]));
        navigator_selected_pane(&mut state);
        state.overlay = None;
        state.tick_navigator_preview(now);
        state.open_navigator_overlay();
        state.compose(120, 36);
        let (endpoint, boot, id) = request(state.tick_navigator_preview(now));
        assert!(state.handle_navigator_preview_result(
            &endpoint,
            0,
            &boot,
            &id,
            Err(ClientShellEndpointError {
                code: Some("endpoint_timeout".into()),
                message: "Preview timed out".into(),
            })
        ));
        assert!(matches!(
            state.navigator_preview_display(state.navigator_preview.area),
            PreviewDisplay::Message("Preview timed out")
        ));
        assert!(state
            .tick_navigator_preview(now + Duration::from_secs(10))
            .actions
            .is_empty());
        assert!(state.visible_endpoint_notice.is_none());
    }

    fn styled_state() -> ClientShellState {
        let mut state = state();
        state.set_endpoint_methods(Some(vec!["pane.read".into(), "pane.snapshot".into()]));
        state
    }

    fn cells(width: u16, height: u16) -> crate::terminal::TerminalCellSnapshot {
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
        buffer.set_style(
            buffer.area,
            Style::default()
                .fg(ratatui::style::Color::Reset)
                .bg(ratatui::style::Color::Reset),
        );
        buffer[(0, 0)].set_symbol("R").set_style(
            Style::default()
                .fg(ratatui::style::Color::Rgb(240, 80, 90))
                .bg(ratatui::style::Color::Indexed(22))
                .add_modifier(Modifier::BOLD),
        );
        crate::terminal::TerminalCellSnapshot {
            default_fg: crate::protocol::color_to_u32(ratatui::style::Color::Reset),
            default_bg: crate::protocol::color_to_u32(ratatui::style::Color::Reset),
            source_cols: width,
            source_rows: height,
            start_row: 0,
            width,
            height,
            cells: buffer
                .content
                .iter()
                .map(crate::protocol::CellData::from_ratatui_cell)
                .collect(),
        }
    }

    fn snapshot_response(
        snapshot: crate::terminal::TerminalCellSnapshot,
    ) -> crate::api::schema::ResponseResult {
        crate::api::schema::ResponseResult::PaneSnapshot {
            snapshot: crate::api::schema::PaneSnapshotResult {
                pane_id: "pane_1".into(),
                snapshot,
            },
        }
    }

    #[test]
    fn advertised_snapshot_uses_visible_geometry_and_legacy_server_keeps_text() {
        for (mut state, styled) in [(styled_state(), true), (state(), false)] {
            let area = state.navigator_preview.area;
            let outcome = state.tick_navigator_preview(Instant::now());
            let [ClientShellAction::NavigatorPreview { request, .. }] = outcome.actions.as_slice()
            else {
                panic!("preview request");
            };
            match &request.method {
                crate::api::schema::Method::PaneSnapshot(params) => {
                    assert!(styled);
                    assert_eq!(
                        (params.max_cols, params.max_rows),
                        (area.width, area.height)
                    );
                }
                crate::api::schema::Method::PaneRead(params) => {
                    assert!(!styled);
                    assert_eq!(params.source, crate::api::schema::ReadSource::Detection);
                    assert_eq!(params.format, crate::api::schema::ReadFormat::Text);
                }
                _ => panic!("unexpected preview method"),
            }
        }
        let mut state = styled_state();
        state.compose(120, 7);
        assert!(state
            .tick_navigator_preview(Instant::now())
            .actions
            .is_empty());
    }

    #[test]
    fn snapshot_busy_keeps_cells_and_retries_without_a_notice() {
        let mut state = styled_state();
        let now = Instant::now();
        let (endpoint, boot, id) = request(state.tick_navigator_preview(now));
        assert!(state.handle_navigator_preview_result(
            &endpoint,
            0,
            &boot,
            &id,
            Ok(snapshot_response(cells(2, 2)))
        ));
        let (_, _, id) = request(state.tick_navigator_preview(now + REFRESH_INTERVAL));
        assert!(!state.handle_navigator_preview_result(
            &endpoint,
            0,
            &boot,
            &id,
            Err(ClientShellEndpointError {
                code: Some("endpoint_busy".into()),
                message: "redrawing".into()
            })
        ));
        assert!(matches!(
            state.navigator_preview_display(state.navigator_preview.area),
            PreviewDisplay::Cells(_)
        ));
        assert!(state.visible_endpoint_notice.is_none());
        request(state.tick_navigator_preview(now + REFRESH_INTERVAL * 2));
    }

    #[test]
    fn skipped_composition_preserves_visible_preview_geometry_and_response() {
        for mismatch in ["pending", "generation", "revision"] {
            let mut state = styled_state();
            let surface = crate::protocol::PaneSurfaceFrame {
                boot_id: "boot-1".into(),
                projection_revision: 1,
                surface_revision: 1,
                frame: crate::protocol::FrameData::from_ratatui_buffer(
                    &Buffer::empty(Rect::new(0, 0, 4, 2)),
                    None,
                ),
                panes: Vec::new(),
                splits: Vec::new(),
                popup: None,
                graphics: crate::protocol::SurfaceGraphicsScene::default(),
            };
            state.set_pane_surface(surface.clone());
            assert!(state.compose(120, 36).is_some());
            let area = state.navigator_preview.area;
            let (endpoint, boot, id) = request(state.tick_navigator_preview(Instant::now()));
            match mismatch {
                "pending" => state.pending_pane_surface = Some(surface),
                "generation" => state.pane_surface_generation = Some(99),
                "revision" => state.pane_surface.as_mut().unwrap().projection_revision = 99,
                _ => unreachable!(),
            }
            assert!(state.compose(70, 24).is_none());
            assert_eq!(state.navigator_preview.area, area);
            assert!(state.handle_navigator_preview_result(
                &endpoint,
                0,
                &boot,
                &id,
                Ok(snapshot_response(cells(2, 2))),
            ));
            assert!(matches!(
                state.navigator_preview_display(state.navigator_preview.area),
                PreviewDisplay::Cells(_)
            ));
        }
    }

    #[test]
    fn snapshot_rejects_stale_geometry_boot_and_generation() {
        for change in ["geometry", "boot", "generation"] {
            let mut state = styled_state();
            state.endpoints[0].snapshot_generation = Some(7);
            let (endpoint, boot, id) = request(state.tick_navigator_preview(Instant::now()));
            let generation = if change == "generation" { 8 } else { 7 };
            let response_boot = if change == "boot" {
                "different-boot"
            } else {
                &boot
            };
            if change == "geometry" {
                state.compose(70, 24);
            }
            assert!(!state.handle_navigator_preview_result(
                &endpoint,
                generation,
                response_boot,
                &id,
                Ok(snapshot_response(cells(2, 2)))
            ));
            assert!(!matches!(
                state.navigator_preview_display(state.navigator_preview.area),
                PreviewDisplay::Cells(_)
            ));
        }
    }

    #[test]
    fn malformed_snapshot_is_preview_local() {
        for change in [
            "length", "width", "height", "source", "overflow", "control", "wide",
        ] {
            let mut state = styled_state();
            let (endpoint, boot, id) = request(state.tick_navigator_preview(Instant::now()));
            let mut snapshot = cells(2, 2);
            match change {
                "length" => {
                    snapshot.cells.pop();
                }
                "width" => snapshot.width = state.navigator_preview.area.width + 1,
                "height" => snapshot.height = 0,
                "source" => snapshot.source_cols = 1,
                "overflow" => snapshot.start_row = u32::MAX,
                "control" => snapshot.cells[0].symbol = "\u{1b}[31m".into(),
                "wide" => snapshot.cells[0].symbol = "abc".into(),
                _ => unreachable!(),
            }
            assert!(state.handle_navigator_preview_result(
                &endpoint,
                0,
                &boot,
                &id,
                Ok(snapshot_response(snapshot))
            ));
            assert!(matches!(
                state.navigator_preview_display(state.navigator_preview.area),
                PreviewDisplay::Message("Preview response unavailable")
            ));
            assert!(state.visible_endpoint_notice.is_none());
        }
    }

    #[test]
    fn styled_previews_replace_cells_without_touching_the_tree() {
        for (width, height) in [(120, 36), (70, 24)] {
            let state = state();
            let ClientShellOverlay::Navigator(navigator) = state.overlay.as_ref().unwrap() else {
                panic!("navigator");
            };
            let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
            let rendered = render::render_navigator_overlay(
                &mut buffer,
                navigator,
                &state.endpoints,
                &state.active_endpoint_id,
                &state.config.palette,
            )
            .unwrap();
            let area = rendered.navigator_content_preview;
            let before = buffer.clone();
            let snapshot = cells(area.width, area.height);
            buffer.set_style(area, Style::default().add_modifier(Modifier::ITALIC));
            render::render_navigator_preview(
                &mut buffer,
                area,
                PreviewDisplay::Cells(&snapshot),
                &state.config.palette,
            );
            for y in 0..height {
                for x in 0..width {
                    if area.contains((x, y).into()) {
                        let expected = &snapshot.cells[usize::from(y - area.y)
                            * usize::from(area.width)
                            + usize::from(x - area.x)];
                        assert_eq!(
                            crate::protocol::CellData::from_ratatui_cell(&buffer[(x, y)]),
                            *expected
                        );
                    } else {
                        assert_eq!(buffer[(x, y)], before[(x, y)]);
                    }
                }
            }
        }
    }

    #[test]
    fn shorter_snapshot_clears_old_cells_with_native_terminal_defaults() {
        let state = state();
        let area = Rect::new(1, 1, 5, 4);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 8, 6));
        buffer.set_string(0, 0, "outside", Style::default());
        let outside = buffer[(0, 0)].clone();
        let previous = cells(area.width, area.height);
        render::render_navigator_preview(
            &mut buffer,
            area,
            PreviewDisplay::Cells(&previous),
            &state.config.palette,
        );
        for background in [
            ratatui::style::Color::Reset,
            ratatui::style::Color::Rgb(17, 34, 51),
        ] {
            for y in area.y..area.bottom() {
                for x in area.x..area.right() {
                    buffer[(x, y)].set_symbol("X").set_style(
                        Style::default()
                            .bg(ratatui::style::Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    );
                }
            }
            let mut snapshot = cells(2, 1);
            snapshot.default_fg =
                crate::protocol::color_to_u32(ratatui::style::Color::Rgb(171, 205, 239));
            snapshot.default_bg = crate::protocol::color_to_u32(background);
            render::render_navigator_preview(
                &mut buffer,
                area,
                PreviewDisplay::Cells(&snapshot),
                &state.config.palette,
            );
            assert_eq!(
                crate::protocol::CellData::from_ratatui_cell(&buffer[(area.x, area.y)]),
                snapshot.cells[0]
            );
            for y in 0..area.height {
                for x in 0..area.width {
                    if y == 0 && x < snapshot.width {
                        continue;
                    }
                    let cell = &buffer[(area.x + x, area.y + y)];
                    assert_eq!(cell.symbol(), " ");
                    assert_eq!(cell.fg, ratatui::style::Color::Rgb(171, 205, 239));
                    assert_eq!(cell.bg, background);
                    assert!(cell.modifier.is_empty());
                    assert!(!cell.skip);
                }
            }
            assert_eq!(buffer[(0, 0)], outside);
        }
    }

    #[test]
    fn styled_preview_blanks_a_wide_character_at_the_clip_boundary() {
        let state = state();
        let mut snapshot = cells(3, 1);
        snapshot.cells[1].symbol = "界".into();
        snapshot.cells[2].symbol.clear();
        let area = Rect::new(1, 1, 2, 1);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 5, 3));
        render::render_navigator_preview(
            &mut buffer,
            area,
            PreviewDisplay::Cells(&snapshot),
            &state.config.palette,
        );
        assert_eq!(buffer[(2, 1)].symbol(), " ");
        assert_eq!(buffer[(3, 1)].symbol(), " ");
    }

    #[test]
    #[ignore = "writes deterministic before/after picker frames for PR screenshots"]
    fn navigator_preview_visual_evidence() {
        let directory = std::path::PathBuf::from(
            std::env::var_os("LAWN_PREVIEW_EVIDENCE_DIR").expect("LAWN_PREVIEW_EVIDENCE_DIR"),
        );
        let state = state();
        let ClientShellOverlay::Navigator(navigator) = state.overlay.as_ref().unwrap() else {
            panic!("navigator");
        };
        let mut before = Buffer::empty(Rect::new(0, 0, 120, 36));
        let rendered = render::render_navigator_overlay(
            &mut before,
            navigator,
            &state.endpoints,
            &state.active_endpoint_id,
            &state.config.palette,
        )
        .unwrap();
        let area = rendered.navigator_content_preview;
        let lines = [
            ("$ just check", ratatui::style::Color::White, Modifier::BOLD),
            (
                "Checking lawn v0.1.0",
                ratatui::style::Color::Cyan,
                Modifier::empty(),
            ),
            (
                "PASS  terminal colors and backgrounds",
                ratatui::style::Color::Green,
                Modifier::BOLD,
            ),
            (
                "PASS  indexed and RGB colors",
                ratatui::style::Color::Green,
                Modifier::empty(),
            ),
            (
                "WARN  1 skipped test",
                ratatui::style::Color::Yellow,
                Modifier::empty(),
            ),
            (
                "diff --git a/src/client.rs b/src/client.rs",
                ratatui::style::Color::White,
                Modifier::DIM,
            ),
            (
                "- plain preview text",
                ratatui::style::Color::Red,
                Modifier::empty(),
            ),
            (
                "+ original terminal styling",
                ratatui::style::Color::Green,
                Modifier::empty(),
            ),
            (
                "ready for review",
                ratatui::style::Color::Rgb(220, 160, 245),
                Modifier::ITALIC,
            ),
        ];
        let mut terminal = Buffer::empty(Rect::new(0, 0, area.width, area.height));
        terminal.set_style(
            terminal.area,
            Style::default()
                .fg(ratatui::style::Color::White)
                .bg(ratatui::style::Color::Rgb(14, 14, 18)),
        );
        for (y, (text, color, modifier)) in lines.iter().enumerate() {
            terminal.set_string(
                0,
                y as u16,
                text,
                Style::default()
                    .fg(*color)
                    .bg(ratatui::style::Color::Rgb(14, 14, 18))
                    .add_modifier(*modifier),
            );
        }
        terminal.set_string(
            0,
            10,
            " selected text ",
            Style::default()
                .fg(ratatui::style::Color::Black)
                .bg(ratatui::style::Color::Cyan),
        );
        let text = lines
            .iter()
            .map(|(text, _, _)| *text)
            .chain(["", " selected text "])
            .collect::<Vec<_>>()
            .join("\n");
        let mut after = before.clone();
        render::render_navigator_preview(
            &mut before,
            area,
            PreviewDisplay::Text(&text),
            &state.config.palette,
        );
        let snapshot = crate::terminal::TerminalCellSnapshot {
            default_fg: crate::protocol::color_to_u32(ratatui::style::Color::White),
            default_bg: crate::protocol::color_to_u32(ratatui::style::Color::Rgb(14, 14, 18)),
            source_cols: area.width,
            source_rows: area.height,
            start_row: 0,
            width: area.width,
            height: area.height,
            cells: terminal
                .content
                .iter()
                .map(crate::protocol::CellData::from_ratatui_cell)
                .collect(),
        };
        render::render_navigator_preview(
            &mut after,
            area,
            PreviewDisplay::Cells(&snapshot),
            &state.config.palette,
        );
        for (name, buffer) in [("before", before), ("after", after)] {
            std::fs::write(
                directory.join(format!("{name}.json")),
                serde_json::to_vec(&crate::protocol::FrameData::from_ratatui_buffer(
                    &buffer, None,
                ))
                .unwrap(),
            )
            .unwrap();
        }
    }
}
