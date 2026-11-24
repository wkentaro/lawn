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

fn wait_for(core: &Core, terminal: &str, kind: &str, then: impl FnOnce()) {
    wait_until(core, terminal, then, |params| params["type"] == kind);
}

fn wait_until(
    core: &Core,
    terminal: &str,
    then: impl FnOnce(),
    done: impl Fn(&Value) -> bool,
) -> Value {
    let mut events = core.subscribe(json!({"terminal": terminal})).unwrap();
    then();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let deadline = tokio::time::Instant::now() + EVENT_DEADLINE;
    let mut pending = Vec::new();
    loop {
        let next = runtime
            .block_on(async { tokio::time::timeout_at(deadline, events.stream.recv()).await });
        match next {
            Ok(Some(crate::hc::Stream::Data(bytes))) => pending.extend_from_slice(&bytes),
            Ok(Some(_)) => {}
            Ok(None) => panic!("the events of {terminal} ended first"),
            Err(_) => panic!("{terminal} sent no awaited event within {EVENT_DEADLINE:?}"),
        }
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let message: Value = serde_json::from_slice(&line).unwrap_or_default();
            if done(&message["params"]) {
                return message["params"].clone();
            }
        }
    }
}

const EVENT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

fn core_with(inherited: &[(&str, &str)]) -> Option<TestCore> {
    let pinned: Vec<(&str, &str)> = inherited
        .iter()
        .copied()
        .chain([("SHELL", "/bin/sh")])
        .collect();
    let previous: Vec<_> = pinned
        .iter()
        .map(|(name, value)| {
            let previous = std::env::var_os(name);
            std::env::set_var(name, value);
            (*name, previous)
        })
        .collect();
    let test = TestCore::start();
    for (name, previous) in previous {
        match previous {
            Some(previous) => std::env::set_var(name, previous),
            None => std::env::remove_var(name),
        }
    }
    test
}

fn recovered_agent(
    test: &TestCore,
    lifecycle: &Lifecycle,
    word: &str,
) -> (String, crate::hc::PendingRequest) {
    let terminal = lifecycle
        .start(workspace(), Path::new("/tmp"), &["sh".to_owned()])
        .unwrap()
        .terminal;
    let before = record(&test.core, &terminal);
    let pid = u32::try_from(before["pid"].as_u64().unwrap()).unwrap();
    let report = json!({
        "source": "test", "agent": "printer", "reporter_id": "r", "seq": 1, "cwd": "/tmp",
        "argv": ["printf", "%s-%s\\n", "lawn", word],
        "program": {"pid": pid, "started_ms": crate::platform::process_start_token(pid)},
    });
    test.core
        .doc_set(
            &format!("{}-resume-{terminal}", crate::config::app_dir_name()),
            None,
            &json!({
                "schema": 1, "record_only": false, "reports": [report],
                "terminal": {"address": terminal, "created_ms": before["created_ms"]},
            }),
            0,
        )
        .unwrap();
    wait_for(&test.core, &terminal, "restarted", || {
        crate::platform::signal_processes(&[pid], crate::platform::Signal::Kill)
    });
    let printed = test
        .core
        .request(
            "terminal.wait",
            json!({"terminal": terminal, "output": format!("lawn-{word}")}),
        )
        .unwrap();
    (terminal, printed)
}

fn typed(test: &TestCore, terminal: &str, line: &str) -> usize {
    let done = test
        .core
        .request(
            "terminal.wait",
            json!({"terminal": terminal, "output": "lawn-done"}),
        )
        .unwrap();
    test.core
        .call(
            "terminal.send",
            json!({"terminal": terminal, "text": "printf '%s-%s\\n' lawn done"}),
        )
        .unwrap();
    done.wait(std::time::Duration::from_secs(30)).unwrap();
    let screen = test
        .core
        .call("terminal.read", json!({"terminal": terminal, "lines": 100}))
        .unwrap();
    screen["text"]
        .as_str()
        .unwrap()
        .lines()
        .filter(|text| text.trim() == line)
        .count()
}

fn sleeping_pace(spacing: std::time::Duration, mut on_wait: impl FnMut() + Send + 'static) -> Pace {
    let start = std::time::Instant::now();
    Pace {
        spacing,
        elapsed: Box::new(move || start.elapsed()),
        wait: Box::new(move |wait| {
            on_wait();
            std::thread::sleep(wait);
            true
        }),
    }
}

#[test]
fn two_clients_of_one_session_send_resumes_no_closer_than_the_spacing() {
    let Some(test) = core_with(&[]) else {
        return;
    };
    let spacing = std::time::Duration::from_secs(1);
    let clients = ["one", "two"].map(|word| {
        let lifecycle = Lifecycle::new(test.core.clone(), "work");
        let (terminal, printed) = recovered_agent(&test, &lifecycle, word);
        (lifecycle, terminal, printed)
    });

    let arrivals: Vec<std::time::Instant> = std::thread::scope(|scope| {
        let waiting: Vec<_> = clients
            .into_iter()
            .map(|(lifecycle, terminal, printed)| {
                scope.spawn(move || {
                    lifecycle
                        .resume_agents(Some(&terminal), &mut sleeping_pace(spacing, || {}))
                        .unwrap();
                });
                scope.spawn(move || {
                    printed.wait(std::time::Duration::from_secs(30)).unwrap();
                    std::time::Instant::now()
                })
            })
            .collect();
        waiting
            .into_iter()
            .map(|arrival| arrival.join().unwrap())
            .collect()
    });
    let gap = arrivals[0].max(arrivals[1]) - arrivals[0].min(arrivals[1]);
    assert!(gap >= spacing.mul_f32(0.9), "{gap:?}");
}

