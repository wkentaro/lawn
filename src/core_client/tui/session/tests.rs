use ratatui::layout::Rect;
use serde_json::json;

use super::{Operation, PaneChrome, Request, Session};
use crate::config::PaneBordersConfig;
use crate::input::KeybindAction;
use crate::session_doc::{Arrangement, Destination, Direction, Layout, Place};

fn layout(value: serde_json::Value) -> Layout {
    serde_json::from_value(value).expect("layout")
}

trait LaidOut {
    fn lay_out(&mut self, layout: Layout);
}

impl LaidOut for Session {
    fn lay_out(&mut self, layout: Layout) {
        let version = self.starts.version() + 1;
        self.update(layout, version);
    }
}

fn placed(session: &Session, terminal: &str) -> Option<(String, u64)> {
    let version = session.starts.version();
    let version = if session.layout.owns(terminal) {
        version
    } else {
        version + 1
    };
    Some((terminal.to_owned(), version))
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
    session.lay_out(two_tabs(&[]));
    assert_eq!(session.focused(), Some("a"));
    assert_eq!(shown(&mut session), ["a", "b"]);
}

#[test]
fn focus_follows_its_terminal_when_it_moves_to_another_tab() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
    assert!(session.focus("b"));
    session.lay_out(layout(json!({"workspaces": [
        {"id": "w0", "tabs": [{"id": "t0", "root": pane("a")}]},
        {"id": "w1", "tabs": [{"id": "t2", "root": split(pane("d"), pane("b"))}]},
    ]})));
    assert_eq!(session.focused(), Some("b"));
    assert_eq!(shown(&mut session), ["d", "b"]);
}

#[test]
fn parking_the_focused_pane_moves_focus_to_the_tabs_next_shown_pane() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
    session.lay_out(two_tabs(&["a"]));
    assert_eq!(session.focused(), Some("b"));
    assert_eq!(shown(&mut session), ["b"]);
}

#[test]
fn a_fully_parked_tab_stays_focused_and_draws_nothing() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
    session.lay_out(two_tabs(&["a", "b"]));
    assert_eq!(session.focused(), None);
    assert!(shown(&mut session).is_empty());
    session.lay_out(two_tabs(&["b"]));
    assert_eq!(session.focused(), Some("a"));
}

#[test]
fn a_removed_tab_falls_back_to_its_workspaces_first_tab() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
    session.lay_out(layout(json!({"workspaces": [
        {"id": "w0", "tabs": [{"id": "t1", "root": pane("c")}]},
    ]})));
    assert_eq!(session.focused(), Some("c"));
}

#[test]
fn a_session_without_workspaces_has_no_focus() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
    session.lay_out(Layout::default());
    assert_eq!(session.focused(), None);
    assert!(shown(&mut session).is_empty());
}

#[test]
fn focus_only_moves_to_a_shown_pane_of_the_focused_tab() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&["b"]));
    assert!(!session.focus("b"));
    assert!(!session.focus("c"));
    assert!(!session.focus("a"));
    assert_eq!(session.focused(), Some("a"));
}

#[test]
fn tiles_mark_the_focused_pane_and_inset_content_inside_borders() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
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
    session.lay_out(two_tabs(&[]));
    let a = session.id("a");
    session.lay_out(two_tabs(&["a"]));
    assert_eq!(session.id("a"), a);
    session.lay_out(layout(json!({"workspaces": [
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
    session.lay_out(grid());
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
    session.lay_out(two_tabs(&[]));
    assert!(navigate(&mut session, KeybindAction::CyclePaneNext));
    assert_eq!(session.focused(), Some("b"));
    assert!(navigate(&mut session, KeybindAction::CyclePaneNext));
    assert_eq!(session.focused(), Some("a"));
    assert!(navigate(&mut session, KeybindAction::CyclePanePrevious));
    assert_eq!(session.focused(), Some("b"));

    session.lay_out(two_tabs(&["a"]));
    assert!(!navigate(&mut session, KeybindAction::CyclePaneNext));
}

#[test]
fn last_pane_returns_to_the_previous_pane_of_the_tab_while_it_is_shown() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
    assert!(!navigate(&mut session, KeybindAction::LastPane));
    session.focus("b");
    assert!(navigate(&mut session, KeybindAction::LastPane));
    assert_eq!(session.focused(), Some("a"));
    assert!(navigate(&mut session, KeybindAction::LastPane));
    assert_eq!(session.focused(), Some("b"));

    session.lay_out(two_tabs(&["a"]));
    assert!(!navigate(&mut session, KeybindAction::LastPane));
    assert_eq!(session.focused(), Some("b"));
}

#[test]
fn switching_tabs_wraps_and_returns_to_the_pane_focused_there() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
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
    session.lay_out(two_tabs(&["c"]));
    assert!(navigate(&mut session, KeybindAction::SwitchTab(1)));
    assert_eq!(session.focused(), None);
    assert!(shown(&mut session).is_empty());
    assert!(navigate(&mut session, KeybindAction::SwitchTab(0)));
    assert_eq!(session.focused(), Some("a"));
}

#[test]
fn switching_workspaces_returns_to_the_tab_and_pane_focused_there() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
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
    session.lay_out(two_tabs(&[]));
    session.focus("b");
    navigate(&mut session, KeybindAction::NextWorkspace);
    session.lay_out(two_tabs(&["b"]));
    navigate(&mut session, KeybindAction::PreviousWorkspace);
    assert_eq!(session.focused(), Some("a"));
}

#[test]
fn zoom_shows_only_the_focused_pane_until_focus_moves() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
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
    session.lay_out(two_tabs(&[]));
    navigate(&mut session, KeybindAction::Zoom);
    session.lay_out(two_tabs(&[]));
    assert_eq!(shown(&mut session), ["a"]);
    session.lay_out(two_tabs(&["a"]));
    assert_eq!(session.focused(), Some("b"));
    session.lay_out(two_tabs(&[]));
    assert_eq!(shown(&mut session), ["a", "b"]);
}

#[test]
fn zoom_needs_a_focused_pane() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&["a", "b"]));
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

fn start(session: &mut Session) -> u64 {
    match operation(session, KeybindAction::NewTab, true) {
        Some(Operation::Start(request, _)) => request,
        other => panic!("not a start: {other:?}"),
    }
}

