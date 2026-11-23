use super::*;
use crate::core_client::tui::keys::{Keys, Mode, Routed};
use crate::core_client::tui::session::Operation;
#[cfg(unix)]
use crate::core_client::view::{Event, Events, Scrollback, Wake};
#[cfg(unix)]
use crate::hc::test_support::TestCore;

fn spread() -> Layout {
    let pane = |terminal: &str| json!({"kind": "pane", "terminal": terminal});
    serde_json::from_value(json!({"workspaces": [
        {"id": "w0", "tabs": [
            {"id": "t0", "root": {
                "kind": "split", "direction": "right", "ratio": 0.5,
                "first": pane("a"), "second": pane("b"),
            }},
            {"id": "t1", "root": pane("c")},
        ]},
        {"id": "w1", "tabs": [{"id": "t2", "root": pane("d")}]},
    ]}))
    .expect("layout")
}

fn one_pane_tabs(count: usize) -> Layout {
    let tabs: Vec<Value> = (0..count)
        .map(|index| json!({"id": format!("tab{index}"), "root": {"kind": "pane", "terminal": format!("p{index}")}}))
        .collect();
    serde_json::from_value(json!({"workspaces": [{"id": "w0", "tabs": tabs}]})).expect("layout")
}

#[cfg(unix)]
fn two_tabs(shown: &str, behind: &str) -> Layout {
    let pane = |terminal: &str| json!({"kind": "pane", "terminal": terminal});
    serde_json::from_value(json!({"workspaces": [{"id": "w0", "tabs": [
        {"id": "t0", "root": pane(shown)},
        {"id": "t1", "root": pane(behind)},
    ]}]}))
    .expect("layout")
}

fn id(surface: &mut Surface, terminal: &str) -> PaneId {
    surface.session.id(terminal)
}

fn settle(surface: &mut Surface) -> Vec<PaneSurfacePatchRow> {
    settle_leaving(surface).0
}

fn settle_leaving(surface: &mut Surface) -> (Vec<PaneSurfacePatchRow>, Vec<PaneId>) {
    let mut left = Vec::new();
    let rows = surface
        .frame(&HashSet::new(), never_attach, |id, _| left.push(id))
        .0;
    (rows, left)
}

fn hidden(surface: &Surface, id: PaneId) -> bool {
    matches!(surface.views[&id].place, super::super::Place::Hidden(_))
}

#[tokio::test]
async fn every_pane_attaches_once_and_only_the_shown_tab_attaches_as_on_screen() {
    let mut surface = surface(60, 12);
    surface.update(spread());

    let placed = placings(&mut surface);

    let order: Vec<(&str, bool)> = placed
        .iter()
        .map(|(terminal, _, placing)| (terminal.as_str(), placing.shown))
        .collect();
    assert_eq!(
        order,
        [("a", true), ("b", true), ("c", false), ("d", false)]
    );
    let requested: Vec<(String, PaneId, Geometry)> = placed
        .into_iter()
        .map(|(terminal, id, placing)| (terminal, id, placing.geometry))
        .collect();
    for (_, id, geometry) in &requested {
        surface.attached(*id, *geometry, Ok(View::test(geometry.cols, geometry.rows)));
    }
    let size = |terminal: &str| {
        requested
            .iter()
            .find(|(name, ..)| name == terminal)
            .expect("requested")
            .2
    };
    assert_ne!(size("c").rows, size("d").rows);
    for (action, terminal) in [
        (KeybindAction::NextTab, "c"),
        (KeybindAction::NextWorkspace, "d"),
    ] {
        surface.act(action, true);
        settle(&mut surface);
        let id = id(&mut surface, terminal);
        assert_eq!(surface.views[&id].geometry, size(terminal));
    }
}

#[tokio::test]
async fn a_pane_not_on_screen_that_attaches_needs_no_frame() {
    let mut surface = surface(60, 12);
    surface.update(spread());
    let requested = requests(&mut surface);
    let (shown, offstage) = requested.split_at(2);
    for (_, id, geometry) in shown {
        surface.attached(*id, *geometry, Ok(View::test(geometry.cols, geometry.rows)));
    }
    settle(&mut surface);

    for (_, id, geometry) in offstage {
        surface.attached(*id, *geometry, Ok(View::test(geometry.cols, geometry.rows)));
    }

    assert!(!surface.needs_frame());
    assert_eq!(surface.views.len(), 4);
}

#[tokio::test]
async fn switching_to_another_tab_and_back_keeps_the_view_and_attaches_nothing() {
    let mut surface = surface(60, 12);
    surface.update(spread());
    frame(&mut surface, &HashSet::new());
    feed(&surface, "a", b"kept-history");

    surface.act(KeybindAction::NextTab, true);
    assert!(!text(&settle(&mut surface)).contains("kept-history"));
    surface.act(KeybindAction::PreviousTab, true);

    assert!(text(&settle(&mut surface)).contains("kept-history"));
}

