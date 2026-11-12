use crate::api::schema::{
    BackgroundReportParams, BackgroundShellReportParams, PaneTarget, ResponseResult,
};
use crate::app::App;
use crate::terminal::TerminalId;

use super::responses::{encode_error, encode_success};

fn received_at_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

impl App {
    fn background_terminal_id(&self, target: &str) -> Option<TerminalId> {
        let (ws_idx, pane_id) = self.parse_pane_id(target)?;
        self.state.terminal_id_for_pane(ws_idx, pane_id)
    }

    pub(super) fn handle_background_report(
        &mut self,
        id: String,
        params: BackgroundReportParams,
    ) -> String {
        let Some(terminal_id) = self.background_terminal_id(&params.pane_id) else {
            return encode_error(id, "pane_not_found", "background report pane not found");
        };
        let Some(terminal) = self.state.terminals.get_mut(&terminal_id) else {
            return encode_error(id, "pane_not_found", "background report terminal not found");
        };
        match terminal.report_background(
            &params.source,
            &params.session_id,
            params.event,
            &params.task_id,
            params.task_type.as_deref(),
            received_at_ms(),
        ) {
            Ok(()) => encode_success(id, ResponseResult::Ok {}),
            Err(code) => encode_error(
                id,
                code,
                "background report requires bounded fields and the current Claude root session",
            ),
        }
    }

    pub(super) fn handle_background_shell_report(
        &mut self,
        id: String,
        params: BackgroundShellReportParams,
    ) -> String {
        let Some(terminal_id) = self.background_terminal_id(&params.pane_id) else {
            return encode_error(id, "pane_not_found", "background report pane not found");
        };
        let Some(terminal) = self.state.terminals.get_mut(&terminal_id) else {
            return encode_error(id, "pane_not_found", "background report terminal not found");
        };
        match terminal.report_background_shell(
            &params.source,
            &params.session_id,
            params.observation,
            received_at_ms(),
        ) {
            Ok(()) => encode_success(id, ResponseResult::Ok {}),
            Err(code) => encode_error(
                id,
                code,
                "background report requires bounded fields and the current Claude root session",
            ),
        }
    }

