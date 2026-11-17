use super::*;

fn pane(terminal: &str) -> Node {
    Node::Pane {
        terminal: terminal.to_owned(),
    }
}

fn split(first: Node, second: Node) -> Node {
    Node::Split {
        direction: Direction::Right,
        ratio: 0.5,
        first: Box::new(first),
        second: Box::new(second),
    }
}

fn add_workspace(layout: &mut Layout, root: Node) -> String {
    let workspace = layout.new_id("ws");
    let tab = layout.new_id("tab");
    layout.workspaces.push(Workspace {
        id: workspace.clone(),
        name: None,
        tabs: vec![Tab {
            id: tab,
            name: None,
            root,
        }],
    });
    workspace
}

#[test]
fn new_id_counts_up_from_next_id() {
    let mut layout = Layout {
        next_id: 3,
        ..Layout::default()
    };
    assert_eq!(layout.new_id("ws"), "ws3");
    assert_eq!(layout.new_id("tab"), "tab4");
    assert_eq!(layout.next_id, 5);
}

#[test]
fn terminals_walk_every_tab_depth_first() {
    let mut layout = Layout::default();
    add_workspace(
        &mut layout,
        split(split(pane("t1"), pane("t2")), pane("t3")),
    );
    add_workspace(&mut layout, pane("t4"));
    assert_eq!(
        layout.terminals().collect::<Vec<_>>(),
        ["t1", "t2", "t3", "t4"]
    );
    assert!(layout.shows("t2"));
    assert!(!layout.shows("t5"));
}

#[test]
fn every_session_shares_one_document() {
    let app = crate::config::app_dir_name();
    assert_eq!(name(), format!("{app}-sessions"));
}

#[cfg(unix)]
mod live;

#[test]
fn a_legacy_parked_pane_returns_to_its_place_in_its_tab() {
    let sessions = decode(
        "doc",
        json!({"schema": 2, "sessions": {"work": {
            "workspaces": [{"id": "ws0", "name": "main", "tabs": [
                {"id": "tab1", "name": "logs", "root": {"kind": "pane", "terminal": "a"}}
            ]}],
            "parked": {"b": {"workspace": "ws0", "tab": "tab1", "beside": "a", "direction": "down"}},
            "next_id": 2
        }}}),
        1,
    )
    .expect("decode");
    let layout = &sessions["work"];
    assert!(!layout.shows("b"));
    assert_eq!(
        layout.workspaces[0].tabs[0].root,
        Node::Split {
            direction: Direction::Down,
            ratio: 0.5,
            first: Box::new(pane("a")),
            second: Box::new(pane("b")),
        }
    );
}

fn legacy(session: Value) -> Result<Sessions, String> {
    decode(
        "doc",
        json!({"schema": 2, "sessions": {"work": session}}),
        1,
    )
}

#[test]
fn legacy_parked_panes_rebuild_pruned_containers_under_their_ids() {
    let sessions = legacy(json!({
        "workspaces": [{"id": "ws0", "name": "main", "tabs": [
            {"id": "tab1", "root": {"kind": "pane", "terminal": "a"}}
        ]}],
        "parked": {
            "c": {"workspace": "ws3", "tab": "tab4", "direction": "right"},
            "b": {"workspace": "ws3", "tab": "tab4", "beside": "c", "direction": "down"},
            "e": {"workspace": "ws0", "tab": "tab2", "direction": "right"}
        },
        "next_id": 9,
        "closing": ["z"],
        "pending_starts": {"doc:4": {"destination": {"kind": "workspace"}}}
    }))
    .expect("decode");
    let layout = &sessions["work"];
    let tab = |w: usize, t: usize| &layout.workspaces[w].tabs[t];
    assert_eq!(layout.workspaces[0].name.as_deref(), Some("main"));
    assert_eq!(
        (tab(0, 1).id.as_str(), &tab(0, 1).root),
        ("tab2", &pane("e"))
    );
    assert_eq!(layout.workspaces[1].id, "ws3");
    assert_eq!(layout.workspaces[1].name, None);
    assert_eq!(tab(1, 0).id, "tab4");
    assert_eq!(
        tab(1, 0).root,
        Node::Split {
            direction: Direction::Down,
            ratio: 0.5,
            first: Box::new(pane("c")),
            second: Box::new(pane("b")),
        }
    );
    assert_eq!(
        layout.parked,
        BTreeSet::from(["b", "c", "e"].map(str::to_owned))
    );
    assert!(layout.shows("a"));
    assert_eq!(layout.next_id, 9);
    assert!(layout.closing.contains("z"));
    assert!(layout.pending_starts.contains_key("doc:4"));
}

