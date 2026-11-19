use std::collections::HashSet;

use crossterm::event::{
    KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use serde_json::{json, Value};

use super::Surface;
use crate::config::Config;
use crate::config::PaneBordersConfig;
use crate::config::ToastClipboardPosition;
use crate::core_client::tui::chrome::Chrome;
use crate::core_client::tui::select::Copying;
use crate::core_client::tui::session::PaneChrome;
use crate::core_client::view::{Geometry, View};
use crate::input::{KeybindAction, TerminalKey};
use crate::layout::PaneId;
use crate::protocol::{CellData, PaneSurfacePatchRow};
use crate::session_doc::Layout;
use crate::ui::theme::Palette;

fn host(cols: u16, rows: u16) -> Geometry {
    Geometry {
        cols,
        rows,
        cell_width_px: 0,
        cell_height_px: 0,
    }
}

fn panes_only() -> Chrome {
    let mut config = Config::default();
    config.ui.mobile_width_threshold = 0;
    config.ui.sidebar_start_collapsed = true;
    config.ui.sidebar_collapsed_mode = crate::config::SidebarCollapsedModeConfig::Hidden;
    config.ui.hide_tab_bar_when_single_tab = true;
    Chrome::new(&config)
}

fn surface(cols: u16, rows: u16) -> Surface {
    surface_with_scrollbars(cols, rows, true)
}

fn surface_with_scrollbars(cols: u16, rows: u16, scrollbars: bool) -> Surface {
    Surface::new(
        host(cols, rows),
        PaneChrome {
            borders: PaneBordersConfig::Auto,
            gaps: false,
            outer_borders: true,
        },
        panes_only(),
        Palette::catppuccin(),
        scrollbars,
        Copying {
            on_select: true,
            toast: Some(ToastClipboardPosition::BottomCenter),
        },
        keybinds(),
    )
}

fn keybinds() -> crate::config::LiveKeybindConfig {
    let config = Config::default();
    crate::config::LiveKeybindConfig {
        prefix: config.prefix_keys(),
        keybinds: config.keybinds(),
    }
}

fn row_of(terminals: &[String]) -> Layout {
    serde_json::from_value(
        json!({"workspaces": [{"id": "w0", "tabs": [{"id": "t0", "root": balanced(terminals)}]}]}),
    )
    .expect("layout")
}

fn balanced(terminals: &[String]) -> Value {
    if let [terminal] = terminals {
        return json!({"kind": "pane", "terminal": terminal});
    }
    let (first, second) = terminals.split_at(terminals.len() / 2);
    json!({
        "kind": "split",
        "direction": "right",
        "ratio": first.len() as f32 / terminals.len() as f32,
        "first": balanced(first),
        "second": balanced(second),
    })
}

fn terminals(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("t{index}")).collect()
}

fn frame(surface: &mut Surface, dirty: &HashSet<PaneId>) -> Vec<PaneSurfacePatchRow> {
    frame_attaching(surface, dirty).0
}

fn requests(surface: &mut Surface) -> Vec<(String, PaneId, Geometry)> {
    let mut requests = Vec::new();
    surface.frame(
        &HashSet::new(),
        |terminal, id, geometry| {
            requests.push((terminal.to_owned(), id, geometry));
        },
        ignore_leaving,
    );
    requests
}

fn frame_attaching(
    surface: &mut Surface,
    dirty: &HashSet<PaneId>,
) -> (Vec<PaneSurfacePatchRow>, Vec<String>) {
    let mut requests = Vec::new();
    let mut rows = surface
        .frame(
            dirty,
            |terminal, id, geometry| {
                requests.push((terminal.to_owned(), id, geometry));
            },
            ignore_leaving,
        )
        .0;
    if requests.is_empty() {
        return (rows, Vec::new());
    }
    for (_, id, geometry) in &requests {
        surface.attached(*id, *geometry, Ok(View::test(geometry.cols, geometry.rows)));
    }
    rows.extend(
        surface
            .frame(dirty, |_, _, _| unreachable!("attached"), ignore_leaving)
            .0,
    );
    (
        rows,
        requests
            .into_iter()
            .map(|(terminal, ..)| terminal)
            .collect(),
    )
}

struct Screen {
    cols: u16,
    cells: Vec<CellData>,
}

impl Screen {
    fn of(surface: &mut Surface) -> Self {
        surface.resize(surface.host);
        let blank = CellData::from_ratatui_cell(&ratatui::buffer::Cell::default());
        let size = usize::from(surface.host.cols) * usize::from(surface.host.rows);
        let mut screen = Self {
            cols: surface.host.cols,
            cells: vec![blank; size],
        };
        screen.show(surface, &HashSet::new());
        screen
    }

    fn show(&mut self, surface: &mut Surface, dirty: &HashSet<PaneId>) {
        let rows = frame(surface, dirty);
        crate::protocol::surface_delta::apply_rows(&mut self.cells, self.cols, &rows);
    }

    fn cell(&self, x: u16, y: u16) -> &CellData {
        &self.cells[usize::from(y) * usize::from(self.cols) + usize::from(x)]
    }

    fn span(&self, y: u16, xs: std::ops::Range<u16>) -> String {
        xs.map(|x| self.cell(x, y).symbol.as_str()).collect()
    }

    fn line(&self, y: u16) -> String {
        self.span(y, 0..self.cols)
    }

    fn lines(&self, ys: std::ops::Range<u16>) -> Vec<String> {
        ys.map(|y| self.line(y).trim_end().to_owned()).collect()
    }
}

fn never_attach(_: &str, _: PaneId, _: Geometry) {
    unreachable!("no pane to attach");
}

fn ignore_leaving(_: PaneId, _: &View) {}

fn text(rows: &[PaneSurfacePatchRow]) -> String {
    rows.iter()
        .flat_map(|row| row.cells.iter().map(|cell| cell.symbol.as_str()))
        .collect()
}

fn id_of(surface: &Surface, terminal: &str) -> PaneId {
    surface
        .tiles
        .tiles
        .iter()
        .find(|tile| tile.terminal == terminal)
        .expect("tile")
        .info
        .id
}

fn feed(surface: &Surface, terminal: &str, bytes: &[u8]) {
    surface.views[&id_of(surface, terminal)]
        .view
        .test_feed(bytes);
}

fn mouse(surface: &mut Surface, event: MouseEvent) -> Option<String> {
    copied_text(surface.mouse(event, std::time::Instant::now()))
}

fn copied_text(pointed: Option<super::Pointed>) -> Option<String> {
    match pointed? {
        super::Pointed::Copied(text) => Some(text),
        pointed => unreachable!("{pointed:?}"),
    }
}

fn click(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[tokio::test]
async fn a_layout_draws_the_whole_screen_with_borders_and_each_pane_inside_them() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    frame(&mut surface, &HashSet::new());
    feed(&surface, "t1", b"right");
    surface.resize(host(40, 10));

    let rows = frame(&mut surface, &HashSet::new());

    assert_eq!(
        rows.iter().map(|row| row.y).collect::<Vec<_>>(),
        (0..10).collect::<Vec<_>>()
    );
    assert!(rows.iter().all(|row| row.x == 0 && row.cells.len() == 40));
    let inner = surface.tiles.tiles[1].info.inner_rect;
    let line: String = rows[usize::from(inner.y)].cells[usize::from(inner.x)..]
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect();
    assert!(line.starts_with("right"), "{line:?}");
    assert_eq!(rows[0].cells[0].symbol, "┌");
}

#[tokio::test]
async fn output_redraws_only_the_panes_that_drew_among_fifteen() {
    let mut surface = surface(150, 10);
    surface.update(row_of(&terminals(15)));
    frame(&mut surface, &HashSet::new());
    feed(&surface, "t3", b"three");
    feed(&surface, "t9", b"nine");

    let dirty = HashSet::from([id_of(&surface, "t3")]);
    let rows = frame(&mut surface, &dirty);

    let inner = surface.tiles.tiles[3].info.inner_rect;
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|row| row.x == inner.x
        && usize::from(row.x) + row.cells.len() <= usize::from(inner.right())
        && row.y >= inner.y
        && row.y < inner.bottom()));
    assert!(text(&rows).contains("thr"));
    assert!(!text(&rows).contains("nin"));
}

#[tokio::test]
async fn clicking_a_pane_focuses_it_and_puts_the_cursor_there() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    frame(&mut surface, &HashSet::new());
    let right = surface.tiles.tiles[1].info.inner_rect;

    mouse(&mut surface, click(right.x + 1, right.y + 1));

    assert!(surface.needs_frame());
    let (_, cursor) = surface.frame(&HashSet::new(), never_attach, ignore_leaving);
    let cursor = cursor.expect("cursor");
    assert_eq!((cursor.x, cursor.y), (right.x, right.y));
    assert!(surface.tiles.tiles[1].info.is_focused);
}

