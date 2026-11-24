#[cfg(unix)]
use super::test_support::TestCore;
use super::*;

#[test]
fn a_missing_hc_stops_the_launch_with_install_help() {
    assert!(require_installed().is_ok(), "PTY panes need no core");
    use_core_for_test();
    std::env::set_var(HC_BIN_ENV, "/nonexistent/hc");

    let err = require_installed().expect_err("hc is missing");

    assert!(err.to_string().contains("install.sh"), "{err}");
}

fn hello(generation: u64, methods: &[&str]) -> Value {
    json!({"generation": generation, "methods": methods, "features": []})
}

#[test]
fn hello_needs_generation_one_and_every_used_method() {
    assert!(check_hello(&hello(1, USES)).is_ok());
    assert!(matches!(
        check_hello(&hello(2, USES)),
        Err(HcError::Incompatible(_))
    ));
    let without_doc: Vec<&str> = USES.iter().copied().filter(|m| *m != "doc.set").collect();
    let Err(HcError::Incompatible(message)) = check_hello(&hello(1, &without_doc)) else {
        panic!("a core without doc.set must be refused");
    };
    assert!(message.contains("doc.set"));
}

#[test]
fn rpc_errors_keep_kind_and_hint() {
    let error = HcError::rpc(
        "doc.set",
        &json!({"code": -32000, "message": "stale", "data": {"kind": "conflict", "hint": "read again"}}),
    );
    assert_eq!(error.kind(), Some("conflict"));
    assert_eq!(error.to_string(), "doc.set: stale (read again)");
}

#[test]
fn connect_error_reads_the_cores_message() {
    let stderr = r#"{"error":{"message":"no core","hint":"run hc server start"}}"#;
    assert_eq!(
        connect_error(stderr).as_deref(),
        Some("no core (run hc server start)")
    );
    assert_eq!(connect_error("not json"), None);
    let warned = format!("warning: no user bus\n{stderr}\n");
    assert_eq!(
        connect_error(&warned).as_deref(),
        Some("no core (run hc server start)")
    );
}

fn reply(id: u64, result: Value) -> Vec<u8> {
    let message = json!({"jsonrpc": "2.0", "id": id, "result": result});
    frame::encode(0, frame::DATA, message.to_string().as_bytes())
}

