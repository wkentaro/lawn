use super::*;
use crate::hc::test_support::TestCore;
use crate::session_doc::{Direction, Placement};
use serde_json::Value;

fn program() -> Vec<String> {
    ["sh", "-c", "sleep 120"].map(str::to_owned).to_vec()
}

fn workspace() -> Destination {
    Destination::Workspace { name: None }
}

fn run_unacknowledged(test: &TestCore, reference: &str) -> String {
    test.core
        .run(json!({"argv": program(), "keep": true, "no_parent": true,
        "labels": {START_LABEL: reference}}))
        .unwrap()
}

fn record(core: &Core, terminal: &str) -> Value {
    core.call("terminal.get", json!({"terminal": terminal}))
        .unwrap()
}

#[test]
fn a_default_tab_destination_stays_bound_to_its_original_workspace() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let first = lifecycle
        .start(workspace(), Path::new("/tmp"), &program())
        .unwrap();
    let second = lifecycle
        .start(workspace(), Path::new("/tmp"), &program())
        .unwrap();
    let reference = lifecycle
        .store
        .reserve_start(&Destination::Tab {
            workspace: None,
            name: None,
        })
        .unwrap();
    lifecycle.close(&first.terminal).unwrap();
    let terminal = run_unacknowledged(&test, &reference);

    assert!(lifecycle
        .recover()
        .unwrap_err()
        .contains("reserved for closing"));
    let layout = lifecycle.store.load().unwrap().layout;
    assert!(layout.closing.contains(&terminal));
    assert!(layout.shows(&second.terminal));
    assert!(!layout.shows(&terminal));
    lifecycle.recover().unwrap();
}

#[test]
fn an_invalid_start_does_not_create_a_session_or_upgrade_its_document() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let document = crate::session_doc::name();
    test.core
        .doc_set(&document, None, &json!({"schema": 1, "sessions": {}}), 0)
        .unwrap();
    let before = test.core.doc_get(&document, None).unwrap();
    let lifecycle = Lifecycle::new(test.core.clone(), "absent");

    assert!(lifecycle
        .start(
            Destination::Tab {
                workspace: None,
                name: None
            },
            Path::new("/tmp"),
            &program()
        )
        .is_err());
    assert!(lifecycle
        .start(
            Destination::Split {
                beside: "missing".to_owned(),
                direction: Direction::Right,
                ratio: 0.5
            },
            Path::new("/tmp"),
            &program()
        )
        .is_err());

    assert_eq!(test.core.doc_get(&document, None).unwrap(), before);
}

#[test]
fn a_received_error_still_reconciles_a_terminal_that_did_start() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let reference = lifecycle.store.reserve_start(&workspace()).unwrap();
    let terminal = run_unacknowledged(&test, &reference);
    lifecycle
        .store
        .edit(|layout| {
            layout
                .pending_starts
                .get_mut(&reference)
                .unwrap()
                .reply_received = true
        })
        .unwrap();

    Lifecycle::new(test.core.clone(), "work").recover().unwrap();

    let layout = lifecycle.store.load().unwrap().layout;
    assert!(layout.pending_starts.is_empty());
    assert!(layout.shows(&terminal));
    assert_eq!(record(&test.core, &terminal)["state"], "live");
}

#[test]
fn a_confirmed_run_rejection_does_not_permanently_block_the_session() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let error = lifecycle
        .start(
            workspace(),
            Path::new("/tmp"),
            &["/nonexistent/lawn-test-program".to_owned()],
        )
        .unwrap_err();
    assert!(error.contains("cannot start"), "{error}");
    assert!(lifecycle
        .store
        .load()
        .unwrap()
        .layout
        .pending_starts
        .is_empty());
    lifecycle
        .start(workspace(), Path::new("/tmp"), &program())
        .unwrap();
}

