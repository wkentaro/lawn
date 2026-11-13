use super::*;
use crate::protocol::endpoint::pane_presentation::{self as contract, SetPanePresentation};

fn request(server: &HeadlessServer, sequence: u64) -> SetPanePresentation {
    SetPanePresentation {
        boot_id: server.client_shell_boot_id.clone(),
        sequence,
        hidden_terminals: Vec::new(),
        read_only_terminals: Vec::new(),
        preview_terminal: None,
        focus_terminal: None,
    }
}

fn apply(server: &mut HeadlessServer, client_id: u64, request: &SetPanePresentation) -> bool {
    server.handle_server_event(ServerEvent::ClientPanePresentation {
        client_id,
        data: serde_json::to_string(request).unwrap(),
    })
}

fn identity_projection(receiver: &std::sync::mpsc::Receiver<Vec<u8>>) -> contract::PaneIdentities {
    let ServerMessage::EndpointControl { kind, data } = read_server_message(
        receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("pane identities"),
    ) else {
        panic!("expected pane identities");
    };
    assert_eq!(kind, contract::IDENTITIES_KIND);
    serde_json::from_str(&data).unwrap()
}

fn client_request(
    server: &mut HeadlessServer,
    method: api::schema::Method,
) -> api::schema::ResponseResult {
    serde_json::from_str::<api::schema::SuccessResponse>(&client_response(server, method))
        .unwrap()
        .result
}

fn client_response(server: &mut HeadlessServer, method: api::schema::Method) -> String {
    let (respond_to, received) = std::sync::mpsc::channel();
    server.handle_client_shell_api_request(
        7,
        api::ApiRequestMessage {
            request: api::schema::Request {
                id: "private-navigation".into(),
                method,
            },
            respond_to,
            response_write_complete: None,
        },
    );
    received.recv().unwrap()
}