#[tokio::test]
async fn a_pane_leaving_the_layout_drops_its_view_and_one_moving_between_tabs_keeps_it() {
    let mut surface = surface(60, 12);
    surface.update(spread());
    frame(&mut surface, &HashSet::new());
    let (c, d) = (id(&mut surface, "c"), id(&mut surface, "d"));
    surface.views[&d].view.test_feed(b"moved");

    let pane = |terminal: &str| json!({"kind": "pane", "terminal": terminal});
    surface.update(
        serde_json::from_value(json!({"workspaces": [{"id": "w0", "tabs": [
            {"id": "t0", "root": {
                "kind": "split", "direction": "right", "ratio": 0.5,
                "first": pane("a"), "second": pane("b"),
            }},
            {"id": "t1", "root": pane("d")},
        ]}]}))
        .expect("layout"),
    );
    let mut left = Vec::new();
    surface.frame(&HashSet::new(), never_attach, |id, _| left.push(id));

    assert!(left.is_empty());
    assert!(!surface.views.contains_key(&c));
    assert_eq!(id(&mut surface, "d"), d);
    surface.act(KeybindAction::NextTab, true);
    assert!(text(&settle(&mut surface)).contains("moved"));
}

#[tokio::test]
async fn a_program_ending_in_a_tab_not_shown_closes_its_pane() {
    let mut surface = surface(60, 12);
    surface.update(spread());
    frame(&mut surface, &HashSet::new());
    let d = id(&mut surface, "d");

    assert_eq!(
        surface.ended(d),
        Some(Operation::Close(vec!["d".to_owned()]))
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn an_attach_refused_as_finished_in_a_tab_not_shown_closes_its_pane() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let shown = test
        .core
        .run(json!({"argv": ["cat"], "no_parent": true}))
        .expect("run");
    let finished = test
        .core
        .run(json!({"argv": ["sh", "-c", "echo done-early"], "no_parent": true}))
        .expect("run");
    test.core
        .call("terminal.wait", json!({"terminal": finished}))
        .expect("wait for the exit");
    let mut surface = surface(60, 12);
    surface.update(two_tabs(&shown, &finished));
    let (wake, mut events) = (Wake::default(), Events::default());

    for (terminal, id, placing) in placings(&mut surface) {
        let view = View::attach(
            test.core.clone(),
            &terminal,
            placing,
            id,
            &wake,
            &events.sender(),
            Scrollback::from(&Config::default()),
        );
        surface.attached(id, placing.geometry, view);
    }
    let ended = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Event::Ended(id) = events.recv().await {
                break id;
            }
        }
    })
    .await
    .expect("the refused pane ends");

    assert!(hidden(&surface, ended));
    assert_eq!(surface.ended(ended), Some(Operation::Close(vec![finished])));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_view_not_on_screen_takes_no_size_in_the_core_until_it_is_shown() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let run = || {
        test.core
            .run(json!({"argv": ["cat"], "no_parent": true}))
            .expect("run")
    };
    let (shown, behind) = (run(), run());
    let cols = |terminal: &str| {
        test.core
            .call("terminal.get", json!({"terminal": terminal}))
            .expect("terminal.get")["cols"]
            .as_u64()
    };
    let (wake, events) = (Wake::default(), Events::default());
    let attach = |terminal: &str, id: PaneId, placing: Placing| {
        View::attach(
            test.core.clone(),
            terminal,
            placing,
            id,
            &wake,
            &events.sender(),
            Scrollback::from(&Config::default()),
        )
    };
    let elsewhere = Placing {
        geometry: host(50, 7),
        shown: true,
    };
    let _other_client = attach(&behind, PaneId::from_raw(999), elsewhere).expect("attach");
    let mut surface = surface(60, 12);
    surface.update(two_tabs(&shown, &behind));

    for (terminal, id, placing) in placings(&mut surface) {
        let view = attach(&terminal, id, placing);
        surface.attached(id, placing.geometry, view);
    }
    settle(&mut surface);
    assert_eq!(cols(&behind), Some(50));
    surface.resize(host(80, 20));
    settle(&mut surface);
    surface.act(KeybindAction::ToggleSidebar, true);
    settle(&mut surface);
    surface.act(KeybindAction::ToggleSidebar, true);
    surface.resize(host(60, 12));
    settle(&mut surface);
    assert_eq!(cols(&behind), Some(50));
    surface.act(KeybindAction::NextTab, true);
    settle(&mut surface);

    let id = id(&mut surface, &behind);
    assert_eq!(
        cols(&behind),
        Some(u64::from(surface.views[&id].geometry.cols))
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_view_attached_off_screen_that_arrives_on_screen_takes_the_size() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let run = || {
        test.core
            .run(json!({"argv": ["cat"], "no_parent": true}))
            .expect("run")
    };
    let (shown, behind) = (run(), run());
    let cols = || {
        test.core
            .call("terminal.get", json!({"terminal": behind}))
            .expect("terminal.get")["cols"]
            .as_u64()
    };
    let (wake, events) = (Wake::default(), Events::default());
    let attach = |terminal: &str, id: PaneId, placing: Placing| {
        View::attach(
            test.core.clone(),
            terminal,
            placing,
            id,
            &wake,
            &events.sender(),
            Scrollback::from(&Config::default()),
        )
    };
    let elsewhere = Placing {
        geometry: host(50, 7),
        shown: true,
    };
    let _other_client = attach(&behind, PaneId::from_raw(999), elsewhere).expect("attach");
    let mut surface = surface(60, 12);
    surface.update(two_tabs(&shown, &behind));
    let placed = placings(&mut surface);

    surface.act(KeybindAction::NextTab, true);
    for (terminal, id, placing) in placed {
        let view = attach(&terminal, id, placing);
        surface.attached(id, placing.geometry, view);
    }
    assert!(surface.needs_frame());
    settle(&mut surface);

    let id = id(&mut surface, &behind);
    assert!(!hidden(&surface, id));
    assert_eq!(cols(), Some(u64::from(surface.views[&id].geometry.cols)));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_view_attached_on_screen_paints_at_its_own_width_while_another_client_leads() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let terminal = test
        .core
        .run(json!({"argv": ["sh", "-c", "read x; printf '%0100d\\n' 0; exec cat"], "no_parent": true}))
        .expect("run");
    let (other_wake, wake, events) = (Wake::default(), Wake::default(), Events::default());
    let attach = |id: PaneId, placing: Placing, wake: &Wake| {
        View::attach(
            test.core.clone(),
            &terminal,
            placing,
            id,
            wake,
            &events.sender(),
            Scrollback::from(&Config::default()),
        )
    };
    let elsewhere = Placing {
        geometry: host(60, 7),
        shown: true,
    };
    let mut other_client = attach(PaneId::from_raw(999), elsewhere, &other_wake).expect("attach");
    other_client.text("\r");
    let wrapped = "0".repeat(60);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !other_client
            .draw()
            .iter()
            .any(|row| text(std::slice::from_ref(row)).contains(&wrapped))
        {
            other_wake.wait().await;
            other_wake.take();
        }
    })
    .await
    .expect("the other client shows the line");
    let mut surface = surface(140, 12);
    surface.update(row_of(std::slice::from_ref(&terminal)));

    for (_, id, placing) in placings(&mut surface) {
        assert!(placing.shown && placing.geometry.cols >= 100);
        surface.attached(id, placing.geometry, attach(id, placing, &wake));
    }
    let line = "0".repeat(100);
    let mut dirty = HashSet::new();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let rows = surface.frame(&dirty, never_attach, ignore_leaving).0;
            if rows
                .iter()
                .any(|row| text(std::slice::from_ref(row)).contains(&line))
            {
                return;
            }
            wake.wait().await;
            dirty = wake.take();
        }
    })
    .await
    .expect("the line shows as one row");
}

