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
        cwd: Some(Path::new("/tmp").to_owned()),
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
    commands::workspace_create(core, SESSION, Some("main".to_owned()), None, &sleeper())
        .expect("workspace create")
}

fn pending_start(core: &Arc<Core>) -> String {
    crate::session_doc::Store::new(core.clone(), SESSION, None)
        .reserve_start(&crate::session_doc::Destination::Workspace { name: None })
        .expect("reserve start")
}

fn started_for(core: &Core, reference: &str) -> String {
    core.run(
        json!({"argv": sleeper().argv, "keep": true, "no_parent": true,
        "labels": {"lawn-start": reference}}),
    )
    .expect("terminal.run")
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
    let reference = pending_start(&test.core);
    let terminal = started_for(&test.core, &reference);

    let error =
        commands::workspace_create(&test.core, SESSION, None, None, &sleeper()).unwrap_err();

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
    let reference = pending_start(&test.core);

    let layout = commands::ls(&test.core, SESSION).unwrap();
    assert!(commands::ls_text(&layout).contains(&reference));
    assert!(
        commands::ls_json(SESSION, &layout)["layout"]["pending_starts"]
            .get(&reference)
            .is_some()
    );
    commands::workspace_create(&test.core, "other", None, None, &sleeper()).unwrap();
    assert!(commands::ls(&test.core, SESSION)
        .unwrap()
        .pending_starts
        .contains_key(&reference));
}

#[test]
fn dropping_a_pending_start_lets_the_session_start_again() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let reference = pending_start(&test.core);
    let refused =
        commands::workspace_create(&test.core, SESSION, None, None, &sleeper()).unwrap_err();
    assert!(
        refused.contains(&format!("start drop {reference}")),
        "{refused}"
    );

    commands::start_drop(&test.core, SESSION, &reference).unwrap();

    assert!(commands::start_drop(&test.core, SESSION, &reference).is_err());
    let created = commands::workspace_create(&test.core, SESSION, None, None, &sleeper()).unwrap();
    let layout = commands::ls(&test.core, SESSION).unwrap();
    assert!(layout.pending_starts.is_empty());
    assert!(layout.shows(&text(&created, "terminal")));
    let listed = test
        .core
        .call("terminal.list", json!({"all": true}))
        .unwrap();
    assert_eq!(listed["terminals"].as_array().unwrap().len(), 1);
}

#[test]
fn a_start_that_recovery_finds_is_placed_instead_of_dropped() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let reference = pending_start(&test.core);
    let terminal = started_for(&test.core, &reference);

    let error = commands::start_drop(&test.core, SESSION, &reference).unwrap_err();

    assert!(error.contains("is not pending"), "{error}");
    let layout = crate::session_doc::Store::new(test.core.clone(), SESSION, None)
        .load()
        .unwrap()
        .layout;
    assert!(layout.shows(&terminal));
    assert_eq!(get(&test.core, &terminal)["state"], "live");
}

#[test]
fn a_start_that_cannot_be_looked_up_is_not_dropped() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let reference = pending_start(&test.core);
    started_for(&test.core, &reference);
    started_for(&test.core, &reference);

    let error = commands::start_drop(&test.core, SESSION, &reference).unwrap_err();

    assert!(error.contains("multiple local terminals"), "{error}");
    let layout = crate::session_doc::Store::new(test.core.clone(), SESSION, None)
        .load()
        .unwrap()
        .layout;
    assert!(layout.pending_starts.contains_key(&reference));
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
        cwd: Some(Path::new("/tmp").to_owned()),
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
    commands::workspace_create(&test.core, "other", None, None, &sleeper()).expect("other session");

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
    commands::workspace_create(&test.core, "other", None, None, &sleeper()).expect("other session");

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
    commands::workspace_create(&test.core, "other", None, None, &sleeper()).expect("other session");

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
    commands::workspace_create(&test.core, "other", None, None, &sleeper()).expect("other session");
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
    commands::workspace_create(&test.core, "other", None, None, &sleeper()).expect("other session");

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

