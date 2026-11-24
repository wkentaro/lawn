use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::{
    follow, forget_reported, name_reported, names, rename, reported, resume_reported, roots,
    snapshot, Report, Status, Terminals,
};
use crate::api::schema::AgentStatus;
use crate::core_lifecycle::Lifecycle;
use crate::detect::AgentState;
use crate::hc::test_support::TestCore;
use crate::pane::{core_status, CoreStatus};
use crate::session_doc::Destination;

fn folders(pairs: &[(&str, &str)]) -> HashMap<String, PathBuf> {
    pairs
        .iter()
        .map(|(terminal, folder)| ((*terminal).to_owned(), folder.into()))
        .collect()
}

fn folders_in(report: Option<Report>) -> Option<HashMap<String, PathBuf>> {
    match report? {
        Report::Snapshot { folders, .. } => Some(folders),
        _ => None,
    }
}

#[test]
fn a_snapshot_reports_each_terminal_in_the_folder_it_started_in() {
    let message = json!({"terminals": [
        {"address": "t1", "cwd": "/srv/app", "foreground": null},
        {"address": "t2", "cwd": "/tmp"},
    ]});

    assert_eq!(
        folders_in(Some(snapshot(&message))),
        Some(folders(&[("t1", "/srv/app"), ("t2", "/tmp")]))
    );
}

#[test]
fn a_snapshot_reports_the_folder_the_program_is_in_now_over_the_one_it_started_in() {
    let message = json!({"terminals": [
        {"address": "t1", "cwd": "/srv/app", "foreground": {"cwd": "/srv/app/src", "pid": 7}},
    ]});

    assert_eq!(
        folders_in(Some(snapshot(&message))),
        Some(folders(&[("t1", "/srv/app/src")]))
    );
}

#[test]
fn a_foreground_event_reports_the_folder_the_program_moved_to() {
    let message = json!({"method": "event", "params": {
        "type": "foreground", "terminal": "t1", "data": {"cwd": "/usr/share", "pid": 7},
    }});

    assert_eq!(
        reported(&message),
        Some(Report::Folder("t1".to_owned(), "/usr/share".into()))
    );
}

#[test]
fn a_foreground_event_without_a_folder_reports_nothing() {
    let message = json!({"method": "event", "params": {
        "type": "foreground", "terminal": "t1", "data": {"cwd": null},
    }});

    assert_eq!(reported(&message), None);
}

#[test]
fn a_created_event_reports_the_new_terminal_in_its_folder() {
    let message = json!({"method": "event", "params": {
        "type": "created",
        "terminal": "t3",
        "data": {"address": "t3", "cwd": "/srv/other", "foreground": null},
    }});

    assert_eq!(
        reported(&message),
        Some(Report::Folder("t3".to_owned(), "/srv/other".into()))
    );
}

#[test]
fn a_restarted_terminal_asks_to_resume_its_agent() {
    let message = json!({"method": "event", "params": {
        "type": "restarted", "terminal": "t4", "data": {"address": "t4", "pid": 77},
    }});

    assert_eq!(
        resume_reported(&message),
        Some(Report::Resume(Some("t4".to_owned())))
    );
}

#[test]
fn a_missed_message_asks_to_resume_every_agent() {
    let message = json!({"method": "missed", "params": {"terminals": []}});

    assert_eq!(resume_reported(&message), Some(Report::Resume(None)));
}

#[test]
fn other_events_ask_for_no_resume() {
    for kind in ["lost", "created", "foreground", "status"] {
        let message = json!({"method": "event", "params": {"type": kind, "terminal": "t4"}});
        assert_eq!(resume_reported(&message), None, "{kind}");
    }
}

#[test]
fn hearing_a_resume_or_forget_request_draws_nothing() {
    let mut terminals = Terminals::default();

    assert!(!terminals.hear(Report::Resume(None), |_| true, |_| true, |_| true));
    assert!(!terminals.hear(Report::Forget(None), |_| true, |_| true, |_| true));
}

fn foreground(terminal: &str, pid: u64) -> Value {
    json!({"method": "event", "params": {
        "type": "foreground", "terminal": terminal, "data": {"cwd": "/srv", "pid": pid},
    }})
}

#[test]
fn the_shell_taking_the_foreground_back_asks_to_forget_its_quit_agent() {
    let mut roots = roots(&json!({"terminals": [{"address": "t1", "pid": 7}]}));

    assert_eq!(
        forget_reported(&foreground("t1", 7), &mut roots),
        Some(Report::Forget(Some("t1".to_owned())))
    );
}

