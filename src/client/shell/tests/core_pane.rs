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
    let core_pane = crate::client::core_render::CorePane::observe(
        "pane_1".into(),
        &address,
        (4, 2),
        loop_events,
        Default::default(),
    )
    .expect("observe");
    state.core_panes.insert("pane_1".into(), core_pane);

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

#[tokio::test]
async fn every_pane_of_the_tab_draws_from_its_own_terminal() {
    let Some(test) = crate::hc::test_support::TestCore::start() else {
        return;
    };
    test.share();
    crate::hc::use_core_for_test();
    let run = |text: &str| {
        let address = test
            .core
            .run(serde_json::json!({
                "argv": ["sh", "-c", format!("printf {text}; sleep 30")],
                "no_parent": true,
                "size": {"cols": 4, "rows": 2},
            }))
            .expect("run");
        test.core
            .attach(serde_json::json!({"terminal": address, "cols": 4, "rows": 2}))
            .expect("attach");
        address
    };
    let (focused, other) = (run("AAAA"), run("BBBB"));
    let mut surface = surface();
    let buffer = Buffer::with_lines(["LIVE PANE", "LIVE PANE"]);
    surface.frame = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
    let mut second = surface.panes[0].clone();
    second.pane_id = "pane_2".into();
    second.focused = false;
    second.rect.x = 5;
    second.inner_rect.x = 5;
    surface.panes.push(second);
    let (loop_events, _loop_events_rx) = tokio::sync::mpsc::channel(8);
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface);
    assert_eq!(
        state.surface_panes(),
        [("pane_1", (4, 2)), ("pane_2", (4, 2))]
    );
    for (pane_id, address) in [("pane_1", &focused), ("pane_2", &other)] {
        let core_pane = crate::client::core_render::CorePane::observe(
            pane_id.into(),
            address,
            (4, 2),
            loop_events.clone(),
            Default::default(),
        )
        .expect("observe");
        state.core_panes.insert(pane_id.into(), core_pane);
    }

    compose_until(&mut state, |rows| {
        shows(rows, "AAAA BBBB") && !shows(rows, "LIVE") && !shows(rows, "PANE")
    })
    .await;
}
