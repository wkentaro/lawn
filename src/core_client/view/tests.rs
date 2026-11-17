use super::*;
use crate::core_client::screen::Screen;

fn view(cols: u16, rows: u16) -> View {
    View::test(cols, rows)
}

fn feed(view: &View, bytes: &[u8]) {
    view.test_feed(bytes);
}

fn damaged(rows: &[PaneSurfacePatchRow]) -> Vec<u16> {
    rows.iter().map(|row| row.y).collect()
}

fn text(row: &PaneSurfacePatchRow) -> String {
    row.cells.iter().map(|cell| cell.symbol.as_str()).collect()
}

#[tokio::test]
async fn the_first_draw_paints_every_row() {
    let mut view = view(20, 4);

    assert_eq!(damaged(&view.draw()), [0, 1, 2, 3]);
}

#[tokio::test]
async fn output_on_one_row_damages_only_that_row() {
    let mut view = view(20, 4);
    feed(&view, b"top\x1b[3;1H");
    view.draw();

    feed(&view, b"abc");
    let rows = view.draw();

    assert_eq!(damaged(&rows), [2]);
    assert!(text(&rows[0]).starts_with("abc"));
    assert_eq!(rows[0].cells.len(), 20);
}

#[tokio::test]
async fn a_resize_damages_every_row() {
    let mut view = view(20, 4);
    view.draw();

    view.resize(Geometry {
        cols: 30,
        rows: 6,
        cell_width_px: 0,
        cell_height_px: 0,
    });
    let rows = view.draw();

    assert_eq!(damaged(&rows), [0, 1, 2, 3, 4, 5]);
    assert!(rows.iter().all(|row| row.cells.len() == 30));
}

#[tokio::test]
async fn synchronized_output_keeps_its_damage_until_it_ends() {
    let mut view = view(20, 4);
    view.draw();

    feed(&view, b"\x1b[?2026hx");
    assert!(view.draw().is_empty());

    feed(&view, b"\x1b[?2026l");
    assert_eq!(damaged(&view.draw()), [0]);
}

#[cfg(unix)]
#[tokio::test]
async fn synchronized_output_holds_the_cursor_until_it_ends() {
    let mut view = view(20, 4);
    feed(&view, b"\x1b[2;3H");
    view.draw();
    let before = view.cursor().expect("cursor");

    feed(&view, b"\x1b[?2026h\x1b[4;9H");
    assert_eq!(view.cursor(), Some(before));

    feed(&view, b"\x1b[?2026l");
    let after = view.cursor().expect("cursor");
    assert_eq!((after.x, after.y), (8, 3));
}

#[tokio::test]
async fn a_sync_start_after_an_earlier_check_withholds_the_cursor_snapshot() {
    let mut view = view(20, 4);
    feed(&view, b"\x1b[2;3H");
    view.draw();
    let before = view.cursor();
    assert!(!view.runtime.synchronized_output_active());

    feed(&view, b"\x1b[?2026h\x1b[4;9H");

    let (held, cursor) = view.runtime.cursor_snapshot();
    assert!(held);
    assert!(cursor.is_none());
    assert_eq!(view.cursor(), before);
}

#[tokio::test]
async fn the_host_reports_all_keys_only_while_the_program_needs_it() {
    let view = view(20, 4);
    assert!(!view.reports_all_keys());

    feed(&view, b"\x1b[>1u");
    assert!(!view.reports_all_keys());

    feed(&view, b"\x1b[=9u");
    assert!(view.reports_all_keys());

    feed(&view, b"\x1b[=2u\x1b[>4;2m");
    assert!(view.reports_all_keys());

    feed(&view, b"\x1b[<u\x1b[>4;0m");
    assert!(!view.reports_all_keys());
}