#[test]
fn a_created_terminal_takes_focus_when_its_layout_arrives_after_the_start() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
    let request = start(&mut session);
    assert!(!session.started(request, placed(&session, "e")).0);
    assert_eq!(session.focused(), Some("a"));
    session.lay_out(with_new_tab());
    assert_eq!(session.focused(), Some("e"));
    session.lay_out(with_new_tab());
    navigate(&mut session, KeybindAction::PreviousTab);
    session.lay_out(with_new_tab());
    assert_eq!(session.focused(), Some("d"));
}

#[test]
fn a_created_terminal_takes_focus_when_its_layout_arrived_first() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
    let request = start(&mut session);
    session.lay_out(with_new_tab());
    assert_eq!(session.focused(), Some("a"));
    assert!(session.started(request, placed(&session, "e")).0);
    assert_eq!(session.focused(), Some("e"));
}

fn orders<T: Copy>(arrivals: &[T]) -> Vec<Vec<T>> {
    if arrivals.is_empty() {
        return vec![Vec::new()];
    }
    (0..arrivals.len())
        .flat_map(|first| {
            let mut rest = arrivals.to_vec();
            let arrival = rest.remove(first);
            orders(&rest).into_iter().map(move |mut order| {
                order.insert(0, arrival);
                order
            })
        })
        .collect()
}

fn beside_a(panes: &[&str]) -> Layout {
    let root = panes
        .iter()
        .fold(pane("a"), |root, terminal| split(root, pane(terminal)));
    layout(json!({"workspaces": [{"id": "w0", "tabs": [{"id": "t0", "root": root}]}]}))
}

#[derive(Clone, Copy, Debug)]
enum Arrival {
    Started(u64, Option<&'static str>),
    Shown(&'static [&'static str]),
}

struct Base {
    focus: &'static str,
    shown: &'static [&'static str],
    unanswered: Vec<u64>,
    waiting: Option<(u64, &'static str)>,
}

impl Base {
    fn arrive(&mut self, arrival: Arrival) {
        match arrival {
            Arrival::Started(request, result) => {
                self.unanswered.retain(|unanswered| *unanswered != request);
                if let Some(terminal) = result {
                    self.waiting = Some((request, terminal));
                }
            }
            Arrival::Shown(panes) => {
                self.shown = panes;
                if !panes.contains(&self.focus) {
                    self.focus = "a";
                }
            }
        }
        if let Some((_, terminal)) = self
            .waiting
            .filter(|(_, terminal)| self.shown.contains(terminal))
        {
            self.focus = terminal;
            self.waiting = None;
        }
    }

    fn tracked(&self) -> Vec<(u64, Option<&str>)> {
        let mut tracked: Vec<_> = self
            .unanswered
            .iter()
            .map(|request| (*request, None))
            .chain(
                self.waiting
                    .map(|(request, terminal)| (request, Some(terminal))),
            )
            .collect();
        tracked.sort_unstable();
        tracked
    }
}

#[test]
fn the_latest_successful_start_result_takes_focus_once_its_pane_appears_in_every_arrival_order() {
    let mut covered = 0;
    for second in [Some("p2"), None] {
        for order in orders(&[
            Arrival::Started(1, Some("p1")),
            Arrival::Started(2, second),
            Arrival::Shown(&["p1"]),
            Arrival::Shown(&["p2"]),
            Arrival::Shown(&["p1", "p2"]),
        ]) {
            let mut session = Session::default();
            session.lay_out(beside_a(&[]));
            assert_eq!([start(&mut session), start(&mut session)], [1, 2]);
            let mut base = Base {
                focus: "a",
                shown: &[],
                unanswered: vec![1, 2],
                waiting: None,
            };
            for (step, arrival) in order.iter().enumerate() {
                match *arrival {
                    Arrival::Started(request, terminal) => {
                        session.started(
                            request,
                            terminal.and_then(|terminal| placed(&session, terminal)),
                        );
                    }
                    Arrival::Shown(panes) => session.lay_out(beside_a(panes)),
                }
                base.arrive(*arrival);
                assert_eq!(session.focused(), Some(base.focus), "{order:?} at {step}");
                assert_eq!(
                    session.starts.tracked(),
                    base.tracked(),
                    "{order:?} at {step}"
                );
            }
            covered += 1;
        }
    }
    assert_eq!(covered, 240);
}

#[test]
fn a_start_result_waits_for_its_pane_to_come_back_after_it_left() {
    let mut session = Session::default();
    session.lay_out(beside_a(&[]));
    navigate(&mut session, KeybindAction::Zoom);
    let request = start(&mut session);
    session.lay_out(beside_a(&["p"]));
    session.lay_out(beside_a(&[]));

    assert!(!session.started(request, placed(&session, "p")).0);
    session.lay_out(beside_a(&["p"]));

    assert_eq!(session.focused(), Some("p"));
    assert!(session.starts.tracked().is_empty());
}

#[test]
fn a_failed_start_is_forgotten() {
    let mut session = Session::default();
    session.lay_out(beside_a(&[]));
    let request = start(&mut session);
    session.lay_out(beside_a(&["p1"]));

    assert!(!session.started(request, None).0);
    assert!(session.starts.tracked().is_empty());
    session.lay_out(beside_a(&["p1"]));
    assert_eq!(session.focused(), Some("a"));
}

fn beside_b(commands: &[&str]) -> Layout {
    let beside = commands
        .iter()
        .rev()
        .fold(None, |nested, command| {
            Some(match nested {
                Some(nested) => split(pane(command), nested),
                None => pane(command),
            })
        })
        .map_or_else(|| pane("b"), |nested| split(pane("b"), nested));
    layout(json!({
        "workspaces": [
            {"id": "w0", "tabs": [
                {"id": "t0", "root": split(pane("a"), beside)},
                {"id": "t1", "root": pane("c")},
            ]},
            {"id": "w1", "tabs": [{"id": "t2", "root": pane("d")}]},
        ],
    }))
}

fn command(session: &mut Session) -> u64 {
    session
        .overlay(Vec::new(), Vec::new(), None)
        .expect("a focused pane")
        .request
}

fn nested_from_b(zoomed: bool) -> Session {
    let mut session = opened_from_b(zoomed);
    for (index, name) in ["f", "g"].into_iter().enumerate() {
        let request = command(&mut session);
        session.lay_out(beside_b(&["e", "f", "g"][..index + 2]));
        assert!(session.started(request, placed(&session, name)).0);
        assert_eq!(session.focused(), Some(name));
    }
    session
}

