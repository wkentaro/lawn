use super::*;
use std::time::{Duration, Instant};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreviewTarget {
    endpoint_id: ClientEndpointId,
    pane_id: String,
    boot_id: String,
    generation: Option<u64>,
}

struct PendingPreview {
    target: PreviewTarget,
    request_id: String,
    epoch: u64,
}

#[derive(Default)]
pub(super) struct NavigatorPreview {
    selected: Option<PreviewTarget>,
    pending: Option<PendingPreview>,
    content: Option<String>,
    error: Option<String>,
    last_requested: Option<Instant>,
    epoch: u64,
}

pub(crate) enum PreviewDisplay<'a> {
    None,
    Loading,
    Text(&'a str),
    Message(&'a str),
}

impl ClientShellState {
    pub(super) fn clear_navigator_preview(&mut self) {
        self.navigator_preview.epoch = self.navigator_preview.epoch.wrapping_add(1);
        self.navigator_preview.selected = None;
        self.navigator_preview.content = None;
        self.navigator_preview.error = None;
        self.navigator_preview.last_requested = None;
    }

    fn selected_preview_target(&self) -> Option<PreviewTarget> {
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
        })
    }

    pub(crate) fn navigator_preview_display(&self) -> PreviewDisplay<'_> {
        let Some(target) = self.selected_preview_target() else {
            return PreviewDisplay::None;
        };
        if self.navigator_preview.selected.as_ref() != Some(&target) {
            return PreviewDisplay::Loading;
        }
        if let Some(message) = self.navigator_preview.error.as_deref() {
            PreviewDisplay::Message(message)
        } else if let Some(text) = self.navigator_preview.content.as_deref() {
            PreviewDisplay::Text(text)
        } else {
            PreviewDisplay::Loading
        }
    }

    pub(crate) fn tick_navigator_preview(&mut self, now: Instant) -> ClientShellInput {
        let target = self.selected_preview_target();
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
                if endpoint
                    .methods
                    .as_ref()
                    .is_none_or(|methods| !methods.contains("pane.read")) =>
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
        let request_id = format!("client-shell-preview:{}", self.next_request_id);
        self.next_request_id = self.next_request_id.saturating_add(1);
        preview.pending = Some(PendingPreview {
            target: target.clone(),
            request_id: request_id.clone(),
            epoch: preview.epoch,
        });
        preview.last_requested = Some(now);
        outcome.actions.push(ClientShellAction::NavigatorPreview {
            endpoint_id: target.endpoint_id,
            boot_id: target.boot_id,
            request: Box::new(crate::api::schema::Request {
                id: request_id,
                method: crate::api::schema::Method::PaneRead(crate::api::schema::PaneReadParams {
                    pane_id: target.pane_id,
                    source: crate::api::schema::ReadSource::Detection,
                    lines: Some(40),
                    format: crate::api::schema::ReadFormat::Text,
                    strip_ansi: true,
                    intent: crate::api::schema::ReadIntent::Passive,
                }),
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
            || self.selected_preview_target().as_ref() != Some(&pending.target)
        {
            return false;
        }
        match result {
            Ok(crate::api::schema::ResponseResult::PaneRead { read })
                if read.pane_id == pending.target.pane_id =>
            {
                self.navigator_preview.content = Some(read.text);
                self.navigator_preview.error = None;
            }
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
            && self.selected_preview_target().as_ref() == Some(&pending.target);
        self.navigator_preview.pending = None;
        if current {
            self.navigator_preview.error = Some("Preview unavailable on this server".into());
        }
        current
    }
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
            state.navigator_preview_display(),
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
            state.navigator_preview_display(),
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
        assert!(state.tick_navigator_preview(now).actions.is_empty());
        assert!(!state.handle_navigator_preview_result(
            &endpoint,
            0,
            &boot,
            &first,
            Ok(read("pane_1", "old"))
        ));
        assert!(matches!(
            state.navigator_preview_display(),
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
            state.navigator_preview_display(),
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
            state.navigator_preview_display(),
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
                state.navigator_preview_display(),
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
            state.navigator_preview_display(),
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
            state.navigator_preview_display(),
            PreviewDisplay::Message(_)
        ));
        assert!(state.visible_endpoint_notice.is_none());
        state.set_endpoint_methods(Some(vec!["pane.read".into()]));
        navigator_selected_pane(&mut state);
        state.overlay = None;
        state.tick_navigator_preview(now);
        state.open_navigator_overlay();
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
            state.navigator_preview_display(),
            PreviewDisplay::Message("Preview timed out")
        ));
        assert!(state
            .tick_navigator_preview(now + Duration::from_secs(10))
            .actions
            .is_empty());
        assert!(state.visible_endpoint_notice.is_none());
    }
}
