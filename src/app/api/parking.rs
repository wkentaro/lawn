use crate::api::schema::{
    EventData, EventEnvelope, EventKind, PaneInfo, PaneTarget, PaneUnparkParams, ResponseResult,
};
use crate::app::App;

use super::responses::{encode_error, encode_success};

impl App {
    pub(super) fn handle_pane_park(&mut self, id: String, target: PaneTarget) -> String {
        let channels = self.tab_channels();
        let parked = self
            .parse_pane_id(&target.pane_id)
            .and_then(|(ws_idx, pane_id)| {
                let layout_target = self.layout_update_target_after_pane_removal(ws_idx, pane_id);
                self.state.park_pane(ws_idx, pane_id, channels)?;
                Some((ws_idx, pane_id, layout_target))
            });
        let Some((ws_idx, pane_id, layout_target)) = parked else {
            return encode_error(id, "pane_not_found", "pane not found or already parked");
        };
        self.schedule_session_save();
        if let Some(pane) = self.pane_info(ws_idx, pane_id) {
            self.emit_event(EventEnvelope {
                event: EventKind::PaneParked,
                data: EventData::PaneParked { pane },
            });
        }
        // The tab it left, unless the pane was its last one and the tab is now hidden.
        if let Some((ws_idx, tab_idx)) = layout_target {
            self.emit_layout_updated_event(ws_idx, tab_idx);
        }
        encode_success(id, ResponseResult::Ok {})
    }