#[tokio::test]
async fn a_host_resize_a_sidebar_toggle_and_a_screen_switch_leave_a_hidden_view_alone_until_shown()
{
    let mut surface = surface(60, 12);
    surface.update(spread());
    frame(&mut surface, &HashSet::new());
    let c = id(&mut surface, "c");
    let (before, size) = (surface.views[&c].geometry, surface.views[&c].view.size());

    surface.resize(host(80, 20));
    settle(&mut surface);
    surface.act(KeybindAction::ToggleSidebar, true);
    settle(&mut surface);
    surface.views[&c].view.test_feed(b"\x1b[?1049h");
    surface.frame(&HashSet::from([c]), never_attach, ignore_leaving);
    assert!(!surface.needs_frame());
    assert_eq!(surface.views[&c].geometry, before);
    assert_eq!(surface.views[&c].view.size(), size);
    surface.act(KeybindAction::NextTab, true);
    settle(&mut surface);

    let shown = surface.views[&c].geometry;
    assert_ne!(shown, before);
    assert_eq!(surface.views[&c].view.size(), (shown.cols, shown.rows));
}

#[tokio::test]
async fn a_pane_hidden_by_zoom_keeps_its_size_through_a_host_resize_until_the_zoom_ends() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    frame(&mut surface, &HashSet::new());
    let right = id_of(&surface, "t1");
    surface.act(KeybindAction::Zoom, true);
    settle(&mut surface);
    let size = surface.views[&right].view.size();

    surface.resize(host(60, 14));
    settle(&mut surface);
    assert_eq!(surface.views[&right].view.size(), size);
    surface.act(KeybindAction::Zoom, true);
    settle(&mut surface);

    let shown = surface.views[&right].geometry;
    assert_ne!(surface.views[&right].view.size(), size);
    assert_eq!(surface.views[&right].view.size(), (shown.cols, shown.rows));
}

#[tokio::test]
async fn zoom_attaches_the_panes_it_hides_once_each_as_not_on_screen() {
    let mut surface = surface(60, 12);
    surface.update(row_of(&terminals(3)));
    surface.act(KeybindAction::Zoom, true);

    let placed: Vec<(String, bool)> = placings(&mut surface)
        .into_iter()
        .map(|(terminal, _, placing)| (terminal, placing.shown))
        .collect();

    assert_eq!(
        placed,
        [
            ("t0".to_owned(), true),
            ("t1".to_owned(), false),
            ("t2".to_owned(), false)
        ]
    );
}

