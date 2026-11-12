use super::*;

use api::schema::{
    BackgroundReportParams, BackgroundShellReportParams, BackgroundTaskKind, BackgroundTaskRef,
    BackgroundWaitSetParams, Method, PaneTarget, Request, WorkspaceTarget,
};

fn background_endpoint_server() -> (HeadlessServer, String) {
    let mut server = test_headless_server();
    server.app.state.workspaces = vec![
        crate::workspace::Workspace::test_new("foreground"),
        crate::workspace::Workspace::test_new("observed"),
    ];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Terminal;
    let pane = server.app.state.workspaces[1].tabs[0].root_pane;
    let terminal_id = server.app.state.workspaces[1].tabs[0]
        .terminal_id(pane)
        .unwrap()
        .clone();
    server
        .app
        .state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
            source: "herdr:claude".into(),
            agent: "claude".into(),
            session_ref: crate::agent_resume::AgentSessionRef::id("root").unwrap(),
        });
    let pane_id = server.app.public_pane_id(1, pane).unwrap();
    (server, pane_id)
}

fn background_report(pane_id: &str) -> Method {
    Method::BackgroundReport(BackgroundReportParams {
        pane_id: pane_id.into(),
        source: "herdr:claude".into(),
        session_id: "root".into(),
        event: api::schema::BackgroundEvent::SubagentStarted,
        task_id: "child".into(),
        task_type: Some("Explore".into()),
    })
}

fn background_shell_report(pane_id: &str) -> Method {
    Method::BackgroundShellReport(BackgroundShellReportParams {
        pane_id: pane_id.into(),
        source: "herdr:claude".into(),
        session_id: "root".into(),
        observation: api::schema::ShellObservation::BashBackgroundObserved {
            task_id: "shell".into(),
            tool_use_id: "tool".into(),
            hook_agent_id: Some("reporter".into()),
            command: None,
        },
    })
}

fn background_shell_snapshot(pane_id: &str) -> Method {
    serde_json::from_value::<Request>(serde_json::json!({
        "id":"snapshot", "method":"background.shell.report", "params":{
            "pane_id":pane_id, "source":"herdr:claude", "session_id":"root",
            "observation":{"event":"snapshot_observed", "hook_event":"Stop", "tasks":[], "truncated":false}
        }
    })).unwrap().method
}

fn background_wait_set(pane_id: &str) -> Method {
    Method::BackgroundWaitSet(BackgroundWaitSetParams {
        pane_id: pane_id.into(),
        source: "herdr:claude".into(),
        session_id: "root".into(),
        tasks: vec![BackgroundTaskRef {
            kind: BackgroundTaskKind::Subagent,
            task_id: "child".into(),
        }],
        waiting: true,
    })
}

fn background_method(pane_id: &str, method: &str) -> Method {
    match method {
        "background.report" => background_report(pane_id),
        "background.wait.set" => background_wait_set(pane_id),
        "background.shell.report" => background_shell_report(pane_id),
        "background.list" => Method::BackgroundList(PaneTarget {
            pane_id: pane_id.into(),
        }),
        _ => panic!("unexpected background method"),
    }
}

fn endpoint_request(server: &mut HeadlessServer, id: &str, method: Method) -> bool {
    server.handle_server_event(ServerEvent::ClientShellEndpointRequest {
        client_id: 8,
        boot_id: server.client_shell_boot_id.clone(),
        request: Box::new(Request {
            id: id.into(),
            method,
        }),
    })
}

fn endpoint_response(control: &std::sync::mpsc::Receiver<Vec<u8>>, id: &str) -> serde_json::Value {
    let ServerMessage::ClientShellEndpointResponseChunk {
        boot_id,
        request_id,
        final_chunk,
        data,
    } = read_server_message(control.recv_timeout(Duration::from_secs(1)).unwrap())
    else {
        panic!("expected endpoint response");
    };
    assert_eq!(boot_id, "test-boot");
    assert_eq!(request_id, id);
    assert!(final_chunk);
    let response: serde_json::Value = serde_json::from_slice(&data).unwrap();
    assert_eq!(response["id"], id);
    response
}

