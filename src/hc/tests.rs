use super::*;

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
        sent.try_recv().unwrap(),
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

#[test]
fn a_reply_already_taken_at_timeout_still_reaches_the_caller() {
    let routes = Mutex::new(Routes::default());
    let (tx, rx) = mpsc::channel();
    // The reader removed the waiter and is about to send.
    let sender = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        tx.send(Ok(json!({"channel": 5}))).unwrap();
    });
    assert_eq!(
        await_reply(&routes, 1, &rx, Duration::from_millis(1)),
        Some(Ok(json!({"channel": 5})))
    );
    sender.join().unwrap();

    let (tx, rx) = mpsc::channel();
    lock(&routes).replies.insert(2, tx);
    assert_eq!(await_reply(&routes, 2, &rx, Duration::from_millis(1)), None);
    assert!(!lock(&routes).replies.contains_key(&2));
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
    assert!(reply_rx.recv().unwrap().is_err());
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

/// A throwaway core under its own `HC_SESSION`, stopped when dropped.
struct TestCore {
    core: Arc<Core>,
    session: String,
    state: std::path::PathBuf,
}

impl TestCore {
    fn start() -> Option<Self> {
        let bin = hc_bin();
        if crate::noninteractive_process::command(&bin)
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping: herdr-core (`hc`) is not installed");
            return None;
        }
        let session = format!(
            "lawn-test-{}-{}",
            std::process::id(),
            NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
        );
        // macOS's temp dir is too deep for a Unix socket path.
        #[cfg(unix)]
        let state = std::path::Path::new("/tmp").join(&session);
        #[cfg(not(unix))]
        let state = std::env::temp_dir().join(&session);
        let core = Core::connect_with(isolated(&session, &state), CALL_TIMEOUT)
            .expect("connect to a fresh core");
        Some(TestCore {
            core,
            session,
            state,
        })
    }
}

/// An `hc` that can't reach the user's core: its own session, state,
/// config and socket.
fn isolated(session: &str, state: &std::path::Path) -> Command {
    let mut command = crate::noninteractive_process::command(hc_bin());
    #[cfg(unix)]
    command.env("HC_SOCKET", state.join("hc.sock"));
    #[cfg(not(unix))]
    command.env_remove("HC_SOCKET");
    command
        .env("HC_SESSION", session)
        .env("XDG_STATE_HOME", state.join("state"))
        .env("XDG_CONFIG_HOME", state.join("config"));
    command
}

static NEXT_SESSION: AtomicU64 = AtomicU64::new(0);

impl Drop for TestCore {
    fn drop(&mut self) {
        let _ = isolated(&self.session, &self.state)
            .args(["server", "stop"])
            .output();
        let _ = std::fs::remove_dir_all(&self.state);
    }
}

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
