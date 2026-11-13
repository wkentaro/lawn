use super::*;
use crate::protocol::endpoint::pane_presentation::{PaneIdentities, SetPanePresentation};
use crossterm::event::KeyEventKind;
use std::time::Instant;

fn tick(state: &mut ClientShellState) -> ClientShellInput {
    let mut outcome = state.tick_parking(Instant::now());
    state.focus_after_parked_close(&mut outcome);
    outcome
}

fn request(state: &mut ClientShellState) -> SetPanePresentation {
    state
        .tick_parking(Instant::now())
        .actions
        .into_iter()
        .find_map(|action| match action {
            ClientShellAction::PanePresentation { request, .. } => Some(request),
            _ => None,
        })
        .expect("presentation request")
}

fn acknowledge(state: &mut ClientShellState, request: &SetPanePresentation, focused: Option<&str>) {
    let mut projected = (**state.snapshot.as_ref().unwrap()).clone();
    projected.revision += 1;
    projected.focused_pane_id = focused.map(str::to_owned);
    state.set_endpoint_pane_identities(
        &ClientEndpointId::Local,
        7,
        PaneIdentities {
            boot_id: projected.boot_id.clone(),
            revision: projected.revision,
            sequence: request.sequence,
            pane_terminals: projected
                .panes
                .iter()
                .map(|pane| (pane.pane_id.clone(), format!("terminal-{}", pane.pane_id)))
                .collect(),
        },
    );
    let revision = projected.revision;
    state.set_endpoint_snapshot_for_generation(&ClientEndpointId::Local, 7, Box::new(projected));
    let mut frame = surface();
    frame.projection_revision = revision;
    if focused.is_none() {
        frame.panes.clear();
    }
    state.set_pane_surface(frame);
}

fn state() -> ClientShellState {
    state_with_snapshot(snapshot())
}

fn state_with_snapshot(projected: ClientShellSnapshot) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_endpoint_parking_supported(&ClientEndpointId::Local, true);
    state.set_endpoint_snapshot_for_generation(&ClientEndpointId::Local, 7, Box::new(projected));
    let first = request(&mut state);
    acknowledge(&mut state, &first, Some("pane_1"));
    assert!(state.parking_ready());
    state
}

fn binding(state: &mut ClientShellState, action: crate::input::KeybindAction) -> ClientShellInput {
    let mut outcome = ClientShellInput::default();
    state.record_binding(crate::input::KeybindMatch::Action(action), &mut outcome);
    outcome
}

fn click(state: &mut ClientShellState, rect: Rect) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })])
}

#[test]
fn parking_last_active_pane_opens_one_shell_in_its_current_directory() {
    for (foreground, cwd, expected) in [
        (Some("/repo/subdir"), Some("/repo"), Some("/repo/subdir")),
        (None, Some("/repo"), Some("/repo")),
        (None, None, None),
    ] {
        let mut projected = snapshot();
        projected.panes[0].foreground_cwd = foreground.map(str::to_owned);
        projected.panes[0].cwd = cwd.map(str::to_owned);
        let mut state = state_with_snapshot(projected);
        let parked = binding(&mut state, crate::input::KeybindAction::ParkPane);
        let creates = parked
            .actions
            .iter()
            .filter_map(|action| match action {
                ClientShellAction::Endpoint { request, .. } => match &request.method {
                    crate::api::schema::Method::TabCreate(params) => Some(params),
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(creates.len(), 1);
        assert_eq!(creates[0].workspace_id.as_deref(), Some("ws_1"));
        assert_eq!(creates[0].cwd.as_deref(), expected);
        assert!(creates[0].focus);
        let policy = request(&mut state);
        acknowledge(&mut state, &policy, None);
        assert!(tick(&mut state).actions.is_empty());
        let mut repeated = ClientShellInput::default();
        state.parking_action(
            ClientEndpointId::Local,
            "pane_1".into(),
            lawn::ParkingAction::Park,
            &mut repeated,
        );
        assert!(
            repeated.actions.is_empty(),
            "already parked panes never create another shell"
        );
    }
}

#[test]
fn parking_with_another_active_pane_does_not_create_a_shell() {
    let mut state = state_with_snapshot(tabbed_snapshot());
    let parked = binding(&mut state, crate::input::KeybindAction::ParkPane);
    assert!(parked.actions.is_empty());
    assert_eq!(request(&mut state).hidden_terminals, ["terminal-pane_1"]);
}

#[test]
fn relative_navigation_from_a_hidden_preview_uses_adjacent_visible_workspaces() {
    for multiple in [false, true] {
        for (hidden, next, previous) in [(1, 2, 3), (2, 3, 1), (3, 1, 2)] {
            let mut projected = tabbed_snapshot();
            for number in [2, 3] {
                let mut workspace = projected.workspaces[0].clone();
                workspace.workspace_id = format!("ws_{number}");
                workspace.active_tab_id = format!("tab_{number}");
                workspace.number = number;
                workspace.focused = false;
                projected.tabs[number - 1].workspace_id = workspace.workspace_id.clone();
                projected.panes[number - 1].workspace_id = workspace.workspace_id.clone();
                projected.workspaces.push(workspace);
            }
            projected.focused_workspace_id = Some(format!("ws_{hidden}"));
            let mut state = state_with_snapshot(projected);
            if multiple {
                state.set_endpoint_catalog(&[crate::client::endpoint::SavedSshEndpoint {
                    id: crate::client::endpoint::ProfileId::parse(
                        "0123456789abcdef0123456789abcdef",
                    )
                    .unwrap(),
                    label: "Build".into(),
                    target: "dev@build.example".into(),
                    session: "agents".into(),
                    enabled: true,
                }]);
            }
            let pane = format!("pane_{hidden}");
            state.parking_action(
                ClientEndpointId::Local,
                pane.clone(),
                lawn::ParkingAction::Park,
                &mut ClientShellInput::default(),
            );
            let parked = request(&mut state);
            acknowledge(&mut state, &parked, None);
            state.parking_action(
                ClientEndpointId::Local,
                pane.clone(),
                lawn::ParkingAction::Preview,
                &mut ClientShellInput::default(),
            );
            let preview = request(&mut state);
            acknowledge(&mut state, &preview, Some(&pane));
            for (action, number) in [
                (crate::input::KeybindAction::NextWorkspace, next),
                (crate::input::KeybindAction::PreviousWorkspace, previous),
            ] {
                let expected = format!("ws_{number}");
                let outcome = binding(&mut state, action);
                assert!(outcome.actions.iter().any(|action| match action {
                    ClientShellAction::Endpoint { endpoint_id, request, .. } => endpoint_id == &ClientEndpointId::Local && matches!(&request.method, crate::api::schema::Method::WorkspaceFocus(target) if target.workspace_id == expected),
                    ClientShellAction::ActivateEndpoint { endpoint_id, target: Some(ClientEndpointFocusTarget::Workspace(workspace)), .. } => endpoint_id == &ClientEndpointId::Local && workspace == &expected,
                    _ => false,
                }), "multiple={multiple}, hidden={hidden}, expected={expected}: {:?}", outcome.actions);
            }
        }
    }
}

#[test]
fn parking_the_last_terminal_keeps_workspace_and_parked_accessible() {
    let mut state = state();
    state.compose(100, 24).unwrap();
    state.open_pane_context_menu("pane_1".into(), 40, 3);
    let Some(ClientShellOverlay::ContextMenu(menu)) = &state.overlay else {
        panic!("pane menu")
    };
    let park = menu
        .items()
        .iter()
        .position(|item| item.action == ClientContextMenuAction::ParkPane)
        .unwrap();
    let mut outcome = ClientShellInput::default();
    state.activate_context_menu_item(park, &mut outcome);
    assert!(outcome.requests.is_empty());
    let parked = request(&mut state);
    assert_eq!(parked.hidden_terminals, ["terminal-pane_1"]);
    assert!(state
        .handle_input_bytes(b"should not type")
        .requests
        .is_empty());
    acknowledge(&mut state, &parked, None);
    let frame = state.compose(100, 24).unwrap();
    assert_eq!(
        state
            .snapshot
            .as_ref()
            .unwrap()
            .focused_workspace_id
            .as_deref(),
        Some("ws_1")
    );
    assert_eq!(state.focused_tab_count(), 0);
    assert!(state.hits.tabs.is_empty());
    assert!(state.hits.panes.is_empty());
    assert!(state.hits.workspaces.is_empty());
    assert!(
        binding(&mut state, crate::input::KeybindAction::NextWorkspace)
            .actions
            .is_empty()
    );
    assert!(!state.hits.new_workspace_empty.is_empty());
    assert!(frame_rows(&frame).join("\n").contains("open parked"));
    let open = state.hits.open_parked;
    assert!(!open.is_empty());
    click(&mut state, open);
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
        panic!("Parked")
    };
    assert!(navigator.parked);
    navigator.selected = Some(ClientNavigatorTarget::Pane {
        endpoint_id: ClientEndpointId::Local,
        pane_id: "pane_1".into(),
    });
    let opened = state.handle_input_bytes(b"\r");
    assert!(opened.actions.is_empty());
    assert!(state.overlay.is_none());
    assert!(state.has_parking_preview());
    assert!(state
        .handle_input_bytes(b"wait for policy")
        .requests
        .is_empty());
    let preview = request(&mut state);
    assert!(preview.hidden_terminals.is_empty());
    assert!(preview.read_only_terminals.is_empty());
    assert_eq!(preview.preview_terminal, None);
    assert_eq!(preview.focus_terminal.as_deref(), Some("terminal-pane_1"));
    acknowledge(&mut state, &preview, Some("pane_1"));
    assert!(!state.handle_input_bytes(b"writable").requests.is_empty());
    assert_eq!(state.focused_tab_count(), 0);
    assert!(state.active_parking().unwrap().hides_pane("pane_1"));
    state.compose(100, 24).unwrap();
    let restore = state.hits.restore_pane;
    assert!(!restore.is_empty());
    click(&mut state, restore);
    let restored = request(&mut state);
    assert!(restored.hidden_terminals.is_empty());
    assert_eq!(restored.focus_terminal.as_deref(), Some("terminal-pane_1"));
    acknowledge(&mut state, &restored, Some("pane_1"));
    assert!(!state.has_parking_preview());
    assert_eq!(state.focused_tab_count(), 1);
}

#[test]
fn parked_peek_accepts_paste_mouse_and_images_then_dismisses_without_closing() {
    for close in 0..5 {
        let mut state = parked_with_active_shell(true);
        state.open_parked();
        state.handle_input_bytes(b"pane_1\r");
        let peek = request(&mut state);
        assert_eq!(
            peek.hidden_terminals,
            ["terminal-pane_2", "terminal-pane_3"]
        );
        assert!(state.clipboard_image_target().is_none());
        acknowledge(&mut state, &peek, Some("pane_1"));
        state.pane_surface.as_mut().unwrap().panes[0].mouse_reporting = true;
        let frame = state.compose(100, 24).unwrap();
        assert!(frame.cursor.is_some());
        let header = frame_rows(&frame).join("\n");
        assert!(header.contains("restore   dismiss"));
        assert_eq!(
            state.clipboard_image_target(),
            Some(crate::protocol::ClientClipboardImageTarget::Pane(
                "pane_1".into()
            ))
        );
        let paste = state.handle_raw_events(vec![RawInputEvent::Paste("reply".into())]);
        assert!(paste.requests.iter().any(|message| matches!(message,
            ClientMessage::ClientShellPaneInput { pane_id, events } if pane_id == "pane_1" && events.iter().any(|event| matches!(event, ClientPaneInputEvent::Paste(text) if text == "reply"))
        )));
        let pane = state.hits.panes[0].inner_rect;
        assert!(!click(&mut state, pane).requests.is_empty());
        state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: pane.x,
            row: pane.y,
            modifiers: KeyModifiers::empty(),
        })]);
        let dismissed = match close {
            0 => state.handle_input_bytes(b"\x1b"),
            1 => {
                let dismiss = state.hits.dismiss_parking_preview;
                click(&mut state, dismiss)
            }
            2 => binding(&mut state, crate::input::KeybindAction::ClosePane),
            3 => binding(&mut state, crate::input::KeybindAction::CloseTab),
            _ => binding(&mut state, crate::input::KeybindAction::CloseWorkspace),
        };
        assert!(!state.has_parking_preview(), "close action {close}");
        assert!(dismissed.actions.iter().all(|action| matches!(action,
            ClientShellAction::Endpoint { request, .. } if matches!(request.method, crate::api::schema::Method::WorkspaceFocus(_))
        )));
        assert!(state.handle_input_bytes(b"must wait").requests.is_empty());
        let dismissed = request(&mut state);
        assert_eq!(dismissed.hidden_terminals, ["terminal-pane_1"]);
        acknowledge(&mut state, &dismissed, Some("pane_2"));
        assert!(state.active_parking().unwrap().hides_pane("pane_1"));
        assert!(!state
            .handle_input_bytes(b"back in shell")
            .requests
            .is_empty());
    }
}

