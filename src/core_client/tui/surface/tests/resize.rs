use ratatui::buffer::Cell;
use ratatui::style::Style;

use super::*;
use crate::core_client::tui::session::Operation;
use crate::session_doc::{Arrangement, Direction, Layout};

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
    laid_out(surface(COLS, ROWS))
}

fn laid_out(mut surface: Surface) -> Surface {
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
        panes_only(),
        Settings {
            chrome: PaneChrome {
                borders: PaneBordersConfig::Off,
                gaps: true,
                outer_borders: false,
            },
            palette: Palette::catppuccin(),
            scrollbars: true,
            copying: Copying {
                on_select: true,
                toast: Some(ToastClipboardPosition::BottomCenter),
            },
            keybinds: keybinds(),
            confirm_close: false,
            scrollback: SCROLLBACK,
            chosen: Chosen::from(&Config::default()),
        },
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

fn gapped() -> Surface {
    laid_out(Surface::new(
        host(COLS, ROWS),
        panes_only(),
        Settings {
            chrome: PaneChrome {
                borders: PaneBordersConfig::Auto,
                gaps: true,
                outer_borders: true,
            },
            palette: Palette::catppuccin(),
            scrollbars: true,
            copying: Copying {
                on_select: true,
                toast: None,
            },
            keybinds: keybinds(),
            confirm_close: false,
            scrollback: SCROLLBACK,
            chosen: Chosen::from(&Config::default()),
        },
    ))
}

fn split(surface: &Surface, path: &[bool]) -> crate::layout::SplitBorder {
    surface
        .tiles
        .splits
        .iter()
        .find(|split| split.path == path)
        .cloned()
        .expect("split")
}

fn pointed(
    surface: &mut Surface,
    kind: MouseEventKind,
    column: u16,
    row: u16,
    now: std::time::Instant,
) -> Option<Operation> {
    match surface.mouse(mouse_at(kind, column, row), now) {
        Some(super::super::Pointed::Run(operation)) => Some(operation),
        _ => None,
    }
}

fn set(first: &str, second: &str, direction: Direction, ratio: f32) -> Option<Operation> {
    Some(Operation::Arrange(Arrangement::SetRatio {
        first: first.to_owned(),
        second: second.to_owned(),
        direction,
        ratio,
    }))
}

const PRESS: MouseEventKind = MouseEventKind::Down(MouseButton::Left);
const DRAG: MouseEventKind = MouseEventKind::Drag(MouseButton::Left);
const RELEASE: MouseEventKind = MouseEventKind::Up(MouseButton::Left);

#[tokio::test]
async fn dragging_a_split_border_sets_that_splits_ratio_under_the_pointer() {
    let mut surface = grid();
    let now = std::time::Instant::now();
    let (outer, inner) = (split(&surface, &[]), split(&surface, &[false]));
    let across = |x: u16| f32::from(x - outer.area.x) / f32::from(outer.area.width);
    let down = |y: u16| f32::from(y - inner.area.y) / f32::from(inner.area.height);

    assert_eq!(pointed(&mut surface, PRESS, outer.pos, 9, now), None);
    assert_eq!(
        pointed(&mut surface, DRAG, 45, 9, now),
        set("b", "c", Direction::Right, across(45))
    );
    pointed(&mut surface, RELEASE, 45, 9, now);

    assert_eq!(pointed(&mut surface, PRESS, 10, inner.pos, now), None);
    assert_eq!(
        pointed(&mut surface, DRAG, 10, 3, now),
        set("a", "b", Direction::Down, down(3))
    );
}

fn landing(
    surface: &mut Surface,
    layout: &mut Layout,
    operation: Option<Operation>,
) -> Option<Operation> {
    if let Some(Operation::Arrange(arrangement)) = &operation {
        layout.arrange(arrangement);
        surface.update(layout.clone());
        frame_attaching(surface, &HashSet::new());
    }
    operation
}

#[tokio::test]
async fn a_split_drag_sends_at_most_every_33_ms_and_on_release_the_ratio_under_the_pointer() {
    let mut surface = grid();
    let mut layout = grid_layout();
    let start = std::time::Instant::now();
    let at = |ms| start + std::time::Duration::from_millis(ms);
    let outer = split(&surface, &[]);
    let across = |x: u16| f32::from(x - outer.area.x) / f32::from(outer.area.width);
    let mut landed = |surface: &mut Surface, kind, x, y, ms| {
        let operation = pointed(surface, kind, x, y, at(ms));
        landing(surface, &mut layout, operation)
    };

    landed(&mut surface, PRESS, outer.pos, 5, 0);
    assert_eq!(
        landed(&mut surface, DRAG, 40, 5, 0),
        set("b", "c", Direction::Right, across(40))
    );
    assert_eq!(landed(&mut surface, DRAG, 41, 5, 32), None);
    assert_eq!(
        landed(&mut surface, DRAG, 42, 5, 33),
        set("b", "c", Direction::Right, across(42))
    );
    assert_eq!(landed(&mut surface, DRAG, 43, 5, 40), None);
    assert_eq!(
        landed(&mut surface, RELEASE, 43, 5, 41),
        set("b", "c", Direction::Right, across(43))
    );

    let outer = split(&surface, &[]);
    landed(&mut surface, PRESS, outer.pos, 5, 100);
    assert_eq!(
        landed(&mut surface, DRAG, 44, 5, 100),
        set("b", "c", Direction::Right, across(44))
    );
    assert_eq!(landed(&mut surface, DRAG, 44, 6, 200), None);
    assert_eq!(landed(&mut surface, RELEASE, 44, 6, 201), None);
    assert_eq!(landed(&mut surface, DRAG, 46, 6, 300), None);
}

#[tokio::test]
async fn a_drag_motion_the_33_ms_limit_holds_back_is_sent_once_the_limit_allows() {
    let mut surface = grid();
    let start = std::time::Instant::now();
    let at = |ms| start + std::time::Duration::from_millis(ms);
    let outer = split(&surface, &[]);
    pointed(&mut surface, PRESS, outer.pos, 5, at(0));
    assert_eq!(
        pointed(&mut surface, DRAG, 40, 5, at(0)),
        set("b", "c", Direction::Right, across(&outer, 40))
    );

    assert_eq!(pointed(&mut surface, DRAG, 43, 5, at(10)), None);

    assert_eq!(surface.deadline(), Some(at(33)));
    assert_eq!(surface.tick(at(32), false), None);
    assert_eq!(
        surface.tick(at(33), false),
        set("b", "c", Direction::Right, across(&outer, 43))
    );
    assert_eq!(surface.deadline(), None);
    assert_eq!(surface.tick(at(80), false), None);
}

#[tokio::test]
async fn a_release_supersedes_a_drag_motion_the_33_ms_limit_holds_back() {
    let mut surface = grid();
    let start = std::time::Instant::now();
    let at = |ms| start + std::time::Duration::from_millis(ms);
    let outer = split(&surface, &[]);
    pointed(&mut surface, PRESS, outer.pos, 5, at(0));
    pointed(&mut surface, DRAG, 40, 5, at(0));
    pointed(&mut surface, DRAG, 43, 5, at(10));

    assert_eq!(
        pointed(&mut surface, RELEASE, 45, 5, at(12)),
        set("b", "c", Direction::Right, across(&outer, 45))
    );
    assert_eq!(surface.deadline(), None);
    assert_eq!(surface.tick(at(33), false), None);
}

#[tokio::test]
async fn a_split_drag_past_the_ratio_limit_sends_the_clamped_ratio_once() {
    let mut surface = laid_out(surface(100, ROWS));
    let mut layout = grid_layout();
    let start = std::time::Instant::now();
    let at = |ms| start + std::time::Duration::from_millis(ms);
    let outer = split(&surface, &[]);
    assert_eq!(outer.area.width, 100);

    pointed(&mut surface, PRESS, outer.pos, 5, at(0));
    let sent: Vec<Operation> = [(92, 0), (95, 33), (98, 66)]
        .into_iter()
        .filter_map(|(offset, ms)| {
            let operation = pointed(&mut surface, DRAG, outer.area.x + offset, 5, at(ms));
            landing(&mut surface, &mut layout, operation)
        })
        .collect();

    assert_eq!(sent, [set("b", "c", Direction::Right, 0.9).expect("set")]);
}

#[tokio::test]
async fn a_split_grabbed_beside_its_line_keeps_that_offset_while_dragging() {
    let mut surface = gapped();
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);
    let across = |x: u16| f32::from(x - outer.area.x) / f32::from(outer.area.width);

    assert_eq!(pointed(&mut surface, PRESS, outer.pos - 1, 5, now), None);
    assert_eq!(
        pointed(&mut surface, DRAG, 40, 5, now),
        set("b", "c", Direction::Right, across(41))
    );
}