#[tokio::test]
async fn a_screen_writes_only_what_changed_and_nothing_twice() {
    let mut view = view(20, 4);
    let mut screen = Screen::default();
    let mut out = Vec::new();
    feed(&view, b"first-row\x1b[3;1H");
    screen
        .present(&mut out, (20, 4), view.draw(), view.cursor())
        .expect("present");
    assert!(String::from_utf8_lossy(&out).contains("first-row"));

    out.clear();
    feed(&view, b"abc");
    screen
        .present(&mut out, (20, 4), view.draw(), view.cursor())
        .expect("present");
    let written = String::from_utf8_lossy(&out);
    assert!(written.contains("abc"), "{written:?}");
    assert!(!written.contains("first-row"), "{written:?}");

    out.clear();
    screen
        .present(&mut out, (20, 4), view.draw(), view.cursor())
        .expect("present");
    assert!(out.is_empty(), "{:?}", String::from_utf8_lossy(&out));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_view_shows_a_core_terminal_until_its_program_ends() {
    let Some(test) = crate::hc::test_support::TestCore::start() else {
        return;
    };
    let address = test
        .core
        .run(serde_json::json!({
            "argv": ["sh", "-c", "printf lawn-view-hello; sleep 1"],
            "no_parent": true,
        }))
        .expect("run");
    let geometry = Geometry {
        cols: 40,
        rows: 5,
        cell_width_px: 0,
        cell_height_px: 0,
    };
    let wake = Wake::default();
    let mut events = Events::default();
    let mut view = View::attach(
        test.core.clone(),
        &address,
        geometry,
        PaneId::from_raw(7),
        &wake,
        &events.sender(),
    )
    .expect("attach");
    let mut screen = vec![String::new(); 5];

    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !screen.iter().any(|row| row.contains("lawn-view-hello")) {
            wake.wait().await;
            wake.take();
            for row in view.draw() {
                screen[usize::from(row.y)] = text(&row);
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the view never showed the output; saw {screen:?}"));
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while events.recv().await != Event::Ended(PaneId::from_raw(7)) {}
    })
    .await
    .expect("the view sees the program end");
}

#[test]
fn a_take_names_every_view_with_output_and_rearms_the_wake() {
    let wake = Wake::default();
    let (left, right) = (PaneId::from_raw(1), PaneId::from_raw(2));
    assert!(wake.dirty.request_pty(left));
    assert!(!wake.dirty.request_pty(right));
    wake.dirty.request_terminal_title(left);

    assert_eq!(wake.take(), HashSet::from([left, right]));

    assert!(!wake.dirty.is_pending());
    assert!(wake.dirty.pending_terminal_title_sources().is_empty());
    assert!(wake.dirty.request_pty(right));
    assert_eq!(wake.take(), HashSet::from([right]));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn each_core_output_wakes_the_host_to_draw_it() {
    let Some(test) = crate::hc::test_support::TestCore::start() else {
        return;
    };
    let address = test
        .core
        .run(serde_json::json!({"argv": ["cat"], "no_parent": true}))
        .expect("run");
    let geometry = Geometry {
        cols: 40,
        rows: 5,
        cell_width_px: 0,
        cell_height_px: 0,
    };
    let wake = Wake::default();
    let id = PaneId::from_raw(7);
    let mut view = View::attach(
        test.core.clone(),
        &address,
        geometry,
        id,
        &wake,
        &Events::default().sender(),
    )
    .expect("attach");
    for word in ["first-word", "second-word"] {
        view.text(&format!("{word}\r"));
        let mut screen = vec![String::new(); 5];
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !screen.iter().any(|row| row.contains(word)) {
                wake.wait().await;
                assert_eq!(wake.take(), HashSet::from([id]));
                for row in view.draw() {
                    screen[usize::from(row.y)] = text(&row);
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{word} never reached the view; saw {screen:?}"));
    }
}

#[tokio::test]
async fn pixel_mouse_reports_use_the_host_cell_size() {
    let mut view = view(20, 4);
    view.resize(Geometry {
        cols: 20,
        rows: 4,
        cell_width_px: 10,
        cell_height_px: 20,
    });
    feed(&view, b"\x1b[?1000h\x1b[?1006h\x1b[?1016h");

    let report = view.mouse_report(MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: 3,
        row: 2,
        modifiers: crossterm::event::KeyModifiers::NONE,
    });

    assert_eq!(report.as_deref(), Some(&b"\x1b[<0;35;50M"[..]));
}

#[tokio::test]
async fn an_osc52_copy_and_a_bell_reach_the_host_and_the_program_end_follows() {
    let mut events = Events::default();
    let events_tx = events.sender.clone();
    events_tx
        .send(AppEvent::ClipboardWrite {
            content: b"copied".to_vec(),
        })
        .await
        .expect("send");
    events_tx
        .send(AppEvent::TerminalBell {
            pane_id: PaneId::from_raw(0),
            count: 2,
        })
        .await
        .expect("send");
    events_tx
        .send(AppEvent::PaneDied {
            pane_id: PaneId::from_raw(3),
            exit_reason: crate::platform::ChildExitReason::Exited,
        })
        .await
        .expect("send");

    assert_eq!(events.recv().await, Event::Copy(b"copied".to_vec()));
    assert_eq!(events.recv().await, Event::Bell(2));
    assert_eq!(events.recv().await, Event::Ended(PaneId::from_raw(3)));
    events_tx
        .send(AppEvent::PaneDied {
            pane_id: PaneId::from_raw(4),
            exit_reason: crate::platform::ChildExitReason::WaitFailed,
        })
        .await
        .expect("send");
    assert_eq!(events.recv().await, Event::Detached(PaneId::from_raw(4)));
}