#[test]
fn legacy_conflicts_refuse_to_load() {
    let tab_elsewhere = legacy(json!({
        "workspaces": [{"id": "ws0", "tabs": [
            {"id": "tab1", "root": {"kind": "pane", "terminal": "a"}}
        ]}],
        "parked": {"b": {"workspace": "ws5", "tab": "tab1", "direction": "right"}}
    }));
    assert!(tab_elsewhere.unwrap_err().contains("ws0 holds it"));

    let shown_and_parked = legacy(json!({
        "workspaces": [{"id": "ws0", "tabs": [
            {"id": "tab1", "root": {"kind": "pane", "terminal": "a"}}
        ]}],
        "parked": {"a": {"workspace": "ws0", "tab": "tab1", "direction": "right"}}
    }));
    assert!(shown_and_parked
        .unwrap_err()
        .contains("both shown and parked"));
}

#[test]
fn ownership_conflicts_refuse_to_load() {
    let tab = |id: &str, terminal: &str| json!({"id": id, "root": {"kind": "pane", "terminal": terminal}});
    let current =
        |sessions: Value| decode("doc", json!({"schema": SCHEMA, "sessions": sessions}), 1);
    let two_sessions = current(json!({
        "a": {"workspaces": [{"id": "ws0", "tabs": [tab("tab1", "t1")]}]},
        "b": {"workspaces": [{"id": "ws0", "tabs": [tab("tab1", "t1")]}]}
    }));
    assert!(two_sessions.unwrap_err().contains("sessions a and b"));
    let reused_id = current(json!({
        "a": {"workspaces": [{"id": "ws0", "tabs": [tab("tab1", "t1"), tab("tab1", "t2")]}]}
    }));
    assert!(reused_id.unwrap_err().contains("reuses id tab1"));
    let paneless = current(json!({
        "a": {"workspaces": [{"id": "ws0", "tabs": [tab("tab1", "t1")]}], "parked": ["t2"]}
    }));
    assert!(paneless.unwrap_err().contains("parks t2 without a pane"));
    let legacy_into_owned = decode(
        "doc",
        json!({"schema": 2, "sessions": {
            "a": {"workspaces": [{"id": "ws0", "tabs": [tab("tab1", "t1")]}]},
            "b": {"parked": {"t1": {"workspace": "ws0", "tab": "tab1", "direction": "right"}}}
        }}),
        1,
    );
    assert!(legacy_into_owned.unwrap_err().contains("sessions a and b"));
}

#[test]
fn the_current_schema_round_trips() {
    let mut layout = Layout::default();
    add_workspace(&mut layout, split(pane("t1"), pane("t2")));
    layout.parked.insert("t2".to_owned());
    let sessions = Sessions::from([("work".to_owned(), layout)]);
    let value = encode(&sessions).expect("encode");
    assert_eq!(value["schema"], 3);
    assert_eq!(decode("doc", value, 1), Ok(sessions));
}

#[test]
fn a_schema_one_layout_defaults_to_no_pending_operations() {
    let sessions = decode(
        "doc",
        json!({"schema": 1, "sessions": {"work": {"workspaces": [], "parked": {}, "next_id": 0}}}),
        1,
    )
    .expect("decode");
    assert_eq!(sessions["work"], Layout::default());
}

#[test]
fn legacy_cycles_and_missing_neighbors_restore_deterministically() {
    let sessions = legacy(json!({
        "workspaces": [{"id": "ws0", "tabs": [
            {"id": "tab1", "root": {"kind": "pane", "terminal": "a"}}
        ]}],
        "parked": {
            "c": {"workspace": "ws0", "tab": "tab5", "beside": "b", "direction": "down"},
            "b": {"workspace": "ws0", "tab": "tab5", "beside": "c", "direction": "right"},
            "d": {"workspace": "ws0", "tab": "tab1", "beside": "gone", "direction": "right"}
        }
    }))
    .expect("decode");
    let tabs = &sessions["work"].workspaces[0].tabs;
    let split = |direction, first: &str, second: &str| Node::Split {
        direction,
        ratio: 0.5,
        first: Box::new(pane(first)),
        second: Box::new(pane(second)),
    };
    assert_eq!(tabs[0].root, split(Direction::Right, "a", "d"));
    assert_eq!(tabs[1].id, "tab5");
    assert_eq!(tabs[1].root, split(Direction::Down, "b", "c"));
}