#[tokio::test]
async fn a_split_drag_ends_when_the_host_loses_focus() {
    let mut surface = grid();
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);
    pointed(&mut surface, PRESS, outer.pos, 9, now);

    surface.host_focus(false);

    assert_eq!(pointed(&mut surface, DRAG, 45, 9, now), None);
    assert_eq!(pointed(&mut surface, RELEASE, 45, 9, now), None);
}

fn nested(ratio: f32) -> Layout {
    serde_json::from_value(
        json!({"workspaces": [{"id": "w0", "tabs": [{"id": "t0", "root": {
            "kind": "split", "direction": "right", "ratio": ratio,
            "first": {"kind": "split", "direction": "right", "ratio": 0.5,
                "first": {"kind": "pane", "terminal": "a"},
                "second": {"kind": "pane", "terminal": "b"}},
            "second": {"kind": "pane", "terminal": "c"},
        }}]}]}),
    )
    .expect("layout")
}

fn across(split: &crate::layout::SplitBorder, x: u16) -> f32 {
    f32::from(x - split.area.x) / f32::from(split.area.width)
}

#[tokio::test]
async fn a_child_split_drag_follows_its_bounds_when_another_client_moves_the_parent() {
    let mut surface = surface(100, ROWS);
    surface.update(nested(0.5));
    frame_attaching(&mut surface, &HashSet::new());
    let now = std::time::Instant::now();
    let pressed = split(&surface, &[false]);
    pointed(&mut surface, PRESS, pressed.pos, 5, now);

    surface.update(nested(0.8));
    frame_attaching(&mut surface, &HashSet::new());
    let moved = split(&surface, &[false]);
    assert!(moved.area.width > pressed.area.width);

    assert_eq!(
        pointed(&mut surface, DRAG, 35, 5, now),
        set("a", "b", Direction::Right, across(&moved, 35))
    );
}

