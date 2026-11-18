use std::collections::HashSet;

use crossterm::event::{
    KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use serde_json::{json, Value};

use super::Surface;
use crate::config::PaneBordersConfig;
use crate::core_client::tui::session::PaneChrome;
use crate::core_client::view::{Geometry, View};
use crate::input::{KeybindAction, TerminalKey};
use crate::layout::PaneId;
use crate::protocol::PaneSurfacePatchRow;
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

fn surface(cols: u16, rows: u16) -> Surface {
    Surface::new(
        host(cols, rows),
        PaneChrome {
            borders: PaneBordersConfig::Auto,
            gaps: false,
            outer_borders: true,
        },
        Palette::catppuccin(),
    )
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
    let rows = surface
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
    let rows = surface
        .frame(dirty, |_, _, _| unreachable!("attached"), ignore_leaving)
        .0;
    (
        rows,
        requests
            .into_iter()
            .map(|(terminal, ..)| terminal)
            .collect(),
    )
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

    surface.mouse(click(right.x + 1, right.y + 1));

    assert!(surface.needs_layout());
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

    surface.mouse(click(right.x, right.y));

    assert!(!surface.needs_layout());
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

    surface.mouse(click(right.x, right.y));
    assert!(surface.needs_layout());
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
    surface.mouse(click(left.x, left.y));

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
    assert!(surface.needs_layout());
    let (_, attached) = frame_attaching(&mut surface, &HashSet::new());

    assert_eq!(attached, ["t1"]);
    assert_eq!(surface.views.len(), 2);
    assert!(!surface.needs_layout());
}

#[tokio::test]
async fn input_follows_a_click_before_the_next_frame() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    frame(&mut surface, &HashSet::new());
    let right = surface.tiles.tiles[1].info.inner_rect;

    surface.mouse(click(right.x, right.y));

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
        Palette::catppuccin(),
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
    surface.mouse(click(pane.x, pane.y));
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

    surface.mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: right.x + 1,
        row: right.y + 1,
        modifiers: KeyModifiers::NONE,
    });

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
    assert!(surface.needs_layout());
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

    assert!(surface.needs_layout());
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
