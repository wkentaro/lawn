use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::super::commands::{self, Program};
use crate::hc::test_support::TestCore;
use crate::hc::Core;
use crate::session_doc::{Direction, Node, Placement};

const SESSION: &str = "live";

fn sleeper() -> Program {
    Program {
        cwd: Path::new("/tmp").to_owned(),
        argv: ["sh", "-c", "sleep 30"].map(str::to_owned).to_vec(),
    }
}

fn get(core: &Core, terminal: &str) -> Value {
    core.call("terminal.get", json!({"terminal": terminal}))
        .expect("terminal.get")
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().expect(key).to_owned()
}

fn workspace(core: &Arc<Core>) -> Value {
    commands::workspace_create(core, SESSION, Some("main".to_owned()), &sleeper())
        .expect("workspace create")
}

fn split_right(core: &Arc<Core>, beside: &str) -> String {
    let split = commands::pane_split(core, SESSION, beside, Direction::Right, 0.5, &sleeper())
        .expect("pane split");
    text(&split, "terminal")
}

#[test]
fn retrying_a_pending_create_recovers_without_starting_a_second_program() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let store = crate::session_doc::Store::new(test.core.clone(), SESSION, None);
    let reference = store
        .reserve_start(&crate::session_doc::Destination::Workspace { name: None })
        .unwrap();
    let terminal = test
        .core
        .run(
            json!({"argv": sleeper().argv, "keep": true, "no_parent": true,
        "labels": {"lawn-start": reference}}),
        )
        .unwrap();

    let error = commands::workspace_create(&test.core, SESSION, None, &sleeper()).unwrap_err();

    assert!(error.contains("no new program was started"), "{error}");
    let layout = commands::ls(&test.core, SESSION).unwrap();
    assert!(layout.pending_starts.is_empty());
    assert_eq!(layout.terminals().collect::<Vec<_>>(), [terminal.as_str()]);
    let listed = test
        .core
        .call("terminal.list", json!({"all": true}))
        .unwrap();
    assert_eq!(listed["terminals"].as_array().unwrap().len(), 1);
}

#[test]
fn listing_an_unresolved_start_keeps_it_visible_without_blocking_other_sessions() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let store = crate::session_doc::Store::new(test.core.clone(), SESSION, None);
    let reference = store
        .reserve_start(&crate::session_doc::Destination::Workspace { name: None })
        .unwrap();

    let layout = commands::ls(&test.core, SESSION).unwrap();
    assert!(commands::ls_text(&layout).contains(&reference));
    assert!(
        commands::ls_json(SESSION, &layout)["layout"]["pending_starts"]
            .get(&reference)
            .is_some()
    );
    commands::workspace_create(&test.core, "other", None, &sleeper()).unwrap();
    assert!(commands::ls(&test.core, SESSION)
        .unwrap()
        .pending_starts
        .contains_key(&reference));
}

#[test]
fn workspace_create_starts_a_kept_terminal() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let created = workspace(&test.core);
    let terminal = text(&created, "terminal");
    let record = get(&test.core, &terminal);
    assert_eq!(record["state"], "live");
    assert_eq!(record["keep"], true);

    let layout = commands::ls(&test.core, SESSION).expect("ls");
    assert_eq!(layout.workspaces[0].id, text(&created, "workspace"));
    assert_eq!(layout.workspaces[0].tabs[0].id, text(&created, "tab"));
    assert_eq!(layout.terminals().collect::<Vec<_>>(), [terminal.as_str()]);
}

#[test]
fn split_returns_a_terminal_the_core_knows() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let first = text(&workspace(&test.core), "terminal");
    let second = split_right(&test.core, &first);
    assert_eq!(get(&test.core, &second)["state"], "live");
    let layout = commands::ls(&test.core, SESSION).expect("ls");
    assert_eq!(
        layout.terminals().collect::<Vec<_>>(),
        [first.as_str(), second.as_str()]
    );
}

#[test]
fn invalid_placement_starts_nothing() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let err = commands::tab_create(&test.core, SESSION, None, None, &sleeper())
        .expect_err("no workspace");
    assert!(err.contains("no workspace"), "{err}");
    let started = || {
        let list = test
            .core
            .call("terminal.list", json!({"all": true}))
            .expect("terminal.list");
        list["terminals"].as_array().cloned().unwrap_or_default()
    };
    assert!(started().is_empty());
}

