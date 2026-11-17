use ratatui::layout::{Direction, Rect};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::config::PaneBordersConfig;
use crate::detect::{Agent, AgentState};
use crate::protocol::FrameData;
use crate::terminal::{TerminalRuntime, TerminalRuntimeRegistry};
use crate::ui::theme::Palette;
use crate::workspace::Workspace;

fn workspace(name: &str) -> Workspace {
    let mut workspace = Workspace::test_new("drawing");
    if name != "single" && name != "always_single" {
        workspace.test_split(Direction::Horizontal);
        workspace.test_split(Direction::Vertical);
    }
    workspace
}

fn capture(name: &str) -> Value {
    let mut app = AppState::test_new();
    app.palette = Palette::catppuccin();
    app.pane_gaps = name == "gaps";
    app.pane_borders = match name {
        "always_single" => PaneBordersConfig::Always,
        "off" => PaneBordersConfig::Off,
        _ => PaneBordersConfig::Auto,
    };
    app.show_agent_labels_on_pane_borders = name == "agent_labels";
    app.workspaces = vec![workspace(name)];
    app.active = Some(0);
    app.ensure_test_terminals();

    let tab = &mut app.workspaces[0].tabs[0];
    let mut panes: Vec<_> = tab.panes.keys().copied().collect();
    panes.sort_by_key(|pane| pane.raw());
    let focused = *panes.last().expect("a pane");
    tab.runtimes.insert(
        focused,
        TerminalRuntime::test_with_screen_bytes(
            40,
            12,
            if name == "hidden_cursor" {
                b"hello\r\nwor\x1b[?25l"
            } else {
                b"hello\r\nwor"
            },
        ),
    );
    let terminal_of = |app: &AppState, index: usize| {
        app.workspaces[0].tabs[0].panes[&panes[index]]
            .attached_terminal_id
            .clone()
    };
    let labels = ["build", "模块组织（已定）", "a much longer label than fits"];
    let labeled = if name == "agent_labels" {
        panes.len() - 1
    } else {
        panes.len()
    };
    for (index, label) in labels.into_iter().enumerate().take(labeled) {
        let terminal = terminal_of(&app, index);
        app.terminals
            .get_mut(&terminal)
            .expect("terminal")
            .set_manual_label(label.to_owned());
    }
    if let Some(index) = panes.len().checked_sub(1).filter(|_| panes.len() > 1) {
        let terminal = terminal_of(&app, index);
        app.terminals
            .get_mut(&terminal)
            .expect("terminal")
            .set_detected_state(Some(Agent::Claude), AgentState::Working);
    }

    let area = match name {
        "narrow" => Rect::new(0, 0, 9, 5),
        _ => Rect::new(0, 0, 32, 10),
    };
    let runtimes = TerminalRuntimeRegistry::new();
    let layout = crate::ui::compute_tab_surface_for(
        &app,
        &runtimes,
        Some(crate::ui::TabSurfaceTarget {
            workspace_index: 0,
            tab_index: 0,
        }),
        area,
        true,
        Default::default(),
    );
    let infos: Vec<_> = layout
        .pane_infos
        .iter()
        .map(|info| {
            json!({
                "rect": [info.rect.x, info.rect.y, info.rect.width, info.rect.height],
                "inner": [info.inner_rect.x, info.inner_rect.y, info.inner_rect.width, info.inner_rect.height],
                "borders": info.borders.bits(),
                "focused": info.is_focused,
            })
        })
        .collect();
    let (buffer, cursor, _, _) =
        crate::server::render_stream::render_tab_surface_virtual(&app, &runtimes, layout, area);
    let frame = FrameData::from_ratatui_buffer(&buffer, cursor.clone());
    json!({
        "name": name,
        "panes": infos,
        "rows": frame
            .cells
            .chunks(usize::from(frame.width))
            .map(|row| row.iter().map(|cell| cell.symbol.as_str()).collect::<String>())
            .collect::<Vec<_>>(),
        "cursor": cursor.map(|c| [c.x, c.y, u16::from(c.visible)]),
        "frame_sha256": format!("{:x}", Sha256::digest(serde_json::to_vec(&frame).expect("frame JSON"))),
    })
}

#[tokio::test]
async fn current_pane_drawing_matches_recorded_cells() {
    let actual = Value::Array(
        [
            "single",
            "always_single",
            "grid",
            "gaps",
            "off",
            "agent_labels",
            "narrow",
            "hidden_cursor",
        ]
        .into_iter()
        .map(capture)
        .collect(),
    );
    let expected: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/pane-drawing.json"))
            .expect("recorded pane drawing");
    if actual != expected {
        eprintln!(
            "PANE_DRAWING_CAPTURE={}",
            serde_json::to_string(&actual).expect("capture JSON")
        );
    }
    assert_eq!(actual, expected);
}