#[tokio::test]
async fn clicking_a_border_changes_nothing() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    frame(&mut surface, &HashSet::new());
    let right = surface.tiles.tiles[1].info.rect;

    mouse(&mut surface, click(right.x, right.y));

    assert!(!surface.needs_frame());
}

#[tokio::test]
async fn a_fully_parked_tab_draws_a_blank_screen() {
    let mut surface = surface(20, 4);
    let mut layout = row_of(&terminals(1));
    layout.parked.insert("t0".to_owned());
    surface.update(layout);

    let rows = frame(&mut surface, &HashSet::new());

    assert_eq!(rows.len(), 4);
    assert!(text(&rows).chars().all(|symbol| symbol == ' '));
    assert!(surface.focused().is_none());
}

#[tokio::test]
async fn a_pane_without_room_gets_no_view() {
    let mut surface = surface(2, 2);
    surface.update(row_of(&terminals(2)));

    frame(&mut surface, &HashSet::new());

    assert!(surface
        .tiles
        .tiles
        .iter()
        .all(|tile| tile.info.inner_rect.is_empty()));
    assert_eq!(surface.tiles.tiles.len(), 2);
    assert!(surface.views.is_empty());
}

#[tokio::test]
async fn a_pane_keeps_its_view_until_it_leaves_the_tab() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    frame(&mut surface, &HashSet::new());

    surface.update(row_of(&terminals(3)));
    let (_, attached) = frame_attaching(&mut surface, &HashSet::new());
    assert_eq!(attached, ["t2"]);

    surface.update(row_of(&terminals(1)));
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.views.len(), 1);
}

#[tokio::test]
async fn a_relayout_keeps_what_a_pane_showed_while_it_holds_output() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    feed_after_first_frame(&mut surface, "t0", b"shown");
    feed(&surface, "t0", b"\x1b[?2026h\r\nheld");
    let right = surface.tiles.tiles[1].info.inner_rect;

    mouse(&mut surface, click(right.x, right.y));
    assert!(surface.needs_frame());
    let rows = frame(&mut surface, &HashSet::new());

    assert!(text(&rows).contains("shown"));
    assert!(!text(&rows).contains("held"));
}

#[tokio::test]
async fn a_detached_pane_attaches_again() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    frame(&mut surface, &HashSet::new());

    surface.forget(id_of(&surface, "t1"));
    let (_, attached) = frame_attaching(&mut surface, &HashSet::new());

    assert_eq!(attached, ["t1"]);
}

#[tokio::test]
async fn panes_keep_drawing_and_taking_input_while_another_pane_attaches() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    frame(&mut surface, &HashSet::new());
    surface.forget(id_of(&surface, "t1"));
    let pending: Vec<_> = requests(&mut surface)
        .into_iter()
        .map(|(terminal, ..)| terminal)
        .collect();
    assert_eq!(pending, ["t1"]);

    feed(&surface, "t0", b"left");
    let rows = surface
        .frame(
            &HashSet::from([id_of(&surface, "t0")]),
            never_attach,
            ignore_leaving,
        )
        .0;
    assert!(text(&rows).contains("left"));
    surface.resize(host(50, 12));
    surface.frame(&HashSet::new(), never_attach, ignore_leaving);
    let left = surface.tiles.tiles[0].info.inner_rect;
    mouse(&mut surface, click(left.x, left.y));

    assert!(surface.focused().is_some());
    assert_eq!(surface.views.len(), 1);
}

#[tokio::test]
async fn a_pane_detached_before_its_attach_finished_attaches_again() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(1)));
    let [(_, id, geometry)] = requests(&mut surface).try_into().expect("one request");

    surface.forget(id);
    surface.attached(id, geometry, Ok(View::test(geometry.cols, geometry.rows)));
    let (_, attached) = frame_attaching(&mut surface, &HashSet::new());

    assert_eq!(attached, ["t0"]);
}

#[tokio::test]
async fn an_attach_that_finishes_after_its_pane_left_is_dropped() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let pending = requests(&mut surface);
    surface.update(row_of(&terminals(1)));
    surface.frame(&HashSet::new(), never_attach, ignore_leaving);

    for (_, id, geometry) in pending {
        surface.attached(id, geometry, Ok(View::test(geometry.cols, geometry.rows)));
    }
    frame(&mut surface, &HashSet::new());

    assert_eq!(surface.views.len(), 1);
    assert!(surface.views.contains_key(&id_of(&surface, "t0")));
}

fn feed_after_first_frame(surface: &mut Surface, terminal: &str, bytes: &[u8]) {
    frame(surface, &HashSet::new());
    feed(surface, terminal, bytes);
    let dirty = HashSet::from([id_of(surface, terminal)]);
    frame(surface, &dirty);
}

#[tokio::test]
async fn a_moved_pane_keeps_what_it_showed_while_it_holds_output() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    feed_after_first_frame(&mut surface, "t0", b"shown");
    feed(&surface, "t0", b"\x1b[?2026h\r\nheld");

    surface.update(row_of(&["t1".to_owned(), "t0".to_owned()]));
    let rows = frame(&mut surface, &HashSet::new());

    let right = surface.tiles.tiles[1].info.inner_rect;
    let line: String = rows[usize::from(right.y)].cells[usize::from(right.x)..]
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect();
    assert!(line.starts_with("shown"), "{line:?}");
}

#[tokio::test]
async fn a_pane_that_fails_to_attach_is_tried_again() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));

    for (terminal, id, geometry) in requests(&mut surface) {
        let view = if terminal == "t1" {
            Err(std::io::Error::other("unavailable"))
        } else {
            Ok(View::test(geometry.cols, geometry.rows))
        };
        surface.attached(id, geometry, view);
    }
    assert!(surface.needs_frame());
    let (_, attached) = frame_attaching(&mut surface, &HashSet::new());

    assert_eq!(attached, ["t1"]);
    assert_eq!(surface.views.len(), 2);
    assert!(!surface.needs_frame());
}

#[tokio::test]
async fn input_follows_a_click_before_the_next_frame() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    frame(&mut surface, &HashSet::new());
    let right = surface.tiles.tiles[1].info.inner_rect;

    mouse(&mut surface, click(right.x, right.y));

    let focused = surface.focused().expect("focused") as *const View;
    assert_eq!(
        focused,
        &surface.views[&id_of(&surface, "t1")].view as *const View
    );
}

#[tokio::test]
async fn a_new_cell_size_reaches_every_view() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    frame(&mut surface, &HashSet::new());

    surface.resize(Geometry {
        cell_width_px: 12,
        cell_height_px: 24,
        ..host(40, 10)
    });
    frame(&mut surface, &HashSet::new());

    assert!(surface
        .views
        .values()
        .all(|shown| shown.geometry.cell_width_px == 12 && shown.geometry.cell_height_px == 24));
}

#[tokio::test]
async fn a_held_pane_keeps_what_it_showed_when_the_host_resizes() {
    let pane = |terminal: &str| json!({"kind": "pane", "terminal": terminal});
    let split = |direction: &str, ratio: f32, first: Value, second: Value| json!({"kind": "split", "direction": direction, "ratio": ratio, "first": first, "second": second});
    let root = split(
        "down",
        0.9,
        pane("a"),
        split(
            "right",
            0.9,
            pane("b"),
            split("right", 0.55, pane("c"), pane("d")),
        ),
    );
    let mut surface = Surface::new(
        host(80, 24),
        PaneChrome {
            borders: PaneBordersConfig::Off,
            gaps: false,
            outer_borders: true,
        },
        panes_only(),
        Palette::catppuccin(),
        true,
        Copying {
            on_select: true,
            toast: None,
        },
        keybinds(),
    );
    surface.update(
        serde_json::from_value(
            json!({"workspaces": [{"id": "w0", "tabs": [{"id": "t0", "root": root}]}]}),
        )
        .expect("layout"),
    );
    feed_after_first_frame(&mut surface, "d", b"d");
    let before = surface.tiles.tiles[3].info.inner_rect;
    feed(&surface, "d", b"\x1b[?2026h\r\nheld");

    for (cols, rows) in [(96, 20), (90, 24)] {
        surface.resize(host(cols, rows));
        let drawn = frame(&mut surface, &HashSet::new());

        let after = surface.tiles.tiles[3].info.inner_rect;
        assert_eq!((before.width, before.height), (after.width, after.height));
        assert_eq!(drawn.len(), usize::from(rows));
        assert_eq!(
            drawn[usize::from(after.y)].cells[usize::from(after.x)].symbol,
            "d"
        );
    }
}

