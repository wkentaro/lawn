use super::*;

fn pane(terminal: &str) -> Node {
    Node::Pane {
        terminal: terminal.to_owned(),
    }
}

fn split(direction: Direction, first: Node, second: Node) -> Node {
    Node::Split {
        direction,
        ratio: 0.5,
        first: Box::new(first),
        second: Box::new(second),
    }
}

fn one_workspace(root: Node) -> Layout {
    Layout {
        workspaces: vec![Workspace {
            id: "ws0".to_owned(),
            name: None,
            tabs: vec![Tab {
                id: "tab1".to_owned(),
                name: None,
                root,
            }],
        }],
        next_id: 2,
        ..Layout::default()
    }
}

fn root(layout: &Layout, workspace: usize, tab: usize) -> &Node {
    &layout.workspaces[workspace].tabs[tab].root
}

#[test]
fn add_workspace_names_it_and_shows_the_terminal_in_one_tab() {
    let mut layout = Layout::default();
    let ids = layout.add_workspace(Some("main".to_owned()), "a".to_owned());
    assert_eq!(ids, ("ws0".to_owned(), "tab1".to_owned()));
    assert_eq!(layout.workspaces[0].name.as_deref(), Some("main"));
    assert_eq!(root(&layout, 0, 0), &pane("a"));
}

#[test]
fn add_tab_appends_to_an_existing_workspace_only() {
    let mut layout = one_workspace(pane("a"));
    assert_eq!(
        layout.add_tab("ws0", Some("logs".to_owned()), "b".to_owned()),
        Some("tab2".to_owned())
    );
    assert_eq!(layout.workspaces[0].tabs[1].name.as_deref(), Some("logs"));
    assert_eq!(root(&layout, 0, 1), &pane("b"));

    let before = layout.clone();
    assert_eq!(layout.add_tab("ws9", None, "c".to_owned()), None);
    assert_eq!(layout, before);
}

#[test]
fn split_puts_the_new_terminal_second() {
    let mut layout = one_workspace(split(Direction::Right, pane("a"), pane("b")));
    assert!(layout.split("b", Direction::Down, 0.3, "c".to_owned()));
    assert_eq!(
        root(&layout, 0, 0),
        &split(
            Direction::Right,
            pane("a"),
            Node::Split {
                direction: Direction::Down,
                ratio: 0.3,
                first: Box::new(pane("b")),
                second: Box::new(pane("c")),
            }
        )
    );
}

#[test]
fn split_clamps_the_ratio() {
    for (asked, kept) in [(0.0, 0.1), (2.0, 0.9), (f32::NAN, 0.5)] {
        let mut layout = one_workspace(pane("a"));
        assert!(layout.split("a", Direction::Right, asked, "b".to_owned()));
        let Node::Split { ratio, .. } = root(&layout, 0, 0) else {
            panic!("not split");
        };
        assert_eq!(*ratio, kept);
    }
}

#[test]
fn split_beside_a_missing_terminal_changes_nothing() {
    let mut layout = one_workspace(pane("a"));
    let before = layout.clone();
    assert!(!layout.split("z", Direction::Right, 0.5, "b".to_owned()));
    assert_eq!(layout, before);
}

#[test]
fn remove_collapses_the_parent_into_the_sibling() {
    let mut layout = one_workspace(split(
        Direction::Down,
        split(Direction::Right, pane("a"), pane("b")),
        pane("c"),
    ));
    assert!(layout.remove("c"));
    assert_eq!(
        root(&layout, 0, 0),
        &split(Direction::Right, pane("a"), pane("b"))
    );
    assert!(layout.remove("a"));
    assert_eq!(root(&layout, 0, 0), &pane("b"));
}

#[test]
fn remove_drops_an_empty_tab_and_then_an_empty_workspace() {
    let mut layout = one_workspace(pane("a"));
    layout.add_tab("ws0", None, "b".to_owned());
    assert!(layout.remove("a"));
    assert_eq!(layout.workspaces[0].tabs.len(), 1);
    assert_eq!(layout.workspaces[0].tabs[0].id, "tab2");
    assert!(layout.remove("b"));
    assert!(layout.workspaces.is_empty());
}

#[test]
fn remove_of_an_unshown_terminal_changes_nothing() {
    let mut layout = one_workspace(pane("a"));
    let before = layout.clone();
    assert!(!layout.remove("z"));
    assert_eq!(layout, before);
}

