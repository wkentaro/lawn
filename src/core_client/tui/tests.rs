use std::time::{Duration, Instant};

use std::path::PathBuf;

use super::session::{Operation, Overlay, ScrollbackText};
use super::{run_with_bounded_shutdown, SHUTDOWN_GRACE};
use crate::session_doc::{Destination, Direction};

#[test]
fn leaving_the_loop_stops_waiting_for_a_stalled_core_call() {
    let (_never, stalled) = std::sync::mpsc::channel::<()>();
    let started = Instant::now();

    run_with_bounded_shutdown(async move {
        tokio::task::spawn_blocking(move || stalled.recv_timeout(Duration::from_secs(10)));
    })
    .expect("runtime");

    assert!(started.elapsed() < SHUTDOWN_GRACE + Duration::from_secs(2));
}

#[test]
fn operations_wait_on_one_blocking_task_until_the_split_before_them_finishes() {
    let bound = Duration::from_secs(10);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .build()
        .expect("runtime");
    let _entered = runtime.enter();
    let (report, reports) = std::sync::mpsc::channel();
    let (open, gate) = std::sync::mpsc::channel::<()>();
    let queue = super::unix::operations(move |operation, _cwd| match operation {
        Operation::Start(..) => {
            report.send(("began", tokio::task::try_id())).ok();
            gate.recv_timeout(bound).ok();
            report.send(("split", tokio::task::try_id())).ok();
        }
        Operation::Park(_) => {
            report.send(("parked", tokio::task::try_id())).ok();
        }
        Operation::Close(_) => {
            report.send(("closed", tokio::task::try_id())).ok();
        }
        Operation::Arrange(_) | Operation::Overlay(_) => {}
    });
    let split = Destination::Split {
        beside: "t1".to_owned(),
        direction: Direction::Right,
        ratio: 0.5,
    };
    queue
        .send((Operation::Start(1, split), PathBuf::new()))
        .expect("queued");
    let began = reports.recv_timeout(bound).expect("the split began");
    for operation in [
        Operation::Park("t1".to_owned()),
        Operation::Close(vec!["t1".to_owned()]),
    ] {
        queue.send((operation, PathBuf::new())).expect("queued");
    }
    open.send(()).expect("opened");
    let rest = [(); 3].map(|()| reports.recv_timeout(bound).expect("the operation ran"));
    let reports = [began, rest[0], rest[1], rest[2]];

    assert_eq!(
        reports.map(|(said, _)| said),
        ["began", "split", "parked", "closed"]
    );
    assert!(began.1.is_some(), "{reports:?}");
    assert!(
        reports.iter().all(|(_, task)| *task == began.1),
        "{reports:?}"
    );
}

fn beside(terminal: &str) -> Destination {
    Destination::Split {
        beside: terminal.to_owned(),
        direction: Direction::Right,
        ratio: 0.5,
    }
}

fn answers_to(
    core: std::io::Result<std::sync::Arc<crate::hc::Core>>,
    operation: Operation,
) -> Vec<super::unix::Answer> {
    let (started, mut answers) = tokio::sync::mpsc::unbounded_channel();
    super::unix::perform(
        core,
        "work",
        operation,
        std::path::Path::new("/tmp"),
        &started,
    );
    drop(started);
    std::iter::from_fn(|| answers.try_recv().ok()).collect()
}

fn refusing_core(asked: std::sync::mpsc::Sender<String>) -> std::sync::Arc<crate::hc::Core> {
    let mut document = (0, serde_json::Value::Null);
    crate::hc::test_support::answering(move |method, params| {
        asked.send(method.to_owned()).ok();
        match method {
            "doc.get" => Ok(serde_json::json!({"version": document.0, "value": document.1})),
            "doc.set" => {
                document = (document.0 + 1, params["value"].clone());
                Ok(serde_json::json!({"version": document.0}))
            }
            "terminal.list" => Ok(serde_json::json!({"terminals": []})),
            _ => Err(serde_json::json!({"code": -32000, "message": "refused"})),
        }
    })
}

#[test]
fn a_start_the_core_refuses_answers_its_request_once_without_a_terminal() {
    let (asked, methods) = std::sync::mpsc::channel();
    let core = refusing_core(asked);

    let answers = answers_to(
        Ok(core),
        Operation::Start(7, Destination::Workspace { name: None }),
    );

    assert!(methods.try_iter().any(|method| method == "terminal.run"));
    assert_eq!(answers, [(7, None, None)]);
}

#[test]
fn a_start_with_the_core_unreachable_answers_its_request_once_without_a_terminal() {
    let answers = answers_to(
        Err(std::io::Error::other("unreachable")),
        Operation::Start(7, beside("t1")),
    );

    assert_eq!(answers, [(7, None, None)]);
}

fn edit(request: u64, text: &str) -> Operation {
    Operation::Overlay(Overlay {
        request,
        destination: Destination::Split {
            beside: "t1".to_owned(),
            direction: Direction::Right,
            ratio: 0.5,
        },
        argv: Vec::new(),
        env: Vec::new(),
        scrollback: Some(ScrollbackText(text.to_owned())),
    })
}

fn scratch_temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lawn-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch directory");
    std::env::set_var("TMPDIR", &dir);
    dir
}

fn files_in(dir: &std::path::Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .expect("scratch directory")
        .map(|entry| entry.expect("entry").path())
        .collect()
}

