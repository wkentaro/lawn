use ratatui::layout::Rect;
use serde_json::json;

use super::{Operation, PaneChrome, Request, Session};
use crate::config::PaneBordersConfig;
use crate::input::KeybindAction;
use crate::session_doc::{Destination, Direction, Layout};

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

const AREA: Rect = Rect::new(0, 0, 40, 10);

fn grid() -> Layout {
    layout(json!({"workspaces": [
        {"id": "w0", "tabs": [
            {"id": "t0", "root": {
                "kind": "split", "direction": "right", "ratio": 0.5,
                "first": {"kind": "split", "direction": "down", "ratio": 0.5,
                    "first": pane("a"), "second": pane("b")},
                "second": pane("c"),
            }},
        ]},
    ]}))
}

fn navigate(session: &mut Session, action: KeybindAction) -> bool {
    session.navigate(action, AREA, CHROME)
}

#[test]
fn directional_focus_moves_to_the_neighbouring_pane() {
    let mut session = Session::default();
    session.update(grid());
    assert!(navigate(&mut session, KeybindAction::FocusPaneDown));
    assert_eq!(session.focused(), Some("b"));
    assert!(navigate(&mut session, KeybindAction::FocusPaneRight));
    assert_eq!(session.focused(), Some("c"));
    assert!(!navigate(&mut session, KeybindAction::FocusPaneRight));
    assert!(navigate(&mut session, KeybindAction::FocusPaneLeft));
    assert_eq!(session.focused(), Some("a"));
    assert!(!navigate(&mut session, KeybindAction::FocusPaneUp));
}

#[test]
fn cycling_wraps_through_the_shown_panes_of_the_tab() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    assert!(navigate(&mut session, KeybindAction::CyclePaneNext));
    assert_eq!(session.focused(), Some("b"));
    assert!(navigate(&mut session, KeybindAction::CyclePaneNext));
    assert_eq!(session.focused(), Some("a"));
    assert!(navigate(&mut session, KeybindAction::CyclePanePrevious));
    assert_eq!(session.focused(), Some("b"));

    session.update(two_tabs(&["a"]));
    assert!(!navigate(&mut session, KeybindAction::CyclePaneNext));
}

#[test]
fn last_pane_returns_to_the_previous_pane_of_the_tab_while_it_is_shown() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    assert!(!navigate(&mut session, KeybindAction::LastPane));
    session.focus("b");
    assert!(navigate(&mut session, KeybindAction::LastPane));
    assert_eq!(session.focused(), Some("a"));
    assert!(navigate(&mut session, KeybindAction::LastPane));
    assert_eq!(session.focused(), Some("b"));

    session.update(two_tabs(&["a"]));
    assert!(!navigate(&mut session, KeybindAction::LastPane));
    assert_eq!(session.focused(), Some("b"));
}

#[test]
fn switching_tabs_wraps_and_returns_to_the_pane_focused_there() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    session.focus("b");
    assert!(navigate(&mut session, KeybindAction::NextTab));
    assert_eq!(session.focused(), Some("c"));
    assert!(navigate(&mut session, KeybindAction::NextTab));
    assert_eq!(session.focused(), Some("b"));
    assert!(navigate(&mut session, KeybindAction::PreviousTab));
    assert_eq!(session.focused(), Some("c"));
    assert!(navigate(&mut session, KeybindAction::SwitchTab(0)));
    assert_eq!(session.focused(), Some("b"));
    assert!(!navigate(&mut session, KeybindAction::SwitchTab(0)));
    assert!(!navigate(&mut session, KeybindAction::SwitchTab(5)));
}

#[test]
fn a_fully_parked_tab_can_be_selected() {
    let mut session = Session::default();
    session.update(two_tabs(&["c"]));
    assert!(navigate(&mut session, KeybindAction::SwitchTab(1)));
    assert_eq!(session.focused(), None);
    assert!(shown(&mut session).is_empty());
    assert!(navigate(&mut session, KeybindAction::SwitchTab(0)));
    assert_eq!(session.focused(), Some("a"));
}