#[tokio::test]
async fn background_endpoint_observations_preserve_presentation_and_pending_commands() {
    for active in [false, true] {
        for in_flight in [false, true] {
            let (mut server, pane_id) = background_endpoint_server();
            let (foreground_control, _) = connect_matching_test_shell(&mut server, 7);
            let _ = client_shell_snapshot(&foreground_control);
            let (control, render) = connect_matching_test_shell(&mut server, 8);
            let _ = client_shell_snapshot(&control);
            let observed_tab = server.app.public_tab_id(1, 0).unwrap();
            assert!(server.focus_shell_client_on_tab(8, &observed_tab));

            let pending_response = if in_flight {
                endpoint_request(
                    &mut server,
                    "pending-presentation",
                    Method::IntegrationList(api::schema::EmptyParams::default()),
                );
                Some(
                    tokio::time::timeout(Duration::from_secs(1), server.server_event_rx.recv())
                        .await
                        .unwrap()
                        .unwrap(),
                )
            } else {
                None
            };
            if !active {
                server.set_client_shell_surface_active(8, false).unwrap();
            }
            server.promote_client_to_foreground(7);
            while control.try_recv().is_ok() {}
            while foreground_control.try_recv().is_ok() {}

            let presentation = |server: &HeadlessServer| {
                (
                    server.app.session_snapshot(),
                    server.app.state.selected,
                    server.app.state.mode,
                    server.foreground_client_id,
                    server.effective_size,
                    server.next_activity_stamp,
                    server.tab_geometry_controllers.clone(),
                    [7, 8].map(|id| {
                        let client = &server.clients[&id];
                        (
                            client.shell_location.clone(),
                            client.shell_surface_active,
                            client.last_activity,
                            client.shell_projection_revision,
                            client.shell_endpoint_command_in_flight,
                            client.shell_endpoint_command_surface_revision,
                            client.shell_deferred_navigation_request_id.clone(),
                            client.shell_deferred_navigation_response.clone(),
                        )
                    }),
                )
            };
            let before = presentation(&server);
            let pane = server.app.state.workspaces[1].tabs[0].root_pane;
            for method in [
                background_report(&pane_id),
                background_shell_report(&pane_id),
                background_shell_snapshot(&pane_id),
            ] {
                server
                    .app
                    .event_tx
                    .try_send(AppEvent::TerminalBell {
                        pane_id: pane,
                        count: 2,
                    })
                    .unwrap();
                assert!(!endpoint_request(&mut server, "report", method));
                let reported = endpoint_response(&control, "report");
                assert_eq!(reported["result"]["type"], "ok", "{reported}");
                assert_eq!(presentation(&server), before);
                assert!(matches!(
                    read_server_message(
                        foreground_control
                            .recv_timeout(Duration::from_secs(1))
                            .unwrap(),
                    ),
                    ServerMessage::TerminalBell { count: 2 }
                ));
            }

            assert!(!endpoint_request(
                &mut server,
                "wait",
                background_wait_set(&pane_id)
            ));
            let declared = endpoint_response(&control, "wait");
            assert_eq!(declared["result"]["type"], "background_wait_set");
            assert_eq!(declared["result"]["session_id"], "root");
            assert_eq!(
                declared["result"]["matched"],
                serde_json::json!([{"kind":"subagent","task_id":"child"}])
            );
            assert_eq!(declared["result"]["unmatched"], serde_json::json!([]));
            assert_eq!(presentation(&server), before);

            server
                .app
                .event_tx
                .try_send(AppEvent::TerminalBell {
                    pane_id: pane,
                    count: 3,
                })
                .unwrap();
            assert!(!endpoint_request(
                &mut server,
                "list",
                Method::BackgroundList(PaneTarget { pane_id }),
            ));
            let listed = endpoint_response(&control, "list");
            assert_eq!(listed["result"]["type"], "background_list");
            assert_eq!(
                listed["result"]["background"]["records"][0]["task_id"],
                "child"
            );
            assert_eq!(
                listed["result"]["background"]["shell_observations"]["bash_reports"][0]["task_id"],
                "shell"
            );
            assert_eq!(
                listed["result"]["background"]["shell_observations"]["last_received_snapshot"]
                    ["tasks"],
                serde_json::json!([])
            );
            assert_eq!(presentation(&server), before);
            assert!(matches!(
                read_server_message(
                    foreground_control
                        .recv_timeout(Duration::from_secs(1))
                        .unwrap(),
                ),
                ServerMessage::TerminalBell { count: 3 }
            ));
            assert!(server.app.event_rx.is_empty());
            assert!(server.server_event_rx.try_recv().is_err());
            assert!(foreground_control.try_recv().is_err());
            assert!(control.try_recv().is_err());
            assert!(render.try_recv().is_err());

            if let Some(response) = pending_response {
                assert!(!server.handle_server_event(response));
                assert_eq!(
                    endpoint_response(&control, "pending-presentation")["result"]["type"],
                    "integration_list"
                );
                assert!(!server.clients[&8].shell_endpoint_command_in_flight);
            }
            shutdown_test_runtimes(&mut server);
        }
    }
}