#[test]
fn edits_start_in_the_order_they_were_asked_whatever_their_write_time() {
    let bound = Duration::from_secs(10);
    let dir = scratch_temp_dir("edit-order");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .build()
        .expect("runtime");
    let _entered = runtime.enter();
    let (report, reports) = std::sync::mpsc::channel();
    let (open, gate) = std::sync::mpsc::channel::<()>();
    let queue = super::unix::operations(move |operation, _cwd| {
        let Operation::Overlay(overlay) = operation else {
            return;
        };
        if overlay.request == 1 {
            gate.recv_timeout(bound).ok();
        }
        let scrollback = overlay.scrollback.expect("a scrollback");
        let (temp_file, _argv) = super::unix::editing(&scrollback).expect("written");
        let written = std::fs::read_to_string(&temp_file).expect("readable");
        std::fs::remove_file(temp_file).ok();
        report.send((overlay.request, written)).ok();
    });

    for (request, text) in [(1, "first"), (2, "second")] {
        queue
            .send((edit(request, text), PathBuf::new()))
            .expect("queued");
    }
    assert!(reports.try_recv().is_err());
    open.send(()).expect("opened");
    let started = [(); 2].map(|()| reports.recv_timeout(bound).expect("the edit ran"));

    assert_eq!(started, [(1, "first".to_owned()), (2, "second".to_owned())]);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_queued_edit_writes_no_file_until_the_worker_reaches_it() {
    let bound = Duration::from_secs(10);
    let dir = scratch_temp_dir("edit-queued");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .build()
        .expect("runtime");
    let _entered = runtime.enter();
    let (report, reports) = std::sync::mpsc::channel();
    let (open, gate) = std::sync::mpsc::channel::<()>();
    let queue = super::unix::operations(move |operation, _cwd| match operation {
        Operation::Start(..) => {
            report.send(None).ok();
            gate.recv_timeout(bound).ok();
        }
        Operation::Overlay(overlay) => {
            let scrollback = overlay.scrollback.expect("a scrollback");
            let (temp_file, _argv) = super::unix::editing(&scrollback).expect("written");
            report.send(Some(temp_file)).ok();
        }
        _ => {}
    });
    let split = Destination::Split {
        beside: "t1".to_owned(),
        direction: Direction::Right,
        ratio: 0.5,
    };
    queue
        .send((Operation::Start(1, split), PathBuf::new()))
        .expect("queued");
    assert_eq!(reports.recv_timeout(bound).expect("the split began"), None);

    queue
        .send((edit(2, "text"), PathBuf::new()))
        .expect("queued");

    assert!(files_in(&dir).is_empty());
    open.send(()).expect("opened");
    let temp_file = reports
        .recv_timeout(bound)
        .expect("the edit ran")
        .expect("a temporary file");
    assert_eq!(files_in(&dir), std::slice::from_ref(&temp_file));
    assert_eq!(
        std::fs::read_to_string(temp_file).expect("readable"),
        "text"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn an_edit_whose_start_recovery_placed_first_keeps_its_file_until_its_pane_closes() {
    let dir = scratch_temp_dir("edit-recovered");
    let (temp_file, _argv) =
        super::unix::editing(&ScrollbackText("text".to_owned())).expect("written");
    let (started, mut answers) = tokio::sync::mpsc::unbounded_channel();
    let recovered = crate::session_doc::Created {
        terminal: "t9".to_owned(),
        workspace: None,
        tab: None,
        version: 4,
    };

    super::unix::answer(&started, 3, Ok(recovered), Some(temp_file.clone()));
    let (request, placed, kept) = answers.try_recv().expect("an answer");
    let mut scrollbacks = std::collections::HashMap::new();
    super::unix::keep(&mut scrollbacks, &placed, kept);

    assert_eq!((request, placed), (3, Some(("t9".to_owned(), 4))));
    assert!(temp_file.exists());
    assert!(super::unix::forget(&mut scrollbacks, ["t8".to_owned()]).is_none());
    assert!(temp_file.exists());
    super::unix::forget(&mut scrollbacks, ["t9".to_owned()])
        .expect("the file is forgotten")
        .await
        .expect("removed");
    assert!(!temp_file.exists());
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn a_shell_command_runs_detached_in_the_pane_directory_with_the_command_env() {
    let dir = std::env::temp_dir().join(format!("lawn-shell-command-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch directory");
    let env = vec![
        ("HOME".to_owned(), dir.display().to_string()),
        ("LAWN_PROBE".to_owned(), "probe".to_owned()),
    ];

    let status = super::unix::detached(
        "printf '%s:%s' \"$LAWN_PROBE\" \"$(pwd -P)\" > out".to_owned(),
        env,
        Some(dir.clone()),
    )
    .await
    .expect("the command started")
    .wait()
    .await
    .expect("the command ended");

    let written = std::fs::read_to_string(dir.join("out"));
    let expected = format!("probe:{}", dir.canonicalize().expect("scratch").display());
    std::fs::remove_dir_all(&dir).expect("remove the scratch directory");
    assert!(status.success());
    assert_eq!(written.expect("the command wrote its output"), expected);
}

#[test]
fn an_edit_under_a_relative_temp_dir_opens_and_tracks_its_file_by_an_absolute_path() {
    let dir = scratch_temp_dir("edit-relative");
    std::env::set_current_dir(dir.parent().expect("temp root")).expect("current directory");
    std::env::set_var("TMPDIR", dir.file_name().expect("scratch name"));

    let (temp_file, argv) =
        super::unix::editing(&ScrollbackText("text".to_owned())).expect("written");

    assert!(temp_file.is_absolute());
    assert_eq!(
        std::fs::read_to_string(&temp_file).expect("the file"),
        "text"
    );
    let shown = temp_file.display().to_string();
    assert!(argv.iter().any(|arg| arg.contains(&shown)), "{argv:?}");
    std::fs::remove_dir_all(&dir).ok();
}
