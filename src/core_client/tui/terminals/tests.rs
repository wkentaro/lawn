use std::path::Path;
use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc;

use super::{follow, reported, snapshot, Report};
use crate::core_lifecycle::Lifecycle;
use crate::hc::test_support::TestCore;
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

async fn until_seen(reports: &mut mpsc::UnboundedReceiver<Report>, terminal: &str) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let folder = match reports.recv().await.expect("the follower stopped") {
                Report::Snapshot(folders) => folders.get(terminal).cloned(),
                Report::Folder(reported, folder) if reported == terminal => Some(folder),
                Report::Folder(..) => None,
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