#[test]
fn park_and_unpark_round_trip_beside_the_same_terminal() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let created = workspace(&test.core);
    let first = text(&created, "terminal");
    let second = split_right(&test.core, &first);

    commands::park(&test.core, SESSION, &second).expect("park");
    assert_eq!(get(&test.core, &second)["keep"], false);
    let layout = commands::ls(&test.core, SESSION).expect("ls");
    assert!(!layout.shows(&second));
    assert!(layout.parked.contains(&second));
    assert_eq!(layout.workspaces[0].id, text(&created, "workspace"));
    assert_eq!(layout.workspaces[0].tabs[0].id, text(&created, "tab"));
    assert!(layout.workspaces[0].tabs[0].root.contains(&second));

    commands::unpark(&test.core, SESSION, &second, None).expect("unpark");
    assert_eq!(get(&test.core, &second)["keep"], true);
    let layout = commands::ls(&test.core, SESSION).expect("ls");
    assert!(layout.parked.is_empty());
    let Node::Split {
        first: left,
        second: right,
        ..
    } = &layout.workspaces[0].tabs[0].root
    else {
        panic!("not split");
    };
    assert!(matches!(&**left, Node::Pane { terminal } if *terminal == first));
    assert!(matches!(&**right, Node::Pane { terminal } if *terminal == second));
}

#[test]
fn a_refused_unpark_leaves_keep_off() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let first = text(&workspace(&test.core), "terminal");
    let second = split_right(&test.core, &first);
    commands::park(&test.core, SESSION, &second).expect("park");
    let to = Placement::NewTab {
        workspace: Some("w9".to_owned()),
    };
    assert!(commands::unpark(&test.core, SESSION, &second, Some(to)).is_err());
    assert_eq!(get(&test.core, &second)["keep"], false);
}

#[test]
fn a_finished_terminal_parks_unparks_and_closes() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let first = text(&workspace(&test.core), "terminal");
    let quick = Program {
        cwd: Path::new("/tmp").to_owned(),
        argv: ["sh", "-c", "exit 0"].map(str::to_owned).to_vec(),
    };
    let split = commands::pane_split(&test.core, SESSION, &first, Direction::Right, 0.5, &quick)
        .expect("pane split");
    let second = text(&split, "terminal");
    let deadline = Instant::now() + Duration::from_secs(10);
    while get(&test.core, &second)["state"] == "live" {
        assert!(Instant::now() < deadline, "the program never ended");
        std::thread::sleep(Duration::from_millis(20));
    }

    commands::park(&test.core, SESSION, &second).expect("park");
    commands::unpark(&test.core, SESSION, &second, None).expect("unpark");
    assert!(commands::ls(&test.core, SESSION)
        .expect("ls")
        .shows(&second));
    commands::pane_close(&test.core, SESSION, &second).expect("close");
    assert!(!commands::ls(&test.core, SESSION)
        .expect("ls")
        .shows(&second));
}

#[test]
fn unparking_an_unknown_terminal_changes_nothing() {
    let Some(test) = TestCore::start() else {
        return;
    };
    workspace(&test.core);
    let before = commands::ls(&test.core, SESSION).expect("ls");
    assert!(commands::unpark(&test.core, SESSION, "t999999", None).is_err());
    assert_eq!(commands::ls(&test.core, SESSION).expect("ls"), before);
}

#[test]
fn a_terminal_shown_in_another_session_cannot_be_unparked() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let first = text(&workspace(&test.core), "terminal");
    commands::workspace_create(&test.core, "other", None, &sleeper()).expect("other session");

    assert!(commands::unpark(&test.core, "other", &first, None).is_err());
    assert!(!commands::ls(&test.core, "other").expect("ls").shows(&first));
    assert_eq!(get(&test.core, &first)["keep"], true);
}

#[test]
fn unparking_a_terminal_another_session_parked_takes_it_over() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let first = text(&workspace(&test.core), "terminal");
    let second = split_right(&test.core, &first);
    commands::park(&test.core, SESSION, &second).expect("park");
    commands::workspace_create(&test.core, "other", None, &sleeper()).expect("other session");

    commands::unpark(&test.core, "other", &second, None).expect("unpark elsewhere");

    assert!(commands::ls(&test.core, "other")
        .expect("ls")
        .shows(&second));
    assert!(!commands::ls(&test.core, SESSION).expect("ls").owns(&second));
    assert_eq!(get(&test.core, &second)["keep"], true);
}