#[tokio::test]
async fn hiding_a_tab_releases_the_keys_its_panes_hold_once_and_keeps_their_views() {
    let mut surface = surface(60, 12);
    surface.update(spread());
    frame(&mut surface, &HashSet::new());
    let (a, b) = (id(&mut surface, "a"), id(&mut surface, "b"));

    surface.act(KeybindAction::NextTab, true);
    let (_, released) = settle_leaving(&mut surface);
    surface.resize(host(80, 20));
    let (_, again) = settle_leaving(&mut surface);

    assert_eq!(
        released.into_iter().collect::<HashSet<_>>(),
        HashSet::from([a, b])
    );
    assert!(again.is_empty());
    assert!(hidden(&surface, a) && hidden(&surface, b));
}

#[tokio::test]
async fn an_empty_pane_area_hides_the_views_and_widening_again_attaches_nothing() {
    for rows in [2, 0] {
        let mut surface = surface(40, 10);
        surface.update(row_of(&terminals(2)));
        frame(&mut surface, &HashSet::new());
        let panes = [id_of(&surface, "t0"), id_of(&surface, "t1")];

        surface.resize(host(40, rows));
        let (_, left) = settle_leaving(&mut surface);
        assert_eq!(left.len(), 2, "{rows} rows");
        assert!(panes.iter().all(|pane| hidden(&surface, *pane)));
        surface.resize(host(40, 10));
        let drawn = settle(&mut surface);

        assert!(!drawn.is_empty());
        assert!(panes.iter().all(|pane| !hidden(&surface, *pane)));
    }
}

#[tokio::test]
async fn a_pane_with_no_room_takes_no_input_and_is_not_presented() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, b"");
    let focused = surface.focused_id().expect("focused pane");

    surface.resize(host(40, 2));
    settle(&mut surface);

    assert!(surface.focused().is_none());
    assert!(surface.view(focused).is_none());
    assert_eq!(surface.presented(), Some(HashSet::new()));
    surface.resize(host(40, 10));
    settle(&mut surface);
    surface.focused().expect("focused view").text("typed");
    assert_eq!(reports(&mut inputs[0]), b"typed");
}

#[tokio::test]
async fn a_pane_hidden_for_lack_of_room_is_told_it_lost_focus_and_told_again_when_shown() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, REPORT_FOCUS);
    assert_eq!(reports(&mut inputs[0]), b"\x1b[I");

    surface.resize(host(40, 2));
    settle(&mut surface);
    assert_eq!(reports(&mut inputs[0]), b"\x1b[O");
    surface.resize(host(40, 10));
    settle(&mut surface);

    assert_eq!(reports(&mut inputs[0]), b"\x1b[I");
}

#[tokio::test]
async fn a_pane_told_it_lacks_focus_that_asks_again_while_hidden_is_told_when_shown() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, REPORT_FOCUS);
    let right = id_of(&surface, "t1");
    assert_eq!(reports(&mut inputs[1]), b"\x1b[O");
    surface.act(KeybindAction::Zoom, true);
    settle(&mut surface);
    assert!(hidden(&surface, right));

    surface.views[&right]
        .view
        .test_feed(b"\x1b[?1004l\x1b[?1004h");
    surface.act(KeybindAction::Zoom, true);
    settle(&mut surface);

    assert_eq!(reports(&mut inputs[1]), b"\x1b[O");
}

#[tokio::test]
async fn a_pane_focused_out_of_zoom_hears_focus_before_the_input_that_follows() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, REPORT_FOCUS);
    surface.act(KeybindAction::Zoom, true);
    settle(&mut surface);
    reports(&mut inputs[1]);

    surface.act(KeybindAction::FocusPaneRight, true);
    surface.focused().expect("focused view").text("x");

    assert_eq!(reports(&mut inputs[1]), b"\x1b[Ix");
}

const REPORT_FOCUS_AND_RELEASES: &[u8] = b"\x1b[?1004h\x1b[>3u";

fn route(surface: &Surface, keys: &mut Keys, key: &TerminalKey) -> Vec<PaneId> {
    keys.route_host(
        key.clone(),
        surface.input_focus(),
        false,
        Mode::Panes,
        |_| None,
    )
    .into_iter()
    .filter_map(|routed| match routed {
        Routed::Key(pane, key) => {
            surface.view(pane)?.key(key);
            Some(pane)
        }
        _ => None,
    })
    .collect()
}

fn end_input(surface: &mut Surface, keys: &mut Keys) -> Vec<PaneId> {
    let mut left = Vec::new();
    surface.end_input(|id, view| {
        left.push(id);
        for release in keys.release(id) {
            view.key(release);
        }
    });
    left
}

fn frame_releasing(surface: &mut Surface, keys: &mut Keys) -> Vec<PaneId> {
    let mut left = Vec::new();
    surface.frame(&HashSet::new(), never_attach, |id, view| {
        left.push(id);
        for release in keys.release(id) {
            view.key(release);
        }
    });
    left
}