#[test]
fn successful_creation_is_kept_and_clears_its_start_record() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let created = lifecycle
        .start(workspace(), Path::new("/tmp"), &program())
        .unwrap();
    let loaded = lifecycle.store.load().unwrap().layout;

    assert!(loaded.shows(&created.terminal));
    assert!(loaded.pending_starts.is_empty());
    assert_eq!(record(&test.core, &created.terminal)["keep"], true);
    assert_eq!(created.workspace.as_deref(), Some("ws0"));
    assert_eq!(created.tab.as_deref(), Some("tab1"));
}

#[test]
fn an_empty_lookup_does_not_clear_or_repeat_a_start() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let first = Lifecycle::new(test.core.clone(), "work");
    let reference = first.store.reserve_start(&workspace()).unwrap();
    let later = Lifecycle::new(test.core.clone(), "work");

    later.recover().unwrap();
    later.recover().unwrap();
    assert!(later
        .store
        .load()
        .unwrap()
        .layout
        .pending_starts
        .contains_key(&reference));
    assert!(later
        .start(workspace(), Path::new("/tmp"), &program())
        .is_err());
    let listed = test
        .core
        .call("terminal.list", json!({"all": true}))
        .unwrap();
    assert!(listed["terminals"].as_array().unwrap().is_empty());

    let terminal = run_unacknowledged(&test, &reference);
    later.recover().unwrap();
    let layout = later.store.load().unwrap().layout;
    assert!(layout.pending_starts.is_empty());
    assert!(layout.shows(&terminal));
    assert_eq!(layout.terminals().count(), 1);
}

#[test]
fn a_fresh_client_recovers_a_start_whose_answer_was_lost() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let first = Lifecycle::new(test.core.clone(), "work");
    let reference = first.store.reserve_start(&workspace()).unwrap();
    let terminal = run_unacknowledged(&test, &reference);
    drop(first);
    let later = Lifecycle::new(test.core.clone(), "work");

    later.recover().unwrap();
    later.recover().unwrap();

    let layout = later.store.load().unwrap().layout;
    assert!(layout.pending_starts.is_empty());
    assert_eq!(layout.terminals().collect::<Vec<_>>(), [terminal.as_str()]);
    let listed = test
        .core
        .call("terminal.list", json!({"all": true}))
        .unwrap();
    assert_eq!(listed["terminals"].as_array().unwrap().len(), 1);
}

#[test]
fn concurrent_recovery_places_a_start_once() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let first = Lifecycle::new(test.core.clone(), "work");
    let reference = first.store.reserve_start(&workspace()).unwrap();
    let terminal = run_unacknowledged(&test, &reference);
    std::thread::scope(|scope| {
        let a = scope.spawn(|| Lifecycle::new(test.core.clone(), "work").recover());
        let b = scope.spawn(|| Lifecycle::new(test.core.clone(), "work").recover());
        a.join().unwrap().unwrap();
        b.join().unwrap().unwrap();
    });

    let layout = first.store.load().unwrap().layout;
    assert_eq!(layout.workspaces.len(), 1);
    assert_eq!(layout.terminals().collect::<Vec<_>>(), [terminal.as_str()]);
}

#[test]
fn another_session_cannot_take_an_unconfirmed_start() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let original = Lifecycle::new(test.core.clone(), "original");
    let reference = original.store.reserve_start(&workspace()).unwrap();
    let terminal = run_unacknowledged(&test, &reference);
    let other = Lifecycle::new(test.core.clone(), "other");

    assert!(other
        .unpark(&terminal, None)
        .unwrap_err()
        .contains("unconfirmed"));
    assert!(!other.store.load().unwrap().layout.shows(&terminal));
    original.recover().unwrap();
    assert!(original.store.load().unwrap().layout.shows(&terminal));
    assert_eq!(record(&test.core, &terminal)["state"], "live");
}

#[test]
fn a_drop_reports_the_start_it_removed_after_a_final_reply() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let reference = lifecycle.store.reserve_start(&workspace()).unwrap();
    lifecycle
        .store
        .edit(|layout| {
            if let Some(start) = layout.pending_starts.get_mut(&reference) {
                start.reply_received = true;
            }
        })
        .unwrap();

    lifecycle.drop_start(&reference).unwrap();

    let layout = lifecycle.store.load().unwrap().layout;
    assert!(layout.pending_starts.is_empty());
}

