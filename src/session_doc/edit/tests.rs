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

fn tab_ids(layout: &Layout) -> Vec<&str> {
    layout.workspaces[0]
        .tabs
        .iter()
        .map(|tab| tab.id.as_str())
        .collect()
}

#[test]
fn arrange_steps_a_tab_forward_or_back_wrapping_at_either_end_and_reports_a_missing_tab() {
    let mut layout = one_workspace(pane("a"));
    layout.add_tab("ws0", None, "b".to_owned());
    layout.add_tab("ws0", None, "c".to_owned());
    let tabs = |layout: &Layout| tab_ids(layout).join(" ");
    let moving = |tab: &str, to| Arrangement::MoveTab {
        tab: tab.to_owned(),
        to,
    };
    assert_eq!(tabs(&layout), "tab1 tab2 tab3");
    assert!(layout.arrange(&moving("tab1", Place::Next)));
    assert_eq!(tabs(&layout), "tab2 tab1 tab3");
    assert!(layout.arrange(&moving("tab3", Place::Next)));
    assert_eq!(tabs(&layout), "tab3 tab2 tab1");
    assert!(layout.arrange(&moving("tab3", Place::Previous)));
    assert_eq!(tabs(&layout), "tab2 tab1 tab3");
    assert!(layout.arrange(&moving("tab1", Place::Previous)));
    assert_eq!(tabs(&layout), "tab1 tab2 tab3");

    let before = layout.clone();
    assert!(!layout.arrange(&moving("tab9", Place::Next)));
    assert_eq!(layout, before);
}

#[test]
fn arrange_leaves_a_lone_tab_where_it_is_and_still_reports_it_found() {
    let mut layout = one_workspace(pane("a"));
    let before = layout.clone();
    for to in [Place::Next, Place::Previous] {
        assert!(layout.arrange(&Arrangement::MoveTab {
            tab: "tab1".to_owned(),
            to,
        }));
    }
    assert_eq!(layout, before);
}

const AREA: Rect = Rect::new(0, 0, 40, 10);

#[test]
fn arrange_swaps_a_pane_with_its_shown_neighbour_toward_the_key_and_reports_only_a_missing_pane() {
    let mut layout = one_workspace(split(Direction::Right, pane("a"), pane("b")));
    layout.add_tab("ws0", None, "c".to_owned());
    let swapping = |terminal: &str, toward| Arrangement::Swap {
        terminal: terminal.to_owned(),
        toward,
        area: AREA,
    };
    assert!(layout.arrange(&swapping("a", NavDirection::Right)));
    assert_eq!(
        layout.workspaces[0].tabs[0].root,
        split(Direction::Right, pane("b"), pane("a"))
    );

    let before = layout.clone();
    assert!(layout.arrange(&swapping("a", NavDirection::Right)));
    assert!(layout.arrange(&swapping("a", NavDirection::Down)));
    assert!(layout.arrange(&swapping("c", NavDirection::Left)));
    assert!(!layout.arrange(&swapping("z", NavDirection::Left)));
    layout.parked.insert("b".to_owned());
    assert!(layout.arrange(&swapping("a", NavDirection::Left)));
    assert!(layout.arrange(&swapping("b", NavDirection::Right)));
    layout.parked.clear();
    assert_eq!(layout, before);
}

#[test]
fn arrange_swaps_two_shown_panes_of_one_tab_and_refuses_any_other_pair() {
    let mut layout = one_workspace(split(Direction::Right, pane("a"), pane("b")));
    layout.add_tab("ws0", None, "c".to_owned());
    let swapping = |terminal: &str, with: &str| Arrangement::SwapWith {
        terminal: terminal.to_owned(),
        with: with.to_owned(),
    };
    assert!(layout.arrange(&swapping("a", "b")));
    assert_eq!(
        layout.workspaces[0].tabs[0].root,
        split(Direction::Right, pane("b"), pane("a"))
    );

    let before = layout.clone();
    assert!(!layout.arrange(&swapping("a", "c")));
    assert!(!layout.arrange(&swapping("a", "z")));
    layout.parked.insert("b".to_owned());
    assert!(!layout.arrange(&swapping("a", "b")));
    assert!(!layout.arrange(&swapping("b", "a")));
    layout.parked.clear();
    assert_eq!(layout, before);
}