fn reports(input: &mut tokio::sync::mpsc::Receiver<bytes::Bytes>) -> Vec<u8> {
    let mut sent = Vec::new();
    while let Ok(bytes) = input.try_recv() {
        sent.extend_from_slice(&bytes);
    }
    sent
}

const REPORT_FOCUS: &[u8] = b"\x1b[?1004h";

fn attach_with_input(
    surface: &mut Surface,
    shown: &[u8],
) -> Vec<tokio::sync::mpsc::Receiver<bytes::Bytes>> {
    requests(surface)
        .into_iter()
        .map(|(_, id, geometry)| {
            let (view, input) = View::test_with_input(geometry.cols, geometry.rows);
            view.test_feed(shown);
            surface.attached(id, geometry, Ok(view));
            input
        })
        .collect()
}

#[tokio::test]
async fn moving_focus_tells_both_panes() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, REPORT_FOCUS);
    frame(&mut surface, &HashSet::new());
    let [left, right] = &mut inputs[..] else {
        unreachable!("two panes")
    };
    assert_eq!(reports(left), b"\x1b[I");
    assert_eq!(reports(right), b"\x1b[O");

    let pane = surface.tiles.tiles[1].info.inner_rect;
    mouse(&mut surface, click(pane.x, pane.y));
    assert_eq!(reports(left), b"\x1b[O");
    assert_eq!(reports(right), b"\x1b[I");

    surface.host_focus(false);
    surface.update(row_of(&terminals(2)));
    assert_eq!(reports(left), b"");
    assert_eq!(reports(right), b"\x1b[O");
}

#[tokio::test]
async fn the_wheel_focuses_and_scrolls_the_pane_under_the_pointer() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, REPORT_FOCUS);
    frame(&mut surface, &HashSet::new());
    let lines: Vec<u8> = (1..=30)
        .flat_map(|line| format!("\r\nline-{line}").into_bytes())
        .collect();
    feed(&surface, "t1", &lines);
    let right = surface.tiles.tiles[1].info.inner_rect;
    let revealed = format!("line-{}", 30 - right.height - 2);
    let dirty = HashSet::from([id_of(&surface, "t1")]);
    assert!(!text(&frame(&mut surface, &dirty)).contains(&revealed));
    reports(&mut inputs[0]);
    reports(&mut inputs[1]);

    mouse(
        &mut surface,
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: right.x + 1,
            row: right.y + 1,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert_eq!(surface.focused_id(), Some(id_of(&surface, "t1")));
    assert_eq!(reports(&mut inputs[0]), b"\x1b[O");
    assert_eq!(reports(&mut inputs[1]), b"\x1b[I");
    assert!(text(&frame(&mut surface, &HashSet::new())).contains(&revealed));
}

#[tokio::test]
async fn parking_the_focused_pane_moves_focus_before_the_next_frame() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, REPORT_FOCUS);
    let [left, right] = &mut inputs[..] else {
        unreachable!("two panes")
    };
    assert_eq!(reports(left), b"\x1b[I");
    assert_eq!(reports(right), b"\x1b[O");

    let mut layout = row_of(&terminals(2));
    layout.parked.insert("t0".to_owned());
    surface.update(layout);

    assert_eq!(reports(left), b"\x1b[O");
    assert_eq!(reports(right), b"\x1b[I");
}

#[tokio::test]
async fn a_pane_reattached_after_the_host_regained_focus_is_told_it_has_focus() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(1)));
    let mut input = attach_with_input(&mut surface, REPORT_FOCUS).remove(0);
    surface.host_focus(false);
    assert_eq!(reports(&mut input), b"\x1b[I\x1b[O");

    surface.forget(id_of(&surface, "t0"));
    surface.host_focus(true);
    let mut input = attach_with_input(&mut surface, REPORT_FOCUS).remove(0);

    assert_eq!(reports(&mut input), b"\x1b[I");
}

#[tokio::test]
async fn a_pane_that_asks_for_focus_reports_after_it_attached_is_told_its_focus() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, b"");
    frame(&mut surface, &HashSet::new());
    let [left, right] = &mut inputs[..] else {
        unreachable!("two panes")
    };
    assert_eq!(reports(left), b"");
    assert_eq!(reports(right), b"");

    feed(&surface, "t0", REPORT_FOCUS);
    feed(&surface, "t1", REPORT_FOCUS);
    let dirty = HashSet::from([id_of(&surface, "t0"), id_of(&surface, "t1")]);
    surface.frame(&dirty, never_attach, ignore_leaving);

    assert_eq!(reports(left), b"\x1b[I");
    assert_eq!(reports(right), b"\x1b[O");
}

#[tokio::test]
async fn moving_focus_by_key_tells_both_panes() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, REPORT_FOCUS);
    frame(&mut surface, &HashSet::new());
    let [left, right] = &mut inputs[..] else {
        unreachable!("two panes")
    };
    reports(left);
    reports(right);

    assert_eq!(surface.act(KeybindAction::FocusPaneRight, true), None);
    assert!(surface.needs_frame());
    assert_eq!(reports(left), b"\x1b[O");
    assert_eq!(reports(right), b"\x1b[I");

    surface.act(KeybindAction::CyclePaneNext, true);
    assert_eq!(reports(left), b"\x1b[I");
    assert_eq!(reports(right), b"\x1b[O");
}

#[tokio::test]
async fn zooming_keeps_the_other_panes_view_for_when_it_ends() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, REPORT_FOCUS);
    frame(&mut surface, &HashSet::new());
    feed(&surface, "t1", b"right");
    let right = id_of(&surface, "t1");
    let size = surface.views[&right].view.size();

    surface.act(KeybindAction::Zoom, true);
    let zoomed = text(&frame(&mut surface, &HashSet::new()));
    assert!(!zoomed.contains("right"));
    assert_eq!(surface.tiles.tiles.len(), 1);
    assert_eq!(surface.views[&right].view.size(), size);

    surface.act(KeybindAction::FocusPaneRight, true);
    let rows = surface
        .frame(&HashSet::new(), never_attach, ignore_leaving)
        .0;
    assert!(text(&rows).contains("right"));
    assert_eq!(surface.views[&right].view.size(), size);
    assert_eq!(reports(&mut inputs[1]), b"\x1b[O\x1b[I");
}

#[tokio::test]
async fn a_created_terminal_takes_focus_and_tells_both_panes() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(1)));
    let mut first = attach_with_input(&mut surface, REPORT_FOCUS).remove(0);
    surface.update(row_of(&terminals(2)));
    let mut second = attach_with_input(&mut surface, REPORT_FOCUS).remove(0);
    reports(&mut first);
    reports(&mut second);

    surface.created("t1".to_owned());

    assert!(surface.needs_frame());
    assert_eq!(reports(&mut first), b"\x1b[O");
    assert_eq!(reports(&mut second), b"\x1b[I");
}

#[tokio::test]
async fn input_reaches_a_pane_focused_out_of_zoom_before_the_next_frame() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, b"");
    frame(&mut surface, &HashSet::new());
    surface.act(KeybindAction::Zoom, true);
    frame(&mut surface, &HashSet::new());

    surface.act(KeybindAction::FocusPaneRight, true);
    assert!(surface.cursor().is_none());
    surface.focused().expect("focused view").text("typed");

    assert_eq!(reports(&mut inputs[1]), b"typed");
}

#[tokio::test]
async fn a_pane_hidden_by_zoom_is_told_its_focus_once_it_asks() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, b"");
    frame(&mut surface, &HashSet::new());
    let right = id_of(&surface, "t1");
    surface.act(KeybindAction::Zoom, true);
    frame(&mut surface, &HashSet::new());

    surface.views[&right].view.test_feed(REPORT_FOCUS);
    surface.views[&right].view.test_feed(b"behind");
    let rows = surface
        .frame(&HashSet::from([right]), never_attach, ignore_leaving)
        .0;

    assert!(!text(&rows).contains("behind"));
    assert_eq!(reports(&mut inputs[1]), b"\x1b[O");
}

#[tokio::test]
async fn a_pane_holding_output_shows_its_last_image_when_zoom_ends() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    feed_after_first_frame(&mut surface, "t1", b"shown");
    let right = id_of(&surface, "t1");
    surface.views[&right].view.test_feed(b"\x1b[?2026h\r\nheld");
    surface.act(KeybindAction::Zoom, true);
    frame(&mut surface, &HashSet::new());

    surface.act(KeybindAction::Zoom, true);
    let rows = surface
        .frame(&HashSet::new(), never_attach, ignore_leaving)
        .0;

    assert!(text(&rows).contains("shown"));
    assert!(!text(&rows).contains("held"));
}