#[test]
fn a_drop_queues_the_close_of_a_terminal_whose_destination_is_gone() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let first = lifecycle
        .start(workspace(), Path::new("/tmp"), &program())
        .unwrap();
    let reference = lifecycle
        .store
        .reserve_start(&Destination::Split {
            beside: first.terminal.clone(),
            direction: Direction::Right,
            ratio: 0.5,
        })
        .unwrap();
    lifecycle.park(&first.terminal).unwrap();
    let terminal = run_unacknowledged(&test, &reference);

    let error = lifecycle.drop_start(&reference).unwrap_err();

    assert!(error.contains("reserved for closing"), "{error}");
    let layout = lifecycle.store.load().unwrap().layout;
    assert!(layout.pending_starts.is_empty());
    assert!(layout.closing.contains(&terminal));
    assert_eq!(record(&test.core, &terminal)["state"], "live");
}

#[test]
fn a_vanished_destination_reserves_cleanup_before_killing() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let first = lifecycle
        .start(workspace(), Path::new("/tmp"), &program())
        .unwrap();
    let reference = lifecycle
        .store
        .reserve_start(&Destination::Split {
            beside: first.terminal.clone(),
            direction: Direction::Right,
            ratio: 0.5,
        })
        .unwrap();
    lifecycle.park(&first.terminal).unwrap();
    let terminal = run_unacknowledged(&test, &reference);

    assert!(lifecycle
        .recover()
        .unwrap_err()
        .contains("reserved for closing"));
    let layout = lifecycle.store.load().unwrap().layout;
    assert!(layout.pending_starts.is_empty());
    assert!(layout.closing.contains(&terminal));
    assert_eq!(record(&test.core, &terminal)["state"], "live");
    assert!(Lifecycle::new(test.core.clone(), "other")
        .unpark(&terminal, None)
        .is_err());

    lifecycle.recover().unwrap();
    assert!(!lifecycle
        .store
        .load()
        .unwrap()
        .layout
        .closing
        .contains(&terminal));
    assert_ne!(record(&test.core, &terminal)["state"], "live");
}

#[test]
fn a_saved_close_is_finished_by_another_client() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let original = Lifecycle::new(test.core.clone(), "work");
    let created = original
        .start(workspace(), Path::new("/tmp"), &program())
        .unwrap();
    original
        .store
        .edit(|layout| {
            layout.remove(&created.terminal);
            layout.closing.insert(created.terminal.clone());
        })
        .unwrap();

    let later = Lifecycle::new(test.core.clone(), "work");
    later.recover().unwrap();

    assert!(later.store.load().unwrap().layout.closing.is_empty());
    assert_ne!(record(&test.core, &created.terminal)["state"], "live");
}

#[test]
fn explicit_unpark_retries_reconcile_keep_without_another_pane() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let first = lifecycle
        .start(workspace(), Path::new("/tmp"), &program())
        .unwrap();
    let second = lifecycle
        .start(
            Destination::Split {
                beside: first.terminal.clone(),
                direction: Direction::Right,
                ratio: 0.5,
            },
            Path::new("/tmp"),
            &program(),
        )
        .unwrap();
    for to in [
        Placement::Beside {
            terminal: first.terminal.clone(),
            direction: Direction::Down,
        },
        Placement::NewTab {
            workspace: first.workspace.clone(),
        },
    ] {
        lifecycle.park(&second.terminal).unwrap();
        assert_eq!(record(&test.core, &second.terminal)["keep"], false);
        lifecycle
            .unpark(&second.terminal, Some(to.clone()))
            .unwrap();
        test.core
            .call(
                "terminal.keep",
                json!({"terminal": second.terminal, "keep": false}),
            )
            .unwrap();
        lifecycle.unpark(&second.terminal, Some(to)).unwrap();
        assert_eq!(record(&test.core, &second.terminal)["keep"], true);
        assert_eq!(
            lifecycle.store.load().unwrap().layout.terminals().count(),
            2
        );
    }
    lifecycle.close(&second.terminal).unwrap();
    assert_ne!(record(&test.core, &second.terminal)["state"], "live");
}