#[tokio::test]
async fn a_split_drag_survives_another_client_setting_that_splits_ratio() {
    let mut surface = grid();
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);
    pointed(&mut surface, PRESS, outer.pos, 9, now);

    let mut moved = grid_layout();
    moved.arrange(&Arrangement::SetRatio {
        first: "b".to_owned(),
        second: "c".to_owned(),
        direction: Direction::Right,
        ratio: 0.7,
    });
    surface.update(moved);
    frame_attaching(&mut surface, &HashSet::new());

    assert_eq!(
        pointed(&mut surface, DRAG, 45, 9, now),
        set("b", "c", Direction::Right, across(&outer, 45))
    );
}

#[tokio::test]
async fn a_split_drag_follows_its_bounds_through_a_host_resize_not_yet_drawn() {
    let mut surface = grid();
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);
    pointed(&mut surface, PRESS, outer.pos, 9, now);

    surface.resize(host(COLS * 2, ROWS));
    let released = pointed(&mut surface, RELEASE, 45, 9, now);
    frame_attaching(&mut surface, &HashSet::new());

    assert_eq!(
        released,
        set(
            "b",
            "c",
            Direction::Right,
            across(&split(&surface, &[]), 45)
        )
    );
}

#[tokio::test]
async fn a_split_drag_follows_its_bounds_through_a_sidebar_toggle_not_yet_drawn() {
    let mut surface = laid_out(surface(100, ROWS));
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);
    pointed(&mut surface, PRESS, outer.pos, 5, now);

    surface.act(KeybindAction::ToggleSidebar, false);
    let dragged = pointed(&mut surface, DRAG, 55, 5, now);
    frame_attaching(&mut surface, &HashSet::new());
    let shifted = split(&surface, &[]);
    assert_ne!(shifted.area, outer.area);

    assert_eq!(
        dragged,
        set("b", "c", Direction::Right, across(&shifted, 55))
    );
}