#[test]
fn switching_workspaces_returns_to_the_tab_and_pane_focused_there() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    navigate(&mut session, KeybindAction::NextTab);
    assert!(navigate(&mut session, KeybindAction::NextWorkspace));
    assert_eq!(session.focused(), Some("d"));
    assert!(navigate(&mut session, KeybindAction::NextWorkspace));
    assert_eq!(session.focused(), Some("c"));
    assert!(navigate(&mut session, KeybindAction::PreviousWorkspace));
    assert_eq!(session.focused(), Some("d"));
    assert!(navigate(&mut session, KeybindAction::SwitchWorkspace(0)));
    assert_eq!(session.focused(), Some("c"));
    assert!(!navigate(&mut session, KeybindAction::SwitchWorkspace(2)));
}

#[test]
fn a_remembered_pane_that_left_falls_back_to_the_first_shown_one() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    session.focus("b");
    navigate(&mut session, KeybindAction::NextWorkspace);
    session.update(two_tabs(&["b"]));
    navigate(&mut session, KeybindAction::PreviousWorkspace);
    assert_eq!(session.focused(), Some("a"));
}

#[test]
fn zoom_shows_only_the_focused_pane_until_focus_moves() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    session.focus("b");
    assert!(navigate(&mut session, KeybindAction::Zoom));
    let tiles = session.tiles(AREA, CHROME);
    assert_eq!(tiles.tiles.len(), 1);
    assert_eq!(tiles.tiles[0].terminal, "b");
    assert_eq!(tiles.tiles[0].info.rect, AREA);
    assert_eq!(tiles.hidden, [(session.id("a"), "a".to_owned())]);

    assert!(navigate(&mut session, KeybindAction::Zoom));
    assert_eq!(shown(&mut session), ["a", "b"]);

    navigate(&mut session, KeybindAction::Zoom);
    assert!(navigate(&mut session, KeybindAction::FocusPaneLeft));
    assert_eq!(session.focused(), Some("a"));
    assert_eq!(shown(&mut session), ["a", "b"]);

    navigate(&mut session, KeybindAction::Zoom);
    navigate(&mut session, KeybindAction::NextTab);
    navigate(&mut session, KeybindAction::PreviousTab);
    assert_eq!(shown(&mut session), ["a", "b"]);
}

#[test]
fn zoom_survives_layout_updates_until_the_zoomed_pane_leaves() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    navigate(&mut session, KeybindAction::Zoom);
    session.update(two_tabs(&[]));
    assert_eq!(shown(&mut session), ["a"]);
    session.update(two_tabs(&["a"]));
    assert_eq!(session.focused(), Some("b"));
    session.update(two_tabs(&[]));
    assert_eq!(shown(&mut session), ["a", "b"]);
}

#[test]
fn zoom_needs_a_focused_pane() {
    let mut session = Session::default();
    session.update(two_tabs(&["a", "b"]));
    assert!(!navigate(&mut session, KeybindAction::Zoom));
}

fn with_new_tab() -> Layout {
    layout(json!({
        "workspaces": [
            {"id": "w0", "tabs": [
                {"id": "t0", "root": split(pane("a"), pane("b"))},
                {"id": "t1", "root": pane("c")},
            ]},
            {"id": "w1", "tabs": [
                {"id": "t2", "root": pane("d")},
                {"id": "t3", "root": pane("e")},
            ]},
        ],
    }))
}

#[test]
fn a_created_terminal_takes_focus_when_its_layout_arrives_after_the_start() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    assert!(!session.created("e".to_owned()));
    assert_eq!(session.focused(), Some("a"));
    session.update(with_new_tab());
    assert_eq!(session.focused(), Some("e"));
    session.update(with_new_tab());
    navigate(&mut session, KeybindAction::PreviousTab);
    session.update(with_new_tab());
    assert_eq!(session.focused(), Some("d"));
}

#[test]
fn a_created_terminal_takes_focus_when_its_layout_arrived_first() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    session.update(with_new_tab());
    assert_eq!(session.focused(), Some("a"));
    assert!(session.created("e".to_owned()));
    assert_eq!(session.focused(), Some("e"));
}