fn opened_from_b(zoomed: bool) -> Session {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
    assert!(session.focus("b"));
    if zoomed {
        navigate(&mut session, KeybindAction::Zoom);
    }
    let request = command(&mut session);
    session.lay_out(beside_b(&["e"]));
    assert!(session.started(request, placed(&session, "e")).0);
    session
}

#[test]
fn a_pane_command_takes_focus_zoomed_once_it_starts() {
    let session = opened_from_b(false);
    assert_eq!(session.focused(), Some("e"));
    assert!(session.zoomed());
}

#[test]
fn a_pane_command_that_exits_returns_focus_and_zoom_to_the_pane_it_opened_from() {
    for zoomed in [false, true] {
        let mut session = opened_from_b(zoomed);
        session.lay_out(two_tabs(&[]));
        assert_eq!(session.focused(), Some("b"), "zoomed {zoomed}");
        assert_eq!(session.zoomed(), zoomed);
    }
}

#[test]
fn nested_pane_commands_that_exit_together_return_to_the_pane_the_first_opened_from() {
    for zoomed in [false, true] {
        for _ in 0..32 {
            let mut session = nested_from_b(zoomed);

            session.lay_out(beside_b(&[]));

            assert_eq!(session.focused(), Some("b"), "zoomed {zoomed}");
            assert_eq!(session.zoomed(), zoomed);
        }
    }
}

#[test]
fn nested_pane_commands_that_exit_one_by_one_return_to_the_nearest_pane_still_open() {
    let orders = [
        ["e", "f", "g"],
        ["e", "g", "f"],
        ["f", "e", "g"],
        ["f", "g", "e"],
        ["g", "e", "f"],
        ["g", "f", "e"],
    ];
    for zoomed in [false, true] {
        for order in orders {
            let mut session = nested_from_b(zoomed);
            let mut open = vec!["e", "f", "g"];
            for exited in order {
                open.retain(|command| *command != exited);
                session.lay_out(beside_b(&open));
                let nearest = open.last().copied().unwrap_or("b");
                assert_eq!(session.focused(), Some(nearest), "{order:?} after {exited}");
                assert_eq!(
                    session.zoomed(),
                    zoomed || !open.is_empty(),
                    "{order:?} after {exited}"
                );
            }
        }
    }
}

#[test]
fn a_pane_command_registered_after_its_opener_exited_returns_to_the_nearest_pane_still_open() {
    for zoomed in [false, true] {
        let mut session = opened_from_b(zoomed);
        let request = command(&mut session);
        session.lay_out(beside_b(&["e", "f"]));
        session.lay_out(beside_b(&["f"]));
        assert_eq!(session.focused(), Some("b"));

        assert!(session.started(request, placed(&session, "f")).0);
        assert_eq!(session.focused(), Some("f"));
        assert!(session.zoomed());

        session.lay_out(beside_b(&[]));
        assert_eq!(session.focused(), Some("b"), "zoomed {zoomed}");
        assert_eq!(session.zoomed(), zoomed);
    }
}

#[test]
fn a_pane_command_that_exits_in_a_tab_left_behind_returns_that_tab_to_the_pane_it_opened_from() {
    let mut session = opened_from_b(false);
    navigate(&mut session, KeybindAction::NextTab);
    session.lay_out(two_tabs(&[]));
    assert_eq!(session.focused(), Some("c"));
    navigate(&mut session, KeybindAction::PreviousTab);
    assert_eq!(session.focused(), Some("b"));
}

#[test]
fn a_command_opened_from_a_command_before_its_start_result_returns_to_the_first_opener() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
    assert!(session.focus("b"));
    navigate(&mut session, KeybindAction::Zoom);
    let e = command(&mut session);
    session.lay_out(beside_b(&["e"]));
    assert!(session.focus("e"));
    let f = command(&mut session);
    session.lay_out(beside_b(&["e", "f"]));
    session.started(e, placed(&session, "e"));
    session.lay_out(beside_b(&["f"]));
    session.started(f, placed(&session, "f"));
    assert_eq!(session.focused(), Some("f"));

    session.lay_out(beside_b(&[]));

    assert_eq!(session.focused(), Some("b"));
    assert!(session.zoomed());
}

#[test]
fn a_command_whose_result_arrives_before_its_layout_keeps_its_return_link() {
    let mut session = Session::default();
    session.update(two_tabs(&[]), 1);
    assert!(session.focus("b"));
    navigate(&mut session, KeybindAction::Zoom);
    let e = command(&mut session);
    let f = command(&mut session);
    session.started(f, Some(("f".to_owned(), 3)));
    session.started(e, Some(("e".to_owned(), 2)));
    session.update(beside_b(&["e"]), 2);
    session.update(beside_b(&["e", "f"]), 3);
    assert!(session.focus("f"));

    session.update(beside_b(&["e"]), 4);

    assert_eq!(session.focused(), Some("b"));
    assert!(session.zoomed());
}

#[test]
fn a_layout_older_than_a_command_result_keeps_the_command_tracked() {
    let mut session = Session::default();
    session.update(two_tabs(&[]), 1);
    assert!(session.focus("b"));
    let e = command(&mut session);
    session.started(e, Some(("e".to_owned(), 3)));

    session.update(two_tabs(&[]), 2);
    assert_eq!(session.starts.tracked(), [(e, Some("e"))]);
    session.update(beside_b(&["e"]), 3);

    assert_eq!(session.focused(), Some("e"));
    assert!(session.zoomed());
}

#[test]
fn a_command_closed_before_any_layout_owned_it_is_forgotten_in_either_order() {
    for result_first in [true, false] {
        let mut session = Session::default();
        session.update(two_tabs(&[]), 1);
        assert!(session.focus("b"));
        let e = command(&mut session);
        if result_first {
            session.started(e, Some(("e".to_owned(), 2)));
            session.update(two_tabs(&[]), 3);
        } else {
            session.update(two_tabs(&[]), 3);
            session.started(e, Some(("e".to_owned(), 2)));
        }

        assert!(
            session.starts.tracked().is_empty(),
            "result first {result_first}"
        );
        assert_eq!(session.focused(), Some("b"));
    }
}

#[test]
fn a_command_whose_opener_is_gone_falls_back_to_the_tabs_first_pane() {
    let mut session = opened_from_b(true);
    session.lay_out(layout(json!({
        "workspaces": [{"id": "w0", "tabs": [{"id": "t0", "root": split(pane("a"), pane("e"))}]}],
    })));
    assert_eq!(session.focused(), Some("e"));

    session.lay_out(layout(json!({
        "workspaces": [{"id": "w0", "tabs": [{"id": "t0", "root": pane("a")}]}],
    })));

    assert_eq!(session.focused(), Some("a"));
    assert!(!session.zoomed());
}