#[cfg(unix)]
fn linked() -> Option<(TestCore, TestCore, crate::hc::test_support::Link)> {
    let (here, there) = (TestCore::start()?, TestCore::start()?);
    let link = here.link(&there, "boxb");
    here.becomes("boxb", "connected");
    Some((here, there, link))
}

#[cfg(unix)]
fn far() -> Program {
    Program {
        cwd: None,
        ..sleeper()
    }
}

#[cfg(unix)]
fn workspace_on_boxb(core: &Arc<Core>) -> Value {
    commands::workspace_create(core, SESSION, None, Some("boxb"), &far()).expect("workspace create")
}

#[cfg(unix)]
#[test]
fn a_workspace_on_a_linked_machine_is_listed_and_changed_under_its_name() {
    let Some((here, there, _link)) = linked() else {
        return;
    };
    let core = &here.core;
    let near = workspace(core);
    let created = workspace_on_boxb(core);
    let workspace = text(&created, "workspace");
    assert_eq!(text(&created, "terminal"), "boxb/t1");
    assert!(workspace.starts_with("boxb/"), "{workspace}");

    let tab = commands::tab_create(core, SESSION, Some(&workspace), None, &far()).unwrap();
    let split =
        commands::pane_split(core, SESSION, "boxb/t1", Direction::Right, 0.5, &far()).unwrap();

    assert_eq!(text(&tab, "terminal"), "boxb/t2");
    assert!(text(&tab, "tab").starts_with("boxb/"), "{tab}");
    assert_eq!(text(&split, "terminal"), "boxb/t3");
    let layout = commands::ls(core, SESSION).unwrap();
    let ids: Vec<&str> = layout.workspaces.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, [text(&near, "workspace"), workspace]);
    for shown in ["t1", "boxb/t1", "boxb/t2", "boxb/t3"] {
        assert!(layout.shows(shown), "{shown}: {layout:?}");
    }
    let error = commands::pane_close(core, SESSION, "boxb/t9").unwrap_err();
    assert_eq!(error, "on boxb: t9 is not in session live");
    commands::pane_close(core, SESSION, "boxb/t1").unwrap();
    assert_eq!(get(core, "t1")["state"], "live");
    assert_ne!(get(&there.core, "t1")["state"], "live");
    let there = commands::ls(&there.core, SESSION).unwrap();
    assert!(there.shows("t2") && there.shows("t3"), "{there:?}");
}

#[cfg(unix)]
#[test]
fn a_parked_pane_returns_only_to_a_place_on_its_own_machine() {
    let Some((here, _there, _link)) = linked() else {
        return;
    };
    let core = &here.core;
    let near = workspace(core);
    let far = workspace_on_boxb(core);
    commands::park(core, SESSION, "boxb/t1").unwrap();
    let beside = |terminal: &str| Placement::Beside {
        terminal: terminal.to_owned(),
        direction: Direction::Right,
    };
    let tab = |workspace: String| Placement::NewTab {
        workspace: Some(workspace),
    };

    for elsewhere in [beside("t1"), tab(text(&near, "workspace"))] {
        let error = commands::unpark(core, SESSION, "boxb/t1", Some(elsewhere)).unwrap_err();
        assert!(error.contains("another machine"), "{error}");
    }
    let error = commands::unpark(core, SESSION, "t1", Some(beside("boxb/t1"))).unwrap_err();
    assert!(error.contains("another machine"), "{error}");

    let shown = commands::unpark(core, SESSION, "boxb/t1", Some(tab(text(&far, "workspace"))));
    assert_eq!(text(&shown.unwrap(), "terminal"), "boxb/t1");
    assert!(commands::ls(core, SESSION).unwrap().shows("boxb/t1"));
}

