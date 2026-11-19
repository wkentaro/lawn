use std::time::{Duration, Instant};

use std::path::PathBuf;

use super::session::Operation;
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
        Operation::Arrange(_) => {}
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
) -> Vec<(u64, Option<String>)> {
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
    assert_eq!(answers, [(7, None)]);
}

#[test]
fn a_start_with_the_core_unreachable_answers_its_request_once_without_a_terminal() {
    let answers = answers_to(
        Err(std::io::Error::other("unreachable")),
        Operation::Start(7, beside("t1")),
    );

    assert_eq!(answers, [(7, None)]);
}