#[test]
fn a_command_whose_opener_moved_to_another_tab_returns_without_its_zoom() {
    let mut session = opened_from_b(true);
    session.lay_out(layout(json!({
        "workspaces": [{"id": "w0", "tabs": [
            {"id": "t0", "root": pane("e")},
            {"id": "t1", "root": pane("b")},
        ]}],
    })));
    assert_eq!(session.focused(), Some("e"));

    session.lay_out(layout(json!({
        "workspaces": [{"id": "w0", "tabs": [{"id": "t1", "root": pane("b")}]}],
    })));

    assert_eq!(session.focused(), Some("b"));
    assert!(!session.zoomed());
}

#[test]
fn moving_focus_by_hand_after_asking_for_a_command_keeps_it_from_taking_focus() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
    assert!(session.focus("b"));
    navigate(&mut session, KeybindAction::Zoom);
    let e = command(&mut session);
    navigate(&mut session, KeybindAction::NextTab);
    assert_eq!(session.focused(), Some("c"));

    session.lay_out(beside_b(&["e"]));
    assert!(!session.started(e, placed(&session, "e")).0);
    session.lay_out(beside_b(&["e"]));

    assert_eq!(session.focused(), Some("c"));
    assert!(!session.zoomed());
    navigate(&mut session, KeybindAction::PreviousTab);
    assert!(session.focus("e"));
    session.lay_out(beside_b(&[]));
    assert_eq!(session.focused(), Some("b"));
    assert!(session.zoomed());
}