#[cfg(unix)]
#[test]
fn a_machine_that_is_away_or_removed_is_left_out_of_the_listing() {
    let Some((here, _there, link)) = linked() else {
        return;
    };
    let core = &here.core;
    workspace(core);
    workspace_on_boxb(core);
    link.cut();
    here.becomes("boxb", "unreachable");

    let away = commands::ls(core, SESSION).unwrap();
    let error = commands::pane_close(core, SESSION, "boxb/t1").unwrap_err();

    assert_eq!(away.workspaces.len(), 1);
    assert!(away.shows("t1") && !away.shows("boxb/t1"), "{away:?}");
    assert!(error.contains("unreachable"), "{error}");
    link.mend();
    core.call("machine.connect", json!({"machine": "boxb"}))
        .unwrap();
    here.becomes("boxb", "connected");
    assert!(commands::ls(core, SESSION).unwrap().shows("boxb/t1"));
    core.call("machine.rm", json!({"machine": "boxb"})).unwrap();
    let removed = commands::ls(core, SESSION).unwrap();
    assert_eq!(removed.workspaces.len(), 1);
    let error = commands::pane_close(core, SESSION, "boxb/t1").unwrap_err();
    assert!(error.contains("boxb"), "{error}");
}

#[cfg(unix)]
#[test]
fn a_machine_named_like_the_id_of_another_is_neither_listed_nor_changed() {
    let Some((here, there, _link)) = linked() else {
        return;
    };
    let Some(third) = TestCore::start() else {
        return;
    };
    let core = &here.core;
    workspace_on_boxb(core);
    let id = core.machines().unwrap().remove(0).id;
    let _third = here.link(&third, &id);
    here.becomes(&id, "connected");
    let terminals = |test: &TestCore| {
        let listed = test.core.call("terminal.list", json!({"all": true}));
        listed.unwrap()["terminals"].as_array().map(Vec::len)
    };

    let listed = commands::ls(core, SESSION).unwrap();
    let created = commands::workspace_create(core, SESSION, None, Some(&id), &far());
    let closed = commands::pane_close(core, SESSION, &format!("{id}/t1"));
    let attached = commands::core_address(core, &format!("{id}/t1"));
    let local = commands::core_address(core, "here/t1").unwrap();

    assert_eq!(
        Some(&*local),
        core.hello["core"]["id"]
            .as_str()
            .map(|id| format!("{id}/t1"))
            .as_deref()
    );
    assert_eq!(listed.workspaces.len(), 1, "{listed:?}");
    assert!(listed.shows("boxb/t1"));
    for error in [created, closed, attached.map(Value::from)] {
        let error = error.unwrap_err();
        assert!(error.contains("nothing was changed"), "{error}");
    }
    assert_eq!((terminals(&there), terminals(&third)), (Some(1), Some(0)));
    assert_eq!(get(&there.core, "t1")["state"], "live");
    assert!(commands::ls(core, SESSION).unwrap().shows("boxb/t1"));
}

#[cfg(unix)]
#[test]
fn a_start_left_pending_on_a_machine_is_dropped_under_its_name() {
    let Some((here, _there, _link)) = linked() else {
        return;
    };
    let core = &here.core;
    let reference = crate::session_doc::Store::new(core.clone(), SESSION, Some("boxb".to_owned()))
        .reserve_start(&crate::session_doc::Destination::Workspace { name: None })
        .unwrap();
    let typed = format!("boxb/{reference}");

    let listed = commands::ls(core, SESSION).unwrap();
    let refused =
        commands::workspace_create(core, SESSION, None, Some("boxb"), &far()).unwrap_err();

    assert!(listed.pending_starts.contains_key(&typed), "{listed:?}");
    assert!(
        refused.contains(&format!("start drop {typed}")),
        "{refused}"
    );
    workspace(core);
    let error = commands::start_drop(core, SESSION, &reference).unwrap_err();
    assert!(error.contains("is not pending"), "{error}");
    commands::start_drop(core, SESSION, &typed).unwrap();
    assert!(commands::ls(core, SESSION)
        .unwrap()
        .pending_starts
        .is_empty());
    workspace_on_boxb(core);
}