#[test]
fn park_hides_the_pane_in_place() {
    let mut layout = one_workspace(split(Direction::Down, pane("a"), pane("b")));
    let before = layout.workspaces.clone();
    assert!(layout.park("b"));
    assert!(!layout.shows("b"));
    assert!(layout.owns("b"));
    assert_eq!(layout.workspaces, before);
    assert!(!layout.park("b"));
    assert!(!layout.park("z"));
}

#[test]
fn unpark_shows_the_pane_where_it_was() {
    let mut layout = one_workspace(split(Direction::Down, pane("a"), pane("b")));
    layout.park("b");
    layout.unpark("b", None).expect("unpark");
    assert_eq!(
        root(&layout, 0, 0),
        &split(Direction::Down, pane("a"), pane("b"))
    );
    assert!(layout.parked.is_empty());
}

#[test]
fn remove_drops_a_parked_pane_and_its_mark() {
    let mut layout = one_workspace(split(Direction::Down, pane("a"), pane("b")));
    layout.park("b");
    assert!(layout.remove("b"));
    assert_eq!(root(&layout, 0, 0), &pane("a"));
    assert!(layout.parked.is_empty());

    layout.park("a");
    assert!(layout.remove("a"));
    assert!(layout.workspaces.is_empty());
    assert!(layout.parked.is_empty());
}

#[test]
fn a_parked_pane_is_not_a_split_or_move_target() {
    let mut layout = one_workspace(split(Direction::Down, pane("a"), pane("b")));
    layout.park("a");
    let before = layout.clone();
    assert!(!layout.split("a", Direction::Right, 0.5, "c".to_owned()));
    let beside_parked = Some(Placement::Beside {
        terminal: "a".to_owned(),
        direction: Direction::Right,
    });
    assert!(layout.unpark("x", beside_parked).is_err());
    assert_eq!(layout, before);
}

#[test]
fn a_fully_parked_workspace_takes_new_tabs() {
    let mut layout = named(one_workspace(pane("a")));
    layout.park("a");
    layout
        .unpark(
            "x",
            Some(Placement::NewTab {
                workspace: Some("ws0".to_owned()),
            }),
        )
        .expect("unpark");
    assert_eq!(root(&layout, 0, 0), &pane("a"));
    assert_eq!(root(&layout, 0, 1), &pane("x"));
    assert!(!layout.shows("a"));
}

#[test]
fn unpark_of_an_unparked_terminal_opens_a_tab_in_the_first_workspace() {
    let mut layout = one_workspace(pane("a"));
    layout.unpark("x", None).expect("unpark");
    assert_eq!(root(&layout, 0, 1), &pane("x"));

    let mut empty = Layout::default();
    empty.unpark("x", None).expect("unpark");
    assert_eq!(empty.terminals().collect::<Vec<_>>(), ["x"]);
}

#[test]
fn unpark_beside_a_chosen_terminal_moves_the_pane() {
    let mut layout = one_workspace(split(Direction::Right, pane("a"), pane("c")));
    layout.add_workspace(None, "b".to_owned());
    layout.park("a");
    layout
        .unpark(
            "a",
            Some(Placement::Beside {
                terminal: "b".to_owned(),
                direction: Direction::Down,
            }),
        )
        .expect("unpark");
    assert_eq!(root(&layout, 0, 0), &pane("c"));
    assert_eq!(
        root(&layout, 1, 0),
        &split(Direction::Down, pane("b"), pane("a"))
    );
    assert!(layout.parked.is_empty());
}

#[test]
fn moving_a_sole_parked_pane_elsewhere_drops_its_emptied_workspace() {
    let mut layout = one_workspace(pane("a"));
    layout.add_workspace(None, "b".to_owned());
    layout.park("a");
    layout
        .unpark(
            "a",
            Some(Placement::NewTab {
                workspace: Some("ws2".to_owned()),
            }),
        )
        .expect("unpark");
    assert_eq!(layout.workspaces.len(), 1);
    assert_eq!(root(&layout, 0, 1), &pane("a"));
}

#[test]
fn unpark_to_a_new_tab_in_a_chosen_workspace() {
    let mut layout = one_workspace(pane("a"));
    layout.add_workspace(None, "b".to_owned());
    layout
        .unpark(
            "x",
            Some(Placement::NewTab {
                workspace: Some("ws2".to_owned()),
            }),
        )
        .expect("unpark");
    assert_eq!(root(&layout, 1, 1), &pane("x"));

    layout
        .unpark("y", Some(Placement::NewTab { workspace: None }))
        .expect("unpark");
    assert_eq!(root(&layout, 0, 1), &pane("y"));
}

