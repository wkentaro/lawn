use super::*;
use crate::protocol::endpoint::pane_presentation::{PaneIdentities, SetPanePresentation};
use std::time::Instant;

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
        assert!(state.tick_parking(Instant::now()).actions.is_empty());
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
fn parking_the_last_terminal_keeps_workspace_and_later_accessible() {
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
    assert!(frame_rows(&frame).join("\n").contains("Open Later"));
    let open = state.hits.open_later;
    assert!(!open.is_empty());
    click(&mut state, open);
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
        panic!("Later")
    };
    assert_eq!(navigator.filter, Some(ClientNavigatorFilter::Later));
    navigator.selected = Some(ClientNavigatorTarget::Pane {
        endpoint_id: ClientEndpointId::Local,
        pane_id: "pane_1".into(),
    });
    state.handle_input_bytes(b"\r");
    let preview = request(&mut state);
    assert_eq!(preview.preview_terminal.as_deref(), Some("terminal-pane_1"));
    acknowledge(&mut state, &preview, Some("pane_1"));
    let frame = state.compose(100, 24).unwrap();
    assert!(frame_rows(&frame).join("\n").contains("Restore"));
    let restore = state.hits.restore_pane;
    assert!(!restore.is_empty());
    assert!(frame.cursor.is_none());
    assert!(state.handle_input_bytes(b"unsafe").requests.is_empty());
    assert!(state
        .handle_raw_events(vec![RawInputEvent::Paste("unsafe".into())])
        .requests
        .is_empty());
    let pane = state.hits.panes[0].inner_rect;
    let selection = click(&mut state, pane);
    assert!(selection.actions.is_empty());
    assert!(state.has_parking_preview());
    let close = binding(&mut state, crate::input::KeybindAction::ClosePane);
    assert!(close.actions.is_empty());
    assert!(state.has_parking_preview());
    click(&mut state, restore);
    assert!(state
        .handle_input_bytes(b"still fenced")
        .requests
        .is_empty());
    let restored = request(&mut state);
    assert!(restored.hidden_terminals.is_empty());
    assert_eq!(restored.focus_terminal.as_deref(), Some("terminal-pane_1"));
    acknowledge(&mut state, &restored, Some("pane_1"));
    assert!(!state.handle_input_bytes(b"writable").requests.is_empty());
    assert_eq!(state.focused_tab_count(), 1);
}

#[test]
fn later_tree_folds_and_jump_does_not_include_parked_terminal() {
    let mut state = state();
    binding(&mut state, crate::input::KeybindAction::ParkPane);
    let parked = request(&mut state);
    acknowledge(&mut state, &parked, None);
    state.open_navigator_overlay();
    let Some(ClientShellOverlay::Navigator(navigator)) = &state.overlay else {
        panic!("Jump")
    };
    let active =
        render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, navigator);
    assert!(!active
        .iter()
        .any(|row| matches!(row.target, ClientNavigatorTarget::Pane { .. })));
    state.open_later();
    state.handle_input_bytes(b"/");
    state.handle_input_bytes(b"client-shell");
    let Some(ClientShellOverlay::Navigator(navigator)) = &state.overlay else {
        panic!("Later search")
    };
    assert_eq!(navigator.filter, Some(ClientNavigatorFilter::Later));
    let rows =
        render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, navigator);
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter().all(|row| row.status.is_none()),
        "ordinary terminal has no agent status"
    );
    state.overlay = None;
    state.later.expanded = true;
    state.compose(100, 24).unwrap();
    assert_eq!(state.hits.later_rows.len(), 3);
    let machine = state.hits.later_rows[0].0;
    click(&mut state, machine);
    state.compose(100, 24).unwrap();
    assert_eq!(state.hits.later_rows.len(), 1);
    let machine = state.hits.later_rows[0].0;
    click(&mut state, machine);
    state.compose(100, 24).unwrap();
    assert_eq!(state.hits.later_rows.len(), 3);
    let workspace = state.hits.later_rows[1].0;
    click(&mut state, workspace);
    state.compose(100, 24).unwrap();
    assert_eq!(state.hits.later_rows.len(), 2);
}

#[test]
fn policy_carries_held_key_release_before_hiding_its_terminal() {
    let mut state = state();
    assert!(!state.handle_input_bytes(b"\x1b[97;5u").requests.is_empty());
    binding(&mut state, crate::input::KeybindAction::ParkPane);
    let outcome = state.tick_parking(Instant::now());
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
    assert!(defaults.open_later.label().is_none());
    let config: Config = toml::from_str(
        "[keys]\npark_pane = 'prefix+f'\nrestore_pane = 'prefix+u'\nopen_later = 'ctrl+g'\n",
    )
    .unwrap();
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
fn retained_updates_obey_pending_policy_and_keep_preview_cursor_and_mouse_read_only() {
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
    assert!(composed.cursor.is_none());
    assert!(!state.hits.panes[0].mouse_reporting);
    assert!(state.handle_input_bytes(b"unsafe").requests.is_empty());
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
    let parked = state.tick_parking(Instant::now());
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
    let preview = state.tick_parking(Instant::now());
    assert!(preview.actions.iter().any(|action| matches!(action, ClientShellAction::PanePresentation { request, releases, .. } if request.preview_terminal.as_deref() == Some("terminal-pane_2") && releases.iter().any(|message| matches!(message, ClientMessage::ClientShellPaneInput { pane_id, events } if pane_id == "pane_1" && events.iter().any(|event| matches!(event, ClientPaneInputEvent::Key { kind: crate::protocol::ClientKeyKind::Release, .. })))))));
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