fn operation(session: &Session, action: KeybindAction, confirm_close: bool) -> Option<Operation> {
    match session.operation(action, confirm_close, None)? {
        Request::Run(operation) => Some(operation),
        Request::Confirm(_) => None,
    }
}

fn close(terminals: &[&str]) -> Option<Operation> {
    Some(Operation::Close(
        terminals
            .iter()
            .map(|terminal| (*terminal).to_owned())
            .collect(),
    ))
}

#[test]
fn starts_split_beside_the_focused_pane_or_open_a_tab_or_workspace() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    session.focus("b");
    assert_eq!(
        operation(&session, KeybindAction::SplitVertical, true),
        Some(Operation::Start(Destination::Split {
            beside: "b".to_owned(),
            direction: Direction::Right,
            ratio: 0.5,
        }))
    );
    assert_eq!(
        operation(&session, KeybindAction::SplitHorizontal, true),
        Some(Operation::Start(Destination::Split {
            beside: "b".to_owned(),
            direction: Direction::Down,
            ratio: 0.5,
        }))
    );
    assert_eq!(
        operation(&session, KeybindAction::NewTab, true),
        Some(Operation::Start(Destination::Tab {
            workspace: Some("w0".to_owned()),
            name: None,
        }))
    );
    assert_eq!(
        operation(&session, KeybindAction::ParkPane, true),
        Some(Operation::Park("b".to_owned()))
    );
    assert_eq!(operation(&session, KeybindAction::Zoom, true), None);

    session.update(Layout::default());
    assert_eq!(
        operation(&session, KeybindAction::NewWorkspace, true),
        Some(Operation::Start(Destination::Workspace { name: None }))
    );
    assert_eq!(
        operation(&session, KeybindAction::SplitVertical, true),
        None
    );
}

#[test]
fn closing_a_pane_asks_only_for_the_last_shown_pane_of_a_lone_tab() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    assert_eq!(
        operation(&session, KeybindAction::ClosePane, true),
        close(&["a"])
    );
    navigate(&mut session, KeybindAction::NextWorkspace);
    assert_eq!(operation(&session, KeybindAction::ClosePane, true), None);
    assert_eq!(
        operation(&session, KeybindAction::ClosePane, false),
        close(&["d"])
    );

    session.update(two_tabs(&["b"]));
    navigate(&mut session, KeybindAction::PreviousWorkspace);
    assert_eq!(
        operation(&session, KeybindAction::ClosePane, true),
        close(&["a"])
    );
}

#[test]
fn closing_a_tab_closes_its_shown_panes_and_asks_for_a_lone_tab() {
    let mut session = Session::default();
    session.update(two_tabs(&["b"]));
    assert_eq!(
        operation(&session, KeybindAction::CloseTab, true),
        close(&["a"])
    );
    navigate(&mut session, KeybindAction::NextWorkspace);
    assert_eq!(operation(&session, KeybindAction::CloseTab, true), None);
    assert_eq!(
        operation(&session, KeybindAction::CloseTab, false),
        close(&["d"])
    );

    session.update(two_tabs(&["c"]));
    navigate(&mut session, KeybindAction::PreviousWorkspace);
    navigate(&mut session, KeybindAction::SwitchTab(1));
    assert_eq!(operation(&session, KeybindAction::CloseTab, true), None);
}

#[test]
fn closing_a_workspace_closes_every_pane_and_always_asks_when_confirming() {
    let mut session = Session::default();
    session.update(two_tabs(&["b"]));
    assert_eq!(
        operation(&session, KeybindAction::CloseWorkspace, true),
        None
    );
    assert_eq!(
        operation(&session, KeybindAction::CloseWorkspace, false),
        close(&["a", "b", "c"])
    );
}