#[tokio::test]
async fn existing_controller_resizes_and_directional_focus_skips_hidden_panes_for_all_viewers() {
    let mut server = test_headless_server();
    let mut workspace = crate::workspace::Workspace::test_new("private-focus");
    let first = workspace.tabs[0].root_pane;
    let middle = workspace.test_split(ratatui::layout::Direction::Horizontal);
    let last = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.tabs[0].layout.focus_pane(first);
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    let ids = [first, middle, last].map(|id| {
        server.app.state.workspaces[0].tabs[0]
            .terminal_id(id)
            .unwrap()
            .clone()
    });
    let mut inputs = Vec::new();
    for id in &ids {
        let (runtime, receiver) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80,
                24,
                0,
                b"\x1b[?1004h",
                4,
            );
        server.app.terminal_runtimes.insert(id.clone(), runtime);
        inputs.push(receiver);
    }
    let (control, render) = connect_test_shell(&mut server, 7, 80, 24);
    let (_other_control, other_render) = connect_test_shell(&mut server, 8, 80, 24);
    let _ = client_shell_snapshot(&control);
    server
        .clients
        .get_mut(&7)
        .unwrap()
        .render_state
        .enable_surface_delta(true);
    for client_id in [7, 8] {
        server.handle_server_event(ServerEvent::ClientShellFocus {
            client_id,
            focused: true,
        });
    }
    server.claim_shell_tab_geometry(7, false);
    server.render_and_stream();
    let _ = recv_pane_surface(&render, "delta baseline");
    let _ = recv_pane_surface(&other_render, "other baseline");
    for input in &mut inputs {
        while input.try_recv().is_ok() {}
    }
    let old_size = server
        .app
        .terminal_runtimes
        .get(&ids[2])
        .unwrap()
        .current_size();
    let mut policy = request(&server, 1);
    policy.hidden_terminals.push(ids[1].to_string());
    assert!(apply(&mut server, 7, &policy));
    assert!(
        server
            .app
            .terminal_runtimes
            .get(&ids[2])
            .unwrap()
            .current_size()
            .1
            > old_size.1
    );
    let first_public = server.app.public_pane_id(0, first).unwrap();
    let last_public = server.app.public_pane_id(0, last).unwrap();
    let target = server.shell_target_for_client(7);
    let area = Rect::new(0, 0, 80, 24);
    let before = server
        .pane_layout_for_client(7, target, area)
        .unwrap()
        .panes
        .iter()
        .find(|pane| pane.id == last)
        .unwrap()
        .rect
        .width;
    let result = client_request(
        &mut server,
        api::schema::Method::PaneResize(api::schema::PaneResizeParams {
            pane_id: Some(last_public.clone()),
            direction: api::schema::PaneDirection::Left,
            amount: Some(0.1),
        }),
    );
    let api::schema::ResponseResult::PaneResize { resize } = result else {
        panic!("resize response");
    };
    assert!(resize.changed);
    let after = server
        .pane_layout_for_client(7, target, area)
        .unwrap()
        .panes
        .iter()
        .find(|pane| pane.id == last)
        .unwrap()
        .rect
        .width;
    assert!(
        after > before,
        "resize must skip the collapsed hidden split"
    );
    let result = client_request(
        &mut server,
        api::schema::Method::PaneFocusDirection(api::schema::PaneFocusDirectionParams {
            pane_id: Some(first_public.clone()),
            direction: api::schema::PaneDirection::Right,
        }),
    );
    let api::schema::ResponseResult::PaneFocusDirection { focus } = result else {
        panic!("directional focus response");
    };
    assert!(focus.changed);
    assert_eq!(server.shell_focus_target(7).unwrap().pane_id, last);
    assert_eq!(server.shell_focus_target(8).unwrap().pane_id, last);
    assert_eq!(inputs[0].try_recv().unwrap(), Bytes::from_static(b"\x1b[O"));
    assert_eq!(inputs[2].try_recv().unwrap(), Bytes::from_static(b"\x1b[I"));
    assert!(inputs[1].try_recv().is_err());
    let result = client_request(
        &mut server,
        api::schema::Method::PaneSwap(api::schema::PaneSwapParams {
            pane_id: Some(last_public),
            direction: Some(api::schema::PaneDirection::Left),
            source_pane_id: None,
            target_pane_id: None,
        }),
    );
    let api::schema::ResponseResult::PaneSwap { swap } = result else {
        panic!("swap response");
    };
    assert!(swap.changed);
    assert_eq!(swap.target_pane_id.as_deref(), Some(first_public.as_str()));
    let mut policy = request(&server, 2);
    policy.focus_terminal = Some(ids[0].to_string());
    assert!(apply(&mut server, 7, &policy));
    let refresh = request(&server, 3);
    assert!(apply(&mut server, 7, &refresh));
    assert_eq!(server.shell_focus_target(7).unwrap().pane_id, first);
    assert_eq!(server.shell_focus_target(8).unwrap().pane_id, last);
    for input in &mut inputs {
        while input.try_recv().is_ok() {}
    }
    let middle_public = server.app.public_pane_id(0, middle).unwrap();
    client_request(
        &mut server,
        api::schema::Method::PaneFocus(api::schema::PaneTarget {
            pane_id: middle_public,
        }),
    );
    assert_eq!(inputs[0].try_recv().unwrap(), Bytes::from_static(b"\x1b[O"));
    assert_eq!(inputs[2].try_recv().unwrap(), Bytes::from_static(b"\x1b[O"));
    assert_eq!(inputs[1].try_recv().unwrap(), Bytes::from_static(b"\x1b[I"));
    let mut policy = request(&server, 4);
    policy.focus_terminal = Some(ids[0].to_string());
    assert!(apply(&mut server, 7, &policy));
    for input in &mut inputs {
        while input.try_recv().is_ok() {}
    }
    server.remove_client(7);
    assert_eq!(inputs[0].try_recv().unwrap(), Bytes::from_static(b"\x1b[O"));
    assert_eq!(server.shell_focus_target(8).unwrap().pane_id, middle);
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn omitted_targets_follow_private_focus_and_explicit_sources_keep_their_own_tab_geometry() {
    let mut server = test_headless_server();
    let mut workspace = crate::workspace::Workspace::test_new("request-targets");
    let root = workspace.tabs[0].root_pane;
    let private = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.tabs[0].layout.focus_pane(root);
    workspace.test_add_tab(Some("other"));
    workspace.switch_tab(1);
    let other_source = workspace.tabs[1].root_pane;
    let other_neighbor = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.switch_tab(0);
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    let private_terminal = server.app.state.workspaces[0].tabs[0]
        .terminal_id(private)
        .unwrap()
        .to_string();
    let (control, _render) = connect_test_shell(&mut server, 7, 80, 24);
    let _ = client_shell_snapshot(&control);
    let mut policy = request(&server, 1);
    policy.focus_terminal = Some(private_terminal.clone());
    assert!(apply(&mut server, 7, &policy));
    client_request(
        &mut server,
        api::schema::Method::PaneZoom(api::schema::PaneZoomParams::default()),
    );
    assert_eq!(
        server.app.state.workspaces[0].tabs[0].layout.focused(),
        private
    );
    assert!(server.app.state.workspaces[0].tabs[0].zoomed);
    policy.sequence = 2;
    assert!(apply(&mut server, 7, &policy));
    let root_public = server.app.public_pane_id(0, root).unwrap();
    client_request(
        &mut server,
        api::schema::Method::PaneZoom(api::schema::PaneZoomParams {
            pane_id: Some(root_public),
            mode: api::schema::PaneZoomMode::On,
        }),
    );
    assert_eq!(server.shell_focus_target(7).unwrap().pane_id, root);
    policy.sequence = 3;
    assert!(apply(&mut server, 7, &policy));
    let source = server.app.public_pane_id(0, other_source).unwrap();
    client_request(
        &mut server,
        api::schema::Method::PaneFocusDirection(api::schema::PaneFocusDirectionParams {
            pane_id: Some(source),
            direction: api::schema::PaneDirection::Right,
        }),
    );
    assert_eq!(
        server.app.state.workspaces[0].tabs[1].layout.focused(),
        other_neighbor
    );
    assert_eq!(
        server.shell_focus_target(7).unwrap().pane_id,
        other_neighbor
    );
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn queued_pane_exit_is_drained_before_projecting_resize_paths() {
    let mut server = test_headless_server();
    let mut workspace = crate::workspace::Workspace::test_new("queued-exit");
    let exiting = workspace.tabs[0].root_pane;
    let remaining = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.test_split(ratatui::layout::Direction::Vertical);
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    let terminal = server.app.state.workspaces[0].tabs[0]
        .terminal_id(remaining)
        .unwrap()
        .to_string();
    let public = server.app.public_pane_id(0, remaining).unwrap();
    let (control, _render) = connect_test_shell(&mut server, 7, 80, 24);
    let _ = client_shell_snapshot(&control);
    let mut policy = request(&server, 1);
    policy.focus_terminal = Some(terminal);
    assert!(apply(&mut server, 7, &policy));
    server
        .app
        .event_tx
        .try_send(AppEvent::PaneDied {
            pane_id: exiting,
            exit_reason: crate::platform::ChildExitReason::Exited,
        })
        .unwrap();
    let result = client_request(
        &mut server,
        api::schema::Method::PaneResize(api::schema::PaneResizeParams {
            pane_id: Some(public),
            direction: api::schema::PaneDirection::Down,
            amount: Some(0.1),
        }),
    );
    let api::schema::ResponseResult::PaneResize { resize } = result else {
        panic!("resize response");
    };
    assert!(resize.changed);
    let splits = server.app.state.workspaces[0].tabs[0]
        .layout
        .splits(Rect::new(0, 0, 80, 24));
    assert_eq!(splits.len(), 1);
    assert!(splits[0].path.is_empty());
    assert!((splits[0].ratio - 0.6).abs() < f32::EPSILON);
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn private_layout_reflows_without_changing_other_clients_or_topology() {
    let mut server = test_headless_server();
    let mut workspace = crate::workspace::Workspace::test_new("private-layout");
    let first = workspace.tabs[0].root_pane;
    let second = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.tabs[0].layout.focus_pane(first);
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    let terminal = server.app.state.workspaces[0].tabs[0]
        .terminal_id(first)
        .unwrap()
        .clone();
    server
        .app
        .state
        .terminals
        .get_mut(&terminal)
        .unwrap()
        .manual_label = Some("parked".into());
    let other_terminal = server.app.state.workspaces[0].tabs[0]
        .terminal_id(second)
        .unwrap()
        .clone();
    server
        .app
        .state
        .terminals
        .get_mut(&other_terminal)
        .unwrap()
        .manual_label = Some("visible".into());
    server.app.configure_window_title("{pane}");
    let (control, render) = connect_test_shell(&mut server, 7, 80, 24);
    let (other_control, other_render) = connect_test_shell(&mut server, 8, 80, 24);
    let original = client_shell_snapshot(&control);
    let _ = client_shell_snapshot(&other_control);
    let mut policy = request(&server, 1);
    policy.hidden_terminals.push(terminal.to_string());
    assert!(apply(&mut server, 7, &policy));
    assert_eq!(
        server.app.state.workspaces[0].tabs[0].layout.focused(),
        first
    );
    assert_eq!(
        server.app.state.workspaces[0].tabs[0]
            .layout
            .pane_ids()
            .len(),
        2
    );
    assert_eq!(server.shell_focus_target(7).unwrap().pane_id, second);
    assert_eq!(server.shell_focus_target(8).unwrap().pane_id, first);
    server.foreground_client_id = Some(7);
    assert_eq!(server.configured_window_title().as_deref(), Some("visible"));
    server.render_and_stream();
    let identities = identity_projection(&control);
    let snapshot = client_shell_snapshot(&control);
    assert!(snapshot.revision > original.revision);
    assert_eq!(
        (
            identities.boot_id.as_str(),
            identities.revision,
            identities.sequence
        ),
        (snapshot.boot_id.as_str(), snapshot.revision, 1)
    );
    let public_first = server.app.public_pane_id(0, first).unwrap();
    assert_eq!(identities.pane_terminals[&public_first], terminal.as_str());
    let surface = recv_pane_surface(&render, "private surface");
    assert_eq!(surface.projection_revision, snapshot.revision);
    assert_eq!(surface.panes.len(), 1);
    assert_eq!(
        surface.panes[0].pane_id,
        server.app.public_pane_id(0, second).unwrap()
    );
    let native = recv_pane_surface(&other_render, "native surface");
    assert_eq!(native.panes.len(), 2);
    assert!(surface.panes[0].rect.width > native.panes[0].rect.width);
    assert!(server.clients[&8].pane_presentation.is_none());
    while other_control.try_recv().is_ok() {}
    policy.sequence = 2;
    policy.hidden_terminals.push(other_terminal.to_string());
    assert!(apply(&mut server, 7, &policy));
    server.render_and_stream();
    let identities = identity_projection(&control);
    let snapshot = client_shell_snapshot(&control);
    let surface = recv_pane_surface(&render, "empty private surface");
    assert_eq!(identities.revision, snapshot.revision);
    assert_eq!(surface.projection_revision, snapshot.revision);
    assert!(surface.panes.is_empty());
    assert!(snapshot.focused_pane_id.is_none());
    assert!(snapshot.focused_workspace_id.is_some());
    let failure = client_response(
        &mut server,
        api::schema::Method::PaneZoom(api::schema::PaneZoomParams::default()),
    );
    assert_eq!(
        serde_json::from_str::<api::schema::ErrorResponse>(&failure)
            .unwrap()
            .error
            .code,
        "pane_not_found"
    );
    assert!(!server.app.state.workspaces[0].tabs[0].zoomed);
    let (response, received) = std::sync::mpsc::channel();
    server.handle_api_request_with_shutdown_check(api::ApiRequestMessage {
        request: api::schema::Request {
            id: "move-hidden".into(),
            method: api::schema::Method::PaneMove(api::schema::PaneMoveParams {
                pane_id: public_first.clone(),
                destination: api::schema::PaneMoveDestination::NewWorkspace {
                    label: None,
                    tab_label: None,
                },
                focus: false,
            }),
        },
        respond_to: response,
        response_write_complete: None,
    });
    let result: api::schema::SuccessResponse =
        serde_json::from_str(&received.recv().unwrap()).unwrap();
    let api::schema::ResponseResult::PaneMove { move_result } = result.result else {
        panic!("expected move");
    };
    assert!(move_result.changed);
    assert_ne!(move_result.pane.pane_id, public_first);
    let (workspace_index, moved_pane) =
        server.app.parse_pane_id(&move_result.pane.pane_id).unwrap();
    assert!(!server.shell_client_views_pane(7, workspace_index, moved_pane));
    let mut preview = request(&server, 3);
    preview.preview_terminal = Some(terminal.to_string());
    assert!(apply(&mut server, 7, &preview));
    assert_eq!(
        server.shell_target_for_client(7).unwrap().workspace_index,
        workspace_index
    );
    assert!(!server.shell_client_accepts_pane_input(7, workspace_index, moved_pane));
    server.app.state.assert_invariants_for_test();
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn preview_rejects_input_and_clipboard_staging_until_explicit_policy_replacement() {
    let mut server = test_headless_server();
    let mut input = install_focused_test_runtime(&mut server, b"");
    server.app.state.ensure_test_terminals();
    let pane = server.app.state.workspaces[0].tabs[0].root_pane;
    let terminal = server.app.state.workspaces[0].tabs[0]
        .terminal_id(pane)
        .unwrap()
        .to_string();
    let public = server.app.public_pane_id(0, pane).unwrap();
    let (control, _render) = connect_test_shell(&mut server, 7, 80, 24);
    let _ = client_shell_snapshot(&control);
    let mut policy = request(&server, 1);
    policy.preview_terminal = Some(terminal);
    assert!(apply(&mut server, 7, &policy));
    let failure = client_response(
        &mut server,
        api::schema::Method::PaneClose(api::schema::PaneTarget {
            pane_id: "missing-pane".into(),
        }),
    );
    assert_eq!(
        serde_json::from_str::<api::schema::ErrorResponse>(&failure)
            .unwrap()
            .error
            .code,
        "pane_not_found"
    );
    assert!(
        server.clients[&7]
            .pane_presentation
            .as_ref()
            .unwrap()
            .preview_active
    );
    assert_eq!(server.shell_focus_target(7).unwrap().pane_id, pane);
    let clipboard = protocol::ClientClipboardImageTarget::Pane(public.clone());
    assert!(!server.client_clipboard_image_target_is_valid(7, &clipboard));
    assert!(!server.paste_client_clipboard_image_path(
        7,
        clipboard.clone(),
        "must not paste".into()
    ));
    for kind in [
        protocol::ClientKeyKind::Press,
        protocol::ClientKeyKind::Repeat,
    ] {
        assert!(
            !server.handle_server_event(ServerEvent::ClientShellPaneInput {
                client_id: 7,
                pane_id: public.clone(),
                events: vec![protocol::ClientPaneInputEvent::Key {
                    code: protocol::ClientKeyCode::Char('p'),
                    modifiers: 0,
                    kind,
                    repeat_count: 1,
                    shifted_codepoint: None,
                    generated_text: None,
                    tracks_release: false,
                    physical_key_id: None,
                    windows_record: None,
                }],
            })
        );
    }
    for event in [
        protocol::ClientPaneInputEvent::TextCommit("must not type".into()),
        protocol::ClientPaneInputEvent::Paste("must not paste".into()),
        protocol::ClientPaneInputEvent::Mouse {
            kind: protocol::ClientMouseKind::Down(protocol::ClientMouseButton::Left),
            position: protocol::ClientMousePosition::Cell { column: 0, row: 0 },
            geometry: None,
            modifiers: 0,
            lines: 1,
        },
    ] {
        assert!(
            !server.handle_server_event(ServerEvent::ClientShellPaneInput {
                client_id: 7,
                pane_id: public.clone(),
                events: vec![event],
            })
        );
    }
    assert!(input.try_recv().is_err());
    server
        .clients
        .get_mut(&7)
        .unwrap()
        .pane_presentation
        .as_mut()
        .unwrap()
        .deactivate_focus();
    assert!(!server.client_clipboard_image_target_is_valid(7, &clipboard));
    let restored = request(&server, 2);
    assert!(apply(&mut server, 7, &restored));
    assert!(server.client_clipboard_image_target_is_valid(7, &clipboard));
    server.handle_server_event(ServerEvent::ClientShellPaneInput {
        client_id: 7,
        pane_id: public,
        events: vec![protocol::ClientPaneInputEvent::TextCommit("x".into())],
    });
    assert_eq!(input.try_recv().unwrap(), Bytes::from_static(b"x"));
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn interactive_peek_policy_accepts_input_only_until_the_parked_policy_returns() {
    let mut server = test_headless_server();
    let mut input = install_focused_test_runtime(&mut server, b"\x1b[?1000h");
    server.app.state.ensure_test_terminals();
    let pane = server.app.state.workspaces[0].tabs[0].root_pane;
    let terminal = server.app.state.workspaces[0].tabs[0]
        .terminal_id(pane)
        .unwrap()
        .to_string();
    let public = server.app.public_pane_id(0, pane).unwrap();
    let (control, _render) = connect_test_shell(&mut server, 7, 80, 24);
    let _ = client_shell_snapshot(&control);
    let clipboard = protocol::ClientClipboardImageTarget::Pane(public.clone());
    let mut parked = request(&server, 1);
    parked.hidden_terminals.push(terminal.clone());
    assert!(apply(&mut server, 7, &parked));
    assert!(!server.client_clipboard_image_target_is_valid(7, &clipboard));

    let mut peek = request(&server, 2);
    peek.focus_terminal = Some(terminal);
    assert!(apply(&mut server, 7, &peek));
    assert!(server.client_clipboard_image_target_is_valid(7, &clipboard));
    for (event, expected) in [
        (
            protocol::ClientPaneInputEvent::TextCommit("reply".into()),
            b"reply".as_slice(),
        ),
        (
            protocol::ClientPaneInputEvent::Paste("paste".into()),
            b"paste".as_slice(),
        ),
        (
            protocol::ClientPaneInputEvent::Mouse {
                kind: protocol::ClientMouseKind::Down(protocol::ClientMouseButton::Left),
                position: protocol::ClientMousePosition::Cell { column: 0, row: 0 },
                geometry: None,
                modifiers: 0,
                lines: 1,
            },
            b"\x1b[M !!".as_slice(),
        ),
    ] {
        server.handle_server_event(ServerEvent::ClientShellPaneInput {
            client_id: 7,
            pane_id: public.clone(),
            events: vec![event],
        });
        assert_eq!(input.try_recv().unwrap().as_ref(), expected);
    }
    assert!(server.paste_client_clipboard_image_path(7, clipboard.clone(), "image.png".into()));
    assert_eq!(input.try_recv().unwrap(), Bytes::from_static(b"image.png"));
    parked.sequence = 3;
    assert!(apply(&mut server, 7, &parked));
    while input.try_recv().is_ok() {} // The policy releases the peek's held mouse button.
    assert!(!server.client_clipboard_image_target_is_valid(7, &clipboard));
    assert!(!server.paste_client_clipboard_image_path(7, clipboard, "must not paste".into()));
    server.handle_server_event(ServerEvent::ClientShellPaneInput {
        client_id: 7,
        pane_id: public,
        events: vec![protocol::ClientPaneInputEvent::TextCommit(
            "must not type".into(),
        )],
    });
    assert!(input.try_recv().is_err());
    server.app.state.assert_invariants_for_test();
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn malformed_wrong_boot_and_stale_policies_leave_the_accepted_view_unchanged() {
    let mut server = test_headless_server();
    let _input = install_focused_test_runtime(&mut server, b"");
    let (control, _render) = connect_test_shell(&mut server, 7, 80, 24);
    let _ = client_shell_snapshot(&control);
    let accepted = request(&server, 4);
    assert!(apply(&mut server, 7, &accepted));
    let mut wrong_boot = request(&server, 5);
    wrong_boot.boot_id = "old-boot".into();
    let mut missing = request(&server, 5);
    missing.preview_terminal = Some("closed-terminal".into());
    for (data, expected) in [
        ("broken".into(), "invalid_request"),
        (serde_json::to_string(&wrong_boot).unwrap(), "wrong_boot"),
        (
            serde_json::to_string(&request(&server, 3)).unwrap(),
            "stale_sequence",
        ),
        (serde_json::to_string(&missing).unwrap(), "pane_not_found"),
    ] {
        assert!(
            !server.handle_server_event(ServerEvent::ClientPanePresentation { client_id: 7, data })
        );
        let ServerMessage::EndpointControl { kind, data } =
            read_server_message(control.recv().unwrap())
        else {
            panic!("expected local error");
        };
        assert_eq!(kind, contract::ERROR_KIND);
        assert_eq!(
            serde_json::from_str::<contract::PanePresentationError>(&data)
                .unwrap()
                .code,
            expected
        );
        assert_eq!(
            server.clients[&7]
                .pane_presentation
                .as_ref()
                .unwrap()
                .request,
            accepted
        );
    }
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn focusing_a_parked_terminal_peeks_without_moving_the_shared_location() {
    let mut server = test_headless_server();
    let mut workspace = crate::workspace::Workspace::test_new("peek");
    let visible = workspace.tabs[0].root_pane;
    let parked = workspace.test_split(ratatui::layout::Direction::Horizontal);
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    let channels = server.app.tab_channels();
    server.app.state.park_pane(0, parked, channels).unwrap();
    let parked_terminal = server.app.state.workspaces[0].tabs[1]
        .terminal_id(parked)
        .unwrap()
        .to_string();
    let (_control, _render) = connect_test_shell(&mut server, 7, 80, 24);
    let home = server.shell_target_for_client(7).unwrap();
    assert_eq!(home.tab_index, 0);

    let mut peek = request(&server, 1);
    peek.focus_terminal = Some(parked_terminal);
    assert!(apply(&mut server, 7, &peek));
    let target = server.shell_target_for_client(7).unwrap();
    assert_eq!(target.tab_index, 1);
    assert_eq!(server.shell_focus_target(7).unwrap().pane_id, parked);
    assert!(server.shell_client_accepts_pane_input(7, 0, parked));
    assert!(!server.shell_client_accepts_pane_input(7, 0, visible));
    server.reconcile_client_shell_locations();
    assert_eq!(server.shell_target_for_client(7).unwrap().tab_index, 1);

    server.deactivate_client_pane_focus(7);
    assert_eq!(server.shell_target_for_client(7).unwrap().tab_index, 0);
    assert_eq!(server.shell_focus_target(7).unwrap().pane_id, visible);
}

/// Two visible tabs, with a pane parked from the first in a hidden third tab.
fn two_tabs_and_a_parked_pane() -> (HeadlessServer, String) {
    let mut server = test_headless_server();
    let mut workspace = crate::workspace::Workspace::test_new("peek");
    workspace.test_add_tab(Some("second"));
    workspace.switch_tab(0);
    let parked = workspace.test_split(ratatui::layout::Direction::Horizontal);
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    let channels = server.app.tab_channels();
    server.app.state.park_pane(0, parked, channels).unwrap();
    let parked_terminal = server.app.state.workspaces[0].tabs[2]
        .terminal_id(parked)
        .unwrap()
        .to_string();
    (server, parked_terminal)
}

fn peek_at(server: &mut HeadlessServer, terminal: String) {
    let mut peek = request(server, 1);
    peek.focus_terminal = Some(terminal);
    assert!(apply(server, 7, &peek));
    assert_eq!(server.shell_target_for_client(7).unwrap().tab_index, 2);
}

#[tokio::test]
async fn clearing_a_peek_returns_to_the_shared_location() {
    let (mut server, parked) = two_tabs_and_a_parked_pane();
    let (_control, _render) = connect_test_shell(&mut server, 7, 80, 24);
    assert_eq!(server.shell_target_for_client(7).unwrap().tab_index, 0);
    peek_at(&mut server, parked);

    let clear = request(&server, 2);
    assert!(apply(&mut server, 7, &clear));
    assert_eq!(server.shell_target_for_client(7).unwrap().tab_index, 0);
}

#[tokio::test]
async fn requests_during_a_peek_keep_the_shared_location() {
    let (mut server, parked) = two_tabs_and_a_parked_pane();
    let (_control, _render) = connect_test_shell(&mut server, 7, 80, 24);
    server.app.state.workspaces[0].switch_tab(1);
    peek_at(&mut server, parked);

    client_request(
        &mut server,
        api::schema::Method::WorkspaceList(api::schema::EmptyParams::default()),
    );
    let clear = request(&server, 2);
    assert!(apply(&mut server, 7, &clear));
    assert_eq!(server.shell_target_for_client(7).unwrap().tab_index, 0);
}

#[tokio::test]
async fn public_focus_navigation_ends_a_peek() {
    let (mut server, parked) = two_tabs_and_a_parked_pane();
    let (_control, _render) = connect_test_shell(&mut server, 7, 80, 24);
    peek_at(&mut server, parked);
    let tab_id = server.app.public_tab_id(0, 1).unwrap();

    let (respond_to, response) = std::sync::mpsc::channel();
    server.handle_api_request_with_shutdown_check(api::ApiRequestMessage {
        request: api::schema::Request {
            id: "public-focus".into(),
            method: api::schema::Method::TabFocus(api::schema::TabTarget { tab_id }),
        },
        respond_to,
        response_write_complete: None,
    });
    response.recv().unwrap();
    assert_eq!(server.shell_target_for_client(7).unwrap().tab_index, 1);
}

#[tokio::test]
async fn parked_peek_geometry_follows_the_remaining_viewer() {
    let mut server = test_headless_server();
    let mut workspace = crate::workspace::Workspace::test_new("peek");
    let parked = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.insert_test_runtime(
        parked,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 23, b"PARKED"),
    );
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    let channels = server.app.tab_channels();
    server.app.state.park_pane(0, parked, channels).unwrap();
    let terminal = server.app.state.workspaces[0].tabs[1]
        .terminal_id(parked)
        .unwrap()
        .to_string();
    let _home = connect_test_shell(&mut server, 9, 80, 30);
    let _small = connect_test_shell(&mut server, 7, 80, 24);
    let _tall = connect_test_shell(&mut server, 8, 80, 40);
    for client in [7, 8] {
        let mut peek = request(&server, 1);
        peek.focus_terminal = Some(terminal.clone());
        assert!(apply(&mut server, client, &peek));
    }
    let size = |server: &HeadlessServer| {
        server.app.state.workspaces[0].test_runtimes[&parked].current_size()
    };
    let tab_id = server.shell_tab_id_for_client(8).unwrap();
    let controller = server.tab_geometry_controllers[&tab_id];
    let other = if controller == 7 { 8 } else { 7 };
    let before = size(&server);

    let boot_id = server.client_shell_boot_id.clone();
    assert!(
        server.handle_server_event(ServerEvent::ClientShellEndpointRequest {
            client_id: controller,
            boot_id,
            request: Box::new(api::schema::Request {
                id: "background".into(),
                method: api::schema::Method::ClientShellSurfaceSet(
                    api::schema::ClientShellSurfaceSetParams { active: false },
                ),
            }),
        })
    );
    assert_eq!(server.tab_geometry_controllers.get(&tab_id), Some(&other));
    assert_ne!(size(&server), before);
    shutdown_test_runtimes(&mut server);
}
