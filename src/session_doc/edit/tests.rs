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

fn position(beside: Option<&str>, direction: Direction) -> ReturnPosition {
    ReturnPosition {
        workspace: "ws0".to_owned(),
        tab: "tab1".to_owned(),
        beside: beside.map(str::to_owned),
        direction,
    }
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
    assert_eq!(
        layout.remove("c"),
        Some(position(Some("a"), Direction::Down))
    );
    assert_eq!(
        root(&layout, 0, 0),
        &split(Direction::Right, pane("a"), pane("b"))
    );
    assert_eq!(
        layout.remove("a"),
        Some(position(Some("b"), Direction::Right))
    );
    assert_eq!(root(&layout, 0, 0), &pane("b"));
}

#[test]
fn remove_names_the_first_terminal_of_a_split_sibling() {
    let mut layout = one_workspace(split(
        Direction::Right,
        pane("a"),
        split(Direction::Down, pane("b"), pane("c")),
    ));
    assert_eq!(
        layout.remove("a"),
        Some(position(Some("b"), Direction::Right))
    );
    assert_eq!(
        root(&layout, 0, 0),
        &split(Direction::Down, pane("b"), pane("c"))
    );
}

#[test]
fn remove_drops_an_empty_tab_and_then_an_empty_workspace() {
    let mut layout = one_workspace(pane("a"));
    layout.add_tab("ws0", None, "b".to_owned());
    assert_eq!(layout.remove("a"), Some(position(None, Direction::Right)));
    assert_eq!(layout.workspaces[0].tabs.len(), 1);
    assert_eq!(layout.workspaces[0].tabs[0].id, "tab2");
    assert!(layout.remove("b").is_some());
    assert!(layout.workspaces.is_empty());
}

#[test]
fn remove_of_an_unshown_terminal_changes_nothing() {
    let mut layout = one_workspace(pane("a"));
    let before = layout.clone();
    assert_eq!(layout.remove("z"), None);
    assert_eq!(layout, before);
}

#[test]
fn park_remembers_where_the_terminal_was() {
    let mut layout = one_workspace(split(Direction::Down, pane("a"), pane("b")));
    assert!(layout.park("b"));
    assert!(!layout.shows("b"));
    assert_eq!(
        layout.parked.get("b"),
        Some(&position(Some("a"), Direction::Down))
    );
    assert!(!layout.park("b"));
}

#[test]
fn unpark_returns_beside_the_same_terminal() {
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
fn unpark_falls_back_to_the_tabs_last_terminal() {
    let mut layout = one_workspace(split(
        Direction::Right,
        pane("a"),
        split(Direction::Right, pane("b"), pane("c")),
    ));
    layout.park("b");
    layout.park("c");
    layout.split("a", Direction::Down, 0.5, "d".to_owned());
    layout.unpark("b", None).expect("unpark b");
    assert_eq!(
        root(&layout, 0, 0),
        &split(
            Direction::Down,
            pane("a"),
            split(Direction::Right, pane("d"), pane("b"))
        )
    );
}

#[test]
fn unpark_opens_a_tab_in_its_workspace_when_its_tab_is_gone() {
    let mut layout = one_workspace(pane("a"));
    layout.add_tab("ws0", None, "b".to_owned());
    layout.park("a");
    layout.unpark("a", None).expect("unpark");
    assert_eq!(layout.workspaces[0].tabs.len(), 2);
    assert_eq!(root(&layout, 0, 1), &pane("a"));
}

#[test]
fn unpark_opens_a_tab_in_the_first_workspace_when_its_workspace_is_gone() {
    let mut layout = one_workspace(pane("a"));
    layout.add_workspace(None, "b".to_owned());
    layout.park("a");
    layout.unpark("a", None).expect("unpark");
    assert_eq!(layout.workspaces.len(), 1);
    assert_eq!(layout.workspaces[0].tabs.len(), 2);
    assert_eq!(root(&layout, 0, 1), &pane("a"));
}

#[test]
fn unpark_with_no_workspace_left_makes_one() {
    let mut layout = one_workspace(pane("a"));
    layout.park("a");
    assert!(layout.workspaces.is_empty());
    layout.unpark("a", None).expect("unpark");
    assert_eq!(layout.terminals().collect::<Vec<_>>(), ["a"]);
    assert!(layout.parked.is_empty());
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
fn unpark_beside_a_chosen_terminal() {
    let mut layout = one_workspace(pane("a"));
    layout.add_workspace(None, "b".to_owned());
    layout.park("a");
    layout
        .parked
        .insert("a".to_owned(), position(None, Direction::Right));
    layout
        .unpark(
            "a",
            Some(Placement::Beside {
                terminal: "b".to_owned(),
                direction: Direction::Down,
            }),
        )
        .expect("unpark");
    assert_eq!(
        root(&layout, 0, 0),
        &split(Direction::Down, pane("b"), pane("a"))
    );
    assert!(layout.parked.is_empty());
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