#[test]
fn a_job_taking_the_foreground_forgets_nothing() {
    let mut roots = roots(&json!({"terminals": [{"address": "t1", "pid": 7}]}));

    assert_eq!(forget_reported(&foreground("t1", 8), &mut roots), None);
    assert_eq!(forget_reported(&foreground("t2", 7), &mut roots), None);
}

#[test]
fn created_and_restarted_terminals_name_the_root_the_foreground_returns_to() {
    let mut roots = roots(&json!({"terminals": [{"address": "t1", "pid": 7}]}));
    for kind in ["created", "restarted"] {
        let pid = if kind == "created" { 70 } else { 71 };
        let message = json!({"method": "event", "params": {
            "type": kind, "terminal": "t1", "data": {"address": "t1", "pid": pid},
        }});
        assert_eq!(forget_reported(&message, &mut roots), None);
        assert_eq!(forget_reported(&foreground("t1", 7), &mut roots), None);
        assert_eq!(
            forget_reported(&foreground("t1", pid), &mut roots),
            Some(Report::Forget(Some("t1".to_owned())))
        );
    }
}

#[test]
fn a_removed_terminal_leaves_the_roots() {
    let mut roots = roots(&json!({"terminals": [
        {"address": "t1", "pid": 7}, {"address": "t2", "pid": 8},
    ]}));
    let message = json!({"method": "event", "params": {
        "type": "removed", "terminal": "t1", "data": {},
    }});

    assert_eq!(forget_reported(&message, &mut roots), None);
    assert_eq!(roots, HashMap::from([("t2".to_owned(), 8)]));
}

#[test]
fn a_missed_message_asks_to_forget_every_quit_agent_and_relearns_roots() {
    let mut roots = roots(&json!({"terminals": [{"address": "t1", "pid": 7}]}));
    let message = json!({"method": "missed", "params": {"terminals": [
        {"address": "t1", "pid": 9},
    ]}});

    assert_eq!(
        forget_reported(&message, &mut roots),
        Some(Report::Forget(None))
    );
    assert_eq!(forget_reported(&foreground("t1", 7), &mut roots), None);
    assert!(forget_reported(&foreground("t1", 9), &mut roots).is_some());
}

fn said(state: &str) -> CoreStatus {
    core_status(&json!({"state": state, "app": "claude"}))
}

fn stated(pairs: &[(&str, CoreStatus)]) -> Report {
    Report::Snapshot {
        folders: HashMap::new(),
        statuses: pairs
            .iter()
            .map(|(terminal, status)| ((*terminal).to_owned(), *status))
            .collect(),
    }
}

#[test]
fn a_missed_message_reports_one_fresh_snapshot_of_folders_and_statuses() {
    let message = json!({"method": "missed", "params": {"terminals": [
        {"address": "t2", "cwd": "/srv/app", "status": {"state": "idle", "app": "claude"}},
    ]}});

    assert_eq!(
        reported(&message),
        Some(Report::Snapshot {
            folders: folders(&[("t2", "/srv/app")]),
            statuses: [("t2".to_owned(), said("idle"))].into_iter().collect(),
        })
    );
}

fn told(terminals: &mut Terminals, states: &[&str]) -> Option<AgentStatus> {
    for state in states {
        terminals.hear(
            Report::Status("t1".to_owned(), said(state)),
            |_| false,
            |_| true,
            |_| false,
        );
    }
    terminals.status("t1").map(Status::shown)
}

#[test]
fn a_snapshot_reports_each_terminals_status_and_no_status_for_one_without() {
    let message = json!({"terminals": [
        {"address": "t1", "status": {"state": "working", "app": "claude"}},
        {"address": "t2", "status": null},
    ]});

    assert_eq!(
        snapshot(&message),
        stated(&[("t1", said("working")), ("t2", core_status(&json!(null)))])
    );
}

#[test]
fn a_status_event_reports_the_status_the_program_sent() {
    let message = json!({"method": "event", "params": {
        "type": "status", "terminal": "t1", "data": {"status": {"state": "blocked", "app": "claude"}},
    }});

    assert_eq!(
        reported(&message),
        Some(Report::Status("t1".to_owned(), said("blocked")))
    );
}