#[tokio::test]
async fn a_pane_that_took_a_key_in_a_tab_left_before_any_frame_takes_no_repeat_and_hears_focus_out_and_the_release(
) {
    let mut surface = surface(40, 10);
    surface.update(one_pane_tabs(2));
    let mut inputs = attach_with_input(&mut surface, REPORT_FOCUS_AND_RELEASES);
    let behind = id(&mut surface, "p1");
    assert!(hidden(&surface, behind));
    reports(&mut inputs[1]);
    let mut keys = Keys::new(keybinds(), false, true);
    let up = TerminalKey::new(KeyCode::Up, KeyModifiers::NONE);

    surface.act(KeybindAction::NextTab, true);
    assert_eq!(end_input(&mut surface, &mut keys), [id(&mut surface, "p0")]);
    assert_eq!(route(&surface, &mut keys, &up), [behind]);
    surface.act(KeybindAction::PreviousTab, true);
    let repeat = up.with_kind(KeyEventKind::Repeat);
    assert!(route(&surface, &mut keys, &repeat).is_empty());
    let left = end_input(&mut surface, &mut keys);

    assert_eq!(left, [behind]);
    assert!(hidden(&surface, behind));
    assert_eq!(
        reports(&mut inputs[1]),
        b"\x1b[I\x1b[1;1:1A\x1b[O\x1b[1;1:3A"
    );
    assert!(route(&surface, &mut keys, &repeat).is_empty());
    assert!(frame_releasing(&mut surface, &mut keys).is_empty());
}

#[tokio::test]
async fn a_pane_that_took_a_key_and_was_zoomed_away_before_any_frame_takes_no_repeat_and_hears_focus_out_and_the_release(
) {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, REPORT_FOCUS_AND_RELEASES);
    let right = id_of(&surface, "t1");
    surface.act(KeybindAction::Zoom, true);
    settle(&mut surface);
    reports(&mut inputs[1]);
    let mut keys = Keys::new(keybinds(), false, true);
    let up = TerminalKey::new(KeyCode::Up, KeyModifiers::NONE);

    surface.act(KeybindAction::FocusPaneRight, true);
    assert_eq!(route(&surface, &mut keys, &up), [right]);
    surface.act(KeybindAction::FocusPaneLeft, true);
    surface.act(KeybindAction::Zoom, true);
    let repeat = up.with_kind(KeyEventKind::Repeat);
    assert!(route(&surface, &mut keys, &repeat).is_empty());
    let left = frame_releasing(&mut surface, &mut keys);

    assert_eq!(left, [right]);
    assert!(hidden(&surface, right));
    assert_eq!(
        reports(&mut inputs[1]),
        b"\x1b[I\x1b[1;1:1A\x1b[O\x1b[1;1:3A"
    );
}

#[tokio::test]
async fn a_pane_holding_a_button_in_a_tab_left_before_any_frame_is_released_at_once_and_takes_no_drag(
) {
    let mut surface = surface(40, 10);
    surface.update(one_pane_tabs(2));
    let mut inputs = attach_with_input(&mut surface, b"\x1b[?1002h\x1b[?1006h");
    let pane = surface.tiles.tiles[0].info.inner_rect;
    reports(&mut inputs[0]);
    mouse(&mut surface, click(pane.x + 1, pane.y + 1));

    surface.act(KeybindAction::NextTab, true);
    assert_eq!(reports(&mut inputs[0]), b"\x1b[<0;2;2M\x1b[<0;2;2m");
    mouse(&mut surface, drag_to(pane.x + 3, pane.y + 1));
    mouse(&mut surface, release_at(pane.x + 3, pane.y + 1));
    settle(&mut surface);

    assert_eq!(reports(&mut inputs[0]), b"");
}

#[tokio::test]
async fn a_pane_in_a_tab_not_shown_whose_tile_has_no_room_attaches_and_closes_when_its_program_ends(
) {
    let mut surface = surface(80, 5);
    surface.chrome.gaps = true;
    let pane = |terminal: &str| json!({"kind": "pane", "terminal": terminal});
    surface.update(
        serde_json::from_value(json!({"workspaces": [{"id": "w0", "tabs": [
            {"id": "t0", "root": pane("a")},
            {"id": "t1", "root": {
                "kind": "split", "direction": "down", "ratio": 0.5,
                "first": pane("b"), "second": pane("c"),
            }},
        ]}]}))
        .expect("layout"),
    );

    let requested = requests(&mut surface);

    let names: Vec<&str> = requested.iter().map(|(name, ..)| name.as_str()).collect();
    assert_eq!(names, ["a", "b", "c"]);
    for (_, id, geometry) in &requested {
        assert!(geometry.cols > 0 && geometry.rows > 0);
        surface.attached(*id, *geometry, Ok(View::test(geometry.cols, geometry.rows)));
    }
    let b = id(&mut surface, "b");
    assert!(hidden(&surface, b));
    assert_eq!(
        surface.ended(b),
        Some(Operation::Close(vec!["b".to_owned()]))
    );
}

#[tokio::test]
async fn a_relayout_that_needs_no_attach_lays_out_only_the_tab_on_screen() {
    let mut surface = surface(60, 12);
    surface.update(one_pane_tabs(15));
    attach_with_input(&mut surface, b"");
    assert_eq!(surface.views.len(), 15);
    let before = surface.session.placements;

    let walks = surface.session.offstage_walks;

    surface.relayout = true;
    settle(&mut surface);

    assert_eq!(surface.session.placements - before, 1);
    assert_eq!(surface.session.offstage_walks, walks);
}