#[test]
fn parked_launcher_includes_parked_terminal_and_goto_excludes_it() {
    let mut state = state();
    binding(&mut state, crate::input::KeybindAction::ParkPane);
    let parked = request(&mut state);
    acknowledge(&mut state, &parked, None);
    state.open_navigator_overlay();
    for query in ["", "client-shell", "main"] {
        let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
            panic!("Jump")
        };
        navigator.query = query.into();
        let active =
            render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, navigator);
        assert!(
            active.is_empty(),
            "parked-only workspace appears for {query:?}"
        );
    }
    state.open_parked();
    state.handle_input_bytes(b"client-shell");
    let Some(ClientShellOverlay::Navigator(navigator)) = &state.overlay else {
        panic!("Parked search")
    };
    assert!(navigator.parked);
    assert_eq!(navigator.query.as_str(), "client-shell");
    let rows =
        render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, navigator);
    assert_eq!(rows.len(), 2);
    assert!(matches!(
        rows[0].target,
        ClientNavigatorTarget::Workspace { .. }
    ));
    assert_eq!(rows[0].depth, 0);
    assert_eq!(rows[1].depth, 1);
    assert!(
        rows.iter().all(|row| row.status.is_none()),
        "ordinary terminal has no agent status"
    );
    state.overlay = None;
    let frame = state.compose(100, 24).unwrap();
    let launcher = state.hits.parked_launcher;
    assert_eq!(launcher.height, 1);
    let rows = frame_rows(&frame);
    assert!(rows[usize::from(launcher.y)].contains("parked"));
    assert!(rows[usize::from(launcher.y)].contains('1'));
    click(&mut state, launcher);
    assert!(matches!(state.overlay, Some(ClientShellOverlay::Navigator(ref nav)) if nav.parked));
}