#[test]
fn a_finish_after_work_or_a_block_reads_done_until_seen() {
    for before in ["working", "blocked"] {
        for finish in ["done", "error"] {
            let mut terminals = Terminals::default();
            assert_eq!(
                told(&mut terminals, &[before, finish]),
                Some(AgentStatus::Done)
            );

            assert!(terminals.see(["t1"]));
            assert_eq!(
                terminals.status("t1").map(Status::shown),
                Some(AgentStatus::Idle)
            );
            assert!(!terminals.see(["t1"]));
        }
    }
}

#[test]
fn an_idle_report_after_an_unseen_finish_keeps_it_done() {
    let mut terminals = Terminals::default();

    assert_eq!(
        told(&mut terminals, &["working", "done", "idle"]),
        Some(AgentStatus::Done)
    );
}

#[test]
fn going_idle_straight_from_work_reads_idle() {
    let mut terminals = Terminals::default();

    assert_eq!(
        told(&mut terminals, &["working", "idle"]),
        Some(AgentStatus::Idle)
    );
}

#[test]
fn a_finish_without_work_before_it_reads_idle() {
    let mut terminals = Terminals::default();

    assert_eq!(told(&mut terminals, &["done"]), Some(AgentStatus::Idle));
}

#[test]
fn work_after_an_unseen_finish_clears_it() {
    let mut terminals = Terminals::default();

    assert_eq!(
        told(&mut terminals, &["working", "done", "working"]),
        Some(AgentStatus::Working)
    );
    assert_eq!(told(&mut terminals, &["idle"]), Some(AgentStatus::Idle));
}

#[test]
fn a_state_lawn_does_not_know_forgets_the_status() {
    let mut terminals = Terminals::default();
    told(&mut terminals, &["working"]);

    assert_eq!(told(&mut terminals, &["napping"]), None);
}

#[test]
fn a_report_draws_only_when_the_agent_or_what_it_shows_changes() {
    let mut terminals = Terminals::default();
    let mut hear = |status: CoreStatus| {
        terminals.hear(
            Report::Status("t1".to_owned(), status),
            |_| false,
            |_| true,
            |_| false,
        )
    };

    assert!(hear(said("working")));
    assert!(!hear(said("working")));
    assert!(hear(core_status(
        &json!({"state": "working", "app": "codex"})
    )));
    assert!(hear(said("idle")));
    assert!(!hear(said("done")));
}

#[test]
fn a_report_for_a_terminal_no_layout_shows_draws_nothing() {
    let mut terminals = Terminals::default();

    assert!(!terminals.hear(
        Report::Status("t1".to_owned(), said("working")),
        |_| false,
        |_| false,
        |_| false
    ));
    assert_eq!(
        terminals.status("t1").map(Status::shown),
        Some(AgentStatus::Working)
    );
}

#[test]
fn a_fresh_status_snapshot_forgets_the_terminals_it_no_longer_lists() {
    let mut terminals = Terminals::default();
    told(&mut terminals, &["working"]);

    assert!(terminals.hear(stated(&[]), |_| false, |_| true, |_| false));
    assert_eq!(terminals.status("t1"), None);
}

fn named(pairs: &[(&str, &str)]) -> Report {
    Report::Names(
        pairs
            .iter()
            .map(|(terminal, name)| ((*terminal).to_owned(), (*name).to_owned()))
            .collect(),
    )
}

#[test]
fn a_snapshot_names_only_the_terminals_with_a_lawn_name_label() {
    let message = json!({"terminals": [
        {"address": "t1", "labels": {"lawn-name": "build", "lawn-start": "r1"}},
        {"address": "t2", "labels": {"lawn": "core", "name": "other"}},
        {"address": "t3", "labels": {}},
    ]});

    assert_eq!(names(&message), named(&[("t1", "build")]));
}

#[test]
fn a_labels_event_reports_the_name_it_now_carries_or_none_when_it_lost_it() {
    let labelled = |labels| {
        json!({"method": "event", "params": {
            "type": "labels", "terminal": "t1", "data": {"labels": labels},
        }})
    };

    assert_eq!(
        name_reported(&labelled(json!({"lawn-name": "build"}))),
        Some(Report::Name("t1".to_owned(), Some("build".to_owned())))
    );
    assert_eq!(
        name_reported(&labelled(json!({"other": "x"}))),
        Some(Report::Name("t1".to_owned(), None))
    );
}

#[test]
fn a_created_event_reports_the_name_the_terminal_started_with() {
    let message = json!({"method": "event", "params": {
        "type": "created",
        "terminal": "t3",
        "data": {"address": "t3", "cwd": "/", "labels": {"lawn-name": "logs"}},
    }});

    assert_eq!(
        name_reported(&message),
        Some(Report::Name("t3".to_owned(), Some("logs".to_owned())))
    );
}