#[tokio::test]
async fn a_press_on_the_resize_bar_over_a_split_border_starts_no_drag_and_one_row_above_does() {
    let mut surface = grid();
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);
    surface.act(KeybindAction::EnterResizeMode, false);
    frame(&mut surface, &HashSet::new());

    assert_eq!(pointed(&mut surface, PRESS, outer.pos, ROWS - 1, now), None);
    assert_eq!(pointed(&mut surface, DRAG, 45, ROWS - 1, now), None);
    assert_eq!(pointed(&mut surface, RELEASE, 45, ROWS - 1, now), None);

    let across = |x: u16| f32::from(x - outer.area.x) / f32::from(outer.area.width);
    pointed(&mut surface, PRESS, outer.pos, ROWS - 2, now);
    assert_eq!(
        pointed(&mut surface, DRAG, 45, ROWS - 2, now),
        set("b", "c", Direction::Right, across(45))
    );
}

#[tokio::test]
async fn a_press_on_the_copy_bar_over_a_split_border_starts_no_drag() {
    let mut surface = grid();
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);
    surface.enter_copy_mode();
    frame(&mut surface, &HashSet::new());
    assert!(surface.copy_mode());

    pointed(&mut surface, PRESS, outer.pos, ROWS - 1, now);

    assert_eq!(pointed(&mut surface, DRAG, 45, ROWS - 1, now), None);
}

fn down(surface: &mut Surface, button: MouseButton, column: u16, row: u16) {
    surface.mouse(
        mouse_at(MouseEventKind::Down(button), column, row),
        std::time::Instant::now(),
    );
    surface.mouse(
        mouse_at(MouseEventKind::Up(button), column, row),
        std::time::Instant::now(),
    );
}

#[tokio::test]
async fn a_press_on_the_resize_or_copy_bar_reaches_no_pane_until_a_frame_takes_the_bar_away() {
    for copying in [false, true] {
        let mut surface = surface_with_scrollbars(20, 10, false);
        surface.update(row_of(&terminals(1)));
        let mut input = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1006h").remove(0);
        match copying {
            true => surface.enter_copy_mode(),
            false => drop(surface.act(KeybindAction::EnterResizeMode, false)),
        }
        frame(&mut surface, &HashSet::new());
        reports(&mut input);

        for button in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
            down(&mut surface, button, 3, 9);
            assert!(reports(&mut input).is_empty(), "{copying} {button:?}");
        }
        down(&mut surface, MouseButton::Middle, 3, 8);
        assert!(!reports(&mut input).is_empty(), "{copying}");

        match copying {
            true => {
                drop(surface.copy_mode_key(&TerminalKey::new(KeyCode::Esc, KeyModifiers::NONE)))
            }
            false => drop(key(&mut surface, KeyCode::Esc)),
        }
        assert!(!surface.copy_mode() && !surface.resizing());
        down(&mut surface, MouseButton::Left, 3, 9);
        assert!(reports(&mut input).is_empty(), "{copying}");

        frame(&mut surface, &HashSet::new());
        down(&mut surface, MouseButton::Left, 3, 9);
        assert!(!reports(&mut input).is_empty(), "{copying}");
    }
}

#[tokio::test]
async fn a_split_drag_ends_when_a_dialog_opens() {
    let mut surface = grid();
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);
    pointed(&mut surface, PRESS, outer.pos, 9, now);

    surface.act(KeybindAction::Help, false);
    surface.dialog_key(&TerminalKey::new(KeyCode::Esc, KeyModifiers::NONE));

    assert_eq!(pointed(&mut surface, DRAG, 45, 9, now), None);
}

#[tokio::test]
async fn a_fresh_press_replaces_a_split_drag_whose_release_was_lost() {
    let mut surface = grid();
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);
    pointed(&mut surface, PRESS, outer.pos, 9, now);

    pointed(&mut surface, PRESS, 10, 9, now);

    assert_eq!(surface.session.focused(), Some("b"));
    assert_eq!(pointed(&mut surface, DRAG, 12, 9, now), None);
}

