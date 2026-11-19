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
        Operation::Start(_) => {
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
        .send((Operation::Start(split), PathBuf::new()))
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