#[test]
fn moving_focus_by_hand_after_asking_for_a_start_keeps_it_from_taking_focus() {
    for result_first in [true, false] {
        let mut session = Session::default();
        session.lay_out(two_tabs(&[]));
        let request = start(&mut session);
        navigate(&mut session, KeybindAction::NextTab);
        assert_eq!(session.focused(), Some("c"));

        if result_first {
            assert!(!session.started(request, placed(&session, "e")).0);
            session.lay_out(with_new_tab());
        } else {
            session.lay_out(with_new_tab());
            assert!(!session.started(request, placed(&session, "e")).0);
        }

        assert_eq!(session.focused(), Some("c"), "result first {result_first}");
        assert!(session.starts.tracked().is_empty());
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    Result(usize),
    Shown(usize),
    Exit(usize),
}

const COMMANDS: [&str; 3] = ["e", "f", "g"];

fn opened(openers: &[Option<usize>], done: &[Step], command: usize) -> bool {
    openers[command].is_none_or(|opener| done.contains(&Step::Shown(opener)))
}

fn step_orders(
    openers: &[Option<usize>],
    exits_early: bool,
    done: &mut Vec<Step>,
    orders: &mut Vec<Vec<Step>>,
) {
    let steps = (0..openers.len()).flat_map(|command| {
        [
            Step::Result(command),
            Step::Shown(command),
            Step::Exit(command),
        ]
    });
    let mut complete = true;
    for step in steps {
        if done.contains(&step) {
            continue;
        }
        complete = false;
        let ready = match step {
            Step::Result(command) | Step::Shown(command) => opened(openers, done, command),
            Step::Exit(command) => {
                done.contains(&Step::Shown(command))
                    && (exits_early || done.contains(&Step::Result(command)))
            }
        };
        if ready {
            done.push(step);
            step_orders(openers, exits_early, done, orders);
            done.pop();
        }
    }
    if complete {
        orders.push(done.clone());
    }
}

fn skippable(openers: &[Option<usize>], order: &[Step]) -> Vec<usize> {
    let documents: Vec<Step> = order
        .iter()
        .copied()
        .filter(|step| !matches!(step, Step::Result(_)))
        .collect();
    documents
        .windows(2)
        .filter_map(|pair| match *pair {
            [Step::Shown(command), Step::Exit(exited)]
                if command == exited && !openers.contains(&Some(command)) =>
            {
                Some(command)
            }
            _ => None,
        })
        .collect()
}

fn ask(session: &mut Session, opener: Option<usize>) -> (u64, (Option<usize>, bool)) {
    assert_eq!(
        session.focused(),
        Some(opener.map_or("b", |opener| COMMANDS[opener]))
    );
    let link = (opener, session.zoomed());
    (command(session), link)
}

fn returns_to_the_nearest_living_opener(
    layouts: &[Layout],
    openers: &[Option<usize>],
    zoomed: bool,
    order: &[Step],
    skipped: &[usize],
) {
    let mut version = 1;
    let mut placed_at = [0; 3];
    for step in order {
        match *step {
            Step::Shown(command) => {
                version += 1;
                placed_at[command] = version;
            }
            Step::Exit(_) => version += 1,
            Step::Result(_) => {}
        }
    }
    let mut session = Session::default();
    session.update(layouts[0].clone(), 1);
    assert!(session.focus("b"));
    if zoomed {
        navigate(&mut session, KeybindAction::Zoom);
    }
    let mut requests = [0; 3];
    let mut links = [(None, false); 3];
    for command in (0..openers.len()).filter(|command| openers[*command].is_none()) {
        (requests[command], links[command]) = ask(&mut session, None);
    }
    let (mut answered, mut shown, mut exited) = ([false; 3], [false; 3], [false; 3]);
    let living = |shown: &[bool; 3], exited: &[bool; 3]| {
        let living = (0..3)
            .filter(|command| shown[*command] && !exited[*command])
            .fold(0, |living, command| living | 1 << command);
        layouts[living].clone()
    };
    let mut version = 1;
    for step in order {
        match *step {
            Step::Result(command) => {
                answered[command] = true;
                let placed = (COMMANDS[command].to_owned(), placed_at[command]);
                session.started(requests[command], Some(placed));
            }
            Step::Shown(command) => {
                version += 1;
                shown[command] = true;
                if skipped.contains(&command) {
                    continue;
                }
                session.update(living(&shown, &exited), version);
                for opened in (0..openers.len()).filter(|opened| openers[*opened] == Some(command))
                {
                    session.focus(COMMANDS[command]);
                    (requests[opened], links[opened]) = ask(&mut session, Some(command));
                }
            }
            Step::Exit(command) => {
                version += 1;
                let focused = session.focused() == Some(COMMANDS[command]);
                exited[command] = true;
                session.update(living(&shown, &exited), version);
                if focused {
                    let mut back = answered[command].then_some(links[command]);
                    while let Some((Some(gone), _)) =
                        back.filter(|(opener, _)| opener.is_some_and(|opener| exited[opener]))
                    {
                        back = answered[gone].then_some(links[gone]);
                    }
                    let (expected, zoom) = match back {
                        Some((opener, zoom)) => {
                            (opener.map_or("b", |opener| COMMANDS[opener]), zoom)
                        }
                        None => ("a", false),
                    };
                    assert_eq!(
                        session.focused(),
                        Some(expected),
                        "{order:?} {skipped:?} at {step:?}"
                    );
                    assert_eq!(session.zoomed(), zoom, "{order:?} {skipped:?} at {step:?}");
                }
            }
        }
    }
    assert!(session.starts.tracked().is_empty(), "{order:?} {skipped:?}");
}

#[test]
fn a_command_pane_that_exits_while_focused_returns_to_its_nearest_living_opener_in_every_arrival_order(
) {
    let shapes = [
        (&[None, None][..], true, &[false, true][..]),
        (&[None, Some(0)], true, &[false, true]),
        (&[None, Some(0), Some(1)], true, &[true]),
        (&[None, Some(0), Some(0)], false, &[true]),
    ];
    let layouts: Vec<Layout> = (0..8)
        .map(|living| {
            let commands: Vec<&str> = (0..3)
                .filter(|command| living & 1 << command != 0)
                .map(|command| COMMANDS[command])
                .collect();
            beside_b(&commands)
        })
        .collect();
    let mut covered = 0;
    for (openers, exits_early, zooms) in shapes {
        let mut orders = Vec::new();
        step_orders(openers, exits_early, &mut Vec::new(), &mut orders);
        for zoomed in zooms {
            for order in &orders {
                let skippable = skippable(openers, order);
                for skipped in [&[][..], &skippable[..]] {
                    returns_to_the_nearest_living_opener(
                        &layouts, openers, *zoomed, order, skipped,
                    );
                    covered += 1;
                    if skippable.is_empty() {
                        break;
                    }
                }
            }
        }
    }
    assert_eq!(covered, 600 + 240 + 6930 + 4292);
}

#[test]
fn a_new_version_of_the_same_layout_that_finds_a_command_gone_still_settles_it() {
    let mut session = opened_from_b(true);
    let version = session.starts.version();
    let f = command(&mut session);
    session.started(f, Some(("f".to_owned(), version + 1)));
    assert_eq!(session.starts.tracked().len(), 2);

    let closed = session.update(beside_b(&["e"]), version + 2);

    assert_eq!(closed, Some(vec!["f".to_owned()]));
    assert_eq!(session.starts.tracked().len(), 1);
    assert_eq!(session.focused(), Some("e"));
    session.update(two_tabs(&[]), version + 3);
    assert_eq!(session.focused(), Some("b"));
    assert!(session.zoomed());
}

#[test]
fn a_new_version_of_an_unchanged_layout_does_not_recheck_command_panes_it_owns() {
    let mut session = nested_from_b(true);
    let version = session.starts.version();
    let checks = std::cell::Cell::new(0);

    let settled = session.starts.laid_out(
        false,
        |_| {
            checks.set(checks.get() + 1);
            false
        },
        |_| None::<()>,
        version + 1,
    );

    assert_eq!(checks.get(), 0);
    assert!(settled.is_empty());
    session.update(beside_b(&["e", "f"]), version + 2);
    assert_eq!(session.focused(), Some("f"));
    assert!(session.zoomed());
}

#[test]
fn an_update_names_a_pane_command_only_once_it_closes() {
    let mut session = Session::default();
    session.update(two_tabs(&[]), 1);
    assert!(session.focus("b"));
    let request = command(&mut session);
    session.started(request, Some(("e".to_owned(), 3)));

    assert!(closed_by(&mut session, two_tabs(&[]), 2).is_empty());
    assert!(closed_by(&mut session, beside_b(&["e"]), 3).is_empty());
    assert_eq!(closed_by(&mut session, two_tabs(&[]), 4), ["e"]);
    assert!(closed_by(&mut session, two_tabs(&[]), 5).is_empty());
}

#[test]
fn an_update_names_each_nested_pane_command_that_closes_with_it() {
    let mut session = nested_from_b(false);
    let version = session.starts.version();

    assert_eq!(
        closed_by(&mut session, beside_b(&["e", "g"]), version + 1),
        ["f"]
    );

    let mut closed = closed_by(&mut session, beside_b(&[]), version + 2);
    closed.sort();
    assert_eq!(closed, ["e", "g"]);
}

#[test]
fn a_start_result_names_a_pane_command_that_closed_before_it_arrived() {
    let mut session = Session::default();
    session.update(two_tabs(&[]), 1);
    assert!(session.focus("b"));
    let request = command(&mut session);
    assert!(closed_by(&mut session, beside_b(&["e"]), 2).is_empty());
    assert!(closed_by(&mut session, two_tabs(&[]), 3).is_empty());

    let (moved, closed) = session.started(request, Some(("e".to_owned(), 2)));

    assert!(!moved);
    assert_eq!(closed, ["e"]);
    assert!(closed_by(&mut session, two_tabs(&[]), 4).is_empty());
    assert!(session.starts.tracked().is_empty());
}

#[test]
fn a_pane_command_that_ended_before_any_layout_owned_it_is_named_once_in_either_order() {
    for result_first in [true, false] {
        let mut session = Session::default();
        session.update(two_tabs(&[]), 1);
        assert!(session.focus("b"));
        let request = command(&mut session);
        session.heard(super::Report::Ended("e".to_owned()), |_| false);

        let closed = if result_first {
            let (_, early) = session.started(request, Some(("e".to_owned(), 2)));
            assert!(early.is_empty());
            closed_by(&mut session, two_tabs(&[]), 3)
        } else {
            assert!(closed_by(&mut session, two_tabs(&[]), 3).is_empty());
            session.started(request, Some(("e".to_owned(), 2))).1
        };

        assert_eq!(closed, ["e"], "result first {result_first}");
        assert!(closed_by(&mut session, two_tabs(&[]), 4).is_empty());
        assert!(session.starts.tracked().is_empty());
    }
}

fn closed_by(session: &mut Session, layout: Layout, version: u64) -> Vec<String> {
    session.update(layout, version).unwrap_or_default()
}

fn operation(
    session: &mut Session,
    action: KeybindAction,
    confirm_close: bool,
) -> Option<Operation> {
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
    session.lay_out(two_tabs(&[]));
    session.focus("b");
    assert_eq!(
        operation(&mut session, KeybindAction::SplitVertical, true),
        Some(Operation::Start(
            1,
            Destination::Split {
                beside: "b".to_owned(),
                direction: Direction::Right,
                ratio: 0.5,
            }
        ))
    );
    assert_eq!(
        operation(&mut session, KeybindAction::SplitHorizontal, true),
        Some(Operation::Start(
            2,
            Destination::Split {
                beside: "b".to_owned(),
                direction: Direction::Down,
                ratio: 0.5,
            }
        ))
    );
    assert_eq!(
        operation(&mut session, KeybindAction::NewTab, true),
        Some(Operation::Start(
            3,
            Destination::Tab {
                workspace: Some("w0".to_owned()),
                name: None,
            }
        ))
    );
    assert_eq!(
        operation(&mut session, KeybindAction::ParkPane, true),
        Some(Operation::Park("b".to_owned()))
    );
    assert_eq!(operation(&mut session, KeybindAction::Zoom, true), None);

    session.lay_out(Layout::default());
    assert_eq!(
        operation(&mut session, KeybindAction::NewWorkspace, true),
        Some(Operation::Start(4, Destination::Workspace { name: None }))
    );
    assert_eq!(
        operation(&mut session, KeybindAction::SplitVertical, true),
        None
    );
}

#[test]
fn closing_a_pane_asks_only_for_the_last_shown_pane_of_a_lone_tab() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
    assert_eq!(
        operation(&mut session, KeybindAction::ClosePane, true),
        close(&["a"])
    );
    navigate(&mut session, KeybindAction::NextWorkspace);
    assert_eq!(
        operation(&mut session, KeybindAction::ClosePane, true),
        None
    );
    assert_eq!(
        operation(&mut session, KeybindAction::ClosePane, false),
        close(&["d"])
    );

    session.lay_out(two_tabs(&["b"]));
    navigate(&mut session, KeybindAction::PreviousWorkspace);
    assert_eq!(
        operation(&mut session, KeybindAction::ClosePane, true),
        close(&["a"])
    );
}