#[tokio::test]
async fn a_split_drag_ends_when_the_panes_around_it_change() {
    let mut surface = grid();
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);
    pointed(&mut surface, PRESS, outer.pos, 9, now);

    let mut parked = grid_layout();
    parked.parked.insert("b".to_owned());
    surface.update(parked);

    assert_eq!(pointed(&mut surface, DRAG, 45, 9, now), None);
}

#[tokio::test]
async fn a_split_drag_ends_when_the_split_between_its_panes_turns_to_the_other_axis() {
    let mut surface = grid();
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);
    pointed(&mut surface, PRESS, outer.pos, 9, now);

    let turned: Layout = serde_json::from_value(
        json!({"workspaces": [{"id": "w0", "tabs": [{"id": "t0", "root": {
            "kind": "split", "direction": "down", "ratio": 0.5,
            "first": {"kind": "split", "direction": "down", "ratio": 0.5,
                "first": {"kind": "pane", "terminal": "a"},
                "second": {"kind": "pane", "terminal": "b"}},
            "second": {"kind": "pane", "terminal": "c"},
        }}]}]}),
    )
    .expect("layout");
    surface.update(turned);

    assert_eq!(pointed(&mut surface, DRAG, 45, 9, now), None);
    assert!(surface.splitting.is_none());
}

#[tokio::test]
async fn a_split_drag_ends_when_the_focused_tab_changes() {
    let mut surface = grid();
    let mut tabs = grid_layout();
    tabs.add_tab("w0", None, "d".to_owned());
    surface.update(tabs);
    frame_attaching(&mut surface, &HashSet::new());
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);
    pointed(&mut surface, PRESS, outer.pos, 9, now);

    surface.act(KeybindAction::NextTab, false);

    assert_eq!(pointed(&mut surface, DRAG, 45, 9, now), None);
}

#[tokio::test]
async fn a_press_on_a_border_laid_out_before_a_tab_switch_starts_no_drag() {
    let mut surface = grid();
    let mut tabs = grid_layout();
    tabs.add_tab("w0", None, "d".to_owned());
    tabs.workspaces[0].tabs[1].root = serde_json::from_value(json!({
        "kind": "split", "direction": "right", "ratio": 0.5,
        "first": {"kind": "pane", "terminal": "d"},
        "second": {"kind": "pane", "terminal": "e"},
    }))
    .expect("root");
    surface.update(tabs);
    frame_attaching(&mut surface, &HashSet::new());
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);

    surface.act(KeybindAction::NextTab, false);
    pointed(&mut surface, PRESS, outer.pos, 9, now);

    assert_eq!(pointed(&mut surface, DRAG, 45, 9, now), None);
    assert_eq!(pointed(&mut surface, RELEASE, 45, 9, now), None);
}

#[tokio::test]
async fn a_split_drag_ends_when_the_pane_zooms() {
    let mut surface = grid();
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);
    pointed(&mut surface, PRESS, outer.pos, 9, now);

    surface.act(KeybindAction::Zoom, false);

    assert_eq!(pointed(&mut surface, DRAG, 45, 9, now), None);
    assert_eq!(pointed(&mut surface, RELEASE, 45, 9, now), None);
}

#[tokio::test]
async fn a_release_sends_its_ratio_when_another_client_set_that_split_since_the_last_send() {
    let mut surface = grid();
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);
    pointed(&mut surface, PRESS, outer.pos, 9, now);
    assert_eq!(
        pointed(&mut surface, DRAG, 45, 9, now),
        set("b", "c", Direction::Right, across(&outer, 45))
    );

    let mut moved = grid_layout();
    moved.arrange(&Arrangement::SetRatio {
        first: "b".to_owned(),
        second: "c".to_owned(),
        direction: Direction::Right,
        ratio: 0.7,
    });
    surface.update(moved);
    frame_attaching(&mut surface, &HashSet::new());

    assert_eq!(
        pointed(&mut surface, RELEASE, 45, 9, now),
        set("b", "c", Direction::Right, across(&outer, 45))
    );
}