#[test]
fn goto_filters_parked_panes_in_separate_and_mixed_tabs() {
    for mixed_tab in [false, true] {
        let mut state = parked_with_active_shell(mixed_tab);
        state.open_navigator_overlay();
        let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
            panic!("Go to")
        };
        for (query, expected) in [
            ("", vec!["pane_2", "pane_3"]),
            ("client-shell", vec!["pane_2"]),
            ("pane_1", vec![]),
            ("pane_2", vec!["pane_2"]),
        ] {
            navigator.query = TextEditor::new(query, false);
            let rows = render::client_navigator_rows(
                &state.endpoints,
                &state.active_endpoint_id,
                navigator,
            );
            let panes = rows
                .iter()
                .filter_map(|row| match &row.target {
                    ClientNavigatorTarget::Pane { pane_id, .. } => Some(pane_id.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(panes, expected, "mixed tab: {mixed_tab}, query: {query}");
        }
    }
}

#[test]
fn active_and_parked_launchers_group_pane_counts_above_the_sidebar_controls() {
    for mixed_tab in [false, true] {
        for remote in [false, true] {
            let mut state = parked_with_active_shell(mixed_tab);
            if remote {
                let profile = crate::client::endpoint::SavedSshEndpoint {
                    id: crate::client::endpoint::ProfileId::parse(
                        "0123456789abcdef0123456789abcdef",
                    )
                    .unwrap(),
                    label: "Build".into(),
                    target: "dev@build.example".into(),
                    session: "agents".into(),
                    enabled: true,
                };
                let endpoint = ClientEndpointId::Ssh(profile.id.clone());
                state.set_endpoint_catalog(&[profile]);
                state.set_endpoint_snapshot(&endpoint, Box::new(tabbed_snapshot()));
            }
            for mode in [
                SidebarCollapsedModeConfig::Compact,
                SidebarCollapsedModeConfig::Hidden,
            ] {
                state.config.sidebar_collapsed_mode = mode;
                for height in [8, 24] {
                    let frame = state.compose(100, height).unwrap();
                    let active = state.hits.active_launcher;
                    let parked = state.hits.parked_launcher;
                    assert_eq!(active.height, 1);
                    assert_eq!(active.bottom(), parked.y);
                    assert_eq!((active.x, active.width), (parked.x, parked.width));
                    assert_eq!(state.hits.workspace_body.bottom(), active.y);
                    assert_eq!(parked.bottom(), state.hits.global_launcher.y);
                    let buffer = frame.to_ratatui_buffer().unwrap();
                    let row = |area: Rect| {
                        (area.x..area.right())
                            .map(|x| buffer[(x, area.y)].symbol())
                            .collect::<String>()
                    };
                    let active_row = row(active);
                    assert!(active_row.starts_with(" active"));
                    assert!(active_row.ends_with(if remote { "5 " } else { "2 " }));
                    let parked_row = row(parked);
                    assert!(parked_row.starts_with(" parked"));
                    assert!(parked_row.ends_with("1 "));
                    assert!(!frame_rows(&frame)[0].contains("jump"));
                }
            }
        }
    }
}

#[test]
fn parked_launcher_counts_cached_terminals_across_machines() {
    let mut state = state();
    let profile = crate::client::endpoint::SavedSshEndpoint {
        id: crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
        label: "Build".into(),
        target: "dev@build.example".into(),
        session: "agents".into(),
        enabled: true,
    };
    state.set_endpoint_catalog(&[profile]);
    let frame = state.compose(100, 24).unwrap();
    let row = usize::from(state.hits.parked_launcher.y);
    let rows = frame_rows(&frame);
    assert!(rows[row].contains("parked"));
    assert!(rows[row].contains("0 "));

    let root = std::env::temp_dir().join(format!(
        "lawn-parked-launcher-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    for (index, endpoint) in state.endpoints.iter_mut().enumerate() {
        let path = root.join(format!("{index}.json"));
        std::fs::write(
            &path,
            format!(r#"{{"version":1,"terminals":["terminal-{index}"]}}"#),
        )
        .unwrap();
        endpoint.parking.set_path(Some(path));
        endpoint.parking.set_supported(true);
        endpoint.parking.poll(Instant::now());
    }
    let frame = state.compose(100, 24).unwrap();
    assert!(frame_rows(&frame)[row].contains("2 "));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn policy_carries_held_key_release_before_hiding_its_terminal() {
    let mut state = state();
    assert!(!state.handle_input_bytes(b"\x1b[97;5u").requests.is_empty());
    binding(&mut state, crate::input::KeybindAction::ParkPane);
    let outcome = tick(&mut state);
    let (releases, hidden) = outcome
        .actions
        .iter()
        .find_map(|action| match action {
            ClientShellAction::PanePresentation {
                releases, request, ..
            } => Some((releases, &request.hidden_terminals)),
            _ => None,
        })
        .unwrap();
    assert_eq!(hidden, &["terminal-pane_1"]);
    assert!(releases.iter().any(|message| matches!(message, ClientMessage::ClientShellPaneInput { pane_id, events } if pane_id == "pane_1" && events.iter().any(|event| matches!(event, ClientPaneInputEvent::Key { kind: crate::protocol::ClientKeyKind::Release, .. })))));
    assert!(
        outcome.requests.is_empty(),
        "release travels in order with policy, exactly once"
    );
}

#[test]
fn parking_shortcuts_are_unbound_until_configured() {
    let defaults = Config::default().keybinds();
    assert!(defaults.park_pane.label().is_none());
    assert!(defaults.restore_pane.label().is_none());
    assert!(defaults.open_parked.label().is_none());
    let config: Config = toml::from_str(
        "[keys]\npark_pane = 'prefix+f'\nrestore_pane = 'prefix+u'\nopen_parked = 'ctrl+g'\n",
    )
    .unwrap();
    assert!(config.keybinds().open_parked.label().is_some());
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_endpoint_parking_supported(&ClientEndpointId::Local, true);
    state.set_endpoint_snapshot_for_generation(&ClientEndpointId::Local, 7, Box::new(snapshot()));
    let first = request(&mut state);
    acknowledge(&mut state, &first, Some("pane_1"));
    state.handle_input_bytes(&[2]);
    assert!(state.handle_input_bytes(b"f").requests.is_empty());
    assert_eq!(request(&mut state).hidden_terminals, ["terminal-pane_1"]);
}

fn agent(pane: &str, status: AgentStatus) -> ClientShellAgent {
    ClientShellAgent {
        pane_id: pane.into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: None,
        display_agent: None,
        agent: Some("codex".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: status,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: pane == "pane_1",
    }
}

#[test]
fn focused_tab_keeps_status_colors_on_the_theme_background() {
    for name in crate::config::THEME_NAMES {
        for indicator_style in [
            crate::config::StatusIndicatorStyle::Dots,
            crate::config::StatusIndicatorStyle::Symbols,
        ] {
            for status in [
                AgentStatus::Idle,
                AgentStatus::Working,
                AgentStatus::Blocked,
            ] {
                for custom_label in [false, true] {
                    let mut config = ClientShellConfig::from_config(&Config::default());
                    config.palette = Palette::from_name(name).expect("built-in theme");
                    config.status_indicators = indicator_style;
                    // A custom terminal palette can make the accent equal a status color.
                    config.palette.accent = status_color(status, &config.palette);
                    let mut projected = snapshot();
                    projected.tabs[0].label = "selected".into();
                    projected.tabs[0].custom_label = custom_label;
                    projected.tabs.push(ClientShellTab {
                        tab_id: "tab_2".into(),
                        label: "other".into(),
                        focused: false,
                        ..projected.tabs[0].clone()
                    });
                    projected.agents = vec![agent("pane_1", status)];
                    let mut state = ClientShellState::new(config);
                    state.set_snapshot(Box::new(projected));
                    state.set_pane_surface(surface());
                    let frame = state.compose(100, 24).expect("focused tab");
                    let rendered = frame.to_ratatui_buffer().expect("rendered buffer");
                    let tab = state.hits.tabs[0].0;
                    let cells = (tab.x..tab.right())
                        .map(|x| &rendered[(x, tab.y)])
                        .collect::<Vec<_>>();
                    assert!(cells
                        .iter()
                        .all(|cell| cell.bg == state.config.palette.panel_bg));
                    let label = cells.iter().find(|cell| cell.symbol() == "s").unwrap();
                    assert_eq!(label.fg, state.config.palette.text, "{name}");
                    assert!(label
                        .modifier
                        .contains(Modifier::BOLD | Modifier::UNDERLINED));
                    let icon = cells
                        .iter()
                        .find(|cell| cell.symbol() == status_icon(status, indicator_style))
                        .expect("status glyph is preserved");
                    assert_eq!(icon.fg, status_color(status, &state.config.palette));
                    assert!(!icon.modifier.contains(Modifier::UNDERLINED));
                    let other = state.hits.tabs[1].0;
                    assert!((other.x..other.right()).all(|x| {
                        let cell = &rendered[(x, other.y)];
                        cell.bg == state.config.palette.surface0
                            && !cell.modifier.contains(Modifier::UNDERLINED)
                    }));
                }
            }
        }
    }
}

#[test]
fn tab_status_prefers_blocked_then_working_then_idle_and_excludes_parked_agents() {
    for (first, second, icon, color, count) in [
        (AgentStatus::Blocked, AgentStatus::Blocked, "●", "red", true),
        (
            AgentStatus::Working,
            AgentStatus::Blocked,
            "●",
            "red",
            false,
        ),
        (
            AgentStatus::Idle,
            AgentStatus::Working,
            "●",
            "yellow",
            false,
        ),
        (AgentStatus::Idle, AgentStatus::Idle, "○", "green", false),
    ] {
        let mut projected = snapshot();
        let mut second_pane = projected.panes[0].clone();
        second_pane.pane_id = "pane_2".into();
        second_pane.focused = false;
        projected.panes.push(second_pane);
        projected.agents = vec![agent("pane_1", first), agent("pane_2", second)];
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_endpoint_parking_supported(&ClientEndpointId::Local, true);
        state.set_endpoint_snapshot_for_generation(
            &ClientEndpointId::Local,
            7,
            Box::new(projected),
        );
        let initial = request(&mut state);
        acknowledge(&mut state, &initial, Some("pane_1"));
        let frame = state.compose(100, 24).unwrap();
        let tab = state.hits.tabs[0].0;
        let row = frame_rows(&frame)[usize::from(tab.y)].clone();
        assert!(row.contains(icon), "{row}");
        assert_eq!(row.contains("●2"), count);
        let expected = match color {
            "red" => state.config.palette.red,
            "yellow" => state.config.palette.yellow,
            _ => state.config.palette.green,
        };
        let rendered = frame.to_ratatui_buffer().unwrap();
        assert!((tab.x..tab.right())
            .any(|x| rendered[(x, tab.y)].symbol() == icon && rendered[(x, tab.y)].fg == expected));
        if count {
            let mut outcome = ClientShellInput::default();
            state.parking_action(
                ClientEndpointId::Local,
                "pane_2".into(),
                lawn::ParkingAction::Park,
                &mut outcome,
            );
            let parked = request(&mut state);
            acknowledge(&mut state, &parked, Some("pane_1"));
            let frame = state.compose(100, 24).unwrap();
            let row = frame_rows(&frame)[usize::from(tab.y)].clone();
            assert!(row.contains("●"));
            assert!(!row.contains("●2"));
        }
    }
}

#[test]
fn tab_navigation_skips_parked_tabs_and_reordering_uses_server_positions() {
    let mut projected = snapshot();
    for number in [2, 3] {
        let mut tab = projected.tabs[0].clone();
        tab.tab_id = format!("tab_{number}");
        tab.number = number;
        tab.focused = false;
        projected.tabs.push(tab);
        let mut pane = projected.panes[0].clone();
        pane.pane_id = format!("pane_{number}");
        pane.tab_id = format!("tab_{number}");
        pane.focused = false;
        projected.panes.push(pane);
    }
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_endpoint_parking_supported(&ClientEndpointId::Local, true);
    state.set_endpoint_snapshot_for_generation(&ClientEndpointId::Local, 7, Box::new(projected));
    let initial = request(&mut state);
    acknowledge(&mut state, &initial, Some("pane_1"));
    state.parking_action(
        ClientEndpointId::Local,
        "pane_2".into(),
        lawn::ParkingAction::Park,
        &mut ClientShellInput::default(),
    );
    let parked = request(&mut state);
    acknowledge(&mut state, &parked, Some("pane_1"));
    for action in [
        crate::input::KeybindAction::SwitchTab(2),
        crate::input::KeybindAction::NextTab,
        crate::input::KeybindAction::PreviousTab,
    ] {
        let outcome = binding(&mut state, action);
        assert!(outcome.actions.iter().any(|action| matches!(action, ClientShellAction::Endpoint { request, .. } if matches!(&request.method, crate::api::schema::Method::TabFocus(target) if target.tab_id == "tab_3"))));
    }
    let outcome = binding(&mut state, crate::input::KeybindAction::MoveTabNext);
    assert!(outcome.actions.iter().any(|action| matches!(action, ClientShellAction::Endpoint { request, .. } if matches!(&request.method, crate::api::schema::Method::TabMove(target) if target.tab_id == "tab_1" && target.insert_index == 3))));
}

fn patch(state: &ClientShellState) -> crate::protocol::PaneSurfacePatch {
    let current = state.pane_surface.as_ref().unwrap();
    crate::protocol::PaneSurfacePatch {
        boot_id: current.boot_id.clone(),
        projection_revision: current.projection_revision,
        base_surface_revision: current.surface_revision,
        surface_revision: current.surface_revision + 1,
        rows: Vec::new(),
        panes: current.panes.clone(),
        cursor: current.frame.cursor.clone(),
    }
}

#[test]
fn retained_updates_obey_pending_policy_and_keep_peek_cursor_and_mouse_interactive() {
    let mut state = state();
    state.compose(100, 24).unwrap();
    binding(&mut state, crate::input::KeybindAction::ParkPane);
    let pending_patch = patch(&state);
    assert!(matches!(
        state.apply_pane_surface_patch(pending_patch),
        ClientPaneSurfacePatchOutcome::Applied(None)
    ));
    let parked = request(&mut state);
    acknowledge(&mut state, &parked, None);
    state.parking_action(
        ClientEndpointId::Local,
        "pane_1".into(),
        lawn::ParkingAction::Preview,
        &mut ClientShellInput::default(),
    );
    let preview = request(&mut state);
    acknowledge(&mut state, &preview, Some("pane_1"));
    state.compose(100, 24).unwrap();
    let mut update = patch(&state);
    update.panes[0].mouse_reporting = true;
    assert!(update.cursor.is_some());
    let ClientPaneSurfacePatchOutcome::Applied(Some(composed)) =
        state.apply_pane_surface_patch(update)
    else {
        panic!("preview retains the update fast path")
    };
    assert!(composed.cursor.is_some());
    assert!(state.hits.panes[0].mouse_reporting);
    assert!(!state.handle_input_bytes(b"reply").requests.is_empty());
    assert!(state.active_parking().unwrap().hides_pane("pane_1"));
    assert!(
        tick(&mut state).actions.is_empty(),
        "acknowledged peek stays settled"
    );
}

#[test]
fn preview_releases_held_input_in_other_visible_panes_before_hiding_them() {
    let mut projected = snapshot();
    let mut second = projected.panes[0].clone();
    second.pane_id = "pane_2".into();
    second.focused = false;
    projected.panes.push(second);
    let mut state = state_with_snapshot(projected);
    assert!(!state.handle_input_bytes(b"\x1b[97;5u").requests.is_empty());
    state.parking_action(
        ClientEndpointId::Local,
        "pane_2".into(),
        lawn::ParkingAction::Park,
        &mut ClientShellInput::default(),
    );
    let parked = tick(&mut state);
    let request = parked
        .actions
        .into_iter()
        .find_map(|action| match action {
            ClientShellAction::PanePresentation {
                request, releases, ..
            } => {
                assert!(releases.is_empty());
                Some(request)
            }
            _ => None,
        })
        .unwrap();
    acknowledge(&mut state, &request, Some("pane_1"));
    state.parking_action(
        ClientEndpointId::Local,
        "pane_2".into(),
        lawn::ParkingAction::Preview,
        &mut ClientShellInput::default(),
    );
    let preview = tick(&mut state);
    assert!(preview.actions.iter().any(|action| matches!(action, ClientShellAction::PanePresentation { request, releases, .. } if request.preview_terminal.is_none() && request.focus_terminal.as_deref() == Some("terminal-pane_2") && releases.iter().any(|message| matches!(message, ClientMessage::ClientShellPaneInput { pane_id, events } if pane_id == "pane_1" && events.iter().any(|event| matches!(event, ClientPaneInputEvent::Key { kind: crate::protocol::ClientKeyKind::Release, .. })))))));
    assert!(state
        .handle_input_bytes(b"\x1b[97;5:3u")
        .requests
        .is_empty());
}

#[test]
fn old_server_disables_parking_controls_and_keeps_terminal_input() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.open_pane_context_menu("pane_1".into(), 40, 3);
    let Some(ClientShellOverlay::ContextMenu(menu)) = &state.overlay else {
        panic!("pane menu")
    };
    assert!(!menu
        .items()
        .iter()
        .any(|item| item.action == ClientContextMenuAction::ParkPane));
    state.overlay = None;
    let outcome = binding(&mut state, crate::input::KeybindAction::ParkPane);
    assert!(outcome.actions.is_empty());
    let frame = state.compose(100, 24).unwrap();
    assert!(frame_rows(&frame)
        .join("\n")
        .contains("Parking is unavailable on this server"));
    assert!(!state
        .handle_input_bytes(b"still writable")
        .requests
        .is_empty());
}

fn tabbed_snapshot() -> ClientShellSnapshot {
    let mut projected = snapshot();
    for number in [2, 3] {
        let mut tab = projected.tabs[0].clone();
        tab.tab_id = format!("tab_{number}");
        tab.number = number;
        tab.focused = false;
        projected.tabs.push(tab);
        let mut pane = projected.panes[0].clone();
        pane.pane_id = format!("pane_{number}");
        pane.tab_id = format!("tab_{number}");
        pane.focused = false;
        projected.panes.push(pane);
    }
    projected
}

fn parked_with_active_shell(mixed_tab: bool) -> ClientShellState {
    let mut projected = tabbed_snapshot();
    let mut other = projected.workspaces[0].clone();
    other.workspace_id = "ws_2".into();
    other.number = 2;
    other.label = "other".into();
    other.focused = false;
    projected.workspaces.push(other);
    projected.tabs[2].workspace_id = "ws_2".into();
    projected.panes[2].workspace_id = "ws_2".into();
    if mixed_tab {
        projected.panes[1].tab_id = "tab_1".into();
        projected.tabs.remove(1);
    }
    let mut state = state_with_snapshot(projected);
    binding(&mut state, crate::input::KeybindAction::ParkPane);
    let policy = request(&mut state);
    acknowledge(&mut state, &policy, Some("pane_2"));
    state
}

fn closed_panes(outcome: &ClientShellInput) -> Vec<&str> {
    outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::Endpoint { request, .. } => match &request.method {
                crate::api::schema::Method::PaneClose(target) => Some(target.pane_id.as_str()),
                other => panic!("unexpected destructive or focus request: {other:?}"),
            },
            _ => None,
        })
        .collect()
}

#[test]
fn workspace_close_preserves_parked_panes() {
    for confirm in [false, true] {
        let mut state = parked_with_active_shell(false);
        state.config.confirm_close = confirm;
        let mut outcome = ClientShellInput::default();
        state.request_workspace_close("ws_1".into(), &mut outcome);
        if confirm {
            assert!(outcome.actions.is_empty());
            state.accept_close_confirmation(&mut outcome);
        }
        assert_eq!(closed_panes(&outcome), ["pane_2"]);
        assert!(state.active_parking().unwrap().hides_pane("pane_1"));
    }
}

#[test]
fn legacy_group_root_close_is_blocked_when_only_linked_workspaces_have_parked_sessions() {
    let mut state = parked_with_active_shell(false);
    state.parking_action(
        ClientEndpointId::Local,
        "pane_1".into(),
        lawn::ParkingAction::Restore,
        &mut ClientShellInput::default(),
    );
    let restored = request(&mut state);
    acknowledge(&mut state, &restored, Some("pane_2"));
    state.parking_action(
        ClientEndpointId::Local,
        "pane_3".into(),
        lawn::ParkingAction::Park,
        &mut ClientShellInput::default(),
    );
    let parked = request(&mut state);
    // An older server still reports ws_1 as the root of a worktree group
    // that contains ws_2.
    let mut projected = (**state.snapshot.as_ref().unwrap()).clone();
    for (workspace, is_linked_worktree) in projected.workspaces.iter_mut().zip([false, true]) {
        workspace.worktree = Some(crate::protocol::ClientShellWorktree {
            key: "repo".into(),
            label: "repo".into(),
            is_linked_worktree,
        });
    }
    projected.panes.retain(|pane| pane.pane_id != "pane_1");
    projected.tabs.retain(|tab| tab.tab_id != "tab_1");
    projected.revision += 1;
    state.set_endpoint_snapshot_for_generation(&ClientEndpointId::Local, 7, Box::new(projected));
    acknowledge(&mut state, &parked, Some("pane_2"));
    state.config.confirm_close = false;
    for method in [
        crate::api::schema::Method::WorkspaceClose(crate::api::schema::WorkspaceCloseParams {
            workspace_id: "ws_1".into(),
            close_group: false,
        }),
        crate::api::schema::Method::TabClose(crate::api::schema::TabTarget {
            tab_id: "tab_2".into(),
        }),
        crate::api::schema::Method::PaneClose(crate::api::schema::PaneTarget {
            pane_id: "pane_2".into(),
        }),
    ] {
        let mut outcome = ClientShellInput::default();
        assert!(!state.push_endpoint_method_with_kind(
            method,
            PendingEndpointKind::Generic,
            &mut outcome
        ));
        assert!(outcome.actions.is_empty());
    }
    assert!(state.active_parking().unwrap().hides_pane("pane_3"));
}

#[test]
fn mixed_tab_and_stale_pane_closes_preserve_parked_panes() {
    for confirm in [false, true] {
        let mut state = parked_with_active_shell(true);
        state.config.confirm_close = confirm;
        let mut outcome = ClientShellInput::default();
        state.request_tab_close("tab_1".into(), &mut outcome);
        if matches!(state.overlay, Some(ClientShellOverlay::ConfirmClose(_))) {
            state.accept_close_confirmation(&mut outcome);
        }
        assert_eq!(closed_panes(&outcome), ["pane_2"]);
        let mut stale = ClientShellInput::default();
        state.request_pane_close("pane_1".into(), &mut stale);
        assert!(stale.actions.is_empty());
    }
}

#[test]
fn permanent_parking_errors_allow_safe_closes_and_keep_known_parked_panes() {
    for parked in [false, true] {
        let mut state = if parked {
            parked_with_active_shell(false)
        } else {
            state()
        };
        state.receive_endpoint_parking_error(
            &ClientEndpointId::Local,
            7,
            crate::protocol::endpoint::pane_presentation::PanePresentationError {
                boot_id: "boot-1".into(),
                sequence: Some(if parked { 2 } else { 1 }),
                code: "unavailable".into(),
                message: "Presentation unavailable".into(),
            },
        );
        assert!(!state.active_parking().unwrap().available());
        state.config.confirm_close = false;
        let mut outcome = ClientShellInput::default();
        state.request_workspace_close("ws_1".into(), &mut outcome);
        if parked {
            assert_eq!(closed_panes(&outcome), ["pane_2"]);
        } else {
            assert!(outcome.actions.iter().any(|action| matches!(action, ClientShellAction::Endpoint { request, .. } if matches!(request.method, crate::api::schema::Method::WorkspaceClose(_)))));
        }
    }
}

#[test]
fn collapsed_machine_workspace_numbers_match_visible_workspace_shortcuts() {
    let mut state = parked_with_active_shell(false);
    state.parking_action(
        ClientEndpointId::Local,
        "pane_2".into(),
        lawn::ParkingAction::Park,
        &mut ClientShellInput::default(),
    );
    let parked = request(&mut state);
    acknowledge(&mut state, &parked, None);
    state.set_endpoint_catalog(&[crate::client::endpoint::SavedSshEndpoint {
        id: crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
        label: "Build".into(),
        target: "dev@build.example".into(),
        session: "agents".into(),
        enabled: true,
    }]);
    state.sidebar_collapsed = true;
    let frame = state.compose(100, 24).unwrap();
    let hit = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.endpoint_id == ClientEndpointId::Local && hit.workspace_id == "ws_2")
        .unwrap();
    assert_eq!(
        frame.to_ratatui_buffer().unwrap()[(hit.rect.x + 1, hit.rect.y)].symbol(),
        "1"
    );
    let selected = binding(&mut state, crate::input::KeybindAction::SwitchWorkspace(0));
    assert!(selected.actions.iter().any(|action| matches!(action, ClientShellAction::Endpoint { request, .. } if matches!(&request.method, crate::api::schema::Method::WorkspaceFocus(target) if target.workspace_id == "ws_2"))));
    assert!(
        !state.indexed_navigation_target_exists(&crate::input::KeybindMatch::Action(
            crate::input::KeybindAction::SwitchWorkspace(1),
        ))
    );
}

#[test]
fn closing_the_active_shell_hides_workspace_and_focuses_another_once() {
    let mut state = parked_with_active_shell(false);
    assert!(tick(&mut state).actions.is_empty());
    let mut projected = (**state.snapshot.as_ref().unwrap()).clone();
    projected.panes.retain(|pane| pane.pane_id != "pane_2");
    projected.tabs.retain(|tab| tab.tab_id != "tab_2");
    let policy = SetPanePresentation {
        boot_id: projected.boot_id.clone(),
        sequence: 2,
        hidden_terminals: vec!["terminal-pane_1".into()],
        read_only_terminals: vec![],
        preview_terminal: None,
        focus_terminal: None,
    };
    projected.revision += 1;
    state.set_endpoint_snapshot_for_generation(&ClientEndpointId::Local, 7, Box::new(projected));
    acknowledge(&mut state, &policy, None);
    let focused = tick(&mut state);
    assert!(focused.actions.iter().any(|action| matches!(action, ClientShellAction::Endpoint { request, .. }
        if matches!(&request.method, crate::api::schema::Method::WorkspaceFocus(target) if target.workspace_id == "ws_2"))));
    assert!(tick(&mut state).actions.is_empty());
    for collapsed in [false, true] {
        state.sidebar_collapsed = collapsed;
        state.compose(100, 24).unwrap();
        assert!(!state
            .hits
            .workspaces
            .iter()
            .any(|hit| hit.workspace_id == "ws_1"));
        assert!(state
            .hits
            .workspaces
            .iter()
            .any(|hit| hit.workspace_id == "ws_2"));
    }
    state.open_navigator_overlay();
    let Some(ClientShellOverlay::Navigator(navigator)) = &state.overlay else {
        panic!("Jump")
    };
    let rows =
        render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, navigator);
    let workspaces = rows
        .iter()
        .filter_map(|row| match &row.target {
            ClientNavigatorTarget::Workspace { workspace_id, .. } => Some(workspace_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(workspaces, ["ws_2"]);
    assert!(state
        .navigation_workspace_entries(state.snapshot.as_deref().unwrap())
        .iter()
        .all(
            |entry| state.snapshot.as_ref().unwrap().workspaces[entry.index].workspace_id != "ws_1"
        ));
}

#[test]
fn close_recovery_uses_the_nearest_workspace_in_sidebar_order() {
    for (order, expected) in [
        (["ws_2", "ws_1", "ws_3"], "ws_3"),
        (["ws_2", "ws_3", "ws_1"], "ws_3"),
        (["ws_1", "ws_2", "ws_3"], "ws_2"),
    ] {
        let mut state = parked_with_active_shell(false);
        let mut projected = state.snapshot.as_deref().unwrap().clone();
        let mut neighbor = projected.workspaces[1].clone();
        neighbor.workspace_id = "ws_3".into();
        neighbor.active_tab_id = "tab_4".into();
        projected.workspaces.push(neighbor);
        let mut tab = projected.tabs[2].clone();
        tab.workspace_id = "ws_3".into();
        tab.tab_id = "tab_4".into();
        projected.tabs.push(tab);
        let mut pane = projected.panes[2].clone();
        pane.workspace_id = "ws_3".into();
        pane.tab_id = "tab_4".into();
        pane.pane_id = "pane_4".into();
        projected.panes.push(pane);
        projected.workspaces.sort_by_key(|workspace| {
            order
                .iter()
                .position(|id| *id == workspace.workspace_id)
                .unwrap()
        });
        projected.revision += 1;
        state.set_endpoint_snapshot_for_generation(
            &ClientEndpointId::Local,
            7,
            Box::new(projected),
        );
        let policy = SetPanePresentation {
            boot_id: "boot-1".into(),
            sequence: 2,
            hidden_terminals: vec!["terminal-pane_1".into()],
            read_only_terminals: vec![],
            preview_terminal: None,
            focus_terminal: None,
        };
        acknowledge(&mut state, &policy, Some("pane_2"));
        let mut closed = state.snapshot.as_deref().unwrap().clone();
        closed.panes.retain(|pane| pane.pane_id != "pane_2");
        closed.tabs.retain(|tab| tab.tab_id != "tab_2");
        closed.revision += 1;
        state.set_endpoint_snapshot_for_generation(&ClientEndpointId::Local, 7, Box::new(closed));
        acknowledge(&mut state, &policy, None);
        let focused = tick(&mut state);
        assert!(
            focused.actions.iter().any(|action| matches!(action,
                ClientShellAction::Endpoint { request, .. }
                    if matches!(&request.method, crate::api::schema::Method::WorkspaceFocus(target)
                        if target.workspace_id == expected)
            )),
            "order={order:?}, expected={expected}: {:?}",
            focused.actions
        );
        assert!(tick(&mut state).actions.is_empty());
    }
}

#[test]
fn close_recovery_skips_a_parked_neighbor_selected_by_the_server() {
    let mut state = parked_with_active_shell(false);
    state.parking_action(
        ClientEndpointId::Local,
        "pane_2".into(),
        lawn::ParkingAction::Park,
        &mut ClientShellInput::default(),
    );
    let policy = request(&mut state);
    let mut projected = state.snapshot.as_deref().unwrap().clone();
    let mut neighbor = projected.workspaces[1].clone();
    neighbor.workspace_id = "ws_3".into();
    neighbor.active_tab_id = "tab_4".into();
    projected.workspaces.push(neighbor);
    let mut tab = projected.tabs[2].clone();
    tab.workspace_id = "ws_3".into();
    tab.tab_id = "tab_4".into();
    projected.tabs.push(tab);
    let mut pane = projected.panes[2].clone();
    pane.workspace_id = "ws_3".into();
    pane.tab_id = "tab_4".into();
    pane.pane_id = "pane_4".into();
    projected.panes.push(pane);
    projected.focused_workspace_id = Some("ws_2".into());
    projected.focused_tab_id = Some("tab_3".into());
    projected.revision += 1;
    state.set_endpoint_snapshot_for_generation(&ClientEndpointId::Local, 7, Box::new(projected));
    acknowledge(&mut state, &policy, Some("pane_3"));
    assert!(tick(&mut state).actions.is_empty());

    let mut closed = state.snapshot.as_deref().unwrap().clone();
    closed
        .workspaces
        .retain(|workspace| workspace.workspace_id != "ws_2");
    closed.tabs.retain(|tab| tab.tab_id != "tab_3");
    closed.panes.retain(|pane| pane.pane_id != "pane_3");
    closed.focused_workspace_id = Some("ws_1".into());
    closed.focused_tab_id = Some("tab_1".into());
    closed.revision += 1;
    state.set_endpoint_snapshot_for_generation(&ClientEndpointId::Local, 7, Box::new(closed));
    acknowledge(&mut state, &policy, None);
    let focused = tick(&mut state);
    assert!(focused.actions.iter().any(|action| matches!(action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(&request.method, crate::api::schema::Method::WorkspaceFocus(target)
                if target.workspace_id == "ws_3")
    )));
    assert!(tick(&mut state).actions.is_empty());
}

#[test]
fn close_recovery_handles_focus_and_shell_exit_in_the_same_snapshot() {
    let mut state = parked_with_active_shell(false);
    let policy = SetPanePresentation {
        boot_id: "boot-1".into(),
        sequence: 2,
        hidden_terminals: vec!["terminal-pane_1".into()],
        read_only_terminals: vec![],
        preview_terminal: None,
        focus_terminal: None,
    };
    let mut projected = state.snapshot.as_deref().unwrap().clone();
    projected.focused_workspace_id = Some("ws_2".into());
    projected.focused_tab_id = Some("tab_3".into());
    projected.revision += 1;
    state.set_endpoint_snapshot_for_generation(&ClientEndpointId::Local, 7, Box::new(projected));
    acknowledge(&mut state, &policy, Some("pane_3"));

    let mut closed = state.snapshot.as_deref().unwrap().clone();
    closed.focused_workspace_id = Some("ws_1".into());
    closed.focused_tab_id = Some("tab_1".into());
    closed.panes.retain(|pane| pane.pane_id != "pane_2");
    closed.tabs.retain(|tab| tab.tab_id != "tab_2");
    closed.revision += 1;
    state.set_endpoint_snapshot_for_generation(&ClientEndpointId::Local, 7, Box::new(closed));
    acknowledge(&mut state, &policy, None);
    let focused = tick(&mut state);
    assert!(focused.actions.iter().any(|action| matches!(action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(&request.method, crate::api::schema::Method::WorkspaceFocus(target)
                if target.workspace_id == "ws_2")
    )));
    assert!(tick(&mut state).actions.is_empty());
}

#[test]
fn close_recovery_falls_back_only_to_online_machines() {
    for remove_workspace in [false, true] {
        for online in [false, true] {
            let mut state = if remove_workspace {
                state()
            } else {
                parked_with_active_shell(false)
            };
            let profile = crate::client::endpoint::SavedSshEndpoint {
                id: crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef")
                    .unwrap(),
                label: "Build".into(),
                target: "dev@build.example".into(),
                session: "agents".into(),
                enabled: true,
            };
            let remote = ClientEndpointId::Ssh(profile.id.clone());
            state.set_endpoint_catalog(&[profile]);
            let mut remote_endpoint = self::state().endpoints[0].clone();
            remote_endpoint.endpoint_id = remote.clone();
            remote_endpoint.label = "Build".into();
            remote_endpoint.status = if online {
                ClientEndpointStatus::Online
            } else {
                ClientEndpointStatus::Reconnecting
            };
            state.endpoints[1] = remote_endpoint;
            let mut closed = state.snapshot.as_deref().unwrap().clone();
            if remove_workspace {
                closed.workspaces.clear();
                closed.tabs.clear();
                closed.panes.clear();
                closed.focused_workspace_id = None;
                closed.focused_tab_id = None;
            } else {
                closed
                    .workspaces
                    .retain(|workspace| workspace.workspace_id == "ws_1");
                closed.tabs.retain(|tab| tab.tab_id == "tab_1");
                closed.panes.retain(|pane| pane.pane_id == "pane_1");
            }
            closed.revision += 1;
            state.set_endpoint_snapshot_for_generation(
                &ClientEndpointId::Local,
                7,
                Box::new(closed),
            );
            let policy = SetPanePresentation {
                boot_id: "boot-1".into(),
                sequence: if remove_workspace { 1 } else { 2 },
                hidden_terminals: if remove_workspace {
                    vec![]
                } else {
                    vec!["terminal-pane_1".into()]
                },
                read_only_terminals: vec![],
                preview_terminal: None,
                focus_terminal: None,
            };
            acknowledge(&mut state, &policy, None);
            let focused = tick(&mut state);
            if online {
                assert!(
                    matches!(focused.actions.as_slice(),
                        [ClientShellAction::ActivateEndpoint { endpoint_id, target: Some(ClientEndpointFocusTarget::Workspace(workspace_id)) }]
                            if endpoint_id == &remote && workspace_id == "ws_1"
                    ),
                    "remove_workspace={remove_workspace}: {:?}",
                    focused.actions
                );
            } else {
                assert!(focused.actions.is_empty());
            }
            assert!(tick(&mut state).actions.is_empty());
        }
    }
}

#[test]
fn close_recovery_does_not_replay_background_departures_on_machine_selection() {
    let mut state = parked_with_active_shell(false);
    let profile = crate::client::endpoint::SavedSshEndpoint {
        id: crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
        label: "Build".into(),
        target: "dev@build.example".into(),
        session: "agents".into(),
        enabled: true,
    };
    let remote = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    let mut remote_endpoint = state.endpoints[0].clone();
    remote_endpoint.endpoint_id = remote.clone();
    state.endpoints[1] = remote_endpoint;
    let mut closed = state.snapshot.as_deref().unwrap().clone();
    closed
        .workspaces
        .retain(|workspace| workspace.workspace_id == "ws_1");
    closed.tabs.retain(|tab| tab.tab_id == "tab_1");
    closed.panes.retain(|pane| pane.pane_id == "pane_1");
    closed.focused_pane_id = None;
    closed.revision += 1;
    let ids = PaneIdentities {
        boot_id: closed.boot_id.clone(),
        revision: closed.revision,
        sequence: 2,
        pane_terminals: [("pane_1".into(), "terminal-pane_1".into())]
            .into_iter()
            .collect(),
    };
    state.set_endpoint_snapshot_for_generation(&remote, 7, Box::new(closed));
    state.set_endpoint_pane_identities(&remote, 7, ids);
    assert!(state.activate_endpoint_projection(&remote));
    assert!(tick(&mut state).actions.is_empty());
    assert_eq!(state.active_endpoint_id, remote);
}

#[test]
fn preview_dismissal_returns_to_the_original_workspace_and_restore_reveals_it() {
    let mut state = parked_with_active_shell(false);
    for _ in 0..2 {
        state.parking_action(
            ClientEndpointId::Local,
            "pane_1".into(),
            lawn::ParkingAction::Preview,
            &mut ClientShellInput::default(),
        );
        let preview = request(&mut state);
        acknowledge(&mut state, &preview, Some("pane_1"));
    }
    let dismissed = state.handle_input_bytes(b"\x1b");
    assert!(!state.has_parking_preview());
    assert!(dismissed.actions.iter().any(|action| matches!(action, ClientShellAction::Endpoint { request, .. } if matches!(&request.method, crate::api::schema::Method::WorkspaceFocus(target) if target.workspace_id == "ws_1"))));
    let dismissed_policy = request(&mut state);
    acknowledge(&mut state, &dismissed_policy, Some("pane_2"));
    state.parking_action(
        ClientEndpointId::Local,
        "pane_1".into(),
        lawn::ParkingAction::Restore,
        &mut ClientShellInput::default(),
    );
    assert!(state.active_parking().unwrap().shows_workspace("ws_1"));
}

#[test]
fn closing_an_empty_parked_view_does_not_close_a_hidden_tab() {
    let mut state = state_with_snapshot(tabbed_snapshot());
    assert!(state.config.confirm_close);
    for pane in ["pane_1", "pane_2", "pane_3"] {
        state.parking_action(
            ClientEndpointId::Local,
            pane.into(),
            lawn::ParkingAction::Park,
            &mut ClientShellInput::default(),
        );
        let policy = request(&mut state);
        acknowledge(&mut state, &policy, None);
    }
    state.compose(100, 24).unwrap();
    assert_eq!(state.focused_tab_count(), 0);
    let closed = binding(&mut state, crate::input::KeybindAction::CloseTab);
    assert!(closed.actions.is_empty());
    assert!(state.overlay.is_none());
    let mut stale_menu = ClientShellInput::default();
    state.request_tab_close("tab_1".into(), &mut stale_menu);
    assert!(stale_menu.actions.is_empty());
}

#[test]
fn numbered_tab_shortcuts_keep_the_numbers_shown_in_the_tab_bar() {
    let mut projected = tabbed_snapshot();
    for tab in &mut projected.tabs {
        tab.label = tab.number.to_string();
    }
    let mut state = state_with_snapshot(projected);
    state.parking_action(
        ClientEndpointId::Local,
        "pane_2".into(),
        lawn::ParkingAction::Park,
        &mut ClientShellInput::default(),
    );
    let policy = request(&mut state);
    acknowledge(&mut state, &policy, Some("pane_1"));
    let frame = state.compose(100, 24).unwrap();
    let row = &frame_rows(&frame)[usize::from(state.hits.tabs[0].0.y)];
    assert!(row.contains(" 1 ") && row.contains(" 3 "));
    for (index, expected) in [(0, Some("tab_1")), (1, None), (2, Some("tab_3"))] {
        let action = crate::input::KeybindAction::SwitchTab(index);
        assert_eq!(
            state.indexed_navigation_target_exists(&crate::input::KeybindMatch::Action(action)),
            expected.is_some()
        );
        let outcome = binding(&mut state, action);
        let focused = outcome.actions.iter().find_map(|action| match action {
            ClientShellAction::Endpoint { request, .. } => match &request.method {
                crate::api::schema::Method::TabFocus(target) => Some(target.tab_id.as_str()),
                _ => None,
            },
            _ => None,
        });
        assert_eq!(focused, expected);
    }
}

#[test]
fn narrow_switcher_numbers_custom_tabs_before_filtering_parked_tabs() {
    let mut projected = tabbed_snapshot();
    projected.tabs[2].number = 9;
    projected.tabs[2].label = "logs".into();
    projected.tabs[2].custom_label = true;
    let mut state = state_with_snapshot(projected);
    state.parking_action(
        ClientEndpointId::Local,
        "pane_2".into(),
        lawn::ParkingAction::Park,
        &mut ClientShellInput::default(),
    );
    let policy = request(&mut state);
    acknowledge(&mut state, &policy, Some("pane_1"));
    state.mode = ClientShellMode::Navigate;
    let frame = state.compose(44, 30).unwrap();
    let text = frame_rows(&frame).join("\n");
    assert!(text.contains("3 · logs"), "{text}");
    assert!(!text.contains("2 · logs") && !text.contains("9 · logs"));
    let outcome = binding(&mut state, crate::input::KeybindAction::SwitchTab(2));
    assert!(outcome.actions.iter().any(|action| matches!(action, ClientShellAction::Endpoint { request, .. } if matches!(&request.method, crate::api::schema::Method::TabFocus(target) if target.tab_id == "tab_3"))));
}

#[test]
fn parking_and_restoring_resize_when_the_tab_bar_appears_or_disappears() {
    let mut projected = tabbed_snapshot();
    projected.tabs.pop();
    projected.panes.pop();
    let mut state = state_with_snapshot(projected);
    state.config.hide_tab_bar_when_single_tab = true;
    state.compose(100, 24).unwrap();
    let initial = state.surface_size(100, 24);
    let parked = binding(&mut state, crate::input::KeybindAction::ParkPane);
    assert!(parked.resize);
    assert_eq!(state.surface_size(100, 24).rows, initial.rows + 1);
    let policy = request(&mut state);
    acknowledge(&mut state, &policy, Some("pane_2"));
    let mut restored = ClientShellInput::default();
    state.parking_action(
        ClientEndpointId::Local,
        "pane_1".into(),
        lawn::ParkingAction::Restore,
        &mut restored,
    );
    assert!(restored.resize);
    assert_eq!(state.surface_size(100, 24), initial);
}

#[test]
fn shared_parking_file_changes_resize_the_active_workspace() {
    let mut projected = tabbed_snapshot();
    projected.tabs.pop();
    projected.panes.pop();
    let root = std::env::temp_dir().join(format!(
        "lawn-parking-resize-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let preferences = root.join("chrome.json");
    let mut state = ClientShellState::new(
        ClientShellConfig::from_config(&Config::default())
            .with_preferences_path(preferences.clone()),
    );
    state.config.hide_tab_bar_when_single_tab = true;
    state.set_endpoint_parking_supported(&ClientEndpointId::Local, true);
    state.set_endpoint_snapshot_for_generation(&ClientEndpointId::Local, 7, Box::new(projected));
    let initial_policy = request(&mut state);
    acknowledge(&mut state, &initial_policy, Some("pane_1"));
    state.compose(100, 24).unwrap();
    let initial = state.surface_size(100, 24);
    std::fs::write(
        preferences.with_extension("parking.json"),
        r#"{"version":1,"terminals":["terminal-pane_1"]}"#,
    )
    .unwrap();
    let changed = state.tick_parking(Instant::now() + std::time::Duration::from_secs(1));
    assert!(changed.resize);
    assert_eq!(state.surface_size(100, 24).rows, initial.rows + 1);
    assert!(changed.actions.iter().any(|action| matches!(action, ClientShellAction::PanePresentation { request, .. } if request.hidden_terminals == ["terminal-pane_1"])));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn tab_drag_edges_follow_scroll_controls_and_preserve_hidden_tab_positions() {
    for (parked_pane, expected_index) in [("pane_3", 0), ("pane_1", 1)] {
        let mut state = state_with_snapshot(tabbed_snapshot());
        state.parking_action(
            ClientEndpointId::Local,
            parked_pane.into(),
            lawn::ParkingAction::Park,
            &mut ClientShellInput::default(),
        );
        let policy = request(&mut state);
        acknowledge(&mut state, &policy, Some("pane_2"));
        state.compose(100, 24).unwrap();
        assert_eq!(state.hits.tabs.len(), 2);
        assert!(state.hits.tab_scroll_left.is_empty());
        assert!(state.hits.tab_scroll_right.is_empty());
        let first = state.hits.tabs[0].0;
        let second = state.hits.tabs[1].0;
        let mut moved = None;
        for (kind, x) in [
            (
                MouseEventKind::Down(MouseButton::Left),
                second.x + second.width / 2,
            ),
            (MouseEventKind::Drag(MouseButton::Left), first.x + 1),
            (MouseEventKind::Up(MouseButton::Left), first.x + 1),
        ] {
            let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
                kind,
                column: x,
                row: first.y,
                modifiers: KeyModifiers::empty(),
            })]);
            for action in outcome.actions {
                if let ClientShellAction::Endpoint { request, .. } = action {
                    if let crate::api::schema::Method::TabMove(params) = request.method {
                        moved = Some(params);
                    }
                }
            }
        }
        assert_eq!(
            moved.expect("drag must request a tab move").insert_index,
            expected_index
        );
    }
}

#[test]
fn dismiss_after_switching_machines_returns_to_current_preview_origin() {
    let mut state = parked_with_active_shell(false);
    let profile = crate::client::endpoint::SavedSshEndpoint {
        id: crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
        label: "Build".into(),
        target: "dev@build.example".into(),
        session: "agents".into(),
        enabled: true,
    };
    let remote = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    // Both servers have an acknowledged parked pane and an active shell.
    let mut remote_endpoint = state.endpoints[0].clone();
    remote_endpoint.endpoint_id = remote.clone();
    remote_endpoint.label = "Build".into();
    state.endpoints[1] = remote_endpoint;

    state.parking_action(
        ClientEndpointId::Local,
        "pane_1".into(),
        lawn::ParkingAction::Preview,
        &mut ClientShellInput::default(),
    );
    let policy = request(&mut state);
    acknowledge(&mut state, &policy, Some("pane_1"));

    // Commit a normal workspace selection on the other machine.
    state.focus_or_activate(
        remote.clone(),
        ClientEndpointFocusTarget::Workspace("ws_1".into()),
        &mut ClientShellInput::default(),
    );
    assert!(state.activate_endpoint_projection(&remote));
    assert!(!state.has_parking_preview());

    assert!(state.parking_action(
        remote.clone(),
        "pane_1".into(),
        lawn::ParkingAction::Preview,
        &mut ClientShellInput::default()
    ));
    let policy = state
        .tick_parking(Instant::now())
        .actions
        .into_iter()
        .find_map(|action| match action {
            ClientShellAction::PanePresentation {
                endpoint_id,
                request,
                ..
            } if endpoint_id == remote => Some(request),
            _ => None,
        })
        .unwrap();
    let mut projected = (**state.snapshot.as_ref().unwrap()).clone();
    projected.revision += 1;
    projected.focused_pane_id = Some("pane_1".into());
    state.set_endpoint_pane_identities(
        &remote,
        7,
        PaneIdentities {
            boot_id: projected.boot_id.clone(),
            revision: projected.revision,
            sequence: policy.sequence,
            pane_terminals: projected
                .panes
                .iter()
                .map(|p| (p.pane_id.clone(), format!("terminal-{}", p.pane_id)))
                .collect(),
        },
    );
    state.set_endpoint_snapshot_for_generation(&remote, 7, Box::new(projected));
    assert!(state.has_parking_preview());
    let mut dismissed = ClientShellInput::default();
    state.dismiss_parking_preview(&mut dismissed);
    assert!(dismissed.actions.iter().any(|action| matches!(action,
        ClientShellAction::Endpoint { endpoint_id, request, .. } if endpoint_id == &remote && matches!(&request.method, crate::api::schema::Method::WorkspaceFocus(target) if target.workspace_id == "ws_1")
    )));
    assert!(
        !dismissed.actions.iter().any(|action| matches!(
            action,
            ClientShellAction::ActivateEndpoint {
                endpoint_id: ClientEndpointId::Local,
                ..
            }
        )),
        "Dismiss jumped back to the previous machine: {:?}",
        dismissed.actions
    );
}

#[test]
fn parked_switcher_qualifies_duplicate_workspace_targets_and_marks_cached_rows() {
    let mut state = parked_with_active_shell(false);
    let profile = crate::client::endpoint::SavedSshEndpoint {
        id: crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
        label: "Build".into(),
        target: "dev@build.example".into(),
        session: "agents".into(),
        enabled: true,
    };
    let remote = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    let mut remote_endpoint = state.endpoints[0].clone();
    remote_endpoint.endpoint_id = remote.clone();
    remote_endpoint.label = "Build".into();
    remote_endpoint.status = ClientEndpointStatus::Reconnecting;
    state.endpoints[1] = remote_endpoint;
    state.open_parked();
    let ClientShellOverlay::Navigator(n) = state.overlay.as_ref().unwrap() else {
        panic!("parked switcher")
    };
    let rows = render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, n);
    assert_eq!(
        rows.iter()
            .filter(|r| matches!(r.target, ClientNavigatorTarget::Pane { .. }))
            .count(),
        2
    );
    assert!(
        matches!(&rows[0].target, ClientNavigatorTarget::Machine { endpoint_id } if endpoint_id == &ClientEndpointId::Local)
    );
    let remote_rows = rows
        .iter()
        .skip_while(|r| {
            r.target
                != ClientNavigatorTarget::Machine {
                    endpoint_id: remote.clone(),
                }
        })
        .collect::<Vec<_>>();
    assert!(
        matches!(&remote_rows[0].target, ClientNavigatorTarget::Machine { endpoint_id } if endpoint_id == &remote)
    );
    assert!(
        matches!(&remote_rows[1].target, ClientNavigatorTarget::Workspace { endpoint_id, workspace_id } if endpoint_id == &remote && workspace_id == "ws_1")
    );
    assert!(
        matches!(&remote_rows[2].target, ClientNavigatorTarget::Pane { endpoint_id, pane_id } if endpoint_id == &remote && pane_id == "pane_1")
    );
    assert!(remote_rows[2].stale);
    state.compose(100, 24).unwrap();
    assert!(!state.hits.navigator_preview.is_empty());
}

#[test]
fn parked_headings_do_not_change_selection_or_toggle_scopes() {
    let mut state = parked_with_active_shell(false);
    state.parking_action(
        ClientEndpointId::Local,
        "pane_3".into(),
        lawn::ParkingAction::Park,
        &mut ClientShellInput::default(),
    );
    let policy = request(&mut state);
    acknowledge(&mut state, &policy, Some("pane_2"));
    state.open_parked();
    state.compose(120, 36).unwrap();
    let headers = state
        .hits
        .navigator_rows
        .iter()
        .filter(|(_, target)| matches!(target, ClientNavigatorTarget::Workspace { .. }))
        .map(|(rect, _)| *rect)
        .collect::<Vec<_>>();
    assert_eq!(headers.len(), 2);
    let before = render::client_navigator_rows(
        &state.endpoints,
        &state.active_endpoint_id,
        match state.overlay.as_ref().unwrap() {
            ClientShellOverlay::Navigator(n) => n,
            _ => unreachable!(),
        },
    );
    let outcome = click(&mut state, headers[1]);
    assert!(outcome.actions.is_empty());
    let ClientShellOverlay::Navigator(n) = state.overlay.as_ref().unwrap() else {
        panic!("parked switcher")
    };
    assert!(n.selected.is_none());
    assert_eq!(
        render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, n).len(),
        before.len()
    );
    state.compose(120, 36).unwrap();
    let close = state.hits.navigator_close;
    click(&mut state, close);
    assert!(state.overlay.is_none());
}

#[test]
fn picker_shortcuts_toggle_active_and_parked() {
    for (active, parked) in [("ctrl+g", "ctrl+e"), ("prefix+g", "prefix+e")] {
        let config: Config = toml::from_str(&format!(
            "[keys]\ngoto = '{active}'\nopen_parked = '{parked}'\n"
        ))
        .unwrap();
        for (shortcut, is_parked) in [(active, false), (parked, true)] {
            let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
            state.set_snapshot(Box::new(snapshot()));
            let bytes: &[u8] = match shortcut {
                "ctrl+g" => b"\x07",
                "ctrl+e" => b"\x05",
                "prefix+g" => b"\x02g",
                _ => b"\x02e",
            };
            let opened = state.handle_input_bytes(bytes);
            assert!(opened.repaint);
            assert!(
                matches!(state.overlay, Some(ClientShellOverlay::Navigator(ref n)) if n.parked == is_parked)
            );
            state.handle_input_bytes(b"query");
            let closed = state.handle_input_bytes(bytes);
            assert!(
                state.overlay.is_none(),
                "{shortcut} should close its picker"
            );
            assert!(closed.repaint);
            assert!(closed.actions.is_empty());
            assert!(closed.requests.is_empty());
            assert_eq!(state.mode, ClientShellMode::Terminal);
        }
    }
}

#[test]
fn picker_shortcuts_switch_between_active_and_parked() {
    for (active, parked, active_bytes, parked_bytes) in [
        ("ctrl+g", "ctrl+e", &b"\x07"[..], &b"\x05"[..]),
        ("prefix+g", "prefix+e", &b"\x02g"[..], &b"\x02e"[..]),
    ] {
        let config: Config = toml::from_str(&format!(
            "[keys]\ngoto = '{active}'\nopen_parked = '{parked}'\n"
        ))
        .unwrap();
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
        state.set_snapshot(Box::new(snapshot()));
        for (bytes, is_parked) in [
            (active_bytes, false),
            (parked_bytes, true),
            (active_bytes, false),
        ] {
            let switched = state.handle_input_bytes(bytes);
            assert!(
                matches!(state.overlay, Some(ClientShellOverlay::Navigator(ref n)) if n.parked == is_parked),
                "{active} / {parked} should switch pickers"
            );
            assert!(switched.repaint);
            assert!(switched.actions.is_empty());
            assert!(switched.requests.is_empty());
            state.handle_input_bytes(b"ge");
            assert!(
                matches!(state.overlay, Some(ClientShellOverlay::Navigator(ref n)) if n.query.as_str() == "ge")
            );
        }
    }
}

#[test]
fn picker_shortcuts_keep_prefix_local_and_ignore_held_keys() {
    let config: Config = toml::from_str(
        "[keys]\ngoto = ['ctrl+g', 'prefix+g']\nopen_parked = ['ctrl+e', 'prefix+e']\n",
    )
    .unwrap();
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.handle_input_bytes(b"\x07\x05");
    state.handle_raw_events(vec![RawInputEvent::Key(
        crate::input::TerminalKey::new(KeyCode::Char('e'), KeyModifiers::CONTROL)
            .with_kind(KeyEventKind::Repeat),
    )]);
    assert!(matches!(state.overlay, Some(ClientShellOverlay::Navigator(ref n)) if n.parked));
    state.handle_input_bytes(b"\x02");
    state.handle_raw_events(vec![RawInputEvent::Key(
        crate::input::TerminalKey::new(KeyCode::Char('b'), KeyModifiers::CONTROL)
            .with_kind(KeyEventKind::Repeat),
    )]);
    state.handle_input_bytes(b"g");
    assert!(matches!(state.overlay, Some(ClientShellOverlay::Navigator(ref n)) if !n.parked));
    let ignored = state.handle_input_bytes(b"\x02q");
    assert!(!ignored.detach);
    assert!(
        matches!(state.overlay, Some(ClientShellOverlay::Navigator(ref n)) if n.query.is_empty())
    );
    state.handle_input_bytes(b"\x02\x1b");
    assert!(state.overlay.is_none());
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn parked_keyboard_browses_panes_without_changing_workspace_focus() {
    let mut state = parked_with_active_shell(false);
    state.parking_action(
        ClientEndpointId::Local,
        "pane_3".into(),
        lawn::ParkingAction::Park,
        &mut ClientShellInput::default(),
    );
    let policy = request(&mut state);
    acknowledge(&mut state, &policy, Some("pane_2"));
    state.open_parked();
    assert_eq!(
        state.snapshot.as_ref().unwrap().focused_pane_id.as_deref(),
        Some("pane_2")
    );
    state.handle_input_bytes(b"jkb");
    assert!(
        matches!(state.overlay, Some(ClientShellOverlay::Navigator(ref n)) if n.query.as_str() == "jkb")
    );
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
    ))]);
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Down,
        KeyModifiers::empty(),
    ))]);
    assert!(
        matches!(state.overlay, Some(ClientShellOverlay::Navigator(ref n))
        if n.selected == Some(ClientNavigatorTarget::Pane {
            endpoint_id: ClientEndpointId::Local,
            pane_id: "pane_3".into(),
        }))
    );
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Up,
        KeyModifiers::empty(),
    ))]);
    assert!(
        matches!(state.overlay, Some(ClientShellOverlay::Navigator(ref n))
        if n.selected == Some(ClientNavigatorTarget::Pane {
            endpoint_id: ClientEndpointId::Local,
            pane_id: "pane_1".into(),
        }))
    );
    assert_eq!(
        state.snapshot.as_ref().unwrap().focused_pane_id.as_deref(),
        Some("pane_2")
    );
    state.handle_input_bytes(b"\x1b");
    assert!(state.overlay.is_none());
}