#[tokio::test]
async fn an_attach_that_fails_off_screen_schedules_no_frame_and_waits_for_the_next_layout() {
    let mut surface = surface(60, 12);
    surface.update(spread());
    let requested = requests(&mut surface);
    let attempt = |terminal: &str| {
        requested
            .iter()
            .find(|(name, ..)| name == terminal)
            .map(|(_, id, geometry)| (*id, *geometry))
            .expect("requested")
    };
    for terminal in ["a", "b", "d"] {
        let (id, geometry) = attempt(terminal);
        surface.attached(id, geometry, Ok(View::test(geometry.cols, geometry.rows)));
    }
    settle(&mut surface);

    let (c, geometry) = attempt("c");
    surface.attached(c, geometry, Err(std::io::Error::other("not_found")));
    assert!(!surface.needs_frame());
    surface.resize(host(60, 12));
    let again = requests(&mut surface);

    assert_eq!(again.len(), 1);
    assert_eq!(again[0].1, c);
}

#[tokio::test]
async fn an_attach_that_fails_on_screen_lays_out_again() {
    let mut surface = surface(60, 12);
    surface.update(spread());
    let requested = requests(&mut surface);
    let (_, a, geometry) = requested
        .iter()
        .find(|(name, ..)| name == "a")
        .expect("requested");

    surface.attached(*a, *geometry, Err(std::io::Error::other("not_found")));

    assert!(surface.needs_frame());
}

#[cfg(unix)]
#[tokio::test]
async fn output_in_fifteen_panes_not_on_screen_schedules_no_frame() {
    let mut surface = surface(60, 12);
    surface.update(one_pane_tabs(16));
    let (_, attached) = frame_attaching(&mut surface, &HashSet::new());
    assert_eq!(attached.len(), 16);
    let wake = Wake::default();
    wake.present(surface.presented().expect("laid out"));

    let hidden: HashSet<PaneId> = (1..16)
        .map(|index| id(&mut surface, &format!("p{index}")))
        .collect();
    for pane in &hidden {
        surface.views[pane].view.test_feed(b"behind");
        wake.request(*pane);
    }

    let last_draw = tokio::time::Instant::now();
    let due = |wake: &Wake| {
        let mut due = None;
        crate::core_client::tui::unix::schedule_output(wake, &mut due, last_draw);
        due
    };
    assert_eq!(due(&wake), None);
    let rows = surface.frame(&hidden, never_attach, ignore_leaving).0;
    assert!(rows.is_empty());
    assert!(!surface.needs_frame());
    wake.request(id(&mut surface, "p0"));
    assert!(due(&wake).is_some());
}

#[tokio::test]
async fn a_parked_pane_keeps_a_view_the_size_of_a_pane_alone_in_its_tab() {
    let mut surface = surface(40, 10);
    let mut layout = row_of(&terminals(2));
    layout.parked.insert("t1".to_owned());
    surface.update(layout);
    frame(&mut surface, &HashSet::new());
    let (shown, parked) = (id(&mut surface, "t0"), id(&mut surface, "t1"));

    assert!(matches!(
        surface.views[&parked].place,
        super::super::Place::Hidden(_)
    ));
    assert_eq!(
        surface.views[&parked].geometry,
        surface.views[&shown].geometry
    );
}

fn roomless_second_tab() -> Surface {
    let mut surface = surface(80, 5);
    surface.chrome.gaps = true;
    let pane = |terminal: &str| json!({"kind": "pane", "terminal": terminal});
    surface.update(
        serde_json::from_value(json!({"workspaces": [{"id": "w0", "tabs": [
            {"id": "t0", "root": pane("a")},
            {"id": "t1", "root": {
                "kind": "split", "direction": "down", "ratio": 0.5,
                "first": pane("b"), "second": pane("c"),
            }},
        ]}]}))
        .expect("layout"),
    );
    surface
}

#[tokio::test]
async fn a_retry_in_a_partly_attached_tab_not_shown_requests_only_the_pane_that_failed() {
    let mut surface = surface(60, 12);
    let pane = |terminal: &str| json!({"kind": "pane", "terminal": terminal});
    surface.update(
        serde_json::from_value(json!({"workspaces": [{"id": "w0", "tabs": [
            {"id": "t0", "root": pane("a")},
            {"id": "t1", "root": {
                "kind": "split", "direction": "right", "ratio": 0.5,
                "first": pane("b"), "second": pane("c"),
            }},
        ]}]}))
        .expect("layout"),
    );
    for (terminal, id, geometry) in requests(&mut surface) {
        let view = match terminal.as_str() {
            "c" => Err(std::io::Error::other("not_found")),
            _ => Ok(View::test(geometry.cols, geometry.rows)),
        };
        surface.attached(id, geometry, view);
    }
    let b = id(&mut surface, "b");
    surface.views[&b].view.test_feed(b"kept");

    surface.resize(host(60, 12));
    let again: Vec<String> = requests(&mut surface)
        .into_iter()
        .map(|(terminal, ..)| terminal)
        .collect();

    assert_eq!(again, ["c"]);
    surface.act(KeybindAction::NextTab, true);
    assert!(text(&settle(&mut surface)).contains("kept"));
}

