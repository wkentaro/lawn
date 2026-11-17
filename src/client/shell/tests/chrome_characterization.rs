use super::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn bounds(rect: Rect) -> [u16; 4] {
    [rect.x, rect.y, rect.width, rect.height]
}

fn projected() -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.workspaces[0].label = "service".into();
    projected.workspaces[0].agent_status = AgentStatus::Working;
    projected.tabs.extend((2..=6).map(|number| ClientShellTab {
        tab_id: format!("tab_{number}"),
        workspace_id: "ws_1".into(),
        number,
        label: format!("task {number}"),
        custom_label: true,
        zoomed: false,
        focused: false,
        agent_status: AgentStatus::Unknown,
    }));
    projected.agents.push(ClientShellAgent {
        pane_id: "pane_1".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some("pi".into()),
        display_agent: Some("pi".into()),
        agent: Some("pi".into()),
        title: Some("Review".into()),
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Working,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: true,
    });
    projected
}

fn capture(name: &str) -> Value {
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.palette = Palette::catppuccin();
    config.hide_tab_bar_when_single_tab = name == "single";
    config.tab_bar_position = if name == "top" {
        TabBarPositionConfig::Top
    } else {
        TabBarPositionConfig::Bottom
    };
    if name == "hidden" {
        config.sidebar_collapsed_mode = SidebarCollapsedModeConfig::Hidden;
    }
    let mut state = ClientShellState::new(config);
    state.sidebar_collapsed = matches!(name, "compact" | "hidden");
    let mut projected = projected();
    if name == "single" {
        projected.tabs.truncate(1);
    }
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let (cols, rows) = match name {
        "mobile" => (44, 12),
        "tiny" => (88, 1),
        _ => (88, 12),
    };
    let layout = state.layout(cols, rows);
    let frame = state.compose(cols, rows).expect("chrome frame");
    let hits = &state.hits;
    json!({
        "name": name,
        "layout": {
            "sidebar": bounds(layout.sidebar),
            "tabs": bounds(layout.tab_bar),
            "mobile": bounds(layout.mobile_header),
            "panes": bounds(layout.pane_surface),
        },
        "rows": frame_rows(&frame),
        "frame_sha256": format!("{:x}", Sha256::digest(serde_json::to_vec(&*frame).expect("frame JSON"))),
        "hits": {
            "workspaces": hits.workspaces.iter().map(|hit| (bounds(hit.rect), &hit.workspace_id)).collect::<Vec<_>>(),
            "tabs": hits.tabs.iter().map(|(rect, id)| (bounds(*rect), id)).collect::<Vec<_>>(),
            "agents": hits.agents.iter().map(|(rect, id)| (bounds(*rect), id)).collect::<Vec<_>>(),
            "new_workspace": bounds(hits.new_workspace),
            "active_launcher": bounds(hits.active_launcher),
            "parked_launcher": bounds(hits.parked_launcher),
            "global_launcher": bounds(hits.global_launcher),
            "new_tab": bounds(hits.new_tab),
            "tab_left": bounds(hits.tab_scroll_left),
            "tab_right": bounds(hits.tab_scroll_right),
            "sidebar_toggle": bounds(hits.sidebar_toggle),
            "sidebar_divider": bounds(hits.sidebar_divider),
            "section_divider": bounds(hits.sidebar_section_divider),
            "agent_sort": bounds(hits.agent_sort_toggle),
            "mobile_switch": bounds(hits.mobile_switch),
        },
    })
}

fn capture_painter(name: &str) -> Value {
    use crate::ui::{ResolvedToken, ResolvedTokenKind};

    let mut config = ClientShellConfig::from_config(&Config::default());
    config.palette = if name.contains("reset") {
        Palette::terminal()
    } else {
        Palette::catppuccin()
    };
    let rows = vec![
        vec![
            ResolvedToken {
                kind: ResolvedTokenKind::StateIcon,
                style: Default::default(),
            },
            ResolvedToken {
                kind: ResolvedTokenKind::Workspace("解析 e\u{301}".into()),
                style: crate::config::SidebarTokenStyle {
                    fg: Some(serde_json::from_str("\"#fa7\"").expect("token color")),
                    bold: Some(false),
                    dim: Some(true),
                },
            },
        ],
        vec![ResolvedToken {
            kind: ResolvedTokenKind::Custom("long continuation label".into()),
            style: Default::default(),
        }],
    ];
    let focused = name.contains("focused");
    let mut buffer = Buffer::with_lines(["...................."; 5]);
    let area = Rect::new(1, 1, 18, 3);
    if name.starts_with("agent") {
        super::super::agent_sidebar::render_agent_row(
            &mut buffer,
            area,
            &super::super::agent_sidebar::AgentRow {
                pane_id: "pane_1".into(),
                status: AgentStatus::Blocked,
                focused,
                rows: if name == "agent_empty" {
                    Vec::new()
                } else {
                    rows
                },
            },
            &config,
        );
    } else {
        super::super::render::sidebar::render_workspace_rows(
            &mut buffer,
            area,
            AgentStatus::Blocked,
            config.status_indicators,
            rows,
            focused,
            name == "workspace_selected",
            name.ends_with("navigate"),
            name == "workspace_dragged",
            &config.palette,
        );
    }
    let frame = FrameData::from_ratatui_buffer(&buffer, None);
    json!({
        "name": name,
        "rows": frame_rows(&frame),
        "frame_sha256": format!("{:x}", Sha256::digest(serde_json::to_vec(&frame).expect("painter JSON"))),
    })
}

#[test]
fn current_sidebar_painters_preserve_tokens_and_backgrounds() {
    let actual = Value::Array(
        [
            "workspace_normal",
            "workspace_focused",
            "workspace_selected",
            "workspace_dragged",
            "workspace_reset_focused",
            "workspace_reset_focused_navigate",
            "agent_normal",
            "agent_focused",
            "agent_empty",
        ]
        .into_iter()
        .map(capture_painter)
        .collect(),
    );
    let expected: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/chrome-painters.json"
    ))
    .expect("recorded painters");
    if actual != expected {
        eprintln!(
            "PAINTER_CAPTURE={}",
            serde_json::to_string(&actual).expect("capture JSON")
        );
    }
    assert_eq!(actual, expected);
}

#[test]
fn current_chrome_matches_recorded_cells_and_targets() {
    let actual = Value::Array(
        [
            "desktop", "top", "compact", "hidden", "single", "mobile", "tiny",
        ]
        .into_iter()
        .map(capture)
        .collect(),
    );
    let expected: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/chrome-characterization.json"
    ))
    .expect("recorded chrome");
    if actual != expected {
        eprintln!(
            "CHROME_CAPTURE={}",
            serde_json::to_string(&actual).expect("capture JSON")
        );
    }
    assert_eq!(actual, expected);
}