#[test]
fn reader_routes_replies_streams_and_early_frames() {
    let routes = Mutex::new(Routes::default());
    let (reply_tx, reply_rx) = mpsc::channel();
    let (stream_tx, mut stream_rx) = tokio_mpsc::unbounded_channel();
    {
        let mut r = lock(&routes);
        r.replies.insert(4, reply_tx);
        r.streams.insert(
            2,
            Route {
                tx: stream_tx,
                automatic_input: false,
            },
        );
    }
    let mut bytes = reply(4, json!({"ok": true}));
    bytes.extend(frame::encode(2, frame::DATA, b"out"));
    bytes.extend(frame::encode(
        2,
        frame::RESIZE,
        &frame::resize_payload(80, 24),
    ));
    bytes.extend(frame::encode(9, frame::DATA, b"early"));
    bytes.extend(frame::encode(2, frame::CLOSE, br#"{"reason":"exited"}"#));

    read_loop(bytes.as_slice(), &routes, &mpsc::channel().0);

    assert_eq!(reply_rx.recv().unwrap(), Ok(json!({"ok": true})));
    assert_eq!(stream_rx.try_recv().unwrap(), Stream::Data(b"out".to_vec()));
    assert_eq!(
        stream_rx.try_recv().unwrap(),
        Stream::Resize { cols: 80, rows: 24 }
    );
    assert_eq!(
        stream_rx.try_recv().unwrap(),
        Stream::Closed(json!({"reason": "exited"}))
    );
    // Closed streams are dropped, so the end of the connection doesn't reach them.
    assert!(stream_rx.try_recv().is_err());
    let r = lock(&routes);
    assert!(r.closed);
    assert_eq!(
        r.early.get(&9),
        Some(&vec![Stream::Data(b"early".to_vec())])
    );
}

#[test]
fn a_late_attach_reply_closes_the_abandoned_stream() {
    let routes = Mutex::new(Routes::default());
    let (writer, sent) = mpsc::channel();
    let mut bytes = frame::encode(5, frame::DATA, b"early");
    // No caller waits for id 7 anymore; its call timed out.
    bytes.extend(reply(7, json!({"channel": 5})));
    bytes.extend(frame::encode(5, frame::DATA, b"late"));
    bytes.extend(frame::encode(5, frame::CLOSE, br#"{"reason":"detached"}"#));
    bytes.extend(frame::encode(6, frame::DATA, b"other"));

    read_loop(bytes.as_slice(), &routes, &writer);

    assert_eq!(
        sent.try_recv().unwrap().0,
        frame::encode(5, frame::CLOSE, &[])
    );
    assert!(sent.try_recv().is_err());
    let r = lock(&routes);
    assert!(!r.early.contains_key(&5));
    assert!(!r.detached.contains(&5));
    assert_eq!(
        r.early.get(&6),
        Some(&vec![Stream::Data(b"other".to_vec())])
    );
}

/// Holds each write until the test lets it through, then fails if told to.
struct GatedWriter {
    gate: mpsc::Receiver<bool>,
    out: Arc<Mutex<Vec<u8>>>,
}

impl Write for GatedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.gate.recv() != Ok(true) {
            return Err(io::Error::other("closed"));
        }
        self.out.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn written_input_waits_for_the_write_and_fails_with_it() {
    let (gate, gate_rx) = mpsc::channel();
    let out = Arc::new(Mutex::new(Vec::new()));
    let (writer, frames) = mpsc::channel::<Outgoing>();
    let writer_out = out.clone();
    let thread = std::thread::spawn(move || {
        write_loop(
            GatedWriter {
                gate: gate_rx,
                out: writer_out,
            },
            &frames,
        )
    });
    let (done, written) = mpsc::channel();
    writer.send((b"one".to_vec(), Some(done))).unwrap();
    // A stalled write keeps the waiter waiting.
    assert!(written.recv_timeout(Duration::from_millis(100)).is_err());
    gate.send(true).unwrap();
    written.recv().unwrap();
    assert_eq!(*out.lock().unwrap(), b"one");

    let (done, written) = mpsc::channel();
    writer.send((b"two".to_vec(), Some(done))).unwrap();
    gate.send(false).unwrap();
    assert!(written.recv().is_err());
    thread.join().unwrap();
}

fn pending_request() -> (PendingRequest, mpsc::Receiver<Outgoing>) {
    let routes = Arc::new(Mutex::new(Routes::default()));
    let (tx, rx) = mpsc::channel();
    let (writer, sent) = mpsc::channel();
    lock(&routes).replies.insert(17, tx);
    (
        PendingRequest {
            id: 17,
            method: "terminal.run".to_owned(),
            reply: Mutex::new(rx),
            routes,
            writer,
            claimed: false,
        },
        sent,
    )
}

fn after_deadline(pending: PendingRequest) -> Box<PendingRequest> {
    match pending.wait(Duration::ZERO) {
        Err(HcError::Unknown {
            pending,
            reason: UnknownReason::Deadline,
        }) => pending,
        other => panic!("expected an unknown outcome, got {other:?}"),
    }
}

#[test]
fn a_deadline_does_not_forget_an_in_flight_reply() {
    let (pending, sent) = pending_request();
    let pending = after_deadline(pending);
    assert!(lock(&pending.routes).replies.contains_key(&17));
    let response = json!({"id": 17, "result": {"address": "t7"}});
    deliver_reply(
        &pending.routes,
        &pending.writer,
        response.to_string().as_bytes(),
    );

    assert_eq!(
        pending.wait(Duration::ZERO).unwrap(),
        json!({"address": "t7"})
    );
    assert!(sent.try_recv().is_err());
}

#[test]
fn a_late_server_error_is_still_a_server_reply() {
    let (pending, _) = pending_request();
    let pending = after_deadline(pending);
    let response = json!({"id": 17, "error": {
        "message": "spawn failed", "data": {"kind": "spawn_failed"}
    }});
    deliver_reply(
        &pending.routes,
        &pending.writer,
        response.to_string().as_bytes(),
    );

    let error = pending.wait(Duration::ZERO).unwrap_err();
    assert_eq!(error.kind(), Some("spawn_failed"));
}

#[test]
fn disconnect_is_unknown_and_not_a_fabricated_server_error() {
    let (pending, _) = pending_request();
    let pending = after_deadline(pending);
    read_loop(&b""[..], &pending.routes, &pending.writer);

    let error = pending.wait(Duration::ZERO).unwrap_err();
    assert!(error.to_string().contains("outcome unknown"));
    assert!(matches!(
        error,
        HcError::Unknown {
            reason: UnknownReason::Disconnected,
            ..
        }
    ));
}

#[test]
fn abandoning_a_delivered_stream_closes_it_and_clears_early_data() {
    let (pending, sent) = pending_request();
    let routes = pending.routes.clone();
    let pending = after_deadline(pending);
    lock(&routes)
        .early
        .insert(5, vec![Stream::Data(b"early".to_vec())]);
    let response = json!({"id": 17, "result": {"channel": 5}});
    deliver_reply(&routes, &pending.writer, response.to_string().as_bytes());

    drop(pending);

    assert_eq!(
        sent.try_recv().unwrap().0,
        frame::encode(5, frame::CLOSE, &[])
    );
    assert!(!lock(&routes).early.contains_key(&5));
    assert!(lock(&routes).replies.is_empty());
}

#[test]
fn abandoning_before_a_reply_still_closes_a_late_stream() {
    let (pending, sent) = pending_request();
    let routes = pending.routes.clone();
    let writer = pending.writer.clone();
    drop(after_deadline(pending));
    assert!(lock(&routes).replies.is_empty());
    let response = json!({"id": 17, "result": {"channel": 5}});
    deliver_reply(&routes, &writer, response.to_string().as_bytes());

    assert_eq!(
        sent.try_recv().unwrap().0,
        frame::encode(5, frame::CLOSE, &[])
    );
}

#[test]
fn abandoning_a_stream_that_already_closed_does_not_close_it_again() {
    let (pending, sent) = pending_request();
    lock(&pending.routes)
        .early
        .insert(5, vec![Stream::Closed(Value::Null)]);
    let response = json!({"id": 17, "result": {"channel": 5}});
    deliver_reply(
        &pending.routes,
        &pending.writer,
        response.to_string().as_bytes(),
    );
    let routes = pending.routes.clone();

    drop(pending);

    assert!(sent.try_recv().is_err());
    assert!(!lock(&routes).early.contains_key(&5));
    assert!(!lock(&routes).detached.contains(&5));
}

#[test]
fn dropping_and_delivering_concurrently_always_closes_an_unclaimed_stream() {
    for _ in 0..100 {
        let (pending, sent) = pending_request();
        let routes = pending.routes.clone();
        let writer = pending.writer.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let dropping = barrier.clone();
        let thread = std::thread::spawn(move || {
            dropping.wait();
            drop(pending);
        });
        barrier.wait();
        let response = json!({"id": 17, "result": {"channel": 5}});
        deliver_reply(&routes, &writer, response.to_string().as_bytes());
        thread.join().unwrap();

        assert_eq!(
            sent.try_recv().unwrap().0,
            frame::encode(5, frame::CLOSE, &[])
        );
        assert!(sent.try_recv().is_err());
        assert!(lock(&routes).replies.is_empty());
    }
}

#[test]
fn claiming_a_delivered_stream_does_not_close_it() {
    let (pending, sent) = pending_request();
    let pending = after_deadline(pending);
    let response = json!({"id": 17, "result": {"channel": 5}});
    deliver_reply(
        &pending.routes,
        &pending.writer,
        response.to_string().as_bytes(),
    );

    assert_eq!(pending.wait(Duration::ZERO).unwrap(), json!({"channel": 5}));
    assert!(sent.try_recv().is_err());
}

pub(super) fn transport_core() -> (Core, mpsc::Receiver<Outgoing>) {
    let (writer, sent) = mpsc::channel();
    let child = Command::new(std::env::current_exe().unwrap())
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    (
        Core {
            hello: hello(1, USES),
            writer,
            routes: Arc::new(Mutex::new(Routes::default())),
            next_id: AtomicU64::new(1),
            child,
        },
        sent,
    )
}

#[test]
fn a_closed_connection_refuses_before_submitting() {
    let (core, sent) = transport_core();
    lock(&core.routes).closed = true;

    assert!(matches!(
        core.request("terminal.run", json!({})),
        Err(HcError::Unavailable(_))
    ));
    assert!(lock(&core.routes).replies.is_empty());
    assert!(sent.try_recv().is_err());
}

#[test]
fn a_missing_writer_refuses_before_submitting() {
    let (core, sent) = transport_core();
    drop(sent);

    assert!(matches!(
        core.request("terminal.run", json!({})),
        Err(HcError::Unavailable(_))
    ));
    assert!(lock(&core.routes).replies.is_empty());
}

#[test]
fn waiting_again_does_not_submit_a_second_request() {
    let (core, sent) = transport_core();
    let pending = core.request("terminal.run", json!({})).unwrap();
    let pending = after_deadline(pending);
    assert!(sent.try_recv().is_ok());
    let pending = after_deadline(*pending);
    assert!(sent.try_recv().is_err());
    let response = json!({"id": 1, "result": {"address": "t7"}});
    deliver_reply(&core.routes, &core.writer, response.to_string().as_bytes());
    assert_eq!(
        pending.wait(Duration::ZERO).unwrap(),
        json!({"address": "t7"})
    );
}

#[test]
fn a_request_over_the_frame_limit_fails_before_sending() {
    let small = request_payload(1, "doc.set", json!({"value": "x"})).unwrap();
    assert!(small.contains("doc.set"));
    let huge = "x".repeat(frame::MAX_PAYLOAD);
    assert!(matches!(
        request_payload(2, "doc.set", json!({"value": huge})),
        Err(HcError::Unavailable(message)) if message.contains("larger")
    ));
}

#[test]
fn losing_the_core_fails_waiting_calls_and_open_streams() {
    let routes = Mutex::new(Routes::default());
    let (reply_tx, reply_rx) = mpsc::channel();
    let (stream_tx, mut stream_rx) = tokio_mpsc::unbounded_channel();
    {
        let mut r = lock(&routes);
        r.replies.insert(1, reply_tx);
        r.streams.insert(
            3,
            Route {
                tx: stream_tx,
                automatic_input: false,
            },
        );
    }
    read_loop(&b""[..], &routes, &mpsc::channel().0);
    assert!(reply_rx.recv().is_err());
    assert_eq!(stream_rx.try_recv().unwrap(), Stream::Lost);
}

#[cfg(unix)]
#[test]
fn connect_gives_up_on_a_core_that_never_says_hello() {
    let mut silent = Command::new("sh");
    silent.args(["-c", "exec sleep 30"]);
    let started = std::time::Instant::now();
    let Err(HcError::Unavailable(message)) = Core::connect_with(silent, Duration::from_millis(200))
    else {
        panic!("a silent core must not connect");
    };
    assert!(message.contains("hello"), "{message}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[cfg(unix)]
fn recv_until(stream: &mut tokio_mpsc::UnboundedReceiver<Stream>, needle: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut seen = String::new();
    while std::time::Instant::now() < deadline {
        match stream.try_recv() {
            Ok(Stream::Data(bytes)) => {
                seen.push_str(&String::from_utf8_lossy(&bytes));
                if seen.contains(needle) {
                    return seen;
                }
            }
            Ok(other) => panic!("unexpected {other:?} before {needle:?}; saw {seen:?}"),
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    panic!("no {needle:?} within 10s; saw {seen:?}");
}

// The programs here are Unix shell lines.
#[cfg(unix)]
#[test]
fn live_core_runs_attaches_and_versions_documents() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let core = &test.core;

    let address = core
        .run(json!({"argv": ["sh", "-c", "echo lawn-$((40+2)); sleep 30"], "no_parent": true}))
        .expect("run");
    let mut attached = core
        .attach(json!({"terminal": address, "cols": 80, "rows": 24}))
        .expect("attach");
    recv_until(&mut attached.stream, "lawn-42");

    let first = core
        .doc_set("lawn-test", None, &json!({"a": 1}), 0)
        .expect("first write");
    let stale = core
        .doc_set("lawn-test", None, &json!({"a": 2}), 0)
        .unwrap_err();
    assert_eq!(stale.kind(), Some("conflict"));
    let doc = core.doc_get("lawn-test", None).expect("read");
    assert_eq!(
        doc,
        Doc {
            value: json!({"a": 1}),
            version: first
        }
    );

    core.call("terminal.kill", json!({"terminal": address}))
        .expect("kill");
}

// The programs here are Unix shell lines.
#[cfg(unix)]
#[test]
fn live_core_streams_events_to_subscribers() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let core = &test.core;
    let mut events = core.subscribe(json!({})).expect("subscribe");
    assert!(events.snapshot.get("seq").is_some());

    let address = core
        .run(json!({"argv": ["sh", "-c", "sleep 30"], "no_parent": true}))
        .expect("run");
    let seen = recv_until(&mut events.stream, "\"created\"");
    assert!(
        seen.contains(&address),
        "created event names {address}: {seen}"
    );

    core.call("terminal.kill", json!({"terminal": address}))
        .expect("kill");
}