#[test]
fn closing_a_tab_closes_its_shown_panes_and_asks_for_a_lone_tab() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&["b"]));
    assert_eq!(
        operation(&mut session, KeybindAction::CloseTab, true),
        close(&["a"])
    );
    navigate(&mut session, KeybindAction::NextWorkspace);
    assert_eq!(operation(&mut session, KeybindAction::CloseTab, true), None);
    assert_eq!(
        operation(&mut session, KeybindAction::CloseTab, false),
        close(&["d"])
    );

    session.lay_out(two_tabs(&["c"]));
    navigate(&mut session, KeybindAction::PreviousWorkspace);
    navigate(&mut session, KeybindAction::SwitchTab(1));
    assert_eq!(operation(&mut session, KeybindAction::CloseTab, true), None);
}

#[test]
fn closing_a_workspace_closes_every_pane_and_always_asks_when_confirming() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&["b"]));
    assert_eq!(
        operation(&mut session, KeybindAction::CloseWorkspace, true),
        None
    );
    assert_eq!(
        operation(&mut session, KeybindAction::CloseWorkspace, false),
        close(&["a", "b", "c"])
    );
}

#[test]
fn a_tab_with_only_parked_panes_does_not_count_against_asking_to_close() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&["b", "c"]));
    assert_eq!(
        operation(&mut session, KeybindAction::ClosePane, true),
        None
    );
    assert_eq!(operation(&mut session, KeybindAction::CloseTab, true), None);
    assert_eq!(
        operation(&mut session, KeybindAction::CloseTab, false),
        close(&["a"])
    );
}

fn asked(session: &mut Session, action: KeybindAction) -> super::Closing {
    match session.operation(action, true, None) {
        Some(Request::Confirm(closing)) => closing,
        other => panic!("{action:?} did not ask: {other:?}"),
    }
}

#[test]
fn asking_to_close_names_the_workspace_and_counts_its_panes_and_the_parked_ones() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
    assert_eq!(
        asked(&mut session, KeybindAction::CloseWorkspace).detail(),
        "workspace — 3 panes"
    );

    let mut named = two_tabs(&["b"]);
    named.workspaces[0].name = Some("main".to_owned());
    session.lay_out(named);
    assert_eq!(
        asked(&mut session, KeybindAction::CloseWorkspace).detail(),
        "main — 3 panes, 1 parked"
    );

    navigate(&mut session, KeybindAction::NextWorkspace);
    assert_eq!(
        asked(&mut session, KeybindAction::ClosePane).detail(),
        "workspace — 1 pane"
    );

    session.lay_out(layout(json!({
        "workspaces": [{"id": "w0", "tabs": [
            {"id": "t0", "root": split(pane("a"), pane("b"))},
        ]}],
        "parked": ["b"],
    })));
    assert_eq!(
        asked(&mut session, KeybindAction::CloseTab).detail(),
        "workspace — 2 panes, 1 parked"
    );
    session.lay_out(layout(json!({
        "workspaces": [
            {"id": "w0", "tabs": [{"id": "t0", "root": pane("a")}]},
            {"id": "w1", "tabs": [{"id": "t1", "root": pane("b")}]},
        ],
        "parked": ["b"],
    })));
    navigate(&mut session, KeybindAction::NextWorkspace);
    assert_eq!(
        asked(&mut session, KeybindAction::CloseWorkspace).detail(),
        "workspace — 1 parked pane"
    );
}