#[tokio::test]
async fn a_pane_shown_without_room_takes_no_input_before_the_frame_that_shows_it() {
    let mut surface = roomless_second_tab();
    for (_, id, geometry) in requests(&mut surface) {
        surface.attached(id, geometry, Ok(View::test(geometry.cols, geometry.rows)));
    }
    settle(&mut surface);
    let (b, c) = (id(&mut surface, "b"), id(&mut surface, "c"));

    surface.act(KeybindAction::NextTab, true);
    assert!(surface.focused().is_none());
    assert!(surface.view(b).is_none() && surface.view(c).is_none());

    surface.act(KeybindAction::Zoom, true);
    assert!(surface.focused().is_some());
    surface.act(KeybindAction::Zoom, true);
    assert!(surface.focused().is_none());

    surface.resize(host(80, 12));
    assert!(surface.focused().is_some());
    surface.resize(host(80, 5));
    assert!(surface.focused().is_none());
    surface.act(KeybindAction::PreviousTab, true);
    assert!(surface.focused().is_some());
}

#[tokio::test]
async fn a_press_after_a_tab_switch_before_any_frame_misses_the_tab_left() {
    let mut surface = surface(40, 10);
    surface.update(one_pane_tabs(2));
    let mut inputs = attach_with_input(&mut surface, REPORT_MOUSE);
    let old = surface.tiles.tiles[0].info.inner_rect;
    reports(&mut inputs[0]);

    surface.act(KeybindAction::NextTab, true);
    mouse(&mut surface, click(old.x + 1, old.y + 1));
    mouse(&mut surface, release_at(old.x + 1, old.y + 1));

    assert_eq!(reports(&mut inputs[0]), b"");
    assert_eq!(surface.focused_id(), Some(id(&mut surface, "p1")));
}

#[tokio::test]
async fn a_press_after_zoom_or_park_before_any_frame_misses_the_pane_hidden() {
    for park in [false, true] {
        let mut surface = surface(40, 10);
        surface.update(row_of(&terminals(2)));
        let mut inputs = attach_with_input(&mut surface, REPORT_MOUSE);
        let old = surface.tiles.tiles[0].info.inner_rect;
        reports(&mut inputs[0]);

        if park {
            let mut layout = row_of(&terminals(2));
            layout.parked.insert("t0".to_owned());
            surface.update(layout);
        } else {
            surface.act(KeybindAction::FocusPaneRight, true);
            surface.act(KeybindAction::Zoom, true);
        }
        mouse(&mut surface, click(old.x + 1, old.y + 1));
        mouse(&mut surface, release_at(old.x + 1, old.y + 1));

        assert_eq!(reports(&mut inputs[0]), b"", "park: {park}");
    }
}

#[tokio::test]
async fn an_arrival_does_not_recompute_which_panes_take_input() {
    let mut surface = surface(300, 10);
    surface.update(row_of(&terminals(15)));
    let requested = requests(&mut surface);
    assert_eq!(requested.len(), 15);
    let before = surface.retargets;

    for (index, (_, id, geometry)) in requested.into_iter().enumerate() {
        let view = match index % 3 {
            0 => Err(std::io::Error::other("not_found")),
            _ => Ok(View::test(geometry.cols, geometry.rows)),
        };
        surface.attached(id, geometry, view);
    }

    assert_eq!(surface.retargets, before);
}

#[tokio::test]
async fn a_press_on_a_pane_moved_by_zoom_or_resize_reaches_its_program_only_after_the_frame_that_places_it(
) {
    for zoom in [true, false] {
        let mut surface = surface(40, 10);
        surface.update(row_of(&terminals(2)));
        let mut inputs = attach_with_input(&mut surface, REPORT_MOUSE);
        reports(&mut inputs[1]);

        if zoom {
            surface.act(KeybindAction::FocusPaneRight, true);
            surface.act(KeybindAction::Zoom, true);
        } else {
            surface.resize(host(60, 10));
        }
        let moved = surface.tiles.tiles.last().expect("tile").info.inner_rect;
        mouse(&mut surface, click(moved.x + 1, moved.y + 1));
        mouse(&mut surface, release_at(moved.x + 1, moved.y + 1));
        assert_eq!(reports(&mut inputs[1]), b"", "zoom: {zoom}");

        settle(&mut surface);
        mouse(&mut surface, click(moved.x + 1, moved.y + 1));
        mouse(&mut surface, release_at(moved.x + 1, moved.y + 1));
        assert_eq!(
            reports(&mut inputs[1]),
            b"\x1b[<0;2;2M\x1b[<0;2;2m",
            "zoom: {zoom}"
        );
    }
}

#[tokio::test]
async fn a_button_held_on_a_pane_a_resize_moves_is_released_at_once_and_takes_no_drag() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(2)));
    let mut inputs = attach_with_input(&mut surface, b"\x1b[?1002h\x1b[?1006h");
    let right = surface.tiles.tiles[1].info.inner_rect;
    reports(&mut inputs[1]);
    mouse(&mut surface, click(right.x + 1, right.y + 1));

    surface.resize(host(60, 10));
    assert_eq!(reports(&mut inputs[1]), b"\x1b[<0;2;2M\x1b[<0;2;2m");
    mouse(&mut surface, drag_to(right.x + 3, right.y + 1));
    mouse(&mut surface, release_at(right.x + 3, right.y + 1));

    assert_eq!(reports(&mut inputs[1]), b"");
}