#[test]
fn a_removed_event_forgets_the_name() {
    let message = json!({"method": "event", "params": {
        "type": "removed", "terminal": "t3", "data": {},
    }});

    assert_eq!(
        name_reported(&message),
        Some(Report::Name("t3".to_owned(), None))
    );
}

#[test]
fn a_missed_message_reports_fresh_names() {
    let message = json!({"method": "missed", "params": {"terminals": [
        {"address": "t2", "labels": {"lawn-name": "logs"}},
    ]}});

    assert_eq!(name_reported(&message), Some(named(&[("t2", "logs")])));
}

#[test]
fn a_name_draws_only_when_a_shown_terminals_name_changes() {
    let mut terminals = Terminals::default();
    let mut hear = |terminal: &str, name: Option<&str>| {
        terminals.hear(
            Report::Name(terminal.to_owned(), name.map(str::to_owned)),
            |_| false,
            |terminal| terminal == "t1",
            |_| false,
        )
    };

    assert!(hear("t1", Some("build")));
    assert!(!hear("t1", Some("build")));
    assert!(hear("t1", Some("test")));
    assert!(!hear("t2", Some("logs")));
    assert!(hear("t1", None));
    assert!(!hear("t1", None));
}

#[test]
fn a_fresh_name_snapshot_forgets_the_names_it_no_longer_lists() {
    let mut terminals = Terminals::default();
    terminals.hear(named(&[("t1", "build")]), |_| false, |_| true, |_| false);
    assert_eq!(terminals.name("t1"), Some("build"));

    assert!(terminals.hear(named(&[]), |_| false, |_| true, |_| false));
    assert_eq!(terminals.name("t1"), None);
}

fn followed() -> mpsc::UnboundedReceiver<Report> {
    let (reports, received) = mpsc::unbounded_channel();
    follow(reports).expect("follow");
    received
}

async fn until_seen(reports: &mut mpsc::UnboundedReceiver<Report>, terminal: &str) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let folder = match reports.recv().await.expect("the follower stopped") {
                Report::Snapshot { folders, .. } => folders.get(terminal).cloned(),
                Report::Folder(reported, folder) if reported == terminal => Some(folder),
                _ => None,
            };
            if let Some(folder) = folder {
                return folder.display().to_string();
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("never saw {terminal}"))
}

#[tokio::test]
async fn the_follower_reports_each_new_terminal_and_follows_again_after_the_core_connection_drops()
{
    let Some(test) = TestCore::start() else {
        return;
    };
    crate::hc::use_core_for_test();
    test.share();
    let argv = ["sleep".to_owned(), "60".to_owned()];
    let mut reports = followed();
    let first = Lifecycle::new(test.core.clone(), "followed")
        .start_first(Path::new("/"), &argv)
        .expect("start")
        .expect("a first shell")
        .terminal;
    assert_eq!(until_seen(&mut reports, &first).await, "/");

    test.disconnect();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !matches!(reports.recv().await, Some(Report::Snapshot { .. }) | None) {}
    })
    .await
    .expect("a fresh snapshot");
    let core = crate::hc::backend()
        .expect("a core backend")
        .expect("a new connection");
    assert!(!std::sync::Arc::ptr_eq(&core, &test.core));
    let second = Lifecycle::new(core, "followed")
        .start(Destination::Workspace { name: None }, Path::new("/"), &argv)
        .expect("start")
        .terminal;

    assert_eq!(until_seen(&mut reports, &second).await, "/");
}

async fn until_working(reports: &mut mpsc::UnboundedReceiver<Report>, terminal: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = match reports.recv().await.expect("the follower stopped") {
                Report::Snapshot { statuses, .. } => statuses
                    .into_iter()
                    .find_map(|(listed, status)| (listed == terminal).then_some(status)),
                Report::Status(reported, status) if reported == terminal => Some(status),
                _ => None,
            };
            if status == Some(said("working")) {
                return;
            }
        }
    })
    .await
    .expect("a working status");
}

