use std::cell::Cell;

use serde_json::json;

use super::*;
use crate::hc::test_support::TestCore;

#[test]
fn a_missing_document_loads_empty() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let store = Store::new(test.core.clone(), "missing", None);
    assert_eq!(
        store.load(),
        Ok(Loaded {
            layout: Layout::default(),
            version: 0
        })
    );
}

#[test]
fn edits_round_trip_through_load() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let store = Store::new(test.core.clone(), "round-trip", None);
    let (workspace, written) = store
        .edit(|layout| {
            let workspace = add_workspace(layout, split(pane("t1"), pane("t2")));
            layout.workspaces[0].name = Some("main".to_owned());
            layout.parked.insert(
                "t3".to_owned(),
                ReturnPosition {
                    workspace: workspace.clone(),
                    tab: "tab1".to_owned(),
                    beside: Some("t2".to_owned()),
                    direction: Direction::Down,
                },
            );
            workspace
        })
        .expect("edit");
    assert_eq!(workspace, "ws0");
    assert!(written.version > 0);
    assert_eq!(store.load(), Ok(written));
}

#[test]
fn a_conflicting_edit_retries_on_top_of_the_other_write() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let a = Store::new(test.core.clone(), "shared", None);
    let b = Store::new(test.core.clone(), "shared", None);
    let stale = a.load().expect("load");
    b.edit(|layout| add_workspace(layout, pane("t1")))
        .expect("edit b");
    assert!(a.load().expect("reload").version > stale.version);

    let calls = Cell::new(0);
    let (_, written) = a
        .edit(|layout| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                let (mut competing, current) = a.load_all().expect("read");
                add_workspace(
                    competing.entry("shared".to_owned()).or_default(),
                    pane("t2"),
                );
                test.core
                    .doc_set(&a.name, None, &encode(&competing).expect("encode"), current)
                    .expect("competing write");
            }
            add_workspace(layout, pane("t3"));
        })
        .expect("edit a");
    assert_eq!(calls.get(), 2);
    assert_eq!(
        written.layout.terminals().collect::<Vec<_>>(),
        ["t1", "t2", "t3"]
    );
    assert_eq!(a.load(), Ok(written));
}

#[test]
fn an_unchanged_edit_keeps_the_version() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let store = Store::new(test.core.clone(), "unchanged", None);
    let (_, first) = store
        .edit(|layout| add_workspace(layout, pane("t1")))
        .expect("edit");
    let (shown, second) = store.edit(|layout| layout.shows("t1")).expect("no-op");
    assert!(shown);
    assert_eq!(second, first);
    assert_eq!(store.load().expect("load").version, first.version);
}

#[test]
fn a_newer_schema_refuses_to_load() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let store = Store::new(test.core.clone(), "newer", None);
    test.core
        .doc_set(
            &store.name,
            None,
            &json!({"schema": SCHEMA + 1, "sessions": {}}),
            0,
        )
        .expect("write");
    let err = store.load().expect_err("newer schema");
    assert!(err.contains("newer Lawn"), "{err}");
    assert!(store.edit(|_| ()).is_err());
    assert!(store.edit_all(|_| ()).is_err());
}

#[test]
fn sessions_edit_one_document_without_clobbering_each_other() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let a = Store::new(test.core.clone(), "a", None);
    let b = Store::new(test.core.clone(), "b", None);
    a.edit(|layout| add_workspace(layout, pane("t1")))
        .expect("edit a");
    let (_, written) = b
        .edit(|layout| add_workspace(layout, pane("t2")))
        .expect("edit b");
    assert_eq!(written.layout.terminals().collect::<Vec<_>>(), ["t2"]);

    let (sessions, version) = a.load_all().expect("load all");
    assert_eq!(version, written.version);
    assert_eq!(sessions.keys().collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(sessions["a"].terminals().collect::<Vec<_>>(), ["t1"]);
    assert_eq!(a.load().expect("load a").layout, sessions["a"]);
}

#[test]
fn edit_all_changes_several_sessions_in_one_write() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let a = Store::new(test.core.clone(), "a", None);
    a.edit(|layout| add_workspace(layout, pane("t1")))
        .expect("edit a");
    let before = a.load().expect("load").version;
    let ((), version) = a
        .edit_all(|sessions| {
            sessions.remove("a");
            add_workspace(sessions.entry("b".to_owned()).or_default(), pane("t1"));
        })
        .expect("edit all");
    assert!(version > before);
    let (sessions, _) = a.load_all().expect("load all");
    assert_eq!(sessions.keys().collect::<Vec<_>>(), ["b"]);

    let (shown, unchanged) = a
        .edit_all(|sessions| sessions.values().any(|layout| layout.shows("t1")))
        .expect("no-op");
    assert!(shown);
    assert_eq!(unchanged, version);
}

#[test]
fn a_session_that_never_changed_is_not_written() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let store = Store::new(test.core.clone(), "idle", None);
    let (_, loaded) = store.edit(|layout| layout.shows("t1")).expect("no-op");
    assert_eq!(loaded.version, 0);
    assert_eq!(store.load_all().expect("load all"), (Sessions::new(), 0));
}
