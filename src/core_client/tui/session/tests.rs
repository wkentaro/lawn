use ratatui::layout::Rect;
use serde_json::json;

use super::{PaneChrome, Session};
use crate::config::PaneBordersConfig;
use crate::session_doc::Layout;

fn layout(value: serde_json::Value) -> Layout {
    serde_json::from_value(value).expect("layout")
}

fn pane(terminal: &str) -> serde_json::Value {
    json!({"kind": "pane", "terminal": terminal})
}

fn split(first: serde_json::Value, second: serde_json::Value) -> serde_json::Value {
    json!({"kind": "split", "direction": "right", "ratio": 0.5, "first": first, "second": second})
}

fn two_tabs(parked: &[&str]) -> Layout {
    layout(json!({
        "workspaces": [
            {"id": "w0", "tabs": [
                {"id": "t0", "root": split(pane("a"), pane("b"))},
                {"id": "t1", "root": pane("c")},
            ]},
            {"id": "w1", "tabs": [{"id": "t2", "root": pane("d")}]},
        ],
        "parked": parked,
    }))
}

const CHROME: PaneChrome = PaneChrome {
    borders: PaneBordersConfig::Auto,
    gaps: false,
    outer_borders: true,
};

fn shown(session: &mut Session) -> Vec<String> {
    session
        .tiles(Rect::new(0, 0, 40, 10), CHROME)
        .tiles
        .into_iter()
        .map(|tile| tile.terminal)
        .collect()
}

#[test]
fn a_new_session_focuses_the_first_pane_of_the_first_tab() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    assert_eq!(session.focused(), Some("a"));
    assert_eq!(shown(&mut session), ["a", "b"]);
}

#[test]
fn focus_follows_its_terminal_when_it_moves_to_another_tab() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    assert!(session.focus("b"));
    session.update(layout(json!({"workspaces": [
        {"id": "w0", "tabs": [{"id": "t0", "root": pane("a")}]},
        {"id": "w1", "tabs": [{"id": "t2", "root": split(pane("d"), pane("b"))}]},
    ]})));
    assert_eq!(session.focused(), Some("b"));
    assert_eq!(shown(&mut session), ["d", "b"]);
}

#[test]
fn parking_the_focused_pane_moves_focus_to_the_tabs_next_shown_pane() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    session.update(two_tabs(&["a"]));
    assert_eq!(session.focused(), Some("b"));
    assert_eq!(shown(&mut session), ["b"]);
}

#[test]
fn a_fully_parked_tab_stays_focused_and_draws_nothing() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    session.update(two_tabs(&["a", "b"]));
    assert_eq!(session.focused(), None);
    assert!(shown(&mut session).is_empty());
    session.update(two_tabs(&["b"]));
    assert_eq!(session.focused(), Some("a"));
}

#[test]
fn a_removed_tab_falls_back_to_its_workspaces_first_tab() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    session.update(layout(json!({"workspaces": [
        {"id": "w0", "tabs": [{"id": "t1", "root": pane("c")}]},
    ]})));
    assert_eq!(session.focused(), Some("c"));
}

#[test]
fn a_session_without_workspaces_has_no_focus() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    session.update(Layout::default());
    assert_eq!(session.focused(), None);
    assert!(shown(&mut session).is_empty());
}

#[test]
fn focus_only_moves_to_a_shown_pane_of_the_focused_tab() {
    let mut session = Session::default();
    session.update(two_tabs(&["b"]));
    assert!(!session.focus("b"));
    assert!(!session.focus("c"));
    assert!(!session.focus("a"));
    assert_eq!(session.focused(), Some("a"));
}

#[test]
fn tiles_mark_the_focused_pane_and_inset_content_inside_borders() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    session.focus("b");
    let tiles = session.tiles(Rect::new(0, 0, 40, 10), CHROME);
    let focused: Vec<_> = tiles
        .tiles
        .iter()
        .map(|tile| (tile.terminal.as_str(), tile.info.is_focused))
        .collect();
    assert_eq!(focused, [("a", false), ("b", true)]);
    for tile in &tiles.tiles {
        assert_eq!(
            tile.info.inner_rect,
            crate::ui::pane_inner_rect(tile.info.rect, tile.info.borders)
        );
        assert!(tile.info.inner_rect.width < tile.info.rect.width);
    }
    assert_eq!(tiles.splits.len(), 1);
}

#[test]
fn pane_ids_stay_stable_while_a_terminal_is_owned() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    let a = session.id("a");
    session.update(two_tabs(&["a"]));
    assert_eq!(session.id("a"), a);
    session.update(layout(json!({"workspaces": [
        {"id": "w0", "tabs": [{"id": "t0", "root": pane("b")}]},
    ]})));
    assert_ne!(session.id("a"), a);
}