#[tokio::test]
async fn a_parked_pane_is_told_before_its_view_goes() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, b"\x1b[>3u");
    frame(&mut surface, &HashSet::new());
    let parked = id_of(&surface, "t0");
    reports(&mut inputs[0]);

    let mut layout = row_of(&terminals(2));
    layout.parked.insert("t0".to_owned());
    surface.update(layout);
    let mut leaving = Vec::new();
    surface.frame(&HashSet::new(), never_attach, |id, view| {
        leaving.push(id);
        view.key(
            TerminalKey::new(KeyCode::Up, KeyModifiers::NONE).with_kind(KeyEventKind::Release),
        );
    });

    assert_eq!(leaving, [parked]);
    assert!(!surface.views.contains_key(&parked));
    assert_eq!(reports(&mut inputs[0]), b"\x1b[1;1:3A");
}

fn lines(count: usize) -> Vec<u8> {
    (1..=count)
        .flat_map(|line| format!("\r\nline-{line}").into_bytes())
        .collect()
}

fn track(surface: &Surface, terminal: &str) -> Rect {
    let pane = surface
        .tiles
        .tiles
        .iter()
        .find(|tile| tile.terminal == terminal)
        .expect("tile")
        .info
        .inner_rect;
    Rect::new(pane.right() - 1, pane.y, 1, pane.height)
}

fn column(surface: &Surface, track: Rect) -> String {
    (track.y..track.bottom())
        .map(|y| {
            surface.canvas[usize::from(y) * usize::from(surface.canvas_cols) + usize::from(track.x)]
                .symbol
                .clone()
        })
        .collect()
}

fn metrics(surface: &Surface, terminal: &str) -> crate::pane::ScrollMetrics {
    surface.views[&id_of(surface, terminal)]
        .view
        .scroll_metrics()
        .expect("metrics")
}

fn mouse_at(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn scrolled_back_pane(surface: &mut Surface) -> (Rect, HashSet<PaneId>) {
    attach_with_input(surface, b"");
    feed_after_first_frame(surface, "t0", &lines(40));
    (track(surface, "t0"), HashSet::from([id_of(surface, "t0")]))
}

#[tokio::test]
async fn a_pane_gives_its_last_column_to_the_scrollbar_only_on_the_primary_screen() {
    let mut surface = surface(20, 8);
    surface.update(row_of(&terminals(1)));
    attach_with_input(&mut surface, b"");
    frame(&mut surface, &HashSet::new());
    let pane = surface.tiles.tiles[0].info.inner_rect;
    let id = id_of(&surface, "t0");
    assert_eq!(
        surface.views[&id].view.size(),
        (pane.width - 1, pane.height)
    );

    feed(&surface, "t0", b"\x1b[?1049h");
    frame(&mut surface, &HashSet::from([id]));
    assert_eq!(surface.views[&id].view.size(), (pane.width, pane.height));

    feed(&surface, "t0", b"\x1b[?1049l");
    frame(&mut surface, &HashSet::from([id]));
    assert_eq!(
        surface.views[&id].view.size(),
        (pane.width - 1, pane.height)
    );
}

#[tokio::test]
async fn a_narrow_pane_or_disabled_scrollbars_reserve_no_column() {
    for mut surface in [surface_with_scrollbars(20, 8, false), surface(4, 3)] {
        surface.update(row_of(&terminals(1)));
        let geometry = requests(&mut surface).remove(0).2;
        let pane = surface.tiles.tiles[0].info.inner_rect;

        assert_eq!((geometry.cols, geometry.rows), (pane.width, pane.height));
    }
}

#[tokio::test]
async fn the_bar_shows_once_there_is_scrollback_and_follows_the_wheel() {
    let mut surface = surface(20, 10);
    surface.update(row_of(&terminals(1)));
    attach_with_input(&mut surface, b"");
    frame(&mut surface, &HashSet::new());
    let track = track(&surface, "t0");
    assert_eq!(
        column(&surface, track),
        " ".repeat(usize::from(track.height))
    );

    feed(&surface, "t0", &lines(40));
    let dirty = HashSet::from([id_of(&surface, "t0")]);
    frame(&mut surface, &dirty);
    let bottom = column(&surface, track);
    assert!(bottom.ends_with('▐'), "{bottom:?}");

    for _ in 0..5 {
        mouse(
            &mut surface,
            mouse_at(MouseEventKind::ScrollUp, track.x - 1, track.y),
        );
    }
    frame(&mut surface, &dirty);
    let scrolled = column(&surface, track);
    assert!(!scrolled.ends_with('▐'), "{scrolled:?}");
    assert!(scrolled.contains('▐'), "{scrolled:?}");
}

#[tokio::test]
async fn the_thumb_stays_with_the_rows_on_screen_while_a_program_holds_synchronized_output() {
    let mut surface = surface(20, 10);
    surface.update(row_of(&terminals(1)));
    let (track, dirty) = scrolled_back_pane(&mut surface);
    let shown = column(&surface, track);

    feed(&surface, "t0", b"\x1b[?2026h");
    for _ in 0..5 {
        mouse(
            &mut surface,
            mouse_at(MouseEventKind::ScrollUp, track.x - 1, track.y),
        );
    }
    frame(&mut surface, &dirty);
    assert_ne!(metrics(&surface, "t0").offset_from_bottom, 0);
    assert_eq!(column(&surface, track), shown);

    feed(&surface, "t0", b"\x1b[?2026l");
    frame(&mut surface, &dirty);
    assert_ne!(column(&surface, track), shown);
}

#[tokio::test]
async fn the_bar_ignores_the_pointer_while_held_output_has_changed_the_terminal() {
    let mut surface = surface(20, 10);
    surface.update(row_of(&terminals(1)));
    let (track, dirty) = scrolled_back_pane(&mut surface);
    for _ in 0..3 {
        mouse(
            &mut surface,
            mouse_at(MouseEventKind::ScrollUp, track.x - 1, track.y),
        );
    }
    frame(&mut surface, &dirty);
    let thumb = crate::ui::scrollbar_thumb(metrics(&surface, "t0"), track).expect("thumb");
    feed(&surface, "t0", b"\x1b[?2026h");
    feed(&surface, "t0", &lines(400));
    let held = metrics(&surface, "t0").offset_from_bottom;
    assert_ne!(held, 0);

    for row in [thumb.top, thumb.top + thumb.len - 1] {
        mouse(&mut surface, click(track.x, row));
        mouse(
            &mut surface,
            mouse_at(MouseEventKind::Drag(MouseButton::Left), track.x, track.y),
        );
        mouse(
            &mut surface,
            mouse_at(MouseEventKind::Up(MouseButton::Left), track.x, row),
        );
        assert_eq!(metrics(&surface, "t0").offset_from_bottom, held, "{row}");
    }
    mouse(&mut surface, click(track.x, track.y));
    mouse(
        &mut surface,
        mouse_at(MouseEventKind::Up(MouseButton::Left), track.x, track.y),
    );
    assert_eq!(metrics(&surface, "t0").offset_from_bottom, held);

    feed(&surface, "t0", b"\x1b[?2026l");
    frame(&mut surface, &dirty);
    mouse(&mut surface, click(track.x, track.bottom() - 1));
    assert_eq!(metrics(&surface, "t0").offset_from_bottom, 0);
}

#[tokio::test]
async fn the_bar_takes_one_press_while_held_output_leaves_the_terminal_as_drawn() {
    let mut surface = surface(20, 10);
    surface.update(row_of(&terminals(1)));
    let (track, _) = scrolled_back_pane(&mut surface);
    feed(&surface, "t0", b"\x1b[?2026hx");

    mouse(&mut surface, click(track.x, track.y));
    mouse(
        &mut surface,
        mouse_at(MouseEventKind::Up(MouseButton::Left), track.x, track.y),
    );
    let top = metrics(&surface, "t0");
    assert_eq!(top.offset_from_bottom, top.max_offset_from_bottom);

    mouse(&mut surface, click(track.x, track.bottom() - 1));
    assert_eq!(
        metrics(&surface, "t0").offset_from_bottom,
        top.max_offset_from_bottom
    );
}

#[tokio::test]
async fn a_press_on_the_track_jumps_and_focuses_the_pane() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    attach_with_input(&mut surface, b"");
    feed_after_first_frame(&mut surface, "t1", &lines(40));
    let track = track(&surface, "t1");

    mouse(&mut surface, click(track.x, track.y));

    assert_eq!(surface.focused_id(), Some(id_of(&surface, "t1")));
    let metrics = metrics(&surface, "t1");
    assert_eq!(metrics.offset_from_bottom, metrics.max_offset_from_bottom);
}

