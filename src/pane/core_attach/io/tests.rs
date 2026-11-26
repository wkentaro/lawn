use super::*;

#[test]
fn dropping_the_owner_cancels_connection_retries() {
    let connection = Arc::new(Connection {
        routing: Mutex::new(Routing {
            current: None,
            events_channel: None,
            size: (24, 80),
            stopped: false,
        }),
        changed: Condvar::new(),
    });
    let (input, _queue) = mpsc::channel();
    let owner = CoreIo {
        connection: connection.clone(),
        address: "t7".into(),
        followed: true,
        input,
    };
    let (started, start) = mpsc::channel();
    let (completed, complete) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        started.send(()).unwrap();
        let result = connection.reconnect("t7", false);
        completed.send(matches!(result, Ok(None))).unwrap();
    });
    start.recv().unwrap();
    drop(owner);
    assert!(complete.recv_timeout(Duration::from_secs(1)).unwrap());
    worker.join().unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn replacement_routing_rejects_old_keys_and_renews_responses_and_size() {
    use super::super::tests::wait_for;
    use serde_json::json;

    let Some(test) = crate::hc::test_support::TestCore::start() else {
        return;
    };
    test.share();
    crate::hc::use_core_for_test();
    let core = &test.core;
    let address = core.run(json!({"argv": ["sh", "-c", "stty -echo; echo ready; read first; echo first-$first; read second; echo second-$second; sleep 30"], "no_parent": true})).unwrap();
    let attached = core
        .attach(json!({"terminal": address, "rows": 24, "cols": 80}))
        .unwrap();
    let io = CoreIo::new(
        core.clone(),
        attached.channel,
        address.clone(),
        true,
        24,
        80,
    )
    .unwrap();
    let text = || {
        core.call("terminal.read", json!({"terminal": address}))
            .unwrap()["text"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    wait_for("the prompt", || text().contains("ready")).await;
    let old = io.connection.current().unwrap();
    io.connection.detach(false);
    assert!(io.send(Bytes::from_static(b"offline\r")).is_err());
    assert!(io.validate_input().is_err());
    assert!(io
        .submit(Bytes::new(), Bytes::new(), Duration::ZERO)
        .is_err());
    io.resize(93, 33);
    let (replacement, _attached) = io.connection.reconnect(&address, false).unwrap().unwrap();
    assert!(!Arc::ptr_eq(&old, &replacement));
    io.input
        .send((old, Input::Bytes(Bytes::from_static(b"stale\r"))))
        .unwrap();
    io.send(Bytes::from_static(b"fresh\r")).unwrap();
    wait_for("fresh input on the renewed channel", || {
        text().contains("first-fresh")
    })
    .await;
    io.respond(b"response\r");
    wait_for("the renewed response route", || {
        text().contains("second-response")
    })
    .await;
    let record = core
        .call("terminal.get", json!({"terminal": address}))
        .unwrap();
    assert_eq!(
        (record["rows"].as_u64(), record["cols"].as_u64()),
        (Some(33), Some(93))
    );
    assert!(!text().contains("stale"));
    assert!(!text().contains("offline"));
    drop(io);
    core.call("terminal.kill", json!({"terminal": address}))
        .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn input_sent_right_before_the_owner_drops_reaches_the_terminal() {
    use super::super::tests::wait_for;
    use serde_json::json;

    let Some(test) = crate::hc::test_support::TestCore::start() else {
        return;
    };
    test.share();
    crate::hc::use_core_for_test();
    let core = &test.core;
    let address = core.run(json!({"argv": ["sh", "-c", "stty -echo; echo ready; read last; echo last-$last; sleep 30"], "no_parent": true})).unwrap();
    let attached = core
        .attach(json!({"terminal": address, "rows": 24, "cols": 80}))
        .unwrap();
    let io = CoreIo::new(
        core.clone(),
        attached.channel,
        address.clone(),
        true,
        24,
        80,
    )
    .unwrap();
    let text = || {
        core.call("terminal.read", json!({"terminal": address}))
            .unwrap()["text"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    wait_for("the prompt", || text().contains("ready")).await;
    io.send(Bytes::from_static(b"words\r")).unwrap();
    drop(io);
    wait_for("the input sent before the drop", || {
        text().contains("last-words")
    })
    .await;
    core.call("terminal.kill", json!({"terminal": address}))
        .unwrap();
}
