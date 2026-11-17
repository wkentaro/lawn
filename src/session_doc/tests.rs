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