#[tokio::test]
async fn a_thumb_drag_follows_the_pointer_off_the_bar_until_the_release() {
    let mut surface = surface(20, 10);
    surface.update(row_of(&terminals(1)));
    let (track, _) = scrolled_back_pane(&mut surface);
    let thumb = crate::ui::scrollbar_thumb(metrics(&surface, "t0"), track).expect("thumb");

    mouse(&mut surface, click(track.x, thumb.top));
    assert_eq!(metrics(&surface, "t0").offset_from_bottom, 0);

    mouse(
        &mut surface,
        mouse_at(MouseEventKind::Drag(MouseButton::Left), 0, track.y),
    );
    let top = metrics(&surface, "t0");
    assert_eq!(top.offset_from_bottom, top.max_offset_from_bottom);

    mouse(
        &mut surface,
        mouse_at(MouseEventKind::Up(MouseButton::Left), 0, track.bottom() - 1),
    );
    assert_eq!(metrics(&surface, "t0").offset_from_bottom, 0);

    mouse(
        &mut surface,
        mouse_at(MouseEventKind::Drag(MouseButton::Left), track.x, track.y),
    );
    assert_eq!(metrics(&surface, "t0").offset_from_bottom, 0);
}

#[tokio::test]
async fn a_press_on_the_bar_never_reaches_a_mouse_reporting_program() {
    let mut surface = surface(20, 10);
    surface.update(row_of(&terminals(1)));
    let mut input = attach_with_input(&mut surface, b"").remove(0);
    frame(&mut surface, &HashSet::new());
    feed(&surface, "t0", &lines(40));
    feed(&surface, "t0", b"\x1b[?1000h\x1b[?1006h");
    let dirty = HashSet::from([id_of(&surface, "t0")]);
    frame(&mut surface, &dirty);
    let track = track(&surface, "t0");
    reports(&mut input);

    mouse(&mut surface, click(track.x, track.y));
    mouse(
        &mut surface,
        mouse_at(MouseEventKind::Up(MouseButton::Left), track.x, track.y),
    );
    assert_eq!(reports(&mut input), b"");

    mouse(&mut surface, click(track.x - 1, track.y));
    assert!(!reports(&mut input).is_empty());
}

#[tokio::test]
async fn output_redraws_the_bar_only_when_its_thumb_moves() {
    let mut surface = surface(20, 10);
    surface.update(row_of(&terminals(1)));
    let (track, dirty) = scrolled_back_pane(&mut surface);
    let on_bar = |rows: &[PaneSurfacePatchRow]| rows.iter().filter(|row| row.x == track.x).count();

    feed(&surface, "t0", b"x");
    assert_eq!(on_bar(&frame(&mut surface, &dirty)), 0);

    mouse(
        &mut surface,
        mouse_at(MouseEventKind::ScrollUp, track.x - 1, track.y),
    );
    assert_eq!(
        on_bar(&frame(&mut surface, &dirty)),
        usize::from(track.height)
    );
}

const REPORT_MOUSE: &[u8] = b"\x1b[?1000h\x1b[?1006h";

fn release_at(column: u16, row: u16) -> MouseEvent {
    mouse_at(MouseEventKind::Up(MouseButton::Left), column, row)
}

#[tokio::test]
async fn a_press_in_the_terminal_released_on_the_bar_reaches_the_program() {
    let mut surface = surface(20, 10);
    surface.update(row_of(&terminals(1)));
    let mut input = attach_with_input(&mut surface, REPORT_MOUSE).remove(0);
    let (track, _) = scrolled_back_pane(&mut surface);
    reports(&mut input);

    mouse(&mut surface, click(2, 3));
    mouse(&mut surface, release_at(track.x, track.y));

    assert_eq!(reports(&mut input), b"\x1b[<0;3;4M\x1b[<0;19;1m");
}

#[tokio::test]
async fn a_press_in_one_pane_released_over_another_reaches_only_the_first() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, REPORT_MOUSE);
    frame(&mut surface, &HashSet::new());
    let [first, second] = [0, 1].map(|index| surface.tiles.tiles[index].info.inner_rect);
    inputs.iter_mut().for_each(|input| {
        reports(input);
    });

    mouse(&mut surface, click(first.x, first.y));
    mouse(&mut surface, release_at(second.x + 1, second.y));

    assert_eq!(reports(&mut inputs[0]), b"\x1b[<0;1;1M\x1b[<0;18;1m");
    assert_eq!(reports(&mut inputs[1]), b"");
}

#[tokio::test]
async fn a_track_press_keeps_its_jump_when_released_in_the_terminal() {
    let mut surface = surface(20, 10);
    surface.update(row_of(&terminals(1)));
    let mut input = attach_with_input(&mut surface, b"").remove(0);
    let (track, _) = scrolled_back_pane(&mut surface);
    feed(&surface, "t0", REPORT_MOUSE);
    reports(&mut input);

    mouse(&mut surface, click(track.x, track.y));
    mouse(
        &mut surface,
        mouse_at(MouseEventKind::Drag(MouseButton::Left), 2, track.y),
    );
    mouse(&mut surface, release_at(2, track.y));

    assert_eq!(reports(&mut input), b"");
    let metrics = metrics(&surface, "t0");
    assert_eq!(metrics.offset_from_bottom, metrics.max_offset_from_bottom);
}

#[tokio::test]
async fn losing_host_focus_releases_a_held_button_once() {
    let mut surface = surface(20, 10);
    surface.update(row_of(&terminals(1)));
    let mut input = attach_with_input(&mut surface, REPORT_MOUSE).remove(0);
    frame(&mut surface, &HashSet::new());
    reports(&mut input);

    mouse(&mut surface, click(2, 3));
    surface.host_focus(false);
    mouse(&mut surface, release_at(4, 5));
    assert_eq!(reports(&mut input), b"\x1b[<0;3;4M\x1b[<0;3;4m");

    surface.host_focus(true);
    mouse(&mut surface, click(4, 5));
    assert_eq!(reports(&mut input), b"\x1b[<0;5;6M");
}

#[tokio::test]
async fn zooming_another_pane_releases_the_held_button_before_hiding_its_pane() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, REPORT_MOUSE);
    frame(&mut surface, &HashSet::new());
    let left = surface.tiles.tiles[0].info.inner_rect;
    let held = id_of(&surface, "t0");
    inputs.iter_mut().for_each(|input| {
        reports(input);
    });

    mouse(&mut surface, click(left.x + 1, left.y + 1));
    surface.act(KeybindAction::FocusPaneRight, true);
    surface.act(KeybindAction::Zoom, true);
    frame(&mut surface, &HashSet::new());
    assert!(matches!(
        surface.views[&held].place,
        super::Place::Hidden(_)
    ));
    assert_eq!(reports(&mut inputs[0]), b"\x1b[<0;2;2M\x1b[<0;2;2m");

    mouse(&mut surface, release_at(left.x + 1, left.y + 1));
    assert_eq!(reports(&mut inputs[0]), b"");
    assert_eq!(reports(&mut inputs[1]), b"");
}