fn assert_background_drains_pane_death(method_name: &str, endpoint: bool) {
    let (mut server, pane_id) = background_endpoint_server();
    let (foreground_control, _) = connect_matching_test_shell(&mut server, 7);
    let _ = client_shell_snapshot(&foreground_control);
    let (control, _) = connect_matching_test_shell(&mut server, 8);
    let _ = client_shell_snapshot(&control);
    server.set_client_shell_surface_active(8, false).unwrap();
    server.promote_client_to_foreground(7);
    let (_, seeded) = socket_request(&mut server, background_report(&pane_id));
    assert_eq!(seeded["result"]["type"], "ok");
    let pane = server.app.state.workspaces[1].tabs[0].root_pane;
    for _ in 0..=crate::app::APP_EVENT_DRAIN_LIMIT {
        server
            .app
            .event_tx
            .try_send(AppEvent::TerminalBell {
                pane_id: pane,
                count: 2,
            })
            .unwrap();
    }
    server
        .app
        .event_tx
        .try_send(AppEvent::PaneDied {
            pane_id: pane,
            exit_reason: crate::platform::ChildExitReason::Exited,
        })
        .unwrap();
    let method = background_method(&pane_id, method_name);
    let (changed, response) = if endpoint {
        let changed = endpoint_request(&mut server, "dead-pane", method);
        (changed, endpoint_response(&control, "dead-pane"))
    } else {
        socket_request(&mut server, method)
    };
    assert_eq!(response["error"]["code"], "pane_not_found", "{response}");
    assert!(changed, "the endpoint must propagate the lifecycle change");
    assert!(server.app.find_pane(pane).is_none());
    assert!(server.app.event_rx.is_empty());
    for _ in 0..=crate::app::APP_EVENT_DRAIN_LIMIT {
        assert!(matches!(
            // Each client's test writer forwards its queue on a separate thread.
            read_server_message(
                foreground_control
                    .recv_timeout(Duration::from_secs(1))
                    .unwrap(),
            ),
            ServerMessage::TerminalBell { count: 2 }
        ));
    }
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn background_endpoint_list_drains_pane_death_beyond_batch_limit() {
    assert_background_drains_pane_death("background.list", true);
}

#[tokio::test]
async fn background_endpoint_report_drains_pane_death_beyond_batch_limit() {
    assert_background_drains_pane_death("background.report", true);
}

#[tokio::test]
async fn background_endpoint_list_chunks_maximum_escaped_commands_without_presentation_changes() {
    let (mut server, pane_id) = background_endpoint_server();
    let command = "\n".repeat(4096);
    let snapshot = serde_json::json!({
        "event":"snapshot_observed", "hook_event":"Stop", "truncated":false,
        "tasks":(0..128).map(|index| serde_json::json!({
            "task_id":format!("snapshot-{index}"), "reported_status":"running", "command":command
        })).collect::<Vec<_>>()
    });
    for observation in std::iter::once(snapshot).chain((0..128).map(|index| {
        serde_json::json!({
            "event":"bash_background_observed", "task_id":format!("bash-{index}"),
            "tool_use_id":format!("tool-{index}"), "command":command
        })
    })) {
        let request: Request = serde_json::from_value(serde_json::json!({
            "id":"seed", "method":"background.shell.report", "params":{
                "pane_id":pane_id, "source":"herdr:claude", "session_id":"root", "observation":observation
            }
        })).unwrap();
        let (_, response) = socket_request(&mut server, request.method);
        assert_eq!(response["result"]["type"], "ok", "{response}");
    }
    let (foreground_control, foreground_render) = connect_matching_test_shell(&mut server, 7);
    let _ = client_shell_snapshot(&foreground_control);
    let (control, render) = connect_matching_test_shell(&mut server, 8);
    let _ = client_shell_snapshot(&control);
    let observed_tab = server.app.public_tab_id(1, 0).unwrap();
    assert!(server.focus_shell_client_on_tab(8, &observed_tab));
    endpoint_request(
        &mut server,
        "pending-presentation",
        Method::IntegrationList(api::schema::EmptyParams::default()),
    );
    let pending_response =
        tokio::time::timeout(Duration::from_secs(1), server.server_event_rx.recv())
            .await
            .unwrap()
            .unwrap();
    server.set_client_shell_surface_active(8, false).unwrap();
    server.promote_client_to_foreground(7);
    while control.try_recv().is_ok() {}
    while foreground_control.try_recv().is_ok() {}
    server.app.state.view.terminal_area = Rect::new(3, 4, 55, 12);
    let pane = server.app.state.workspaces[1].tabs[0].root_pane;
    server.app.render_dirty.take();
    server.app.render_dirty.request_pty(pane);
    let presentation = |server: &HeadlessServer| {
        (
            server.app.session_snapshot(),
            server.app.state.selected,
            server.app.state.mode,
            server.app.state.view.terminal_area,
            server.foreground_client_id,
            server.effective_size,
            server.next_activity_stamp,
            server.tab_geometry_controllers.clone(),
            [7, 8].map(|id| {
                let client = &server.clients[&id];
                (
                    client.writer.is_some(),
                    client.shell_location.clone(),
                    client.shell_surface_active,
                    client.last_activity,
                    client.shell_projection_revision,
                    client.shell_endpoint_command_in_flight,
                    client.shell_endpoint_command_surface_revision,
                    client.shell_deferred_navigation_request_id.clone(),
                    client.shell_deferred_navigation_response.clone(),
                )
            }),
        )
    };
    let before = presentation(&server);
    assert!(server.clients[&8].shell_endpoint_command_in_flight);
    let method = Method::BackgroundList(PaneTarget { pane_id });
    let expected = server
        .app
        .handle_api_request_after_internal_events_drained(Request {
            id: "large-list".into(),
            method: method.clone(),
        });
    assert!(expected.len() > MAX_FRAME_SIZE);
    assert!(!endpoint_request(&mut server, "large-list", method));
    assert_eq!(presentation(&server), before);
    assert!(server.server_event_rx.try_recv().is_err());
    let dirty = server.app.render_dirty.take();
    assert_eq!(dirty.pty_sources, HashSet::from([pane]));
    assert!(!dirty.generic);

    // The pending response is a FIFO barrier for every neutral response frame.
    assert!(!server.handle_server_event(pending_response));
    let mut received = Vec::new();
    let mut frames = 0;
    let mut final_chunks = 0;
    loop {
        let frame = control.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(frame.len() < MAX_FRAME_SIZE);
        let ServerMessage::ClientShellEndpointResponseChunk {
            boot_id,
            request_id,
            final_chunk,
            data,
        } = read_server_message(frame)
        else {
            panic!("expected endpoint response chunk");
        };
        assert_eq!(boot_id, "test-boot");
        if request_id == "pending-presentation" {
            assert!(final_chunk);
            let response: serde_json::Value = serde_json::from_slice(&data).unwrap();
            assert_eq!(response["result"]["type"], "integration_list");
            break;
        }
        assert_eq!(request_id, "large-list");
        assert_eq!(final_chunks, 0, "no frames may follow the final chunk");
        frames += 1;
        final_chunks += usize::from(final_chunk);
        received.extend(data);
    }
    assert!(
        frames > 1,
        "maximum-valid commands require multiple wire frames"
    );
    assert_eq!(final_chunks, 1);
    assert_eq!(received, expected.as_bytes());
    assert!(!server.clients[&8].shell_endpoint_command_in_flight);
    assert!(foreground_control.try_recv().is_err());
    assert!(foreground_render.try_recv().is_err());
    assert!(control.try_recv().is_err());
    assert!(render.try_recv().is_err());
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn background_endpoint_requests_preserve_shutdown_and_queued_events() {
    thread_local! {
        static PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        static SHUTDOWN_AT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }
    fn shutdown_during_drain() -> bool {
        PROBES.with(|probes| {
            let count = probes.get();
            probes.set(count + 1);
            SHUTDOWN_AT.with(|shutdown_at| count >= shutdown_at.get())
        })
    }
    for method_name in [
        "background.list",
        "background.report",
        "background.shell.report",
        "background.wait.set",
    ] {
        for shutdown in ["server", "host", "before-first-event", "after-first-event"] {
            let (mut server, pane_id) = background_endpoint_server();
            let (control, _) = connect_matching_test_shell(&mut server, 8);
            let _ = client_shell_snapshot(&control);
            let pane = server.app.state.workspaces[1].tabs[0].root_pane;
            let terminal_id = server.app.state.terminal_id_for_pane(1, pane).unwrap();
            server
                .app
                .event_tx
                .try_send(AppEvent::HookMetadataReported {
                    pane_id: pane,
                    source: "user:queued".into(),
                    agent_label: None,
                    applies_to_source: None,
                    title: Some("queued metadata".into()),
                    display_agent: None,
                    state_labels: HashMap::new(),
                    clear_title: false,
                    clear_display_agent: false,
                    clear_state_labels: false,
                    seq: None,
                    ttl: None,
                })
                .unwrap();
            server
                .app
                .event_tx
                .try_send(AppEvent::PaneDied {
                    pane_id: pane,
                    exit_reason: crate::platform::ChildExitReason::Exited,
                })
                .unwrap();
            match shutdown {
                "server" => server.shutting_down = true,
                "host" => server
                    .host_shutdown_requested
                    .store(true, Ordering::Release),
                "before-first-event" | "after-first-event" => {
                    PROBES.with(|probes| probes.set(0));
                    // Probe before draining, before dequeuing, and before handling each event.
                    SHUTDOWN_AT.with(|at| {
                        at.set(if shutdown == "after-first-event" {
                            3
                        } else {
                            1
                        })
                    });
                    server.host_shutdown_probe = shutdown_during_drain;
                }
                _ => unreachable!(),
            }
            let mut before = server.app.session_snapshot();
            let processed = shutdown == "after-first-event";
            if processed {
                before.panes[1].title = Some("queued metadata".into());
            }
            let method = background_method(&pane_id, method_name);
            let changed = endpoint_request(&mut server, "shutdown", method);
            let response = endpoint_response(&control, "shutdown");
            assert_eq!(
                response["error"]["code"], "server_unavailable",
                "{shutdown}: {response}"
            );
            assert_eq!(changed, processed, "{shutdown}");
            assert_eq!(server.app.session_snapshot(), before);
            assert_eq!(server.app.event_rx.len(), if processed { 1 } else { 2 });
            let terminal = &server.app.state.terminals[&terminal_id];
            assert_eq!(
                terminal.agent_metadata.contains_key("user:queued"),
                processed
            );
            assert!(terminal.background_snapshot().records.is_empty());
            assert!(terminal.background_snapshot().shell_observations.is_none());
            assert!(server.app.find_pane(pane).is_some());
            shutdown_test_runtimes(&mut server);
        }
    }
}

#[tokio::test]
async fn background_endpoint_requests_preserve_boot_and_connection_mode_checks() {
    for method_name in [
        "background.list",
        "background.report",
        "background.shell.report",
        "background.wait.set",
    ] {
        let (mut server, pane_id) = background_endpoint_server();
        let (control, _) = connect_matching_test_shell(&mut server, 8);
        let _ = client_shell_snapshot(&control);
        server.set_client_shell_surface_active(8, false).unwrap();
        let method = background_method(&pane_id, method_name);
        server
            .clients
            .get_mut(&8)
            .unwrap()
            .shell_endpoint_command_in_flight = true;
        let before = server.app.session_snapshot();
        assert!(
            !server.handle_server_event(ServerEvent::ClientShellEndpointRequest {
                client_id: 8,
                boot_id: "earlier-boot".into(),
                request: Box::new(Request {
                    id: "stale".into(),
                    method: method.clone(),
                }),
            })
        );
        let ServerMessage::ClientShellEndpointResponseChunk { boot_id, data, .. } =
            read_server_message(control.recv_timeout(Duration::from_secs(1)).unwrap())
        else {
            panic!("expected stale boot error");
        };
        assert_eq!(boot_id, "earlier-boot");
        let error: api::schema::ErrorResponse = serde_json::from_slice(&data).unwrap();
        assert_eq!(error.id, "stale");
        assert_eq!(error.error.code, "stale_boot");
        assert_eq!(server.app.session_snapshot(), before);
        assert!(server.clients[&8].shell_endpoint_command_in_flight);
        assert!(server.app.state.terminals.values().all(|terminal| {
            let snapshot = terminal.background_snapshot();
            snapshot.records.is_empty() && snapshot.shell_observations.is_none()
        }));

        server.shutting_down = true;
        assert!(!endpoint_request(&mut server, "shutdown", method.clone()));
        assert_eq!(
            endpoint_response(&control, "shutdown")["error"]["code"],
            "server_unavailable"
        );
        assert!(server.clients[&8].shell_endpoint_command_in_flight);
        assert!(server.app.state.terminals.values().all(|terminal| {
            let snapshot = terminal.background_snapshot();
            snapshot.records.is_empty() && snapshot.shell_observations.is_none()
        }));
        server.shutting_down = false;

        server.clients.get_mut(&8).unwrap().mode = ClientConnectionMode::TerminalPending;
        assert!(endpoint_request(&mut server, "wrong-mode", method));
        assert!(!server.clients.contains_key(&8));
        assert!(server.app.state.terminals.values().all(|terminal| {
            let snapshot = terminal.background_snapshot();
            snapshot.records.is_empty() && snapshot.shell_observations.is_none()
        }));
        shutdown_test_runtimes(&mut server);
    }
}

#[tokio::test]
async fn background_endpoint_exception_preserves_ordinary_mutating_dispatch() {
    let (mut server, _) = background_endpoint_server();
    let (control, _) = connect_matching_test_shell(&mut server, 8);
    let _ = client_shell_snapshot(&control);
    server.set_client_shell_surface_active(8, false).unwrap();
    let method = Method::WorkspaceFocus(WorkspaceTarget {
        workspace_id: server.app.public_workspace_id(1),
    });
    let before = server.app.session_snapshot();
    assert!(!endpoint_request(&mut server, "inactive", method.clone()));
    assert_eq!(
        endpoint_response(&control, "inactive")["error"]["code"],
        "surface_inactive"
    );
    assert_eq!(server.app.session_snapshot(), before);
    assert!(!server.clients[&8].shell_endpoint_command_in_flight);

    server.set_client_shell_surface_active(8, true).unwrap();
    while control.try_recv().is_ok() {}
    assert!(endpoint_request(&mut server, "active", method.clone()));
    assert_eq!(server.foreground_client_id, Some(8));
    assert_eq!(server.app.state.active, Some(1));
    assert!(server.clients[&8].shell_endpoint_command_in_flight);
    assert!(!endpoint_request(&mut server, "busy", method));
    assert_eq!(
        endpoint_response(&control, "busy")["error"]["code"],
        "endpoint_busy"
    );
    let ready = tokio::time::timeout(Duration::from_secs(1), server.server_event_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(!server.handle_server_event(ready));
    assert_eq!(
        endpoint_response(&control, "active")["result"]["type"],
        "workspace_info"
    );
    assert!(!server.clients[&8].shell_endpoint_command_in_flight);
    shutdown_test_runtimes(&mut server);
}

fn socket_request(server: &mut HeadlessServer, method: Method) -> (bool, serde_json::Value) {
    let (respond_to, response_rx) = std::sync::mpsc::channel();
    let changed = server.handle_api_request_with_shutdown_check(api::ApiRequestMessage {
        request: Request {
            id: "socket-background".into(),
            method,
        },
        respond_to,
        response_write_complete: None,
    });
    let response = response_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    (changed, serde_json::from_str(&response).unwrap())
}

#[tokio::test]
async fn background_socket_requests_preserve_presentation_and_pending_work() {
    let (mut server, pane_id) = background_endpoint_server();
    let (foreground_control, foreground_render) = connect_matching_test_shell(&mut server, 7);
    let _ = client_shell_snapshot(&foreground_control);
    let (control, render) = connect_matching_test_shell(&mut server, 8);
    let _ = client_shell_snapshot(&control);
    let observed_tab = server.app.public_tab_id(1, 0).unwrap();
    assert!(server.focus_shell_client_on_tab(8, &observed_tab));
    endpoint_request(
        &mut server,
        "pending-presentation",
        Method::IntegrationList(api::schema::EmptyParams::default()),
    );
    let pending_response =
        tokio::time::timeout(Duration::from_secs(1), server.server_event_rx.recv())
            .await
            .unwrap()
            .unwrap();
    server.set_client_shell_surface_active(8, false).unwrap();
    server.promote_client_to_foreground(7);
    while control.try_recv().is_ok() {}
    while foreground_control.try_recv().is_ok() {}

    // Distinct client and App geometry exposes an accidental foreground sync.
    server.app.state.view.terminal_area = Rect::new(3, 4, 55, 12);
    server.app.pixel_mouse_available = true;
    server.app.state.outer_terminal_focus = Some(false);
    let pane = server.app.state.workspaces[1].tabs[0].root_pane;
    server.app.render_dirty.take();
    server.app.render_dirty.request_pty(pane);
    let save_deadline = Instant::now() + Duration::from_secs(60);
    server.app.session_save_deadline = Some(save_deadline);
    let presentation = |server: &HeadlessServer| {
        (
            server.app.session_snapshot(),
            server.app.state.selected,
            server.app.state.mode,
            server.app.state.view.terminal_area,
            server.app.pixel_mouse_available,
            server.app.state.outer_terminal_focus,
            server.foreground_client_id,
            server.effective_size,
            server.next_activity_stamp,
            server.tab_geometry_controllers.clone(),
            [7, 8].map(|id| {
                let client = &server.clients[&id];
                (
                    client.shell_location.clone(),
                    client.shell_surface_active,
                    client.last_activity,
                    client.shell_projection_revision,
                    client.shell_endpoint_command_in_flight,
                    client.shell_endpoint_command_surface_revision,
                    client.shell_deferred_navigation_request_id.clone(),
                    client.shell_deferred_navigation_response.clone(),
                )
            }),
        )
    };
    let before = presentation(&server);
    for (method, expected) in [
        (background_report(&pane_id), "ok"),
        (background_shell_report(&pane_id), "ok"),
        (background_shell_snapshot(&pane_id), "ok"),
        (background_wait_set(&pane_id), "background_wait_set"),
        (
            Method::BackgroundList(PaneTarget { pane_id }),
            "background_list",
        ),
    ] {
        let (changed, response) = socket_request(&mut server, method);
        assert!(!changed);
        assert_eq!(response["result"]["type"], expected, "{response}");
        if expected == "background_list" {
            assert_eq!(
                response["result"]["background"]["records"][0]["task_id"],
                "child"
            );
            assert_eq!(
                response["result"]["background"]["shell_observations"]["bash_reports"][0]
                    ["task_id"],
                "shell"
            );
            assert_eq!(
                response["result"]["background"]["shell_observations"]["last_received_snapshot"]
                    ["tasks"],
                serde_json::json!([])
            );
        }
        assert_eq!(presentation(&server), before);
        assert_eq!(server.app.session_save_deadline, Some(save_deadline));
        assert!(server.app.render_dirty.is_pending());
        assert!(server.server_event_rx.try_recv().is_err());
        assert!(foreground_control.try_recv().is_err());
        assert!(foreground_render.try_recv().is_err());
        assert!(control.try_recv().is_err());
        assert!(render.try_recv().is_err());
    }
    let dirty = server.app.render_dirty.take();
    assert_eq!(dirty.pty_sources, std::collections::HashSet::from([pane]));
    assert!(!dirty.generic);
    assert!(!server.handle_server_event(pending_response));
    assert_eq!(
        endpoint_response(&control, "pending-presentation")["result"]["type"],
        "integration_list"
    );
    assert!(!server.clients[&8].shell_endpoint_command_in_flight);

    server.app.state.workspaces.clear();
    server.app.state.active = None;
    for method in [
        background_report("missing"),
        background_shell_report("missing"),
        background_wait_set("missing"),
        Method::BackgroundList(PaneTarget {
            pane_id: "missing".into(),
        }),
    ] {
        let (changed, response) = socket_request(&mut server, method);
        assert!(!changed);
        assert_eq!(response["error"]["code"], "pane_not_found");
        assert!(server.app.state.workspaces.is_empty());
    }
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn background_socket_requests_preserve_expiry_title_sync_and_event_forwarding() {
    for method_name in [
        "background.list",
        "background.report",
        "background.shell.report",
        "background.wait.set",
    ] {
        let (mut server, pane_id) = background_endpoint_server();
        let (control, _) = connect_matching_test_shell(&mut server, 8);
        let _ = client_shell_snapshot(&control);
        let pane = server.app.state.workspaces[1].tabs[0].root_pane;
        let terminal_id = server.app.state.terminal_id_for_pane(1, pane).unwrap();
        let terminal = server.app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state(
            Some(crate::detect::Agent::Claude),
            crate::detect::AgentState::Working,
        );
        terminal.set_agent_metadata(crate::terminal::AgentMetadataReport {
            source: "user:expired".into(),
            agent_label: None,
            applies_to_source: None,
            title: Some("expired".into()),
            display_agent: None,
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: Some(Duration::ZERO),
            seq: None,
        });
        server.app.sync_agent_metadata_deadline();
        assert!(server.app.agent_metadata_deadline.unwrap() <= Instant::now());
        let runtime = crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b"");
        runtime.test_process_pty_bytes(b"\x1b]0;latest title\x07");
        server
            .app
            .terminal_runtimes
            .insert(terminal_id.clone(), runtime);
        server.app.render_dirty.request_terminal_title(pane);
        for _ in 0..=crate::app::APP_EVENT_DRAIN_LIMIT {
            server
                .app
                .event_tx
                .try_send(AppEvent::TerminalBell {
                    pane_id: pane,
                    count: 2,
                })
                .unwrap();
        }
        server
            .app
            .event_tx
            .try_send(AppEvent::StateChanged {
                pane_id: pane,
                agent: Some(crate::detect::Agent::Claude),
                state: crate::detect::AgentState::Blocked,
                visible_blocker: true,
                visible_working: false,
                process_exited: false,
                observed_at: Instant::now(),
            })
            .unwrap();
        server
            .app
            .event_tx
            .try_send(AppEvent::HookMetadataReported {
                pane_id: pane,
                source: "user:queued".into(),
                agent_label: None,
                applies_to_source: None,
                title: Some("queued metadata".into()),
                display_agent: None,
                state_labels: HashMap::new(),
                clear_title: false,
                clear_display_agent: false,
                clear_state_labels: false,
                seq: None,
                ttl: Some(Duration::from_secs(3600)),
            })
            .unwrap();
        let method = background_method(&pane_id, method_name);
        let (changed, response) = socket_request(&mut server, method);
        assert!(changed);
        assert!(response.get("result").is_some(), "{response}");
        let terminal = &server.app.state.terminals[&terminal_id];
        assert!(!terminal.agent_metadata.contains_key("user:expired"));
        assert_eq!(
            terminal.effective_title().as_deref(),
            Some("queued metadata")
        );
        assert_eq!(terminal.terminal_title.as_deref(), Some("latest title"));
        assert_eq!(terminal.state, crate::detect::AgentState::Blocked);
        assert_eq!(terminal.last_agent_completion_seq, None);
        assert!(server.app.agent_metadata_deadline.unwrap() > Instant::now());
        assert!(server.app.event_rx.try_recv().is_err());
        assert!(server
            .app
            .render_dirty
            .pending_terminal_title_sources()
            .contains(&pane));
        assert!(pane_updated_events(&server.app.event_hub) > 0);
        let mut bells = 0;
        loop {
            match read_server_message(control.recv_timeout(Duration::from_secs(1)).unwrap()) {
                ServerMessage::TerminalBell { count: 2 } => bells += 1,
                ServerMessage::SemanticNotification(notification) => {
                    assert_eq!(
                        notification.kind,
                        protocol::SemanticNotificationKind::NeedsAttention
                    );
                    break;
                }
                other => panic!("unexpected forwarded event: {other:?}"),
            }
        }
        assert_eq!(bells, crate::app::APP_EVENT_DRAIN_LIMIT + 1);
        shutdown_test_runtimes(&mut server);
    }
}

#[test]
fn background_socket_requests_preserve_shutdown_checks_and_queued_events() {
    thread_local! {
        static PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }
    fn shutdown_during_drain() -> bool {
        PROBES.with(|probes| {
            let count = probes.get();
            probes.set(count + 1);
            count > 0
        })
    }
    for method_name in [
        "background.list",
        "background.report",
        "background.shell.report",
        "background.wait.set",
    ] {
        for shutdown in ["server", "host", "during-drain"] {
            let (mut server, pane_id) = background_endpoint_server();
            server
                .app
                .event_tx
                .try_send(AppEvent::UpdateReady {
                    version: "pending".into(),
                    install_command: "pending install".into(),
                })
                .unwrap();
            match shutdown {
                "server" => server.shutting_down = true,
                "host" => server
                    .host_shutdown_requested
                    .store(true, Ordering::Release),
                "during-drain" => {
                    PROBES.with(|probes| probes.set(0));
                    server.host_shutdown_probe = shutdown_during_drain;
                }
                _ => unreachable!(),
            }
            let before = server.app.session_snapshot();
            let method = background_method(&pane_id, method_name);
            let (changed, response) = socket_request(&mut server, method);
            assert!(!changed);
            assert_eq!(response["error"]["code"], "server_unavailable");
            assert_eq!(server.app.session_snapshot(), before);
            assert_eq!(server.app.event_rx.len(), 1);
            assert!(server.app.state.update_available.is_none());
            assert!(server.app.state.terminals.values().all(|terminal| {
                let snapshot = terminal.background_snapshot();
                snapshot.records.is_empty() && snapshot.shell_observations.is_none()
            }));
        }
    }
}

#[tokio::test]
async fn background_shell_both_transports_drain_pane_death_beyond_batch_limit() {
    for endpoint in [false, true] {
        assert_background_drains_pane_death("background.shell.report", endpoint);
    }
}

#[tokio::test]
async fn background_wait_both_transports_drain_pane_death_beyond_batch_limit() {
    for endpoint in [false, true] {
        assert_background_drains_pane_death("background.wait.set", endpoint);
    }
}