#[tokio::test]
async fn the_follower_reports_the_status_a_program_sends_and_a_later_follower_finds_it_in_the_snapshot(
) {
    let Some(test) = TestCore::start() else {
        return;
    };
    crate::hc::use_core_for_test();
    test.share();
    let argv = [
        "sh".to_owned(),
        "-c".to_owned(),
        r"printf '\033]7501;state=working:app=claude\033\\'; sleep 60".to_owned(),
    ];
    let mut reports = followed();
    let terminal = Lifecycle::new(test.core.clone(), "followed")
        .start_first(Path::new("/"), &argv)
        .expect("start")
        .expect("a first shell")
        .terminal;
    until_working(&mut reports, &terminal).await;

    let mut later = followed();
    until_working(&mut later, &terminal).await;
}

async fn until_named(
    reports: &mut mpsc::UnboundedReceiver<Report>,
    terminal: &str,
    name: Option<&str>,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let heard = match reports.recv().await.expect("the follower stopped") {
                Report::Names(names) => Some(names.get(terminal).cloned()),
                Report::Name(reported, name) if reported == terminal => Some(name),
                _ => None,
            };
            if heard.is_some_and(|heard| heard.as_deref() == name) {
                return;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("never saw {terminal} named {name:?}"));
}

#[tokio::test]
async fn a_rename_reaches_every_follower_and_an_empty_name_removes_it() {
    let Some(test) = TestCore::start() else {
        return;
    };
    crate::hc::use_core_for_test();
    test.share();
    let argv = ["sleep".to_owned(), "60".to_owned()];
    let mut reports = followed();
    let terminal = Lifecycle::new(test.core.clone(), "followed")
        .start_first(Path::new("/"), &argv)
        .expect("start")
        .expect("a first shell")
        .terminal;

    rename(&test.core, &terminal, "build").expect("renamed");
    until_named(&mut reports, &terminal, Some("build")).await;
    let mut later = followed();
    until_named(&mut later, &terminal, Some("build")).await;

    rename(&test.core, &terminal, "").expect("cleared");
    until_named(&mut reports, &terminal, None).await;
    until_named(&mut later, &terminal, None).await;
}

#[test]
fn a_removed_event_reports_the_terminal_gone() {
    let message =
        json!({"method": "event", "params": {"type": "removed", "terminal": "t1", "data": {}}});

    assert_eq!(reported(&message), Some(Report::Removed("t1".to_owned())));
}

fn held(terminals: &mut Terminals) -> Vec<(String, AgentState, AgentState)> {
    terminals
        .moves(|_| true)
        .into_iter()
        .map(|moved| (moved.terminal, moved.from, moved.to))
        .collect()
}

#[test]
fn a_change_for_a_terminal_no_layout_holds_is_kept_alone_until_one_does() {
    let mut terminals = Terminals::default();
    told(&mut terminals, &["working", "blocked"]);
    assert!(terminals.moves(|_| false).is_empty());
    told(&mut terminals, &["working", "done"]);

    assert_eq!(
        held(&mut terminals),
        [("t1".to_owned(), AgentState::Working, AgentState::Idle)]
    );
    assert!(held(&mut terminals).is_empty());
}

#[test]
fn a_kept_change_drops_when_the_core_removes_its_terminal() {
    let mut terminals = Terminals::default();
    told(&mut terminals, &["blocked"]);

    assert!(terminals.hear(
        Report::Removed("t1".to_owned()),
        |_| false,
        |_| true,
        |_| false
    ));
    assert_eq!(terminals.status("t1"), None);
    assert!(held(&mut terminals).is_empty());
}

#[test]
fn a_kept_change_drops_when_a_fresh_snapshot_does_not_list_its_terminal() {
    let mut terminals = Terminals::default();
    terminals.hear(stated(&[]), |_| false, |_| true, |_| false);
    told(&mut terminals, &["blocked"]);
    terminals.hear(
        Report::Status("t2".to_owned(), said("blocked")),
        |_| false,
        |_| true,
        |_| false,
    );

    terminals.hear(
        stated(&[("t2", said("blocked"))]),
        |_| false,
        |_| true,
        |_| false,
    );

    assert_eq!(
        held(&mut terminals),
        [("t2".to_owned(), AgentState::Unknown, AgentState::Blocked)]
    );
}

#[tokio::test]
async fn a_fresh_subscription_asks_to_resume_and_forget_every_agent() {
    let Some(test) = TestCore::start() else {
        return;
    };
    crate::hc::use_core_for_test();
    test.share();
    let mut reports = followed();

    tokio::time::timeout(Duration::from_secs(10), async {
        while reports.recv().await.expect("the follower stopped") != Report::Resume(None) {}
        assert_eq!(reports.recv().await, Some(Report::Forget(None)));
    })
    .await
    .expect("a resume and a forget request");
}