#[tokio::test]
async fn a_pixel_report_press_released_on_the_bar_reaches_the_program() {
    let mut surface = surface(20, 10);
    surface.update(row_of(&terminals(1)));
    let mut input = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1006h\x1b[?1016h").remove(0);
    surface.resize(Geometry {
        cell_width_px: 10,
        cell_height_px: 20,
        ..host(20, 10)
    });
    frame(&mut surface, &HashSet::new());
    let track = track(&surface, "t0");
    reports(&mut input);

    mouse(&mut surface, click(2, 3));
    mouse(&mut surface, release_at(track.x, track.bottom()));

    let sent = String::from_utf8(reports(&mut input)).expect("utf8");
    assert_eq!(sent.matches("\x1b[<0;").count(), 2, "{sent:?}");
    assert!(sent.ends_with('m'), "{sent:?}");
}

#[tokio::test]
async fn a_release_after_the_pane_shrank_lands_inside_it() {
    let mut surface = surface(20, 10);
    surface.update(row_of(&terminals(1)));
    let mut input = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1006h\x1b[?1016h").remove(0);
    let pixels = |cols, rows| Geometry {
        cell_width_px: 10,
        cell_height_px: 20,
        ..host(cols, rows)
    };
    surface.resize(pixels(20, 10));
    frame(&mut surface, &HashSet::new());
    reports(&mut input);

    mouse(&mut surface, click(18, 9));
    surface.resize(pixels(10, 5));
    frame(&mut surface, &HashSet::new());
    surface.host_focus(false);

    let sent = String::from_utf8(reports(&mut input)).expect("utf8");
    assert_eq!(sent.matches("\x1b[<0;").count(), 2, "{sent:?}");
    assert!(sent.ends_with('m'), "{sent:?}");
}

#[tokio::test]
async fn a_press_the_program_did_not_see_is_never_released_to_it() {
    let mut surface = surface(20, 10);
    surface.update(row_of(&terminals(1)));
    let mut input = attach_with_input(&mut surface, b"").remove(0);
    let (track, _) = scrolled_back_pane(&mut surface);
    mouse(&mut surface, mouse_at(MouseEventKind::ScrollUp, 2, 3));
    let offset = metrics(&surface, "t0").offset_from_bottom;
    assert!(offset > 0);
    reports(&mut input);

    mouse(&mut surface, click(2, 3));
    feed(&surface, "t0", REPORT_MOUSE);
    surface.host_focus(false);
    mouse(&mut surface, release_at(track.x - 1, 3));

    assert_eq!(reports(&mut input), b"");
    assert_eq!(metrics(&surface, "t0").offset_from_bottom, offset);
}

fn selecting(
    on_select: bool,
    shown: &[u8],
) -> (Surface, Rect, tokio::sync::mpsc::Receiver<bytes::Bytes>) {
    let mut surface = surface(40, 10);
    surface.select = super::Select::new(on_select);
    surface.update(row_of(&terminals(2)));
    let input = attach_with_input(&mut surface, shown).remove(0);
    frame(&mut surface, &HashSet::new());
    let (_, inner) = super::visible(&surface.views, id_of(&surface, "t0")).expect("visible");
    (surface, inner, input)
}

fn drag_to(column: u16, row: u16) -> MouseEvent {
    mouse_at(MouseEventKind::Drag(MouseButton::Left), column, row)
}

fn shown_text(surface: &Surface) -> Option<String> {
    let selection = surface.select.shown()?;
    surface.views[&selection.pane_id]
        .view
        .selected_text(selection)
}

fn highlighted(surface: &Surface, rows: &[PaneSurfacePatchRow], x: u16, y: u16) -> Option<bool> {
    rows.iter()
        .find(|row| row.y == y && row.x <= x && x < row.x + row.cells.len() as u16)
        .map(|row| row.cells[usize::from(x - row.x)].bg == surface.highlight.bg)
}

#[tokio::test]
async fn a_drag_selects_and_its_release_copies_the_text() {
    let (mut surface, inner, _input) = selecting(true, b"hello world\r\nsecond line");
    let now = std::time::Instant::now();

    assert_eq!(surface.mouse(click(inner.x, inner.y), now), None);
    surface.mouse(drag_to(inner.x + 5, inner.y + 1), now);
    let copied = copied_text(surface.mouse(release_at(inner.x + 5, inner.y + 1), now));

    assert_eq!(copied.as_deref(), Some("hello world\nsecond"));
    assert!(surface.select.shown().is_none());
}

#[tokio::test]
async fn a_click_without_a_drag_copies_nothing_and_leaves_no_selection() {
    let (mut surface, inner, _input) = selecting(true, b"hello world");
    let now = std::time::Instant::now();

    surface.mouse(click(inner.x + 1, inner.y), now);
    assert_eq!(surface.mouse(release_at(inner.x + 1, inner.y), now), None);

    assert!(surface.select.pane().is_none());
}

#[tokio::test]
async fn without_copy_on_select_the_selection_stays_until_the_copy_key() {
    let (mut surface, inner, _input) = selecting(false, b"hello world");
    let now = std::time::Instant::now();
    assert!(!surface.copyable());

    surface.mouse(click(inner.x, inner.y), now);
    surface.mouse(drag_to(inner.x + 4, inner.y), now);
    assert_eq!(surface.mouse(release_at(inner.x + 4, inner.y), now), None);

    assert!(surface.copyable());
    assert_eq!(surface.copy().as_deref(), Some("hello"));
    assert!(!surface.copyable());
    assert!(surface.select.shown().is_none());
}

#[tokio::test]
async fn a_double_click_selects_and_copies_a_word_that_flashes_briefly() {
    let (mut surface, inner, _input) = selecting(true, b"hello world");
    let now = std::time::Instant::now();
    let later = now + std::time::Duration::from_millis(200);

    surface.mouse(click(inner.x + 7, inner.y), now);
    surface.mouse(release_at(inner.x + 7, inner.y), now);
    surface.mouse(click(inner.x + 8, inner.y), later);
    assert_eq!(shown_text(&surface).as_deref(), Some("world"));
    let copied = copied_text(surface.mouse(release_at(inner.x + 8, inner.y), later));

    assert_eq!(copied.as_deref(), Some("world"));
    let flash = later + std::time::Duration::from_millis(500);
    assert_eq!(surface.deadline(), Some(flash));
    surface.tick(flash - std::time::Duration::from_millis(1));
    assert!(surface.select.shown().is_some());
    surface.tick(flash);
    assert!(surface.select.shown().is_none());
    assert_eq!(surface.deadline(), None);
}

#[tokio::test]
async fn clicks_too_far_apart_in_time_or_place_are_not_a_double_click() {
    let (mut surface, inner, _input) = selecting(true, b"hello world");
    let now = std::time::Instant::now();

    for (column, at) in [
        (inner.x + 9, now + std::time::Duration::from_millis(100)),
        (inner.x + 7, now + std::time::Duration::from_millis(1000)),
    ] {
        surface.mouse(click(inner.x + 7, inner.y), now);
        surface.mouse(release_at(inner.x + 7, inner.y), now);
        surface.mouse(click(column, inner.y), at);

        assert!(surface.select.shown().is_none());
        surface.mouse(release_at(column, inner.y), at);
    }
}

#[tokio::test]
async fn a_drag_after_a_double_click_extends_by_whole_words() {
    let (mut surface, inner, _input) = selecting(true, b"hello world\r\nsecond line");
    let now = std::time::Instant::now();

    surface.mouse(click(inner.x + 7, inner.y), now);
    surface.mouse(release_at(inner.x + 7, inner.y), now);
    surface.mouse(click(inner.x + 7, inner.y), now);
    surface.mouse(drag_to(inner.x + 2, inner.y + 1), now);
    assert_eq!(shown_text(&surface).as_deref(), Some("world\nsecond"));
    surface.mouse(drag_to(inner.x + 1, inner.y), now);
    assert_eq!(shown_text(&surface).as_deref(), Some("hello world"));

    let copied = copied_text(surface.mouse(release_at(inner.x + 1, inner.y), now));
    assert_eq!(copied.as_deref(), Some("hello world"));
    assert!(surface.select.shown().is_none());
}

#[tokio::test]
async fn a_drag_past_the_top_scrolls_at_once_and_then_on_each_tick() {
    let (mut surface, inner, _input) = selecting(true, &lines(40));
    let now = std::time::Instant::now();

    surface.mouse(click(inner.x, inner.y + 3), now);
    surface.mouse(drag_to(inner.x + 2, inner.y - 1), now);
    assert_eq!(metrics(&surface, "t0").offset_from_bottom, 3);
    assert!(shown_text(&surface).expect("text").starts_with("ne-30\n"));

    let tick = now + std::time::Duration::from_millis(30);
    assert_eq!(surface.deadline(), Some(tick));
    surface.tick(tick);
    assert_eq!(metrics(&surface, "t0").offset_from_bottom, 4);

    let copied = copied_text(surface.mouse(release_at(inner.x + 2, inner.y - 1), tick));
    assert_eq!(copied.as_deref().map(|text| &text[..6]), Some("ne-29\n"));
    assert_eq!(surface.deadline(), None);
}

#[tokio::test]
async fn the_wheel_during_a_drag_scrolls_and_extends_to_the_pointer() {
    let (mut surface, inner, _input) = selecting(true, &lines(40));
    let now = std::time::Instant::now();

    surface.mouse(click(inner.x, inner.y + 3), now);
    surface.mouse(drag_to(inner.x + 2, inner.y + 4), now);
    surface.mouse(
        mouse_at(MouseEventKind::ScrollUp, inner.x + 2, inner.y + 1),
        now,
    );

    assert_eq!(metrics(&surface, "t0").offset_from_bottom, 3);
    let copied = copied_text(surface.mouse(release_at(inner.x + 2, inner.y + 1), now));
    assert_eq!(
        copied.as_deref(),
        Some("ne-31\nline-32\nline-33\nline-34\nline-35\nl")
    );
}

#[tokio::test]
async fn a_selection_survives_output_and_scrolling_and_copies_the_same_text() {
    let (mut surface, inner, _input) = selecting(false, &lines(40));
    let now = std::time::Instant::now();
    let dirty = HashSet::from([id_of(&surface, "t0")]);
    surface.mouse(click(inner.x, inner.y + 3), now);
    surface.mouse(drag_to(inner.x + 6, inner.y + 3), now);
    surface.mouse(release_at(inner.x + 6, inner.y + 3), now);

    feed(&surface, "t0", b"\r\nmore");
    frame(&mut surface, &dirty);
    surface.mouse(mouse_at(MouseEventKind::ScrollUp, inner.x, inner.y), now);
    frame(&mut surface, &dirty);

    assert_eq!(surface.copy().as_deref(), Some("line-36"));
}

#[tokio::test]
async fn any_left_press_clears_the_selection() {
    let (mut surface, inner, _input) = selecting(false, b"hello world");
    let now = std::time::Instant::now();
    surface.mouse(click(inner.x, inner.y), now);
    surface.mouse(drag_to(inner.x + 4, inner.y), now);
    surface.mouse(release_at(inner.x + 4, inner.y), now);
    assert!(surface.select.shown().is_some());

    surface.mouse(click(0, 0), now);

    assert!(surface.select.shown().is_none());
}

fn kept_selection(surface: &mut Surface, terminal: &str) {
    let (_, inner) = super::visible(&surface.views, id_of(surface, terminal)).expect("visible");
    let now = std::time::Instant::now();
    surface.mouse(click(inner.x, inner.y), now);
    surface.mouse(drag_to(inner.x + 2, inner.y), now);
    surface.mouse(release_at(inner.x + 2, inner.y), now);
    assert!(surface.select.shown().is_some());
}

#[tokio::test]
async fn moving_focus_away_clears_the_selection() {
    let mut surface = surface(40, 10);
    surface.select = super::Select::new(false);
    surface.update(row_of(&terminals(2)));
    attach_with_input(&mut surface, b"text");
    frame(&mut surface, &HashSet::new());
    kept_selection(&mut surface, "t0");

    surface.act(KeybindAction::FocusPaneRight, true);

    assert!(surface.select.shown().is_none());
}

#[tokio::test]
async fn a_detached_pane_loses_its_selection() {
    let (mut surface, _, _input) = selecting(false, b"hello world");
    kept_selection(&mut surface, "t0");

    surface.forget(id_of(&surface, "t0"));
    frame(&mut surface, &HashSet::new());

    assert!(surface.select.pane().is_none());
}

#[tokio::test]
async fn losing_host_focus_ends_a_drag_and_its_autoscroll() {
    let (mut surface, inner, _input) = selecting(true, &lines(40));
    let now = std::time::Instant::now();
    surface.mouse(click(inner.x, inner.y + 3), now);
    surface.mouse(drag_to(inner.x + 2, inner.y), now);
    assert!(surface.deadline().is_some());

    surface.host_focus(false);
    surface.mouse(drag_to(inner.x + 4, inner.y + 5), now);

    assert_eq!(surface.deadline(), None);
    assert!(shown_text(&surface).expect("kept").starts_with("ne-33"));
    assert_eq!(
        surface.mouse(release_at(inner.x + 4, inner.y + 5), now),
        None
    );
}

#[tokio::test]
async fn a_new_terminal_size_clears_the_selection() {
    let (mut surface, _, _input) = selecting(false, b"hello world");
    kept_selection(&mut surface, "t0");

    surface.resize(host(30, 10));
    frame(&mut surface, &HashSet::new());

    assert!(surface.select.pane().is_none());
}

#[tokio::test]
async fn switching_screens_clears_the_selection() {
    let mut surface = surface_with_scrollbars(20, 10, false);
    surface.select = super::Select::new(false);
    surface.update(row_of(&terminals(1)));
    attach_with_input(&mut surface, b"hello world");
    frame(&mut surface, &HashSet::new());
    kept_selection(&mut surface, "t0");

    feed(&surface, "t0", b"\x1b[?1049h");
    let dirty = HashSet::from([id_of(&surface, "t0")]);
    frame(&mut surface, &dirty);

    assert!(surface.select.pane().is_none());
}

#[tokio::test]
async fn a_press_a_mouse_reporting_program_takes_starts_no_selection() {
    let (mut surface, inner, _input) = selecting(true, REPORT_MOUSE);
    let now = std::time::Instant::now();

    surface.mouse(click(inner.x, inner.y), now);
    surface.mouse(drag_to(inner.x + 4, inner.y), now);

    assert!(surface.select.pane().is_none());
    assert_eq!(surface.mouse(release_at(inner.x + 4, inner.y), now), None);
}

#[tokio::test]
async fn a_press_on_the_scrollbar_starts_no_selection() {
    let (mut surface, _, _input) = selecting(true, &lines(40));
    let track = track(&surface, "t0");
    let now = std::time::Instant::now();

    surface.mouse(click(track.x, track.y + 3), now);
    surface.mouse(drag_to(track.x - 4, track.y + 3), now);

    assert!(surface.select.pane().is_none());
    assert_eq!(
        surface.mouse(release_at(track.x - 4, track.y + 3), now),
        None
    );
}

#[tokio::test]
async fn a_drag_over_the_scrollbar_selects_to_the_last_terminal_column() {
    let (mut surface, inner, _input) = selecting(true, b"0123456789abcdefghij");
    let now = std::time::Instant::now();

    surface.mouse(click(inner.x, inner.y), now);
    let copied = copied_text(surface.mouse(release_at(inner.right(), inner.y), now));
    assert_eq!(copied, None);
    surface.mouse(
        click(inner.x, inner.y),
        now + std::time::Duration::from_secs(1),
    );
    surface.mouse(drag_to(inner.right(), inner.y), now);
    let copied = copied_text(surface.mouse(release_at(inner.right(), inner.y), now));

    let row = "0123456789abcdefghij";
    assert_eq!(copied.as_deref(), Some(&row[..usize::from(inner.width)]));
}

#[tokio::test]
async fn the_highlight_follows_the_selection_on_both_draw_paths() {
    let (mut surface, inner, _input) = selecting(false, b"hello world");
    let (x, y) = (inner.x + 1, inner.y);
    let now = std::time::Instant::now();
    surface.mouse(click(inner.x, y), now);
    surface.mouse(drag_to(inner.x + 2, y), now);
    assert!(surface.needs_frame());

    let rows = frame(&mut surface, &HashSet::new());
    assert_eq!(highlighted(&surface, &rows, x, y), Some(true));
    assert_eq!(highlighted(&surface, &rows, inner.x + 3, y), Some(false));
    assert!(!surface.needs_frame());

    feed(&surface, "t0", b"\x1b[1;1HJ");
    let dirty = HashSet::from([id_of(&surface, "t0")]);
    let rows = frame(&mut surface, &dirty);
    assert_eq!(highlighted(&surface, &rows, x, y), Some(true));
    assert!(text(&rows).contains("Jello"));

    surface.resize(host(40, 10));
    let rows = frame(&mut surface, &HashSet::new());
    assert_eq!(rows.len(), 10);
    assert_eq!(highlighted(&surface, &rows, x, y), Some(true));

    surface.deselect();
    assert!(surface.needs_frame());
    let rows = frame(&mut surface, &HashSet::new());
    assert_eq!(highlighted(&surface, &rows, x, y), Some(false));
}

#[tokio::test]
async fn the_highlight_stays_on_its_text_while_a_program_holds_synchronized_output() {
    let (mut surface, inner, _input) =
        selecting(false, &[lines(20), b"\r\ntarget".to_vec()].concat());
    let bottom = inner.bottom() - 1;
    let now = std::time::Instant::now();
    surface.mouse(click(inner.x, bottom), now);
    surface.mouse(drag_to(inner.x + 5, bottom), now);
    surface.mouse(release_at(inner.x + 5, bottom), now);
    frame(&mut surface, &HashSet::new());
    let dirty = HashSet::from([id_of(&surface, "t0")]);

    feed(&surface, "t0", b"\x1b[?2026h\r\nnext");
    surface.copied(now);
    let rows = frame(&mut surface, &dirty);
    assert_eq!(highlighted(&surface, &rows, inner.x, bottom), Some(true));
    assert_ne!(
        highlighted(&surface, &rows, inner.x, bottom - 1),
        Some(true)
    );

    feed(&surface, "t0", b"\x1b[?2026l");
    let rows = frame(&mut surface, &dirty);
    assert_eq!(
        highlighted(&surface, &rows, inner.x, bottom - 1),
        Some(true)
    );
    assert_eq!(highlighted(&surface, &rows, inner.x, bottom), Some(false));
    assert_eq!(surface.copy().as_deref(), Some("target"));
}

#[tokio::test]
async fn a_press_starts_no_selection_while_a_program_holds_synchronized_output() {
    for double_click in [true, false] {
        let (mut surface, inner, _input) =
            selecting(true, &[lines(20), b"\r\ntarget".to_vec()].concat());
        let bottom = inner.bottom() - 1;
        let now = std::time::Instant::now();
        let gesture = |surface: &mut Surface| {
            if double_click {
                surface.mouse(click(inner.x + 1, bottom), now);
                surface.mouse(release_at(inner.x + 1, bottom), now);
                surface.mouse(click(inner.x + 1, bottom), now);
                copied_text(surface.mouse(release_at(inner.x + 1, bottom), now))
            } else {
                surface.mouse(click(inner.x, bottom), now);
                surface.mouse(drag_to(inner.x + 5, bottom), now);
                copied_text(surface.mouse(release_at(inner.x + 5, bottom), now))
            }
        };

        feed(&surface, "t0", b"\x1b[?2026h\r\x1b[2Khidden");
        assert_eq!(gesture(&mut surface), None, "{double_click}");
        assert!(surface.select.shown().is_none());

        feed(&surface, "t0", b"\x1b[?2026l");
        let dirty = HashSet::from([id_of(&surface, "t0")]);
        frame(&mut surface, &dirty);
        assert_eq!(
            gesture(&mut surface).as_deref(),
            Some("hidden"),
            "{double_click}"
        );
    }
}

#[tokio::test]
async fn a_double_click_reads_no_word_until_the_screen_shows_the_new_text() {
    let (mut surface, inner, _input) =
        selecting(true, &[lines(20), b"\r\ntarget".to_vec()].concat());
    let bottom = inner.bottom() - 1;
    let now = std::time::Instant::now();
    let click_once = |surface: &mut Surface| {
        surface.mouse(click(inner.x + 1, bottom), now);
        copied_text(surface.mouse(release_at(inner.x + 1, bottom), now))
    };
    feed(&surface, "t0", b"\x1b[?2026h\r\x1b[2Khidden\x1b[?2026l");

    assert_eq!(click_once(&mut surface), None);
    assert_eq!(click_once(&mut surface), None);

    let dirty = HashSet::from([id_of(&surface, "t0")]);
    frame(&mut surface, &dirty);
    assert_eq!(click_once(&mut surface).as_deref(), Some("hidden"));
}

#[tokio::test]
async fn a_press_starts_no_selection_in_a_pane_that_has_not_drawn() {
    let mut view = View::test(20, 5);
    view.test_feed(&lines(20));
    let id = PaneId::from_raw(1);
    let inner = Rect::new(0, 0, 20, 5);
    let now = std::time::Instant::now();
    let mut select = super::Select::new(true);
    let mut gesture = |view: &View| {
        select.press(Some((id, view, inner)), click(0, 2), now);
        select.drag(view, inner, 6, 2, now);
        select.release(Some((view, inner)), now)
    };

    assert_eq!(gesture(&view), None);

    view.draw();
    assert_eq!(gesture(&view).as_deref(), Some("line-18"));
}

#[tokio::test]
async fn the_copied_toast_shows_for_two_seconds() {
    let (mut surface, _, _input) = selecting(true, b"");
    let now = std::time::Instant::now();

    surface.copied(now);
    assert!(surface.needs_frame());
    assert!(text(&frame(&mut surface, &HashSet::new())).contains("copied to clipboard"));
    let expiry = now + std::time::Duration::from_secs(2);
    assert_eq!(surface.deadline(), Some(expiry));

    surface.tick(expiry);
    assert!(surface.needs_frame());
    let rows = frame(&mut surface, &HashSet::new());
    assert!(!rows.is_empty());
    assert!(!text(&rows).contains("copied"));
    assert_eq!(surface.deadline(), None);
}

#[tokio::test]
async fn output_during_a_held_double_click_drops_the_word_but_a_scroll_keeps_it() {
    for (framed, dragged) in [(true, true), (false, true), (false, false)] {
        let (mut surface, inner, _input) =
            selecting(true, &[lines(20), b"\r\none two".to_vec()].concat());
        let now = std::time::Instant::now();
        let id = id_of(&surface, "t0");
        let dirty = HashSet::from([id]);
        let bottom = inner.bottom() - 1;
        surface.mouse(click(inner.x + 1, bottom), now);
        surface.mouse(release_at(inner.x + 1, bottom), now);
        surface.mouse(click(inner.x + 1, bottom), now);
        assert_eq!(shown_text(&surface).as_deref(), Some("one"));

        surface.views[&id].view.scroll_to(1);
        frame(&mut surface, &dirty);
        surface.views[&id].view.scroll_to(0);
        frame(&mut surface, &dirty);
        assert_eq!(shown_text(&surface).as_deref(), Some("one"));

        feed(&surface, "t0", b"\rlongerword");
        if framed {
            frame(&mut surface, &dirty);
            assert!(surface.select.pane().is_none());
        }
        if dragged {
            surface.mouse(drag_to(inner.x + 3, bottom), now);
            assert!(surface.select.pane().is_none());
        }
        assert_eq!(surface.mouse(release_at(inner.x + 3, bottom), now), None);
        assert!(surface.select.pane().is_none());
    }
}

#[tokio::test]
async fn a_selection_made_before_a_screen_switch_copies_nothing() {
    for on_select in [true, false] {
        let (mut surface, inner, _input) = selecting(on_select, b"hello");
        let now = std::time::Instant::now();
        surface.mouse(click(inner.x, inner.y), now);
        surface.mouse(drag_to(inner.x + 4, inner.y), now);
        assert_eq!(shown_text(&surface).as_deref(), Some("hello"));

        feed(&surface, "t0", b"\x1b[?1049h\x1b[Hworld");
        assert_eq!(surface.mouse(release_at(inner.x + 4, inner.y), now), None);
        assert_eq!(surface.copy(), None);
        assert!(surface.select.pane().is_none());
    }
}

#[tokio::test]
async fn the_copy_key_on_a_held_word_whose_text_changed_copies_nothing() {
    let (mut surface, inner, _input) = selecting(false, b"one two");
    let now = std::time::Instant::now();
    surface.mouse(click(inner.x + 1, inner.y), now);
    surface.mouse(release_at(inner.x + 1, inner.y), now);
    surface.mouse(click(inner.x + 1, inner.y), now);
    assert_eq!(shown_text(&surface).as_deref(), Some("one"));

    feed(&surface, "t0", b"\rlongerword");
    assert_eq!(surface.copy(), None);
    assert!(surface.select.pane().is_none());
}

#[tokio::test]
async fn losing_host_focus_drops_a_held_word_whose_text_changed_and_keeps_one_that_did_not() {
    for changed in [true, false] {
        let (mut surface, inner, _input) = selecting(false, b"one two");
        let now = std::time::Instant::now();
        surface.mouse(click(inner.x + 1, inner.y), now);
        surface.mouse(release_at(inner.x + 1, inner.y), now);
        surface.mouse(click(inner.x + 1, inner.y), now);
        if changed {
            feed(&surface, "t0", b"\rlongerword");
        }
        surface.host_focus(false);
        let copied = surface.copy();
        assert_eq!(copied.as_deref(), (!changed).then_some("one"));
    }
}

#[tokio::test]
async fn a_double_click_in_a_pane_narrower_than_its_terminal_takes_only_the_shown_word() {
    let mut surface = surface_with_scrollbars(6, 10, false);
    surface.update(row_of(&terminals(2)));
    for (_, id, geometry) in requests(&mut surface) {
        let view = View::test(geometry.cols.max(crate::pane::MIN_PANE_COLS), geometry.rows);
        view.test_feed(b"abcd\r\nefgh");
        surface.attached(id, geometry, Ok(view));
    }
    frame(&mut surface, &HashSet::new());
    let (view, inner) = super::visible(&surface.views, id_of(&surface, "t0")).expect("visible");
    assert_eq!(
        (inner.width, view.size().0),
        (2, crate::pane::MIN_PANE_COLS)
    );
    let now = std::time::Instant::now();

    surface.mouse(click(inner.x, inner.y), now);
    surface.mouse(release_at(inner.x, inner.y), now);
    surface.mouse(click(inner.x, inner.y), now);
    let word = copied_text(surface.mouse(release_at(inner.x, inner.y), now));
    assert_eq!(word.as_deref(), Some("ab"));
}

mod chrome;
mod copy_mode;
mod dialog;
mod notices;