#[test]
fn a_failed_takeover_leaves_the_other_session_parked() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let first = text(&workspace(&test.core), "terminal");
    let second = split_right(&test.core, &first);
    commands::park(&test.core, SESSION, &second).expect("park");
    commands::workspace_create(&test.core, "other", None, &sleeper()).expect("other session");

    let to = Placement::NewTab {
        workspace: Some("ws999".to_owned()),
    };
    assert!(commands::unpark(&test.core, "other", &second, Some(to)).is_err());

    assert!(commands::ls(&test.core, SESSION)
        .expect("ls")
        .parked
        .contains(&second));
    assert!(!commands::ls(&test.core, "other")
        .expect("ls")
        .shows(&second));
    assert_eq!(get(&test.core, &second)["keep"], false);
}

#[test]
fn a_stale_park_keeps_a_terminal_another_session_shows() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let first = text(&workspace(&test.core), "terminal");
    let second = split_right(&test.core, &first);
    commands::park(&test.core, SESSION, &second).expect("park");
    commands::workspace_create(&test.core, "other", None, &sleeper()).expect("other session");
    commands::unpark(&test.core, "other", &second, None).expect("unpark elsewhere");

    crate::core_lifecycle::Lifecycle::new(test.core.clone(), SESSION)
        .settle_keep(&second)
        .expect("settle keep");

    assert_eq!(get(&test.core, &second)["keep"], true);
}

#[test]
fn a_terminal_being_closed_cannot_be_claimed() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let first = text(&workspace(&test.core), "terminal");
    let second = split_right(&test.core, &first);
    commands::park(&test.core, SESSION, &second).expect("park");
    crate::session_doc::Store::new(test.core.clone(), SESSION, None)
        .edit(|layout| {
            layout.remove(&second);
            layout.closing.insert(second.clone());
        })
        .expect("reserve");
    commands::workspace_create(&test.core, "other", None, &sleeper()).expect("other session");

    assert!(commands::unpark(&test.core, "other", &second, None).is_err());
    assert!(!commands::ls(&test.core, "other")
        .expect("ls")
        .shows(&second));
    commands::pane_close(&test.core, SESSION, &second).expect("finish the close");
    assert_eq!(get(&test.core, &second)["state"], "finished");
    assert!(commands::ls(&test.core, SESSION)
        .expect("ls")
        .closing
        .is_empty());
}

#[test]
fn repeating_park_or_unpark_finishes_keep() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let first = text(&workspace(&test.core), "terminal");
    let second = split_right(&test.core, &first);
    let set_keep = |keep: bool| {
        test.core
            .call("terminal.keep", json!({"terminal": second, "keep": keep}))
            .expect("terminal.keep");
    };

    commands::park(&test.core, SESSION, &second).expect("park");
    set_keep(true);
    commands::park(&test.core, SESSION, &second).expect("park again");
    assert_eq!(get(&test.core, &second)["keep"], false);

    commands::unpark(&test.core, SESSION, &second, None).expect("unpark");
    set_keep(false);
    commands::unpark(&test.core, SESSION, &second, None).expect("unpark again");
    assert_eq!(get(&test.core, &second)["keep"], true);
}

#[test]
fn pane_close_kills_the_terminal_and_collapses_the_split() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let first = text(&workspace(&test.core), "terminal");
    let second = split_right(&test.core, &first);

    commands::pane_close(&test.core, SESSION, &second).expect("pane close");
    let deadline = Instant::now() + Duration::from_secs(5);
    let record = loop {
        let record = get(&test.core, &second);
        if record["state"] == "finished" || Instant::now() > deadline {
            break record;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(record["state"], "finished");
    assert_eq!(record["reason"], "killed");
    let layout = commands::ls(&test.core, SESSION).expect("ls");
    assert_eq!(
        layout.workspaces[0].tabs[0].root,
        Node::Pane { terminal: first }
    );
}

#[test]
fn parking_the_last_pane_keeps_its_workspace_and_starts_nothing() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let created = workspace(&test.core);
    let first = text(&created, "terminal");
    let before = commands::ls(&test.core, SESSION).expect("ls").workspaces;

    commands::park(&test.core, SESSION, &first).expect("park");

    let layout = commands::ls(&test.core, SESSION).expect("ls");
    assert!(!layout.shows(&first));
    assert_eq!(layout.workspaces, before);
    assert_eq!(get(&test.core, &first)["keep"], false);
    let listed = test
        .core
        .call("terminal.list", json!({"all": true}))
        .expect("list");
    assert_eq!(listed["terminals"].as_array().map(Vec::len), Some(1));
}