#[tokio::test]
async fn a_split_whose_area_is_empty_along_its_axis_gets_no_ratio_from_a_drag_or_release() {
    let mut surface = grid();
    let now = std::time::Instant::now();
    let inner = split(&surface, &[false]);
    pointed(&mut surface, PRESS, 10, inner.pos, now);

    surface.resize(host(COLS, 0));
    frame_attaching(&mut surface, &HashSet::new());
    assert_eq!(split(&surface, &[false]).area.height, 0);

    assert_eq!(pointed(&mut surface, DRAG, 10, 0, now), None);
    assert_eq!(pointed(&mut surface, RELEASE, 10, 0, now), None);
}

#[tokio::test]
async fn a_split_whose_area_is_empty_across_its_axis_gets_no_ratio_as_when_the_mobile_header_takes_every_row(
) {
    let mut surface = laid_out(Surface::new(
        host(COLS, ROWS),
        Chrome::new(&Config::default()),
        Settings {
            chrome: PaneChrome {
                borders: PaneBordersConfig::Auto,
                gaps: false,
                outer_borders: true,
            },
            palette: Palette::catppuccin(),
            scrollbars: true,
            copying: Copying {
                on_select: true,
                toast: None,
            },
            keybinds: keybinds(),
            confirm_close: false,
            scrollback: SCROLLBACK,
            chosen: Chosen::from(&Config::default()),
        },
    ));
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);
    pointed(&mut surface, PRESS, outer.pos, outer.area.y + 1, now);
    assert!(surface.splitting.is_some());

    surface.resize(host(COLS, 2));
    frame_attaching(&mut surface, &HashSet::new());
    let hidden = split(&surface, &[]);
    assert_eq!((hidden.area.width, hidden.area.height), (COLS, 0));

    assert_eq!(pointed(&mut surface, DRAG, 45, 0, now), None);
    assert_eq!(pointed(&mut surface, RELEASE, 45, 0, now), None);
}

#[tokio::test]
async fn a_release_back_where_the_split_was_sends_its_ratio_while_the_drags_last_write_is_unanswered(
) {
    let mut surface = grid();
    let now = std::time::Instant::now();
    let outer = split(&surface, &[]);
    assert_eq!(across(&outer, outer.pos), outer.ratio);
    pointed(&mut surface, PRESS, outer.pos, 9, now);
    assert_eq!(
        pointed(&mut surface, DRAG, 45, 9, now),
        set("b", "c", Direction::Right, across(&outer, 45))
    );

    assert_eq!(
        pointed(&mut surface, RELEASE, outer.pos, 9, now),
        set("b", "c", Direction::Right, outer.ratio)
    );
}

#[tokio::test]
async fn a_drag_motion_the_33_ms_limit_holds_back_is_dropped_when_it_falls_due_while_input_is_held()
{
    let mut surface = grid();
    let start = std::time::Instant::now();
    let at = |ms| start + std::time::Duration::from_millis(ms);
    let outer = split(&surface, &[]);
    pointed(&mut surface, PRESS, outer.pos, 5, at(0));
    pointed(&mut surface, DRAG, 40, 5, at(0));
    pointed(&mut surface, DRAG, 43, 5, at(10));

    assert_eq!(surface.tick(at(33), true), None);

    assert_eq!(surface.deadline(), None);
    assert_eq!(surface.tick(at(40), false), None);
}

#[tokio::test]
async fn catching_up_at_the_deadline_sends_a_drag_motion_the_33_ms_limit_held_back() {
    let mut surface = grid();
    let start = std::time::Instant::now();
    let at = |ms| start + std::time::Duration::from_millis(ms);
    let outer = split(&surface, &[]);
    pointed(&mut surface, PRESS, outer.pos, 5, at(0));
    pointed(&mut surface, DRAG, 40, 5, at(0));
    pointed(&mut surface, DRAG, 43, 5, at(10));

    let (operation, _) = surface.caught_up(
        &mut crate::core_client::tui::gate::Gate::default(),
        &mut tokio::sync::mpsc::unbounded_channel().1,
        at(33),
    );

    assert_eq!(
        operation,
        set("b", "c", Direction::Right, across(&outer, 43))
    );
}