#[test]
fn a_confirmed_close_takes_what_its_target_holds_when_accepted_and_nothing_once_it_is_gone() {
    let mut session = Session::default();
    session.lay_out(two_tabs(&[]));
    let workspace = asked(&mut session, KeybindAction::CloseWorkspace);
    navigate(&mut session, KeybindAction::NextWorkspace);
    let last_pane = asked(&mut session, KeybindAction::ClosePane);
    let lone_tab = asked(&mut session, KeybindAction::CloseTab);

    let mut grown = two_tabs(&[]);
    grown.workspaces[0].tabs[1].root =
        serde_json::from_value(split(pane("c"), pane("e"))).expect("root");
    grown.workspaces[1].tabs[0].root =
        serde_json::from_value(split(pane("d"), pane("f"))).expect("root");
    session.lay_out(grown);
    assert_eq!(session.confirmed(&workspace), close(&["a", "b", "c", "e"]));
    assert_eq!(session.confirmed(&last_pane), close(&["d"]));
    assert_eq!(session.confirmed(&lone_tab), close(&["d", "f"]));

    let mut moved = two_tabs(&[]);
    moved.workspaces[1].tabs[0].root = serde_json::from_value(pane("f")).expect("root");
    session.lay_out(moved);
    assert_eq!(session.confirmed(&last_pane), None);
    assert_eq!(session.confirmed(&lone_tab), close(&["f"]));

    session.lay_out(two_tabs(&["d"]));
    assert_eq!(session.confirmed(&last_pane), None);

    session.lay_out(layout(
        json!({"workspaces": [{"id": "w0", "tabs": [{"id": "t0", "root": pane("a")}]}]}),
    ));
    assert_eq!(session.confirmed(&workspace), close(&["a"]));
    assert_eq!(session.confirmed(&lone_tab), None);

    session.lay_out(layout(
        json!({"workspaces": [{"id": "w0", "tabs": [{"id": "t1", "root": pane("c")}]}]}),
    ));
    assert_eq!(session.confirmed(&workspace), close(&["c"]));
}

fn arranged(session: &mut Session, action: KeybindAction) -> Option<Arrangement> {
    session.arrangement(action, AREA)
}

fn three_tabs() -> Layout {
    layout(json!({"workspaces": [
        {"id": "w0", "tabs": [
            {"id": "t0", "root": pane("a")},
            {"id": "t1", "root": pane("b")},
            {"id": "t2", "root": pane("c")},
        ]},
        {"id": "w1", "tabs": [{"id": "t3", "root": pane("d")}]},
    ]}))
}

fn moved(tab: &str, to: Place) -> Option<Arrangement> {
    Some(Arrangement::MoveTab {
        tab: tab.to_owned(),
        to,
    })
}

#[test]
fn moving_a_tab_names_the_focused_tab() {
    let mut session = Session::default();
    session.update(three_tabs(), 0);
    navigate(&mut session, KeybindAction::SwitchTab(1));
    assert_eq!(
        arranged(&mut session, KeybindAction::MoveTabNext),
        moved("t1", Place::Next)
    );
    assert_eq!(
        arranged(&mut session, KeybindAction::MoveTabPrevious),
        moved("t1", Place::Previous)
    );
}

#[test]
fn a_tab_move_asked_before_a_new_tab_lands_still_moves_past_it() {
    let mut session = Session::default();
    session.update(three_tabs(), 0);
    navigate(&mut session, KeybindAction::NextWorkspace);
    let asked = [arranged(&mut session, KeybindAction::MoveTabNext)];

    let mut opened = three_tabs();
    opened.add_tab("w1", None, "e".to_owned());
    let after = landed(opened, &asked);

    let ids: Vec<&str> = after.workspaces[1]
        .tabs
        .iter()
        .map(|tab| tab.id.as_str())
        .collect();
    assert_eq!(ids[1], "t3");
}

#[test]
fn swapping_trades_places_with_the_unzoomed_neighbour_and_does_nothing_at_an_edge() {
    let mut session = Session::default();
    session.update(grid(), 0);
    let swapped = |session: &mut Session, action| {
        let asked = [arranged(session, action)];
        order(&landed(grid(), &asked)).join(" ")
    };
    assert_eq!(swapped(&mut session, KeybindAction::SwapPaneRight), "c b a");
    assert_eq!(swapped(&mut session, KeybindAction::SwapPaneDown), "b a c");
    assert_eq!(swapped(&mut session, KeybindAction::SwapPaneLeft), "a b c");
    assert_eq!(swapped(&mut session, KeybindAction::SwapPaneUp), "a b c");
    navigate(&mut session, KeybindAction::Zoom);
    assert_eq!(swapped(&mut session, KeybindAction::SwapPaneRight), "c b a");
}

#[test]
fn a_swapped_pane_keeps_focus_and_zoom_in_its_new_place() {
    let mut session = Session::default();
    session.update(grid(), 0);
    navigate(&mut session, KeybindAction::Zoom);
    let asked = [arranged(&mut session, KeybindAction::SwapPaneRight)];
    session.update(landed(grid(), &asked), 0);
    assert_eq!(session.focused(), Some("a"));
    assert_eq!(shown(&mut session), ["a"]);
    navigate(&mut session, KeybindAction::Zoom);
    assert_eq!(shown(&mut session), ["c", "b", "a"]);
}

fn resized(session: &mut Session, action: KeybindAction) -> Vec<f32> {
    let asked = [arranged(session, action)];
    ratios(&landed(grid(), &asked).workspaces[0].tabs[0].root)
}

#[test]
fn resizing_steps_the_nearest_split_toward_the_key_or_else_the_one_behind_the_pane() {
    let mut session = Session::default();
    session.update(grid(), 0);
    assert_eq!(
        resized(&mut session, KeybindAction::ResizePaneRight),
        [0.55, 0.5]
    );
    assert_eq!(
        resized(&mut session, KeybindAction::ResizePaneLeft),
        [0.45, 0.5]
    );
    assert_eq!(
        resized(&mut session, KeybindAction::ResizePaneDown),
        [0.5, 0.55]
    );
    assert_eq!(
        resized(&mut session, KeybindAction::ResizePaneUp),
        [0.5, 0.45]
    );
    navigate(&mut session, KeybindAction::FocusPaneRight);
    assert_eq!(
        resized(&mut session, KeybindAction::ResizePaneDown),
        [0.5, 0.5]
    );
    assert_eq!(
        resized(&mut session, KeybindAction::ResizePaneRight),
        [0.55, 0.5]
    );
}

#[test]
fn resizing_skips_splits_a_parked_pane_leaves_hidden() {
    let mut session = Session::default();
    let mut parked = grid();
    parked.parked.insert("b".to_owned());
    session.update(parked.clone(), 0);
    let asked = [
        arranged(&mut session, KeybindAction::ResizePaneDown),
        arranged(&mut session, KeybindAction::ResizePaneRight),
    ];
    assert_eq!(
        ratios(&landed(parked, &asked).workspaces[0].tabs[0].root),
        [0.55, 0.5]
    );
}

fn one_tab(root: serde_json::Value) -> Layout {
    layout(json!({"workspaces": [{"id": "w0", "tabs": [{"id": "t0", "root": root}]}]}))
}