fn ratios(node: &Node) -> Vec<f32> {
    match node {
        Node::Pane { .. } => Vec::new(),
        Node::Split {
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
fn arrange_steps_the_split_beside_a_pane_along_its_key_within_its_bounds_and_reports_only_a_missing_pane(
) {
    let mut layout = one_workspace(split(
        Direction::Right,
        split(Direction::Down, pane("a"), pane("b")),
        pane("c"),
    ));
    let resizing = |terminal: &str, toward| Arrangement::Resize {
        terminal: terminal.to_owned(),
        toward,
        area: AREA,
    };
    let root = |layout: &Layout| ratios(&layout.workspaces[0].tabs[0].root);
    assert!(layout.arrange(&resizing("a", NavDirection::Down)));
    assert_eq!(root(&layout), [0.5, 0.55]);
    assert!(layout.arrange(&resizing("c", NavDirection::Right)));
    assert_eq!(root(&layout), [0.55, 0.55]);
    for _ in 0..20 {
        layout.arrange(&resizing("b", NavDirection::Left));
        layout.arrange(&resizing("b", NavDirection::Down));
    }
    assert_eq!(root(&layout), [MIN_RATIO, MAX_RATIO]);

    let before = layout.clone();
    assert!(layout.arrange(&resizing("c", NavDirection::Down)));
    assert!(layout.arrange(&resizing("b", NavDirection::Left)));
    assert!(!layout.arrange(&resizing("z", NavDirection::Left)));
    assert_eq!(layout, before);
}

#[test]
fn arrange_moves_a_workspace_before_another_or_to_the_end_and_reports_a_missing_one() {
    let mut layout = Layout::default();
    for terminal in ["a", "b", "c"] {
        layout.add_workspace(None, terminal.to_owned());
    }
    let workspaces = |layout: &Layout| {
        layout
            .workspaces
            .iter()
            .map(|workspace| workspace.id.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let moving = |workspace: &str, to| Arrangement::MoveWorkspace {
        workspace: workspace.to_owned(),
        to,
    };
    let before = |workspace: &str| Place::Before(workspace.to_owned());
    assert_eq!(workspaces(&layout), "ws0 ws2 ws4");
    assert!(layout.arrange(&moving("ws0", before("ws4"))));
    assert_eq!(workspaces(&layout), "ws2 ws0 ws4");
    assert!(layout.arrange(&moving("ws4", before("ws2"))));
    assert_eq!(workspaces(&layout), "ws4 ws2 ws0");
    assert!(layout.arrange(&moving("ws4", Place::End)));
    assert_eq!(workspaces(&layout), "ws2 ws0 ws4");

    let unmoved = layout.clone();
    assert!(!layout.arrange(&moving("ws0", before("ws9"))));
    assert!(!layout.arrange(&moving("ws9", Place::End)));
    assert_eq!(layout, unmoved);
}

#[test]
fn arrange_sets_the_split_between_two_panes_along_its_axis_within_its_bounds_and_reports_only_a_missing_pane(
) {
    let mut layout = one_workspace(split(
        Direction::Right,
        split(Direction::Down, pane("a"), pane("b")),
        pane("c"),
    ));
    let setting = |first: &str, second: &str, ratio| Arrangement::SetRatio {
        first: first.to_owned(),
        second: second.to_owned(),
        direction: if second == "c" {
            Direction::Right
        } else {
            Direction::Down
        },
        ratio,
    };
    let root = |layout: &Layout| ratios(&layout.workspaces[0].tabs[0].root);
    assert!(layout.arrange(&setting("a", "b", 0.3)));
    assert_eq!(root(&layout), [0.5, 0.3]);
    assert!(layout.arrange(&setting("b", "c", 0.7)));
    assert_eq!(root(&layout), [0.7, 0.3]);
    assert!(layout.arrange(&setting("b", "c", 0.95)));
    assert_eq!(root(&layout), [MAX_RATIO, 0.3]);
    assert!(layout.arrange(&setting("a", "b", 0.0)));
    assert_eq!(root(&layout), [MAX_RATIO, MIN_RATIO]);

    let before = layout.clone();
    assert!(layout.arrange(&setting("b", "a", 0.5)));
    assert!(layout.arrange(&setting("a", "z", 0.5)));
    assert!(!layout.arrange(&setting("z", "b", 0.5)));
    assert!(layout.arrange(&Arrangement::SetRatio {
        first: "a".to_owned(),
        second: "b".to_owned(),
        direction: Direction::Right,
        ratio: 0.5,
    }));
    assert_eq!(layout, before);
}

fn right_ratio(first: &str, second: &str) -> Arrangement {
    Arrangement::SetRatio {
        first: first.to_owned(),
        second: second.to_owned(),
        direction: Direction::Right,
        ratio: 0.3,
    }
}

#[test]
fn a_ratio_lands_only_on_the_split_whose_border_the_two_panes_share() {
    let row = |first, second, third| {
        one_workspace(split(
            Direction::Right,
            split(Direction::Right, pane(first), pane(second)),
            pane(third),
        ))
    };
    let mut layout = row("a", "b", "c");
    assert!(layout.arrange(&right_ratio("a", "b")));
    assert_eq!(ratios(&layout.workspaces[0].tabs[0].root), [0.5, 0.3]);

    let mut swapped = row("a", "c", "b");
    let before = swapped.clone();
    assert!(swapped.arrange(&right_ratio("a", "b")));
    assert!(swapped.arrange(&right_ratio("b", "c")));
    assert_eq!(swapped, before);
}

#[test]
fn a_parked_pane_hands_its_border_to_the_pane_shown_beside_it() {
    let mut layout = one_workspace(split(
        Direction::Right,
        split(Direction::Right, pane("a"), pane("b")),
        pane("c"),
    ));
    layout.parked.insert("b".to_owned());
    let before = layout.clone();

    assert!(layout.arrange(&right_ratio("b", "c")));
    assert_eq!(layout, before);
    assert!(layout.arrange(&right_ratio("a", "c")));
    assert_eq!(ratios(&layout.workspaces[0].tabs[0].root), [0.3, 0.5]);
}

fn two_workspaces() -> Layout {
    let mut layout = named(one_workspace(split(Direction::Right, pane("a"), pane("b"))));
    layout.add_tab("ws0", None, "c".to_owned());
    layout.add_workspace(None, "d".to_owned());
    layout
}

fn moving(terminal: &str, to: Destination) -> Arrangement {
    Arrangement::MovePane {
        terminal: terminal.to_owned(),
        to,
        tab: None,
    }
}

fn moving_into(terminal: &str, tab: &str, beside: &str) -> Arrangement {
    Arrangement::MovePane {
        terminal: terminal.to_owned(),
        to: Destination::Split {
            beside: beside.to_owned(),
            direction: Direction::Right,
            ratio: 0.5,
        },
        tab: Some(tab.to_owned()),
    }
}

fn new_tab_in(workspace: &str) -> Destination {
    Destination::Tab {
        workspace: Some(workspace.to_owned()),
        name: None,
    }
}

fn valid(layout: &Layout) {
    let sessions = super::super::Sessions::from([("main".to_owned(), layout.clone())]);
    super::super::validate(&sessions).expect("valid layout");
}

#[test]
fn moving_a_pane_to_a_workspace_appends_it_as_a_new_tab_and_closes_its_gap() {
    let mut layout = two_workspaces();
    let to = Destination::Tab {
        workspace: Some("ws3".to_owned()),
        name: None,
    };
    assert!(layout.arrange(&moving("a", to)));
    assert_eq!(root(&layout, 0, 0), &pane("b"));
    assert_eq!(layout.workspaces[0].tabs[0].name.as_deref(), Some("logs"));
    assert_eq!(layout.workspaces[1].tabs.len(), 2);
    assert_eq!(root(&layout, 1, 1), &pane("a"));
    assert_eq!(layout.workspaces[1].tabs[1].name, None);
    valid(&layout);
}

#[test]
fn moving_a_pane_into_a_tab_splits_it_to_the_right_of_the_chosen_pane() {
    let mut layout = two_workspaces();
    let to = Destination::Split {
        beside: "d".to_owned(),
        direction: Direction::Right,
        ratio: 0.5,
    };
    assert!(layout.arrange(&moving("b", to)));
    assert_eq!(root(&layout, 0, 0), &pane("a"));
    assert_eq!(
        root(&layout, 1, 0),
        &split(Direction::Right, pane("d"), pane("b"))
    );
    valid(&layout);
}

#[test]
fn moving_a_pane_to_a_new_workspace_appends_one_named_as_asked_holding_only_the_pane() {
    let mut layout = two_workspaces();
    let to = Destination::Workspace {
        name: Some("build".to_owned()),
    };
    assert!(layout.arrange(&moving("a", to)));
    assert_eq!(layout.workspaces.len(), 3);
    let added = &layout.workspaces[2];
    assert_eq!(added.name.as_deref(), Some("build"));
    assert_eq!(added.tabs.len(), 1);
    assert_eq!(root(&layout, 2, 0), &pane("a"));
    valid(&layout);
}

#[test]
fn moving_the_last_pane_of_a_tab_removes_the_tab_and_of_a_workspace_removes_the_workspace() {
    let mut layout = two_workspaces();
    let into_d = |terminal: &str| {
        moving(
            terminal,
            Destination::Split {
                beside: "d".to_owned(),
                direction: Direction::Right,
                ratio: 0.5,
            },
        )
    };
    assert!(layout.arrange(&into_d("c")));
    assert_eq!(tab_ids(&layout), ["tab1"]);
    valid(&layout);

    let to_ws0 = || Destination::Tab {
        workspace: Some("ws0".to_owned()),
        name: None,
    };
    assert!(layout.arrange(&moving("d", to_ws0())));
    assert_eq!(layout.workspaces.len(), 2);
    assert!(layout.arrange(&moving("c", to_ws0())));
    assert_eq!(layout.workspaces.len(), 1);
    assert_eq!(layout.workspaces[0].id, "ws0");
    assert_eq!(root(&layout, 0, 1), &pane("d"));
    assert_eq!(root(&layout, 0, 2), &pane("c"));
    valid(&layout);
}

#[test]
fn a_pane_alone_in_its_tab_sent_to_a_new_tab_of_its_own_workspace_keeps_that_tab_and_moves_it_last()
{
    let mut layout = named(one_workspace(pane("a")));
    let before = layout.clone();
    assert!(layout.arrange(&moving("a", new_tab_in("ws0"))));
    assert_eq!(layout, before);

    layout.add_tab("ws0", None, "b".to_owned());
    let mut ending = layout.clone();
    ending.workspaces[0].tabs.rotate_left(1);
    assert!(layout.arrange(&moving("a", new_tab_in("ws0"))));
    assert_eq!(layout, ending);
    assert_eq!(layout.workspaces[0].tabs[1].name.as_deref(), Some("logs"));
    valid(&layout);
}

#[test]
fn moving_a_pane_into_a_tab_splits_beside_the_named_pane_only_while_it_is_in_that_tab() {
    let mut layout = two_workspaces();
    let d_tab = layout.workspaces[1].tabs[0].id.clone();
    let c_tab = layout.workspaces[0].tabs[1].id.clone();
    let before = layout.clone();
    for refusal in [
        moving_into("a", &c_tab, "d"),
        moving_into("a", "tab9", "d"),
        Arrangement::MovePane {
            terminal: "a".to_owned(),
            to: new_tab_in("ws0"),
            tab: Some(c_tab.clone()),
        },
    ] {
        assert!(!layout.arrange(&refusal), "{refusal:?}");
        assert_eq!(layout, before, "{refusal:?}");
    }
    assert!(layout.arrange(&moving_into("a", &d_tab, "d")));
    assert_eq!(
        root(&layout, 1, 0),
        &split(Direction::Right, pane("d"), pane("a"))
    );
}

#[test]
fn a_parked_departed_or_misplaced_move_changes_nothing_and_reports_nothing_found() {
    let mut layout = two_workspaces();
    layout.park("c");
    let before = layout.clone();
    let beside = |terminal: &str| Destination::Split {
        beside: terminal.to_owned(),
        direction: Direction::Right,
        ratio: 0.5,
    };
    let refusals = [
        moving("c", Destination::Workspace { name: None }),
        moving("z", Destination::Workspace { name: None }),
        moving(
            "a",
            Destination::Tab {
                workspace: Some("ws9".to_owned()),
                name: None,
            },
        ),
        moving("a", beside("c")),
        moving("a", beside("z")),
        moving("a", beside("b")),
        moving("a", beside("a")),
    ];
    for refusal in refusals {
        assert!(!layout.arrange(&refusal), "{refusal:?}");
        assert_eq!(layout, before, "{refusal:?}");
    }
}