#[test]
fn a_tab_with_only_parked_panes_does_not_count_against_asking_to_close() {
    let mut session = Session::default();
    session.update(two_tabs(&["b", "c"]));
    assert_eq!(operation(&session, KeybindAction::ClosePane, true), None);
    assert_eq!(operation(&session, KeybindAction::CloseTab, true), None);
    assert_eq!(
        operation(&session, KeybindAction::CloseTab, false),
        close(&["a"])
    );
}

fn asked(session: &Session, action: KeybindAction) -> super::Closing {
    match session.operation(action, true, None) {
        Some(Request::Confirm(closing)) => closing,
        other => panic!("{action:?} did not ask: {other:?}"),
    }
}

#[test]
fn asking_to_close_names_the_workspace_and_counts_its_panes_and_the_parked_ones() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    assert_eq!(
        asked(&session, KeybindAction::CloseWorkspace).detail(),
        "workspace — 3 panes"
    );

    let mut named = two_tabs(&["b"]);
    named.workspaces[0].name = Some("main".to_owned());
    session.update(named);
    assert_eq!(
        asked(&session, KeybindAction::CloseWorkspace).detail(),
        "main — 3 panes, 1 parked"
    );

    navigate(&mut session, KeybindAction::NextWorkspace);
    assert_eq!(
        asked(&session, KeybindAction::ClosePane).detail(),
        "workspace — 1 pane"
    );

    session.update(layout(json!({
        "workspaces": [{"id": "w0", "tabs": [
            {"id": "t0", "root": split(pane("a"), pane("b"))},
        ]}],
        "parked": ["b"],
    })));
    assert_eq!(
        asked(&session, KeybindAction::CloseTab).detail(),
        "workspace — 2 panes, 1 parked"
    );
    session.update(layout(json!({
        "workspaces": [
            {"id": "w0", "tabs": [{"id": "t0", "root": pane("a")}]},
            {"id": "w1", "tabs": [{"id": "t1", "root": pane("b")}]},
        ],
        "parked": ["b"],
    })));
    navigate(&mut session, KeybindAction::NextWorkspace);
    assert_eq!(
        asked(&session, KeybindAction::CloseWorkspace).detail(),
        "workspace — 1 parked pane"
    );
}

#[test]
fn a_confirmed_close_takes_what_its_target_holds_when_accepted_and_nothing_once_it_is_gone() {
    let mut session = Session::default();
    session.update(two_tabs(&[]));
    let workspace = asked(&session, KeybindAction::CloseWorkspace);
    navigate(&mut session, KeybindAction::NextWorkspace);
    let last_pane = asked(&session, KeybindAction::ClosePane);
    let lone_tab = asked(&session, KeybindAction::CloseTab);

    let mut grown = two_tabs(&[]);
    grown.workspaces[0].tabs[1].root =
        serde_json::from_value(split(pane("c"), pane("e"))).expect("root");
    grown.workspaces[1].tabs[0].root =
        serde_json::from_value(split(pane("d"), pane("f"))).expect("root");
    session.update(grown);
    assert_eq!(session.confirmed(&workspace), close(&["a", "b", "c", "e"]));
    assert_eq!(session.confirmed(&last_pane), close(&["d"]));
    assert_eq!(session.confirmed(&lone_tab), close(&["d", "f"]));

    let mut moved = two_tabs(&[]);
    moved.workspaces[1].tabs[0].root = serde_json::from_value(pane("f")).expect("root");
    session.update(moved);
    assert_eq!(session.confirmed(&last_pane), None);
    assert_eq!(session.confirmed(&lone_tab), close(&["f"]));

    session.update(two_tabs(&["d"]));
    assert_eq!(session.confirmed(&last_pane), None);

    session.update(layout(
        json!({"workspaces": [{"id": "w0", "tabs": [{"id": "t0", "root": pane("a")}]}]}),
    ));
    assert_eq!(session.confirmed(&workspace), close(&["a"]));
    assert_eq!(session.confirmed(&lone_tab), None);

    session.update(layout(
        json!({"workspaces": [{"id": "w0", "tabs": [{"id": "t1", "root": pane("c")}]}]}),
    ));
    assert_eq!(session.confirmed(&workspace), close(&["c"]));
}