fn down(first: serde_json::Value, second: serde_json::Value) -> serde_json::Value {
    json!({"kind": "split", "direction": "down", "ratio": 0.5, "first": first, "second": second})
}

fn order(layout: &Layout) -> Vec<&str> {
    layout.workspaces[0].tabs[0].root.terminals()
}

fn landed(mut layout: Layout, arrangements: &[Option<Arrangement>]) -> Layout {
    for arrangement in arrangements.iter().flatten() {
        layout.arrange(arrangement);
    }
    layout
}

#[test]
fn two_swaps_asked_before_either_lands_move_the_pane_two_places() {
    let row = one_tab(split(pane("a"), split(pane("b"), pane("c"))));
    let mut session = Session::default();
    session.update(row.clone(), 0);

    let asked = [
        arranged(&mut session, KeybindAction::SwapPaneRight),
        arranged(&mut session, KeybindAction::SwapPaneRight),
    ];

    assert_eq!(order(&landed(row, &asked)), ["b", "c", "a"]);
}

#[test]
fn a_swap_back_asked_before_the_first_swap_lands_returns_the_pane() {
    let row = one_tab(split(pane("a"), split(pane("b"), pane("c"))));
    let mut session = Session::default();
    session.update(row.clone(), 0);

    let asked = [
        arranged(&mut session, KeybindAction::SwapPaneRight),
        arranged(&mut session, KeybindAction::SwapPaneLeft),
    ];

    assert_eq!(order(&landed(row, &asked)), ["a", "b", "c"]);
}

#[test]
fn a_resize_asked_after_a_swap_that_gives_the_pane_its_split_still_lands() {
    let columns = one_tab(split(pane("a"), down(pane("b"), pane("c"))));
    let mut session = Session::default();
    session.update(columns.clone(), 0);

    let asked = [
        arranged(&mut session, KeybindAction::SwapPaneRight),
        arranged(&mut session, KeybindAction::ResizePaneDown),
    ];

    let after = landed(columns, &asked);
    assert_eq!(order(&after), ["b", "a", "c"]);
    assert_eq!(ratios(&after.workspaces[0].tabs[0].root), [0.5, 0.55]);
}

#[test]
fn a_swap_passes_over_a_neighbour_parked_before_it_lands() {
    let row = one_tab(split(pane("a"), split(pane("b"), pane("c"))));
    let mut session = Session::default();
    session.update(row.clone(), 0);
    let asked = [arranged(&mut session, KeybindAction::SwapPaneRight)];

    let mut parked = row;
    parked.parked.insert("b".to_owned());

    assert_eq!(order(&landed(parked, &asked)), ["c", "b", "a"]);
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

#[test]
fn a_resize_asked_before_a_swap_lands_still_resizes_along_its_key() {
    let mut session = Session::default();
    session.update(one_tab(split(down(pane("a"), pane("b")), pane("c"))), 0);
    let asked = [arranged(&mut session, KeybindAction::ResizePaneDown)];

    let swapped = one_tab(split(down(pane("a"), pane("c")), pane("b")));
    let after = landed(swapped, &asked);

    assert_eq!(ratios(&after.workspaces[0].tabs[0].root), [0.5, 0.55]);
}

#[test]
fn a_tab_move_asked_before_an_earlier_tab_closes_still_steps_past_its_neighbour() {
    let tabs = layout(json!({"workspaces": [{"id": "w0", "tabs": [
        {"id": "t0", "root": pane("a")},
        {"id": "t1", "root": pane("b")},
        {"id": "t2", "root": pane("c")},
        {"id": "t3", "root": pane("d")},
    ]}]}));
    let mut session = Session::default();
    session.update(tabs.clone(), 0);
    navigate(&mut session, KeybindAction::SwitchTab(1));
    let asked = [arranged(&mut session, KeybindAction::MoveTabNext)];

    let mut closed = tabs;
    closed.workspaces[0].tabs.remove(0);
    let after = landed(closed, &asked);

    let ids: Vec<&str> = after.workspaces[0]
        .tabs
        .iter()
        .map(|tab| tab.id.as_str())
        .collect();
    assert_eq!(ids, ["t2", "t1", "t3"]);
}

#[test]
fn the_tiles_are_exactly_the_terminals_the_session_says_are_on_screen() {
    let zoomed = |parked: &[&str], focused: &str| {
        let mut session = Session::default();
        session.lay_out(two_tabs(parked));
        session.focus(focused);
        assert!(navigate(&mut session, KeybindAction::Zoom));
        session
    };
    let focused = |parked: &[&str], focused: &str| {
        let mut session = Session::default();
        session.lay_out(two_tabs(parked));
        session.focus(focused);
        session
    };
    let mut other_tab = focused(&[], "a");
    navigate(&mut other_tab, KeybindAction::NextTab);
    let mut other_workspace = focused(&[], "a");
    navigate(&mut other_workspace, KeybindAction::NextWorkspace);
    let sessions = [
        ("split", focused(&[], "a")),
        ("zoomed", zoomed(&[], "b")),
        ("parked", focused(&["b"], "a")),
        ("zoomed beside a parked pane", zoomed(&["b"], "a")),
        ("another tab", other_tab),
        ("another workspace", other_workspace),
    ];
    for (name, mut session) in sessions {
        let mut on_screen: Vec<&str> = ["a", "b", "c", "d"]
            .into_iter()
            .filter(|terminal| session.on_screen(terminal))
            .collect();
        on_screen.sort_unstable();
        let tiles = session.tiles(AREA, CHROME);
        let mut tiled: Vec<&str> = tiles
            .tiles
            .iter()
            .map(|tile| tile.terminal.as_str())
            .collect();
        tiled.sort_unstable();
        assert_eq!(tiled, on_screen, "{name}");
        assert!(!on_screen.is_empty(), "{name}");
        let mut hidden: Vec<&str> = tiles
            .hidden
            .iter()
            .map(|(_, terminal)| terminal.as_str())
            .collect();
        hidden.sort_unstable();
        let unzoomed = session.arranged(AREA, CHROME, false);
        let mut beside: Vec<&str> = unzoomed
            .tiles
            .iter()
            .map(|tile| tile.terminal.as_str())
            .filter(|terminal| !on_screen.contains(terminal))
            .collect();
        beside.sort_unstable();
        assert_eq!(hidden, beside, "{name}");
    }
}