#[test]
fn a_pane_moved_to_another_session_during_the_wait_is_not_claimed() {
    let Some(test) = core_with(&[]) else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let (terminal, printed) = recovered_agent(&test, &lifecycle, "moved");
    let pace_name = format!("{}-resume-pace", crate::config::app_dir_name());
    test.core
        .doc_set(&pace_name, None, &json!({"work": {"sent": 1}}), 0)
        .unwrap();
    let core = test.core.clone();
    let moving = terminal.clone();
    let mut waits = 0;
    let mut pace = sleeping_pace(std::time::Duration::from_millis(1), move || {
        waits += 1;
        if waits == 2 {
            Lifecycle::new(core.clone(), "work").park(&moving).unwrap();
            Lifecycle::new(core.clone(), "play")
                .unpark(&moving, None)
                .unwrap();
        }
    });
    let name = format!("{}-resume-{terminal}", crate::config::app_dir_name());
    let before = test.core.doc_get(&name, None).unwrap();

    lifecycle.resume_agents(Some(&terminal), &mut pace).unwrap();

    let after = test.core.doc_get(&name, None).unwrap();
    assert_eq!(after.version, before.version);
    assert_eq!(after.value["reports"], before.value["reports"]);

    let (_stop, stopped) = std::sync::mpsc::channel();
    Lifecycle::new(test.core.clone(), "play")
        .resume_agents(
            Some(&terminal),
            &mut Pace::new(std::time::Duration::ZERO, stopped),
        )
        .unwrap();
    printed.wait(std::time::Duration::from_secs(30)).unwrap();
    assert_eq!(typed(&test, &terminal, "lawn-moved"), 1);
}

#[test]
fn a_recovered_shell_types_the_lost_agents_resume_command_once() {
    let Some(test) = core_with(&[]) else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let (terminal, printed) = recovered_agent(&test, &lifecycle, "resumed");

    let (requests, queue) = std::sync::mpsc::channel();
    requests.send(None).unwrap();
    requests.send(Some(terminal.clone())).unwrap();
    drop(requests);
    let (_stop, stopped) = std::sync::mpsc::channel();
    crate::core_lifecycle::drain(
        queue,
        Pace::new(std::time::Duration::ZERO, stopped),
        |only, pace| {
            lifecycle.resume_agents(only.as_deref(), pace).unwrap();
        },
    );
    printed.wait(std::time::Duration::from_secs(30)).unwrap();
    assert_eq!(typed(&test, &terminal, "lawn-resumed"), 1);
}

#[test]
fn an_agent_that_quit_from_a_shown_shell_is_forgotten() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let terminal = lifecycle
        .start(workspace(), Path::new("/tmp"), &["sh".to_owned()])
        .unwrap()
        .terminal;
    let before = record(&test.core, &terminal);
    let pid = u32::try_from(before["pid"].as_u64().unwrap()).unwrap();
    let job = wait_until(
        &test.core,
        &terminal,
        || {
            test.core
                .call(
                    "terminal.send",
                    json!({"terminal": terminal, "text": "cat"}),
                )
                .unwrap();
        },
        |params| {
            params["type"] == "foreground"
                && params["data"]["pid"]
                    .as_u64()
                    .is_some_and(|job| job != u64::from(pid))
        },
    )["data"]["pid"]
        .as_u64()
        .unwrap();
    let job = u32::try_from(job).unwrap();
    let name = format!("{}-resume-{terminal}", crate::config::app_dir_name());
    let report = json!({
        "source": "test", "agent": "printer", "reporter_id": "r", "seq": 1, "cwd": "/tmp",
        "argv": ["printf", "unexpected\\n"],
        "program": {"pid": pid, "started_ms": crate::platform::process_start_token(pid)},
        "recorded": 1, "job": job,
    });
    test.core
        .doc_set(
            &name,
            None,
            &json!({
                "schema": 1, "record_only": false, "reports": [report],
                "terminal": {"address": terminal, "created_ms": before["created_ms"]},
            }),
            0,
        )
        .unwrap();

    wait_until(
        &test.core,
        &terminal,
        || crate::platform::signal_processes(&[job], crate::platform::Signal::Kill),
        |params| params["type"] == "foreground" && params["data"]["pid"] == json!(pid),
    );
    lifecycle.forget_agents(Some(&terminal)).unwrap();
    assert_eq!(
        test.core.doc_get(&name, None).unwrap().value["reports"],
        json!([])
    );
}