#[test]
fn unpark_refusals_change_nothing() {
    let mut layout = one_workspace(split(Direction::Right, pane("a"), pane("b")));
    layout.park("b");
    let before = layout.clone();
    let refusals = [
        ("a", None),
        (
            "b",
            Some(Placement::NewTab {
                workspace: Some("ws9".to_owned()),
            }),
        ),
        (
            "b",
            Some(Placement::Beside {
                terminal: "z".to_owned(),
                direction: Direction::Right,
            }),
        ),
    ];
    for (terminal, to) in refusals {
        assert!(layout.unpark(terminal, to).is_err());
        assert_eq!(layout, before);
    }
}

fn named(mut layout: Layout) -> Layout {
    layout.workspaces[0].name = Some("main".to_owned());
    layout.workspaces[0].tabs[0].name = Some("logs".to_owned());
    layout
}

#[test]
fn parking_the_last_pane_keeps_its_workspace_and_tab() {
    let mut layout = named(one_workspace(pane("a")));
    let before = layout.workspaces.clone();
    assert!(layout.park("a"));
    assert!(!layout.shows("a"));
    assert_eq!(layout.workspaces, before);

    layout.unpark("a", None).expect("unpark");
    assert!(layout.shows("a"));
    assert_eq!(layout.workspaces, before);
}

#[test]
fn unparking_returns_panes_to_their_places_after_their_neighbors_change() {
    let original = split(
        Direction::Right,
        pane("a"),
        split(Direction::Down, pane("b"), pane("c")),
    );
    let mut layout = one_workspace(original.clone());
    layout.park("b");
    layout.park("c");
    layout.unpark("c", None).expect("unpark c");
    layout.unpark("b", None).expect("unpark b");
    assert_eq!(root(&layout, 0, 0), &original);
}

#[test]
fn moving_the_sole_parked_pane_to_a_new_tab_in_its_own_workspace_keeps_the_workspace() {
    let mut layout = named(one_workspace(pane("a")));
    layout.park("a");
    layout
        .unpark(
            "a",
            Some(Placement::NewTab {
                workspace: Some("ws0".to_owned()),
            }),
        )
        .expect("unpark");
    assert_eq!(layout.workspaces.len(), 1);
    assert_eq!(layout.workspaces[0].name.as_deref(), Some("main"));
    assert_eq!(layout.workspaces[0].tabs.len(), 1);
    assert_eq!(root(&layout, 0, 0), &pane("a"));
    assert!(layout.shows("a"));
}

#[test]
fn only_the_workspace_a_pane_leaves_is_dropped() {
    let mut layout = one_workspace(pane("a"));
    layout.workspaces.push(Workspace {
        id: "ws5".to_owned(),
        name: Some("saved".to_owned()),
        tabs: Vec::new(),
    });
    layout.add_workspace(None, "b".to_owned());
    let before = layout.clone();
    assert!(!layout.remove("missing"));
    assert_eq!(layout, before);

    layout.park("a");
    layout
        .unpark(
            "a",
            Some(Placement::Beside {
                terminal: "b".to_owned(),
                direction: Direction::Right,
            }),
        )
        .expect("unpark");
    assert_eq!(
        layout
            .workspaces
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        ["ws5", "ws2"]
    );
    assert!(layout.remove("a"));
    assert!(layout.remove("b"));
    assert_eq!(layout.workspaces.len(), 1);
    assert_eq!(layout.workspaces[0].name.as_deref(), Some("saved"));
}

#[test]
fn arrange_names_a_workspace_or_tab_and_reports_a_missing_one() {
    let mut layout = one_workspace(pane("a"));
    assert!(layout.arrange(&Arrangement::NameWorkspace {
        workspace: "ws0".to_owned(),
        name: "main".to_owned(),
    }));
    assert!(layout.arrange(&Arrangement::NameTab {
        tab: "tab1".to_owned(),
        name: "logs".to_owned(),
    }));
    assert_eq!(layout.workspaces[0].name.as_deref(), Some("main"));
    assert_eq!(layout.workspaces[0].tabs[0].name.as_deref(), Some("logs"));

    let before = layout.clone();
    assert!(!layout.arrange(&Arrangement::NameWorkspace {
        workspace: "ws9".to_owned(),
        name: "x".to_owned(),
    }));
    assert!(!layout.arrange(&Arrangement::NameTab {
        tab: "tab9".to_owned(),
        name: "x".to_owned(),
    }));
    assert_eq!(layout, before);
}
