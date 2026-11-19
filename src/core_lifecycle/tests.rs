use super::*;

#[test]
fn remote_matches_never_resolve_a_local_start() {
    assert_eq!(
        local_start(&json!({"terminals": [{"address": "box/t1"}]})).unwrap(),
        None
    );
    assert_eq!(
        local_start(&json!({"terminals": [
            {"address": "box/t1"}, {"address": "t2"}, {"address": "other/t3"}
        ]}))
        .unwrap(),
        Some("t2")
    );
    assert!(local_start(&json!({"terminals": [{"address": "t1"}, {"address": "t2"}]})).is_err());
}

#[test]
fn resumes_from_separate_requests_keep_one_spacing() {
    let (requests, queue) = std::sync::mpsc::channel();
    requests.send(()).unwrap();
    requests.send(()).unwrap();
    drop(requests);
    let at = std::time::Instant::now();
    let spacing = std::time::Duration::from_secs(60);
    let mut waits = Vec::new();
    let (_stop, stopped) = std::sync::mpsc::channel();
    drain(queue, stopped, spacing, |(), pace| {
        waits.push(pace.wait(at));
        pace.sent(at);
    });
    assert_eq!(waits, [std::time::Duration::ZERO, spacing]);
}

#[test]
fn a_request_buffered_behind_a_wait_is_not_started_after_the_stop() {
    let (requests, queue) = std::sync::mpsc::channel();
    requests.send(()).unwrap();
    requests.send(()).unwrap();
    drop(requests);
    let (stop, stopped) = std::sync::mpsc::channel();
    let mut stop = Some(stop);
    let mut started = 0;
    drain(
        queue,
        stopped,
        std::time::Duration::from_secs(3600),
        |(), pace| {
            if pace.ready(std::time::Instant::now()) {
                started += 1;
                pace.sent(std::time::Instant::now());
                stop.take();
            }
        },
    );
    assert_eq!(started, 1);
}

#[cfg(unix)]
mod close;
#[cfg(unix)]
mod first;
#[cfg(unix)]
mod live;