    pub(super) fn handle_pane_unpark(&mut self, id: String, params: PaneUnparkParams) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return encode_error(id, "pane_not_found", "pane not found");
        };
        if self
            .state
            .unpark_pane(ws_idx, pane_id, params.focus)
            .is_none()
        {
            return encode_error(id, "pane_not_parked", "pane is not parked");
        }
        self.schedule_session_save();
        let Some(pane) = self.pane_info(ws_idx, pane_id) else {
            return encode_error(id, "pane_not_found", "unparked pane not found");
        };
        self.emit_event(EventEnvelope {
            event: EventKind::PaneUnparked,
            data: EventData::PaneUnparked { pane: pane.clone() },
        });
        if let Some(tab_idx) = self.state.workspaces[ws_idx].find_tab_index_for_pane(pane_id) {
            self.emit_layout_updated_event(ws_idx, tab_idx);
        }
        encode_success(id, ResponseResult::PaneInfo { pane })
    }

    pub(super) fn parked_pane_infos(
        &self,
        pane_info: fn(&Self, usize, crate::layout::PaneId) -> Option<PaneInfo>,
    ) -> Vec<PaneInfo> {
        self.state
            .workspaces
            .iter()
            .enumerate()
            .flat_map(|(ws_idx, ws)| {
                ws.parked_tabs()
                    .iter()
                    .filter_map(move |tab| pane_info(self, ws_idx, tab.root_pane))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::api::schema::{AgentStatus, EventData, Request};
    use crate::app::App;
    use crate::config::Config;
    use crate::events::AppEvent;
    use crate::layout::PaneId;
    use crate::workspace::Workspace;
    use ratatui::layout::Direction;

    fn app(workspaces: Vec<Workspace>) -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.workspaces = workspaces;
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app
    }

    fn call(app: &mut App, method: &str, params: serde_json::Value) -> serde_json::Value {
        let request: Request = serde_json::from_value(serde_json::json!({
            "id": "req",
            "method": method,
            "params": params,
        }))
        .unwrap();
        serde_json::from_str(&app.handle_api_request(request)).unwrap()
    }

    /// Parks a pane and returns its public pane id.
    fn park(app: &mut App, pane: PaneId) -> String {
        let pane_id = app.public_pane_id(0, pane).unwrap();
        let response = call(app, "pane.park", serde_json::json!({ "pane_id": pane_id }));
        assert!(response.get("result").is_some(), "{response}");
        app.state.assert_invariants_for_test();
        pane_id
    }

    fn unpark(app: &mut App, pane_id: &str, focus: bool) -> serde_json::Value {
        let response = call(
            app,
            "pane.unpark",
            serde_json::json!({ "pane_id": pane_id, "focus": focus }),
        );
        assert!(response.get("result").is_some(), "{response}");
        app.state.assert_invariants_for_test();
        response
    }

    fn split_app() -> (App, PaneId) {
        let mut workspace = Workspace::test_new("split");
        let parked = workspace.test_split(Direction::Horizontal);
        (app(vec![workspace]), parked)
    }

    fn has_terminal(app: &App, terminal_id: &str) -> bool {
        app.state
            .terminals
            .keys()
            .any(|id| id.as_str() == terminal_id)
    }

    #[test]
    fn parking_the_last_pane_keeps_the_workspace_and_its_terminal() {
        let mut app = app(vec![Workspace::test_new("only")]);
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let workspace_id = app.public_workspace_id(0);

        let pane_id = park(&mut app, pane);

        assert_eq!(app.state.workspaces[0].visible_tab_count(), 0);
        let snapshot = app.session_snapshot();
        assert!(snapshot.panes.is_empty());
        let [parked] = snapshot.parked_panes.as_slice() else {
            panic!("expected one parked pane");
        };
        assert_eq!(parked.pane_id, pane_id);
        assert_eq!(parked.workspace_id, workspace_id);
        assert!(has_terminal(&app, &parked.terminal_id));

        let response = unpark(&mut app, &pane_id, true);
        assert_eq!(response["result"]["pane"]["pane_id"], pane_id, "{response}");
        assert_eq!(app.state.workspaces[0].tabs.len(), 1);
        assert!(app.session_snapshot().parked_panes.is_empty());
    }

    #[test]
    fn unparking_returns_the_pane_to_the_tab_it_left() {
        let (mut app, parked) = split_app();
        let tab_id = app.public_tab_id(0, 0).unwrap();

        let pane_id = park(&mut app, parked);
        assert_eq!(app.state.workspaces[0].tabs[0].panes.len(), 1);
        let response = unpark(&mut app, &pane_id, true);

        assert_eq!(response["result"]["pane"]["tab_id"], tab_id, "{response}");
        assert_eq!(app.state.workspaces[0].tabs[0].panes.len(), 2);
        assert_eq!(app.state.workspaces[0].tabs[0].layout.focused(), parked);
    }

    #[test]
    fn parking_and_unparking_announce_the_pane() {
        let (mut app, parked) = split_app();
        let tab_id = app.public_tab_id(0, 0).unwrap();

        let pane_id = park(&mut app, parked);
        unpark(&mut app, &pane_id, false);

        let events: Vec<_> = app
            .event_hub
            .events_after(0)
            .into_iter()
            .map(|(_, envelope)| envelope.data)
            .collect();
        let [EventData::PaneParked { pane: parked }, EventData::LayoutUpdated { layout: left }, EventData::PaneUnparked { pane: unparked }, EventData::LayoutUpdated { layout: returned }] =
            events.as_slice()
        else {
            panic!("expected parked then unparked with layouts, got {events:?}");
        };
        assert_eq!(left.panes.len(), 1);
        assert_eq!(returned.panes.len(), 2);
        assert_eq!(parked.pane_id, pane_id);
        assert_eq!(unparked.pane_id, pane_id);
        assert_eq!(unparked.tab_id, tab_id);
    }

    #[test]
    fn unparking_without_focus_leaves_focus_where_it_was() {
        let (mut app, parked) = split_app();
        let pane_id = park(&mut app, parked);
        let focused = app.state.workspaces[0].tabs[0].layout.focused();

        unpark(&mut app, &pane_id, false);

        assert_eq!(app.state.workspaces[0].tabs[0].layout.focused(), focused);
    }

    #[test]
    fn a_parked_pane_answers_to_its_pane_id() {
        let (mut app, parked) = split_app();
        let pane_id = park(&mut app, parked);

        assert_eq!(app.parse_pane_id(&pane_id), Some((0, parked)));
        let response = call(
            &mut app,
            "pane.report_agent",
            serde_json::json!({
                "pane_id": pane_id,
                "source": "test",
                "agent": "codex",
                "state": "working",
            }),
        );
        assert!(response.get("result").is_some(), "{response}");
        assert_eq!(
            app.session_snapshot().parked_panes[0].agent_status,
            AgentStatus::Working
        );
    }

    #[tokio::test]
    async fn a_parked_pane_stays_reachable_by_pane_and_agent_requests() {
        let (mut app, parked) = split_app();
        let pane_id = park(&mut app, parked);
        app.handle_internal_event(AppEvent::StateChanged {
            pane_id: parked,
            agent: Some(crate::detect::Agent::Codex),
            state: crate::detect::AgentState::Working,
            visible_blocker: false,
            visible_working: false,
            process_exited: false,
            observed_at: std::time::Instant::now(),
        });

        let (runtime, _rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(parked, runtime);

        let renamed = call(
            &mut app,
            "pane.rename",
            serde_json::json!({ "pane_id": pane_id, "label": "renamed" }),
        );
        assert_eq!(renamed["result"]["pane"]["label"], "renamed", "{renamed}");
        let read = call(
            &mut app,
            "pane.read",
            serde_json::json!({ "pane_id": pane_id, "source": "recent" }),
        );
        assert!(read.get("result").is_some(), "{read}");
        let agents = call(&mut app, "agent.list", serde_json::json!({}));
        assert!(
            agents
                .to_string()
                .contains(&format!("\"pane_id\":\"{pane_id}\"")),
            "{agents}"
        );
        assert!(app.pane_launch_env(0, parked, Vec::new()).is_some());
    }

    #[test]
    fn focusing_a_parked_pane_is_rejected_without_moving_focus() {
        let (mut app, parked) = split_app();
        let pane_id = park(&mut app, parked);
        let before = app.state.workspaces[0].active_tab_index();
        let focus = call(
            &mut app,
            "pane.focus",
            serde_json::json!({ "pane_id": pane_id }),
        );
        assert_eq!(focus["error"]["code"], "pane_parked", "{focus}");
        assert!(!app.state.focus_pane_in_workspace(0, parked));
        assert_eq!(app.state.workspaces[0].active_tab_index(), before);
        app.state.workspaces[0].assert_invariants_for_test();
    }

    #[test]
    fn a_parked_tab_is_hidden_from_tab_requests() {
        let (mut app, parked) = split_app();
        let parked_pane_id = park(&mut app, parked);
        let parked_tab_id = app.session_snapshot().parked_panes[0].tab_id.clone();

        let snapshot = app.session_snapshot();
        assert_eq!(snapshot.tabs.len(), 1);
        assert!(snapshot.tabs.iter().all(|tab| tab.tab_id != parked_tab_id));
        assert_eq!(snapshot.workspaces[0].tab_count, 1);
        let focus = call(
            &mut app,
            "tab.focus",
            serde_json::json!({ "tab_id": parked_tab_id }),
        );
        assert!(focus.get("error").is_some(), "{focus}");
        let split = call(
            &mut app,
            "pane.split",
            serde_json::json!({
                "target_pane_id": parked_pane_id,
                "direction": "right",
            }),
        );
        assert!(split.get("error").is_some(), "{split}");
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn moving_or_closing_the_last_tab_keeps_a_workspace_with_parked_panes() {
        for method in ["tab.move_to_workspace", "tab.close"] {
            let mut workspace = Workspace::test_new("split");
            let parked = workspace.test_split(Direction::Horizontal);
            let mut app = app(vec![workspace, Workspace::test_new("other")]);
            park(&mut app, parked);

            let tab_id = app.public_tab_id(0, 0).unwrap();
            let params = match method {
                "tab.move_to_workspace" => serde_json::json!({
                    "tab_id": tab_id,
                    "workspace_id": app.public_workspace_id(1),
                }),
                _ => serde_json::json!({ "tab_id": tab_id }),
            };
            let response = call(&mut app, method, params);
            assert!(response.get("result").is_some(), "{method}: {response}");

            app.state.assert_invariants_for_test();
            assert_eq!(app.state.workspaces.len(), 2, "{method}");
            assert_eq!(app.state.workspaces[0].visible_tab_count(), 0, "{method}");
            assert_eq!(app.session_snapshot().parked_panes.len(), 1, "{method}");
        }
    }

    #[test]
    fn a_parked_agent_keeps_reporting_its_state() {
        let (mut app, parked) = split_app();
        park(&mut app, parked);

        app.handle_internal_event(AppEvent::StateChanged {
            pane_id: parked,
            agent: Some(crate::detect::Agent::Codex),
            state: crate::detect::AgentState::Working,
            visible_blocker: false,
            visible_working: false,
            process_exited: false,
            observed_at: std::time::Instant::now(),
        });

        let info = &app.session_snapshot().parked_panes[0];
        assert_eq!(info.agent.as_deref(), Some("codex"));
        assert_eq!(info.agent_status, AgentStatus::Working);
    }

    #[test]
    fn closing_a_workspace_closes_its_parked_terminals() {
        let mut first = Workspace::test_new("first");
        let parked = first.test_split(Direction::Horizontal);
        let mut app = app(vec![first, Workspace::test_new("second")]);
        park(&mut app, parked);
        let terminal_id = app.session_snapshot().parked_panes[0].terminal_id.clone();
        let workspace_id = app.public_workspace_id(0);

        let response = call(
            &mut app,
            "workspace.close",
            serde_json::json!({ "workspace_id": workspace_id }),
        );

        assert!(response.get("result").is_some(), "{response}");
        assert_eq!(app.state.workspaces.len(), 1);
        assert!(!has_terminal(&app, &terminal_id));
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn closing_the_last_tiled_pane_keeps_a_workspace_with_parked_panes() {
        let (mut app, parked) = split_app();
        park(&mut app, parked);
        let remaining = app.state.workspaces[0].tabs[0].root_pane;
        let remaining_id = app.public_pane_id(0, remaining).unwrap();

        let response = call(
            &mut app,
            "pane.close",
            serde_json::json!({ "pane_id": remaining_id }),
        );

        assert!(response.get("result").is_some(), "{response}");
        assert_eq!(app.state.workspaces.len(), 1);
        assert_eq!(app.state.workspaces[0].visible_tab_count(), 0);
        assert_eq!(app.session_snapshot().parked_panes.len(), 1);
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn a_parked_terminal_that_exits_leaves_the_workspace() {
        let (mut app, parked) = split_app();
        park(&mut app, parked);
        let terminal_id = app.session_snapshot().parked_panes[0].terminal_id.clone();

        app.handle_internal_event(AppEvent::PaneDied {
            pane_id: parked,
            exit_reason: crate::platform::ChildExitReason::Exited,
        });

        assert!(app.session_snapshot().parked_panes.is_empty());
        assert!(!has_terminal(&app, &terminal_id));
        assert_eq!(app.state.workspaces[0].tabs[0].panes.len(), 1);
        app.state.assert_invariants_for_test();
    }

    #[cfg(unix)]
    #[test]
    fn a_parked_pane_stays_parked_across_live_handoff() {
        let (mut source, parked) = split_app();
        let tab_id = source.public_tab_id(0, 0).unwrap();
        let pane_id = park(&mut source, parked);
        let terminal_id = source.session_snapshot().parked_panes[0]
            .terminal_id
            .clone();

        // Capture the way the old server does before handing off.
        let mut snapshot = crate::persist::capture(
            &source.state.workspaces,
            &source.state.terminals,
            &source.terminal_runtimes,
            source.state.active,
            source.state.selected,
        );
        // A missing cwd keeps the import from spawning shells for the panes.
        let missing = std::env::temp_dir().join("__lawn_missing_parked_handoff_directory__");
        assert!(!missing.exists());
        for pane in snapshot
            .workspaces
            .iter_mut()
            .flat_map(|ws| &mut ws.tabs)
            .flat_map(|tab| tab.panes.values_mut())
        {
            pane.cwd = missing.clone();
        }
        let mut manifest =
            crate::server::handoff::manifest_for(snapshot, Vec::new(), None, None, None, None);
        manifest.terminal_ids = source
            .state
            .workspaces
            .iter()
            .flat_map(|ws| &ws.tabs)
            .flat_map(|tab| &tab.panes)
            .map(|(pane, state)| (pane.raw(), state.attached_terminal_id.clone()))
            .collect();
        let manifest: crate::server::handoff::HandoffManifest =
            serde_json::from_slice(&serde_json::to_vec(&manifest).unwrap()).unwrap();

        // Import the way `App::new_from_handoff` does, minus its plugin and
        // persistence side effects.
        let mut target = app(Vec::new());
        let (workspaces, terminals, runtimes) = crate::persist::restore_handoff(
            &manifest.snapshot,
            &manifest.terminal_ids,
            0,
            "/bin/sh",
            crate::config::ShellModeConfig::NonLogin,
            &mut std::collections::HashMap::new(),
            target.event_tx.clone(),
            target.render_notify.clone(),
            target.render_dirty.clone(),
        )
        .unwrap();
        assert!(runtimes.is_empty());
        target.state.pane_id_aliases =
            crate::persist::handoff_pane_aliases(&manifest.snapshot, &workspaces);
        target.state.workspaces = workspaces;
        target.state.terminals = terminals;
        target.state.active = manifest.snapshot.active;
        target.state.selected = manifest.snapshot.selected;
        target.state.assert_invariants_for_test();

        let snapshot = target.session_snapshot();
        let [still_parked] = snapshot.parked_panes.as_slice() else {
            panic!("expected one parked pane, got {:?}", snapshot.parked_panes);
        };
        assert_eq!(still_parked.pane_id, pane_id);
        assert_eq!(still_parked.terminal_id, terminal_id);
        assert!(snapshot.panes.iter().all(|pane| pane.pane_id != pane_id));
        assert_eq!(target.state.workspaces[0].visible_tab_count(), 1);
        assert_eq!(target.state.workspaces[0].tabs[0].panes.len(), 1);
        assert!(target.parse_pane_id(&pane_id).is_some());

        let response = unpark(&mut target, &pane_id, true);
        assert_eq!(response["result"]["pane"]["pane_id"], pane_id, "{response}");
        assert_eq!(response["result"]["pane"]["tab_id"], tab_id, "{response}");
        assert_eq!(target.state.workspaces[0].tabs[0].panes.len(), 2);
        assert!(target.session_snapshot().parked_panes.is_empty());
    }
}
