use super::*;

async fn compose_until(
    state: &mut ClientShellState,
    ready: impl Fn(&[String]) -> bool,
) -> crate::client::frame_output::ComposedFrame {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let frame = state.compose(106, 20).expect("composed frame");
        let rows = frame_rows(&frame);
        if ready(&rows) {
            return frame;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out; last frame: {rows:#?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

fn shows(rows: &[String], text: &str) -> bool {
    rows.iter().any(|row| row.contains(text))
}

#[tokio::test]
async fn a_core_pane_draws_its_terminal_only_while_it_matches_the_pane() {
    let Some(test) = crate::hc::test_support::TestCore::start() else {
        return;
    };
    test.share();
    crate::hc::use_core_for_test();
    let address = test
        .core
        .run(serde_json::json!({
            "argv": ["sh", "-c", "printf CORE; sleep 30"],
            "no_parent": true,
            "size": {"cols": 4, "rows": 2},
        }))
        .expect("run");
    let viewer = test
        .core
        .attach(serde_json::json!({"terminal": address, "cols": 4, "rows": 2}))
        .expect("attach");
    let (loop_events, _loop_events_rx) = tokio::sync::mpsc::channel(8);
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let server_cursor = state
        .compose(106, 20)
        .expect("composed frame")
        .cursor
        .clone();
    state.core_pane = crate::client::core_render::CorePane::observe(
        "pane_1".into(),
        &address,
        (4, 2),
        loop_events,
    );
    assert!(state.core_pane.is_some());

    let frame = compose_until(&mut state, |rows| {
        shows(rows, "CORE") && !shows(rows, "LIVE")
    })
    .await;
    assert!(server_cursor.is_some());
    assert_eq!(frame.cursor, server_cursor, "the server keeps the cursor");

    test.core.resize(viewer.channel, 10, 3);
    compose_until(&mut state, |rows| {
        shows(rows, "LIVE") && !shows(rows, "CORE")
    })
    .await;

    test.core.resize(viewer.channel, 4, 2);
    compose_until(&mut state, |rows| shows(rows, "CORE")).await;
    test.core
        .call("terminal.kill", serde_json::json!({"terminal": address}))
        .expect("kill");
    compose_until(&mut state, |rows| {
        shows(rows, "LIVE") && !shows(rows, "CORE")
    })
    .await;
}