#[test]
fn picker_row_click_survives_pointer_motion_until_open_is_clicked() {
    for parked in [false, true] {
        let mut state = if parked {
            parked_with_active_shell(false)
        } else {
            state_with_snapshot(tabbed_snapshot())
        };
        state.parking_action(
            ClientEndpointId::Local,
            "pane_3".into(),
            lawn::ParkingAction::Park,
            &mut ClientShellInput::default(),
        );
        let policy = request(&mut state);
        acknowledge(&mut state, &policy, Some("pane_2"));
        if parked {
            state.open_parked();
        } else {
            state.open_navigator_overlay();
        }
        state.compose(120, 36).unwrap();
        let pane = state.hits.navigator_rows.iter().find_map(|(rect, target)| {
        matches!(target, ClientNavigatorTarget::Pane { endpoint_id: ClientEndpointId::Local, pane_id } if pane_id == "pane_1").then_some(*rect)
    }).expect("pane row");
        assert!(click(&mut state, pane).actions.is_empty());
        assert_eq!(
            state.snapshot.as_ref().unwrap().focused_pane_id.as_deref(),
            Some("pane_2")
        );
        assert!(
            matches!(state.overlay, Some(ClientShellOverlay::Navigator(ref n))
        if n.selected == Some(ClientNavigatorTarget::Pane { endpoint_id: ClientEndpointId::Local, pane_id: "pane_1".into() }))
        );
        assert!(!state.has_parking_preview());
        let crossed_rows = state
            .hits
            .navigator_rows
            .iter()
            .filter(|(rect, _)| rect.y > pane.y)
            .map(|(rect, _)| *rect)
            .collect::<Vec<_>>();
        assert!(!crossed_rows.is_empty());
        for rect in crossed_rows {
            let moved = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                column: rect.x,
                row: rect.y,
                modifiers: KeyModifiers::empty(),
            })]);
            assert!(moved.actions.is_empty());
        }
        assert!(
            matches!(state.overlay, Some(ClientShellOverlay::Navigator(ref n))
        if n.selected == Some(ClientNavigatorTarget::Pane { endpoint_id: ClientEndpointId::Local, pane_id: "pane_1".into() }))
        );
        state.compose(120, 36).unwrap();
        let open = state.hits.navigator_preview;
        assert!(!open.is_empty());
        let opened = click(&mut state, open);
        assert!(state.overlay.is_none());
        if !parked {
            assert!(opened.actions.iter().any(|action| matches!(action,
                ClientShellAction::Endpoint { request, .. }
                if matches!(&request.method, crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_1")
            )));
            continue;
        }
        assert!(opened.actions.is_empty());
        let preview = request(&mut state);
        assert_eq!(preview.preview_terminal, None);
        assert_eq!(preview.focus_terminal.as_deref(), Some("terminal-pane_1"));
        assert_eq!(
            preview.hidden_terminals,
            ["terminal-pane_2", "terminal-pane_3"]
        );
        acknowledge(&mut state, &preview, Some("pane_1"));
        assert!(state.has_parking_preview());
        assert!(state.active_parking().unwrap().hides_pane("pane_1"));
        assert!(state.active_parking().unwrap().hides_pane("pane_3"));
        assert!(!state.active_parking().unwrap().hides_pane("pane_2"));
    }
}

