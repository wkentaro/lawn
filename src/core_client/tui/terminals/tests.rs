use std::path::Path;
use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc;

use super::{follow, reported, snapshot, status_reported, statuses, Report, Status, Terminals};
use crate::api::schema::AgentStatus;
use crate::core_lifecycle::Lifecycle;
use crate::hc::test_support::TestCore;
use crate::pane::{core_status, CoreStatus};
use crate::session_doc::Destination;

fn folders(pairs: &[(&str, &str)]) -> Report {
    Report::Snapshot(
        pairs
            .iter()
            .map(|(terminal, folder)| ((*terminal).to_owned(), folder.into()))
            .collect(),
    )
}

#[test]
fn a_snapshot_reports_each_terminal_in_the_folder_it_started_in() {
    let message = json!({"terminals": [
        {"address": "t1", "cwd": "/srv/app", "foreground": null},
        {"address": "t2", "cwd": "/tmp"},
    ]});

    assert_eq!(
        snapshot(&message),
        folders(&[("t1", "/srv/app"), ("t2", "/tmp")])
    );
}

#[test]
fn a_snapshot_reports_the_folder_the_program_is_in_now_over_the_one_it_started_in() {
    let message = json!({"terminals": [
        {"address": "t1", "cwd": "/srv/app", "foreground": {"cwd": "/srv/app/src", "pid": 7}},
    ]});

    assert_eq!(snapshot(&message), folders(&[("t1", "/srv/app/src")]));
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
fn a_finished_or_removed_terminal_reports_its_end() {
    for kind in ["finished", "removed"] {
        let message = json!({"method": "event", "params": {
            "type": kind, "terminal": "t1", "data": {},
        }});

        assert_eq!(reported(&message), Some(Report::Ended("t1".to_owned())));
    }
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
fn a_missed_message_reports_a_fresh_snapshot() {
    let message = json!({"method": "missed", "params": {"terminals": [
        {"address": "t2", "cwd": "/srv/app"},
    ]}});

    assert_eq!(reported(&message), Some(folders(&[("t2", "/srv/app")])));
}

fn said(state: &str) -> CoreStatus {
    core_status(&json!({"state": state, "app": "claude"}))
}

fn told(terminals: &mut Terminals, states: &[&str]) -> Option<AgentStatus> {
    for state in states {
        terminals.hear(
            Report::Status("t1".to_owned(), said(state)),
            |_| false,
            |_| true,
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
        statuses(&message),
        Report::Statuses(
            [
                ("t1".to_owned(), said("working")),
                ("t2".to_owned(), core_status(&json!(null))),
            ]
            .into_iter()
            .collect()
        )
    );
}

#[test]
fn a_status_event_reports_the_status_the_program_sent() {
    let message = json!({"method": "event", "params": {
        "type": "status", "terminal": "t1", "data": {"status": {"state": "blocked", "app": "claude"}},
    }});

    assert_eq!(
        status_reported(&message),
        Some(Report::Status("t1".to_owned(), said("blocked")))
    );
}

#[test]
fn a_missed_message_reports_fresh_statuses() {
    let message = json!({"method": "missed", "params": {"terminals": [
        {"address": "t2", "status": {"state": "idle", "app": "claude"}},
    ]}});

    assert_eq!(
        status_reported(&message),
        Some(Report::Statuses(
            [("t2".to_owned(), said("idle"))].into_iter().collect()
        ))
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
        terminals.hear(Report::Status("t1".to_owned(), status), |_| false, |_| true)
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

    assert!(terminals.hear(Report::Statuses(Default::default()), |_| false, |_| true));
    assert_eq!(terminals.status("t1"), None);
}

async fn until_seen(reports: &mut mpsc::UnboundedReceiver<Report>, terminal: &str) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let folder = match reports.recv().await.expect("the follower stopped") {
                Report::Snapshot(folders) => folders.get(terminal).cloned(),
                Report::Folder(reported, folder) if reported == terminal => Some(folder),
                Report::Folder(..)
                | Report::Statuses(_)
                | Report::Status(..)
                | Report::Ended(_) => None,
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
    let mut reports = follow().expect("follow");
    let first = Lifecycle::new(test.core.clone(), "followed")
        .start_first(Path::new("/"), &argv)
        .expect("start")
        .expect("a first shell")
        .terminal;
    assert_eq!(until_seen(&mut reports, &first).await, "/");

    test.disconnect();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !matches!(reports.recv().await, Some(Report::Snapshot(_)) | None) {}
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
                Report::Statuses(statuses) => statuses.get(terminal).copied(),
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
    let mut reports = follow().expect("follow");
    let terminal = Lifecycle::new(test.core.clone(), "followed")
        .start_first(Path::new("/"), &argv)
        .expect("start")
        .expect("a first shell")
        .terminal;
    until_working(&mut reports, &terminal).await;

    let mut later = follow().expect("follow");
    until_working(&mut later, &terminal).await;
}