#[test]
fn a_known_claim_is_never_killed_by_start_recovery() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let reference = lifecycle.store.reserve_start(&workspace()).unwrap();
    let terminal = run_unacknowledged(&test, &reference);
    Store::new(test.core.clone(), "other", None)
        .edit(|layout| {
            layout.add_workspace(None, terminal.clone());
        })
        .unwrap();

    assert!(lifecycle.recover().unwrap_err().contains("already owned"));
    assert_eq!(record(&test.core, &terminal)["state"], "live");
    assert!(lifecycle
        .store
        .load()
        .unwrap()
        .layout
        .pending_starts
        .contains_key(&reference));
}

#[test]
fn an_arrangement_that_finds_nothing_to_change_succeeds_without_a_write() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let (_, before) = lifecycle
        .store
        .edit(|layout| {
            layout.add_workspace(None, "t1".to_owned());
        })
        .unwrap();
    let edge = crate::session_doc::Arrangement::Swap {
        terminal: "t1".to_owned(),
        toward: crate::layout::NavDirection::Right,
        area: ratatui::layout::Rect::new(0, 0, 80, 24),
    };

    assert_eq!(lifecycle.arrange(&edge), Ok(None));
    assert_eq!(lifecycle.store.load().unwrap().version, before.version);
}

#[test]
fn an_arrangement_whose_tab_or_pane_is_gone_fails_without_a_write() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let (_, before) = lifecycle
        .store
        .edit(|layout| {
            layout.add_workspace(None, "t1".to_owned());
        })
        .unwrap();
    let rename = crate::session_doc::Arrangement::NameTab {
        tab: "closed".to_owned(),
        name: "renamed".to_owned(),
    };
    let swap = crate::session_doc::Arrangement::Swap {
        terminal: "closed".to_owned(),
        toward: crate::layout::NavDirection::Right,
        area: ratatui::layout::Rect::new(0, 0, 80, 24),
    };

    assert!(lifecycle.arrange(&rename).is_err());
    assert!(lifecycle.arrange(&swap).is_err());
    assert_eq!(lifecycle.store.load().unwrap().version, before.version);
}

#[test]
fn a_resize_held_at_the_ratio_limit_reports_no_write() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let at_limit: crate::session_doc::Layout = serde_json::from_value(serde_json::json!({
        "workspaces": [{"id": "w0", "tabs": [{"id": "t0", "root": {
            "kind": "split", "direction": "right", "ratio": 0.9,
            "first": {"kind": "pane", "terminal": "a"},
            "second": {"kind": "pane", "terminal": "b"},
        }}]}]
    }))
    .unwrap();
    let (_, before) = lifecycle
        .store
        .edit(|layout| *layout = at_limit.clone())
        .unwrap();
    let wider = crate::session_doc::Arrangement::Resize {
        terminal: "a".to_owned(),
        toward: crate::layout::NavDirection::Right,
        area: ratatui::layout::Rect::new(0, 0, 80, 24),
    };

    assert_eq!(lifecycle.arrange(&wider), Ok(None));
    assert_eq!(lifecycle.store.load().unwrap().version, before.version);
}

#[test]
fn an_arrangement_that_writes_returns_the_version_it_wrote() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    lifecycle
        .store
        .edit(|layout| {
            layout.add_workspace(None, "t1".to_owned());
        })
        .unwrap();
    let tab = lifecycle.store.load().unwrap().layout.workspaces[0].tabs[0]
        .id
        .clone();
    let rename = crate::session_doc::Arrangement::NameTab {
        tab,
        name: "renamed".to_owned(),
    };

    let written = lifecycle.arrange(&rename);

    assert_eq!(written, Ok(Some(lifecycle.store.load().unwrap().version)));
}
