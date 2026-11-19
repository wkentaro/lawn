use ratatui::buffer::Cell;
use ratatui::style::Style;

use super::*;
use crate::core_client::tui::session::Operation;
use crate::session_doc::Layout;

const COLS: u16 = 60;
const ROWS: u16 = 12;

fn grid_layout() -> Layout {
    serde_json::from_value(
        json!({"workspaces": [{"id": "w0", "tabs": [{"id": "t0", "root": {
            "kind": "split", "direction": "right", "ratio": 0.5,
            "first": {"kind": "split", "direction": "down", "ratio": 0.5,
                "first": {"kind": "pane", "terminal": "a"},
                "second": {"kind": "pane", "terminal": "b"}},
            "second": {"kind": "pane", "terminal": "c"},
        }}]}]}),
    )
    .expect("layout")
}

fn grid() -> Surface {
    let mut surface = surface(COLS, ROWS);
    surface.update(grid_layout());
    frame_attaching(&mut surface, &HashSet::new());
    surface
}

fn key(surface: &mut Surface, code: KeyCode) -> Option<Operation> {
    surface.resize_key(&TerminalKey::new(code, KeyModifiers::NONE))
}

fn landed(layout: &mut Layout, operation: Option<Operation>) {
    if let Some(Operation::Arrange(arrangement)) = operation {
        layout.arrange(&arrangement);
    }
}

fn ratios(node: &crate::session_doc::Node) -> Vec<f32> {
    match node {
        crate::session_doc::Node::Pane { .. } => Vec::new(),
        crate::session_doc::Node::Split {
            ratio,
            first,
            second,
            ..
        } => [*ratio]
            .into_iter()
            .chain(ratios(first))
            .chain(ratios(second))
            .collect(),
    }
}

#[tokio::test]
async fn resize_mode_resizes_with_h_j_k_l_or_the_arrows_and_swallows_every_other_key() {
    let mut surface = grid();
    assert_eq!(surface.act(KeybindAction::EnterResizeMode, false), None);
    assert!(surface.resizing());
    for (codes, resized) in [
        ([KeyCode::Char('l'), KeyCode::Right], [0.55, 0.5]),
        ([KeyCode::Char('h'), KeyCode::Left], [0.45, 0.5]),
        ([KeyCode::Char('j'), KeyCode::Down], [0.5, 0.55]),
        ([KeyCode::Char('k'), KeyCode::Up], [0.5, 0.45]),
    ] {
        for code in codes {
            let mut layout = grid_layout();
            landed(&mut layout, key(&mut surface, code));
            assert_eq!(
                ratios(&layout.workspaces[0].tabs[0].root),
                resized,
                "{code:?}"
            );
        }
    }
    for code in [
        KeyCode::Char('x'),
        KeyCode::Char('q'),
        KeyCode::Tab,
        KeyCode::Backspace,
    ] {
        assert_eq!(key(&mut surface, code), None, "{code:?}");
        assert!(surface.resizing(), "{code:?}");
    }
}

#[tokio::test]
async fn resize_mode_ends_on_esc_enter_or_its_own_binding() {
    let mut surface = grid();
    for code in [KeyCode::Esc, KeyCode::Enter, KeyCode::Char('r')] {
        surface.act(KeybindAction::EnterResizeMode, false);
        assert_eq!(key(&mut surface, code), None, "{code:?}");
        assert!(!surface.resizing(), "{code:?}");
    }
}

fn styled(style: Style) -> CellData {
    let mut cell = Cell::default();
    cell.set_style(style);
    CellData::from_ratatui_cell(&cell)
}

#[tokio::test]
async fn resize_mode_shows_its_bar_on_the_bottom_row_over_the_copy_bar_until_it_ends() {
    let mut surface = grid();
    surface.enter_copy_mode();
    let mut screen = Screen::of(&mut surface);
    let bottom = ROWS - 1;
    assert!(screen.line(bottom).starts_with(" COPY "));

    surface.act(KeybindAction::EnterResizeMode, false);
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(
        screen.line(bottom).trim_end(),
        " RESIZE   h/l width  j/k height  esc done"
    );
    let palette = Palette::catppuccin();
    assert_eq!(
        screen.cell(1, bottom).bg,
        styled(Style::default().bg(palette.mauve)).bg
    );
    assert_eq!(
        screen.cell(10, bottom).fg,
        styled(Style::default().fg(palette.accent)).fg
    );

    key(&mut surface, KeyCode::Esc);
    screen.show(&mut surface, &HashSet::new());
    assert!(screen.line(bottom).starts_with(" COPY "));
}

#[tokio::test]
async fn resize_mode_paints_its_bar_over_the_copied_toast() {
    let mut surface = grid();
    surface.copied(std::time::Instant::now());
    let mut screen = Screen::of(&mut surface);
    let bottom = ROWS - 1;
    assert!(screen.line(bottom - 1).contains("copied"));
    assert!(screen.line(bottom).contains("└──"));

    surface.act(KeybindAction::EnterResizeMode, false);
    screen.show(&mut surface, &HashSet::new());

    assert_eq!(
        screen.line(bottom).trim_end(),
        " RESIZE   h/l width  j/k height  esc done"
    );
}

#[tokio::test]
async fn the_copy_bar_is_painted_over_the_copied_toast() {
    let mut surface = grid();
    surface.enter_copy_mode();
    surface.copied(std::time::Instant::now());
    let screen = Screen::of(&mut surface);
    let bottom = ROWS - 1;

    assert!(screen.line(bottom - 1).contains("copied"));
    assert!(
        !screen.line(bottom).contains('┘'),
        "{}",
        screen.line(bottom)
    );
}

#[tokio::test]
async fn resize_mode_keeps_copy_mode_but_hides_its_cursor_until_it_ends() {
    let mut surface = grid();
    surface.enter_copy_mode();
    let cursor = surface.overlays().copy.cursor;
    assert!(!cursor.is_empty());

    surface.act(KeybindAction::EnterResizeMode, false);
    assert!(surface.copy_mode());
    assert!(surface.overlays().copy.cursor.is_empty());

    key(&mut surface, KeyCode::Esc);
    assert_eq!(surface.overlays().copy.cursor, cursor);
}

#[tokio::test]
async fn swapping_picks_the_neighbour_by_the_panes_before_gaps_shrink_them() {
    let mut surface = Surface::new(
        host(COLS, ROWS),
        PaneChrome {
            borders: PaneBordersConfig::Off,
            gaps: true,
            outer_borders: false,
        },
        panes_only(),
        Palette::catppuccin(),
        true,
        Copying {
            on_select: true,
            toast: Some(ToastClipboardPosition::BottomCenter),
        },
        keybinds(),
    );
    let mut tall: Layout = serde_json::from_value(
        json!({"workspaces": [{"id": "w0", "tabs": [{"id": "t0", "root": {
            "kind": "split", "direction": "right", "ratio": 0.5,
            "first": {"kind": "pane", "terminal": "a"},
            "second": {"kind": "split", "direction": "down", "ratio": 0.5,
                "first": {"kind": "pane", "terminal": "b"},
                "second": {"kind": "pane", "terminal": "c"}},
        }}]}]}),
    )
    .expect("layout");
    surface.update(tall.clone());
    frame_attaching(&mut surface, &HashSet::new());

    landed(&mut tall, surface.act(KeybindAction::SwapPaneRight, false));

    assert_eq!(tall.workspaces[0].tabs[0].root.terminals(), ["b", "a", "c"]);
}