#[tokio::test]
async fn a_program_turning_on_focus_reports_before_its_first_frame_hears_its_focus() {
    let mut surface = surface(40, 10);
    surface.update(row_of(&terminals(1)));
    let (_, id, geometry) = requests(&mut surface).remove(0);
    let (view, mut input) = View::test_with_input(geometry.cols, geometry.rows);
    surface.attached(id, geometry, Ok(view));

    surface.views[&id].view.test_feed(REPORT_FOCUS);
    surface.frame(&HashSet::from([id]), never_attach, ignore_leaving);

    assert_eq!(reports(&mut input), b"\x1b[I");
}

#[tokio::test]
async fn a_key_pressed_in_a_pane_without_room_leaves_no_lease_when_the_pane_gains_room() {
    let mut surface = roomless_second_tab();
    for (_, id, geometry) in requests(&mut surface) {
        surface.attached(id, geometry, Ok(View::test(geometry.cols, geometry.rows)));
    }
    settle(&mut surface);
    surface.act(KeybindAction::NextTab, true);
    let mut keys = Keys::new(keybinds(), false, true);
    let up = TerminalKey::new(KeyCode::Up, KeyModifiers::NONE);

    assert!(route(&surface, &mut keys, &up).is_empty());
    surface.resize(host(80, 12));
    assert!(surface.focused().is_some());

    assert!(route(
        &surface,
        &mut keys,
        &up.clone().with_kind(KeyEventKind::Repeat)
    )
    .is_empty());
    assert!(route(
        &surface,
        &mut keys,
        &up.clone().with_kind(KeyEventKind::Release)
    )
    .is_empty());
}

#[tokio::test]
async fn a_thumb_drag_ends_when_its_track_goes_though_the_terminal_keeps_its_width() {
    let mut surface = surface(5, 10);
    surface.update(row_of(&terminals(1)));
    attach_with_input(&mut surface, b"");
    let id = id(&mut surface, "t0");
    surface.views[&id].view.test_feed(&lines(40));
    surface.frame(&HashSet::from([id]), never_attach, ignore_leaving);
    let pane = surface.tiles.tiles[0].info.inner_rect;
    assert_eq!(pane.width, 5);
    let track = Rect::new(pane.right() - 1, pane.y, 1, pane.height);
    let metrics = surface.views[&id].view.scroll_metrics().expect("metrics");
    let thumb = crate::ui::scrollbar_thumb(metrics, track).expect("thumb");
    mouse(&mut surface, click(track.x, thumb.top));

    surface.resize(host(4, 10));
    assert_eq!(surface.tiles.tiles[0].info.inner_rect.width, 4);
    mouse(
        &mut surface,
        mouse_at(MouseEventKind::Drag(MouseButton::Left), track.x, track.y),
    );

    let after = surface.views[&id].view.scroll_metrics().expect("metrics");
    assert_eq!(after.offset_from_bottom, 0);
}

#[tokio::test]
async fn a_pane_first_shown_on_the_alternate_screen_keeps_its_view_and_primary_history() {
    let mut surface = surface(40, 10);
    surface.update(one_pane_tabs(2));
    frame(&mut surface, &HashSet::new());
    let behind = id(&mut surface, "p1");
    let before = surface.views[&behind].geometry;
    surface.views[&behind]
        .view
        .test_feed(b"kept-history\r\n\x1b[?1049h");

    surface.act(KeybindAction::NextTab, true);
    assert!(placings(&mut surface).is_empty());
    assert_eq!(surface.views[&behind].geometry.cols, before.cols + 1);
    surface.views[&behind].view.test_feed(b"\x1b[?1049l");
    surface.relayout = true;

    assert!(text(&settle(&mut surface)).contains("kept-history"));
}

#[tokio::test]
async fn a_key_pressed_in_a_pane_resized_before_its_first_frame_gets_its_release() {
    let mut surface = surface(40, 10);
    surface.update(one_pane_tabs(2));
    let mut inputs = attach_with_input(&mut surface, REPORT_FOCUS_AND_RELEASES);
    let behind = id(&mut surface, "p1");
    surface.resize(host(60, 10));
    settle(&mut surface);
    reports(&mut inputs[1]);
    let mut keys = Keys::new(keybinds(), false, true);
    let up = TerminalKey::new(KeyCode::Up, KeyModifiers::NONE);

    surface.act(KeybindAction::NextTab, true);
    assert_eq!(route(&surface, &mut keys, &up), [behind]);
    assert!(placings(&mut surface).is_empty());
    let release = up.with_kind(KeyEventKind::Release);

    assert_eq!(route(&surface, &mut keys, &release), [behind]);
    assert_eq!(reports(&mut inputs[1]), b"\x1b[I\x1b[1;1:1A\x1b[1;1:3A");
}