#[test]
fn parked_stale_selected_pane_never_opens_another_terminal() {
    for delta in [-1, 1] {
        let mut state = parked_with_active_shell(false);
        state.open_parked();
        let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
            panic!("parked switcher")
        };
        navigator.selected = Some(ClientNavigatorTarget::Pane {
            endpoint_id: ClientEndpointId::Local,
            pane_id: "removed-pane".into(),
        });
        let accepted = state.handle_input_bytes(b"\r");
        assert!(accepted.actions.is_empty());
        assert!(matches!(
            state.overlay,
            Some(ClientShellOverlay::Navigator(_))
        ));
        assert!(!state.has_parking_preview());
        state.move_navigator_selection(delta);
        let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
            panic!("parked switcher")
        };
        assert!(
            matches!(navigator.selected.as_ref(), Some(ClientNavigatorTarget::Pane {
            endpoint_id: ClientEndpointId::Local,
            pane_id,
        }) if pane_id == "pane_1")
        );
        assert!(state.active_parking().unwrap().hides_pane("pane_1"));
    }
}

#[test]
#[ignore = "manual render scaling profile"]
fn parked_switcher_render_scale_profile() {
    for separate_workspaces in [false, true] {
        for count in [1, 15] {
            for query in ["", "terminal"] {
                let mut projected = snapshot();
                let workspace = projected.workspaces[0].clone();
                let tab = projected.tabs[0].clone();
                let pane = projected.panes[0].clone();
                projected.workspaces.clear();
                projected.tabs.clear();
                projected.panes.clear();
                projected.agents.clear();
                for index in 1..=count {
                    let number = if separate_workspaces { index } else { 1 };
                    let workspace_id = format!("ws_{number}");
                    let tab_id = format!("tab_{number}");
                    if index == 1 || separate_workspaces {
                        let mut workspace = workspace.clone();
                        workspace.workspace_id = workspace_id.clone();
                        workspace.active_tab_id = tab_id.clone();
                        workspace.number = number;
                        projected.workspaces.push(workspace);
                        let mut tab = tab.clone();
                        tab.workspace_id = workspace_id.clone();
                        tab.tab_id = tab_id.clone();
                        projected.tabs.push(tab);
                    }
                    let mut pane = pane.clone();
                    pane.pane_id = format!("pane_{index}");
                    pane.label = Some(format!("terminal {index}"));
                    pane.workspace_id = workspace_id;
                    pane.tab_id = tab_id;
                    let mut agent = agent(&pane.pane_id, AgentStatus::Idle);
                    agent.workspace_id = pane.workspace_id.clone();
                    agent.tab_id = pane.tab_id.clone();
                    projected.agents.push(agent);
                    projected.panes.push(pane);
                }
                let panes = projected
                    .panes
                    .iter()
                    .map(|pane| pane.pane_id.clone())
                    .collect::<Vec<_>>();
                let mut state = state_with_snapshot(projected);
                for pane in panes {
                    state.endpoints[0].parking.park(&pane).unwrap();
                    let policy = request(&mut state);
                    acknowledge(&mut state, &policy, None);
                }
                state.open_parked();
                if let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() {
                    navigator.query = query.into();
                }
                for _ in 0..20 {
                    std::hint::black_box(state.compose(106, 30).unwrap());
                }
                let start = Instant::now();
                for _ in 0..1000 {
                    std::hint::black_box(state.compose(106, 30).unwrap());
                }
                eprintln!("parked_switcher separate_workspaces={separate_workspaces} panes={count} query={query:?} viewport=106x30 us_per_frame={:.1}", start.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
}

#[test]
fn parked_and_goto_use_the_same_geometry_without_filter_controls() {
    for (width, height) in [(24, 12), (40, 24), (120, 36)] {
        let mut state = parked_with_active_shell(false);
        state.open_navigator_overlay();
        state.compose(width, height).unwrap();
        let goto = (state.hits.navigator_popup, state.hits.navigator_search);
        state.open_parked();
        let frame = state.compose(width, height).unwrap();
        assert_eq!(
            (state.hits.navigator_popup, state.hits.navigator_search),
            goto
        );
        assert!(frame.cursor.as_ref().is_some_and(|cursor| cursor.visible));
        assert!(state
            .hits
            .navigator_rows
            .iter()
            .any(|(_, target)| matches!(target, ClientNavigatorTarget::Pane { .. })));
        let text = frame_rows(&frame).join("\n");
        assert!(
            !text.contains("blocked   working   idle   done"),
            "{width}x{height}: {text}"
        );
    }
}

#[test]
fn parked_launcher_keeps_picker_usable_in_short_windows() {
    for width in [100, 24, 40, 120] {
        for height in [10, 8, 9] {
            for federated in [false, true] {
                let mut state = parked_with_active_shell(false);
                if federated {
                    let mut remote = state.endpoints[0].clone();
                    remote.endpoint_id = ClientEndpointId::Ssh(
                        crate::client::endpoint::ProfileId::parse(
                            "0123456789abcdef0123456789abcdef",
                        )
                        .unwrap(),
                    );
                    state.endpoints.push(remote);
                }
                state.compose(width, height).expect("sidebar frame");
                let launcher = state.hits.parked_launcher;
                if launcher.is_empty() {
                    state.open_parked();
                } else {
                    click(&mut state, launcher);
                }
                state.compose(width, height).expect("visible parked picker");
                assert!(state.hits.navigator_scroll_metrics.unwrap().viewport_rows > 0);
                assert!(state
                    .hits
                    .navigator_rows
                    .iter()
                    .any(|(_, target)| { matches!(target, ClientNavigatorTarget::Pane { .. }) }));
                let close = state.hits.navigator_close;
                assert!(!close.is_empty());
                click(&mut state, close);
                assert!(state.overlay.is_none());
            }
        }
    }
}

#[test]
fn parked_switcher_remains_dismissible_when_resized_too_short() {
    let mut state = parked_with_active_shell(false);
    state.open_parked();
    state.compose(100, 24).unwrap();
    let frame = state.compose(24, 6).expect("visible resize notice");
    assert!(frame_rows(&frame).join("\n").contains("Enlarge terminal"));
    assert!(state.hits.navigator_rows.is_empty());
    assert!(state.hits.navigator_preview.is_empty());
    let close = state.hits.navigator_close;
    assert!(!close.is_empty());
    click(&mut state, close);
    assert!(state.overlay.is_none());
}

#[test]
fn parked_switcher_reserves_results_in_short_windows() {
    for width in [24, 40, 70, 120] {
        for height in [11, 12, 14] {
            let mut state = parked_with_active_shell(false);
            let mut remote = state.endpoints[0].clone();
            remote.endpoint_id = ClientEndpointId::Ssh(
                crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef")
                    .unwrap(),
            );
            state.endpoints.push(remote);
            state.open_parked();
            state.compose(width, height).unwrap();
            assert!(state.hits.navigator_scroll_metrics.unwrap().viewport_rows >= 2);
            assert!(
                state
                    .hits
                    .navigator_rows
                    .iter()
                    .any(|(_, target)| matches!(target, ClientNavigatorTarget::Pane { .. })),
                "{width}x{height}"
            );
            if height == 11 || height == 14 {
                let metrics = state.hits.navigator_scroll_metrics.unwrap();
                state.scroll_navigator_to(0, metrics.viewport_rows);
                state.compose(width, height).unwrap();
                let metrics = state.hits.navigator_scroll_metrics.unwrap();
                assert_eq!(
                    metrics.offset_from_bottom, metrics.max_offset_from_bottom,
                    "{width}x{height}: scrolling to the top must keep the machine heading visible"
                );
                assert!(matches!(
                    state.hits.navigator_rows.first(),
                    Some((_, ClientNavigatorTarget::Machine { .. }))
                ));
                if metrics.viewport_rows < 3 {
                    assert!(state.handle_input_bytes(b"\r").actions.is_empty());
                    assert!(matches!(
                        state.overlay,
                        Some(ClientShellOverlay::Navigator(_))
                    ));
                    assert!(matches!(
                        state.navigator_preview_display(state.navigator_preview.area),
                        PreviewDisplay::None
                    ));
                    state.handle_input_bytes(b"client-shell");
                    state.compose(width, height).unwrap();
                    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref()
                    else {
                        panic!("parked picker remains open");
                    };
                    let rows = render::client_navigator_rows(
                        &state.endpoints,
                        &state.active_endpoint_id,
                        navigator,
                    );
                    assert!(matches!(
                        aggregate_navigation::selected_navigator_target(&rows, navigator),
                        Some(ClientNavigatorTarget::Pane { .. })
                    ));
                    if state.navigator_preview.area.is_empty() {
                        assert!(matches!(
                            state.navigator_preview_display(state.navigator_preview.area),
                            PreviewDisplay::None
                        ));
                        assert!(state
                            .tick_navigator_preview(Instant::now())
                            .actions
                            .is_empty());
                    } else {
                        assert!(matches!(
                            state.navigator_preview_display(state.navigator_preview.area),
                            PreviewDisplay::Loading
                        ));
                    }
                }
            }
        }
    }
}

#[test]
fn parked_search_matches_context_across_fields_without_changing_preview_target() {
    let mut projected = tabbed_snapshot();
    projected.panes[0].label = Some("Build logs".into());
    projected.panes[0].cwd = Some("/projects/alpha".into());
    projected.panes[0].foreground_cwd = Some("/projects/alpha".into());
    projected.tabs[0].label = "review".into();
    projected.tabs[0].custom_label = true;
    let mut parked_agent = agent("pane_1", AgentStatus::Blocked);
    parked_agent.title = Some("Repair title".into());
    parked_agent.display_agent = Some("Operator".into());
    projected.agents.push(parked_agent);
    let mut state = state_with_snapshot(projected);
    binding(&mut state, crate::input::KeybindAction::ParkPane);
    let policy = request(&mut state);
    acknowledge(&mut state, &policy, Some("pane_2"));
    state.open_parked();
    state.handle_raw_events(vec![RawInputEvent::Paste(
        "build client-shell alpha codex review repair operator".into(),
    )]);
    let ClientShellOverlay::Navigator(n) = state.overlay.as_ref().unwrap() else {
        panic!("parked switcher")
    };
    let rows = render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, n);
    assert_eq!(
        rows.iter()
            .filter(|r| matches!(r.target, ClientNavigatorTarget::Pane { .. }))
            .count(),
        1
    );
    assert!(
        matches!(super::super::aggregate_navigation::selected_navigator_target(&rows, n),
        Some(ClientNavigatorTarget::Pane { endpoint_id: ClientEndpointId::Local, pane_id }) if pane_id == "pane_1")
    );
    state.handle_input_bytes(b"b/");
    let ClientShellOverlay::Navigator(n) = state.overlay.as_ref().unwrap() else {
        panic!("parked switcher")
    };
    assert_eq!(
        n.query.as_str(),
        "build client-shell alpha codex review repair operatorb/"
    );
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
    ))]);
    let ClientShellOverlay::Navigator(n) = state.overlay.as_ref().unwrap() else {
        panic!("parked switcher")
    };
    assert!(n.query.as_str().is_empty());
}

#[test]
fn empty_parked_picker_immediately_shows_select_terminal_without_polling() {
    let mut state = state();
    state.set_endpoint_methods(Some(vec!["pane.snapshot".into()]));
    state.open_parked();
    let frame = state.compose(120, 36).expect("coherent parked picker");
    let text = frame_rows(&frame).join("\n");
    assert!(text.contains("Select a terminal"), "{text}");
    assert!(!text.contains("Loading…"), "{text}");
    let tick = state.tick_navigator_preview(Instant::now());
    assert!(tick.actions.is_empty());
    assert!(
        !tick.repaint,
        "an empty picker must render its final state immediately"
    );
}