    pub(super) fn handle_background_list(&self, id: String, params: PaneTarget) -> String {
        let Some(terminal_id) = self.background_terminal_id(&params.pane_id) else {
            return encode_error(id, "pane_not_found", "background list pane not found");
        };
        let Some(terminal) = self.state.terminals.get(&terminal_id) else {
            return encode_error(id, "pane_not_found", "background list terminal not found");
        };
        encode_success(
            id,
            ResponseResult::BackgroundList {
                terminal_id: terminal_id.to_string(),
                background: terminal.background_snapshot(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_resume::{AgentSessionRef, PersistedAgentSession};
    use crate::api::schema::{Method, PaneMoveDestination, PaneMoveParams, Request};
    use crate::detect::{Agent, AgentState};

    fn app() -> (App, String, TerminalId) {
        let (_, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state = crate::app::state::AppState::test_with_adversarial_identity_state();
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let pane_id = app.public_pane_id(0, pane).unwrap();
        let terminal_id = app.state.workspaces[0].tabs[0]
            .terminal_id(pane)
            .unwrap()
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state(Some(Agent::Claude), AgentState::Working);
        terminal.set_persisted_agent_session(PersistedAgentSession {
            source: "herdr:claude".into(),
            agent: "claude".into(),
            session_ref: AgentSessionRef::id("root").unwrap(),
        });
        app.state.assert_invariants_for_test();
        (app, pane_id, terminal_id)
    }

    fn request(app: &mut App, method: Method) -> serde_json::Value {
        assert!(crate::server::client_commands::supports_client_shell_method(&method));
        serde_json::from_str(&app.handle_api_request(Request {
            id: "background-test".into(),
            method,
        }))
        .unwrap()
    }

    fn report(
        pane_id: String,
        session_id: &str,
        event: crate::terminal::background::BackgroundEvent,
    ) -> Method {
        Method::BackgroundReport(BackgroundReportParams {
            pane_id,
            source: "herdr:claude".into(),
            session_id: session_id.into(),
            event,
            task_id: "child".into(),
            task_type: Some("Explore".into()),
        })
    }

    fn shell_report(pane_id: String, session_id: &str) -> Method {
        Method::BackgroundShellReport(BackgroundShellReportParams {
            pane_id,
            source: "herdr:claude".into(),
            session_id: session_id.into(),
            observation: crate::terminal::background::ShellObservation::BashBackgroundObserved {
                task_id: "shell".into(),
                tool_use_id: "tool".into(),
                hook_agent_id: None,
                command: None,
            },
        })
    }

    #[test]
    fn background_shell_json_dispatch_is_observational_and_owner_scoped() {
        let (mut app, pane_id, terminal_id) = app();
        let before = app.session_snapshot();
        assert_eq!(
            request(&mut app, shell_report(pane_id.clone(), "foreign"))["error"]["code"],
            "background_owner_mismatch"
        );
        assert!(app.state.terminals[&terminal_id]
            .background_snapshot()
            .shell_observations
            .is_none());
        assert_eq!(
            request(&mut app, shell_report(pane_id.clone(), "root"))["result"]["type"],
            "ok"
        );
        let listed = request(&mut app, Method::BackgroundList(PaneTarget { pane_id }));
        let shell = &listed["result"]["background"]["shell_observations"];
        assert_eq!(shell["bash_reports"][0]["task_id"], "shell");
        assert!(shell["bash_reports"][0]["received_at_ms"].as_u64().unwrap() > 0);
        assert_eq!(shell["last_received_snapshot"], serde_json::Value::Null);
        assert_eq!(shell["current_state"], "unknown");
        assert_eq!(shell["outcome"], "unknown");
        assert_eq!(app.session_snapshot(), before);
        let terminal = &app.state.terminals[&terminal_id];
        assert_eq!(terminal.state, AgentState::Working);
        assert_eq!(terminal.last_agent_completion_seq, None);
        assert!(terminal.hook_authority.is_none());
        assert!(terminal.hook_report_is_newer("herdr:claude", Some(1)));
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn background_json_dispatch_is_observational_and_owner_scoped() {
        let (mut app, pane_id, terminal_id) = app();
        let response = request(
            &mut app,
            report(
                pane_id.clone(),
                "foreign",
                crate::terminal::background::BackgroundEvent::SubagentStarted,
            ),
        );
        assert_eq!(response["error"]["code"], "background_owner_mismatch");
        for event in [
            crate::terminal::background::BackgroundEvent::SubagentStarted,
            crate::terminal::background::BackgroundEvent::SubagentStopObserved,
        ] {
            assert_eq!(
                request(&mut app, report(pane_id.clone(), "root", event))["result"]["type"],
                "ok"
            );
        }
        let response = request(&mut app, Method::BackgroundList(PaneTarget { pane_id }));
        assert_eq!(response["result"]["type"], "background_list");
        assert_eq!(response["result"]["terminal_id"], terminal_id.to_string());
        assert_eq!(
            response["result"]["background"]["coverage"],
            "observations_only"
        );
        assert_eq!(response["result"]["background"]["current_state"], "unknown");
        assert_eq!(
            response["result"]["background"]["records"][0]["observation"],
            "stop_observed"
        );
        let terminal = &app.state.terminals[&terminal_id];
        assert_eq!(terminal.state, AgentState::Working);
        assert_eq!(terminal.last_agent_completion_seq, None);
        assert!(terminal.hook_authority.is_none());
        assert!(terminal.hook_report_is_newer("herdr:claude", Some(1)));
        app.state.assert_invariants_for_test();
        assert_eq!(
            request(
                &mut app,
                Method::BackgroundList(PaneTarget {
                    pane_id: "missing".into()
                })
            )["error"]["code"],
            "pane_not_found"
        );
    }

    #[test]
    fn background_observations_follow_terminal_when_pane_moves() {
        let (mut app, pane_id, terminal_id) = app();
        assert_eq!(
            request(&mut app, shell_report(pane_id.clone(), "root"))["result"]["type"],
            "ok"
        );
        request(
            &mut app,
            report(
                pane_id.clone(),
                "root",
                crate::terminal::background::BackgroundEvent::SubagentStarted,
            ),
        );
        let moved: serde_json::Value = serde_json::from_str(&app.handle_api_request(Request {
            id: "move".into(),
            method: Method::PaneMove(PaneMoveParams {
                pane_id,
                destination: PaneMoveDestination::NewWorkspace {
                    label: None,
                    tab_label: None,
                },
                focus: false,
            }),
        }))
        .unwrap();
        assert_eq!(moved["result"]["move_result"]["changed"], true, "{moved}");
        let new_pane_id = moved["result"]["move_result"]["pane"]["pane_id"]
            .as_str()
            .unwrap();
        let listed = request(
            &mut app,
            Method::BackgroundList(PaneTarget {
                pane_id: new_pane_id.into(),
            }),
        );
        assert_eq!(listed["result"]["terminal_id"], terminal_id.to_string());
        assert_eq!(
            listed["result"]["background"]["records"][0]["task_id"],
            "child"
        );
        assert_eq!(
            listed["result"]["background"]["shell_observations"]["bash_reports"][0]["task_id"],
            "shell"
        );
        assert_eq!(
            request(&mut app, shell_report(new_pane_id.into(), "root"))["result"]["type"],
            "ok"
        );
        app.state.assert_invariants_for_test();
    }
}
