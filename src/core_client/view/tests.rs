use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

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
async fn a_view_keeps_the_frame_it_drew_while_a_program_holds_synchronized_output() {
    let (mut view, _input, _wake) = scrolled(4, 3);
    assert_eq!(view.frame(), None);

    view.draw();
    let drawn = view.frame().expect("frame");
    assert_eq!(drawn.scroll, view.runtime.scroll_metrics());

    feed(&view, b"\x1b[?2026h");
    view.scroll_to(5);
    view.draw();
    assert_eq!(view.frame(), Some(drawn));
    assert_ne!(drawn.scroll, view.runtime.scroll_metrics());

    feed(&view, b"\x1b[?2026l");
    view.draw();
    assert_eq!(
        view.frame().map(|frame| frame.scroll),
        Some(view.runtime.scroll_metrics())
    );
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
        Placing {
            geometry,
            shown: true,
        },
        PaneId::from_raw(7),
        &wake,
        &events.sender(),
        Scrollback::from(&crate::config::Config::default()),
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
        Placing {
            geometry,
            shown: true,
        },
        id,
        &wake,
        &Events::default().sender(),
        Scrollback::from(&crate::config::Config::default()),
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

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_view_attached_off_screen_takes_the_size_only_once_shown() {
    let Some(test) = crate::hc::test_support::TestCore::start() else {
        return;
    };
    let address = test
        .core
        .run(serde_json::json!({"argv": ["cat"], "no_parent": true}))
        .expect("run");
    let cols = || {
        test.core
            .call("terminal.get", serde_json::json!({"terminal": address}))
            .expect("terminal.get")["cols"]
            .as_u64()
    };
    let (wake, events) = (Wake::default(), Events::default());
    let geometry = |cols| Geometry {
        cols,
        rows: 5,
        cell_width_px: 8,
        cell_height_px: 16,
    };
    let attach = |id, cols, shown| {
        View::attach(
            test.core.clone(),
            &address,
            Placing {
                geometry: geometry(cols),
                shown,
            },
            PaneId::from_raw(id),
            &wake,
            &events.sender(),
            Scrollback::from(&crate::config::Config::default()),
        )
        .expect("attach")
    };
    let mut other = attach(1, 50, true);
    assert_eq!(cols(), Some(50));

    let mut behind = attach(2, 30, false);
    assert_eq!(cols(), Some(50));
    behind.show(geometry(30));
    assert_eq!(cols(), Some(30));
    other.resize(geometry(51));
    assert_eq!(cols(), Some(51));
    behind.show(geometry(30));
    assert_eq!(cols(), Some(30));
    let _shown = attach(3, 40, true);
    assert_eq!(cols(), Some(40));
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
        modifiers: KeyModifiers::NONE,
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

fn scrolled_id() -> PaneId {
    PaneId::from_raw(5)
}

fn scrolled(rows: u16, wheel_lines: usize) -> (View, mpsc::Receiver<Bytes>, Wake) {
    let (runtime, input) =
        TerminalRuntime::test_with_channel_and_scrollback_bytes(20, rows, 100_000, b"", 16);
    let wake = Wake::default();
    let view = View::new(runtime, scrolled_id(), wake.clone(), wheel_lines);
    for line in 1..=30 {
        feed(&view, format!("\r\nline-{line}").as_bytes());
    }
    (view, input, wake)
}

fn offset(view: &View) -> usize {
    view.runtime
        .scroll_metrics()
        .map_or(0, |metrics| metrics.offset_from_bottom)
}

fn sent(input: &mut mpsc::Receiver<Bytes>) -> Vec<u8> {
    let mut sent = Vec::new();
    while let Ok(bytes) = input.try_recv() {
        sent.extend_from_slice(&bytes);
    }
    sent
}

fn mouse(kind: MouseEventKind, modifiers: KeyModifiers) -> MouseEvent {
    MouseEvent {
        kind,
        column: 0,
        row: 0,
        modifiers,
    }
}

fn wheel(kind: MouseEventKind) -> MouseEvent {
    mouse(kind, KeyModifiers::NONE)
}

fn page(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> TerminalKey {
    TerminalKey::new(code, modifiers).with_kind(kind)
}

fn screen_text(view: &mut View) -> String {
    view.draw().iter().map(text).collect::<Vec<_>>().join("|")
}

#[tokio::test]
async fn the_wheel_scrolls_the_scrollback_by_the_lines_per_notch_and_redraws() {
    let (mut view, mut input, wake) = scrolled(4, 5);
    view.draw();
    wake.take();

    view.mouse(wheel(MouseEventKind::ScrollUp));
    view.mouse(mouse(MouseEventKind::ScrollUp, KeyModifiers::CONTROL));

    assert_eq!(offset(&view), 10);
    assert_eq!(wake.take(), HashSet::from([scrolled_id()]));
    assert!(screen_text(&mut view).contains("line-17"));
    view.mouse(wheel(MouseEventKind::ScrollDown));
    assert_eq!(offset(&view), 5);
    assert!(sent(&mut input).is_empty());
}

#[tokio::test]
async fn a_wheel_notch_scrolls_at_least_one_line() {
    let (view, _input, _wake) = scrolled(4, 0);

    view.mouse(wheel(MouseEventKind::ScrollUp));

    assert_eq!(offset(&view), 1);
}

#[tokio::test]
async fn a_mouse_reporting_program_gets_one_report_per_notch_from_the_bottom() {
    let (view, mut input, _wake) = scrolled(4, 3);
    view.mouse(wheel(MouseEventKind::ScrollUp));
    feed(&view, b"\x1b[?1000h\x1b[?1006h");
    view.runtime.scroll_up(3);

    view.mouse(wheel(MouseEventKind::ScrollUp));
    view.mouse(wheel(MouseEventKind::ScrollDown));

    assert_eq!(offset(&view), 0);
    assert_eq!(sent(&mut input), b"\x1b[<64;1;1M\x1b[<65;1;1M");
}

#[tokio::test]
async fn the_alternate_screen_gets_one_arrow_per_notch_with_alternate_scroll() {
    let (view, mut input, _wake) = scrolled(4, 3);
    feed(&view, b"\x1b[?1049h\x1b[?1007h");

    view.mouse(wheel(MouseEventKind::ScrollUp));
    view.mouse(wheel(MouseEventKind::ScrollUp));
    view.mouse(wheel(MouseEventKind::ScrollDown));

    assert_eq!(sent(&mut input), b"\x1b[A\x1b[A\x1b[B");
}

#[tokio::test]
async fn a_horizontal_wheel_reaches_only_a_mouse_reporting_program() {
    let (view, mut input, _wake) = scrolled(4, 3);
    view.mouse(wheel(MouseEventKind::ScrollUp));

    view.mouse(wheel(MouseEventKind::ScrollLeft));
    assert_eq!(offset(&view), 3);
    assert!(sent(&mut input).is_empty());

    feed(&view, b"\x1b[?1000h\x1b[?1006h");
    view.mouse(wheel(MouseEventKind::ScrollRight));
    assert_eq!(offset(&view), 0);
    assert_eq!(sent(&mut input), b"\x1b[<67;1;1M");
}

#[tokio::test]
async fn keys_text_and_paste_return_a_scrolled_view_to_the_bottom() {
    let (view, mut input, wake) = scrolled(4, 3);
    let inputs: [&dyn Fn(&View); 3] = [
        &|view| view.key(TerminalKey::new(KeyCode::Char('a'), KeyModifiers::NONE)),
        &|view| view.text("typed"),
        &|view| view.paste("pasted".to_owned()),
    ];
    for deliver in inputs {
        view.mouse(wheel(MouseEventKind::ScrollUp));
        wake.take();

        deliver(&view);

        assert_eq!(offset(&view), 0);
        assert_eq!(wake.take(), HashSet::from([scrolled_id()]));
    }
    assert_eq!(sent(&mut input), b"atypedpasted");
}

#[tokio::test]
async fn a_key_release_leaves_a_scrolled_view_where_it_is() {
    let (view, _input, _wake) = scrolled(4, 3);
    view.mouse(wheel(MouseEventKind::ScrollUp));
    let scrolled = offset(&view);
    assert!(scrolled > 0);

    view.key(
        TerminalKey::new(KeyCode::Char('a'), KeyModifiers::NONE).with_kind(KeyEventKind::Release),
    );

    assert_eq!(offset(&view), scrolled);
}

#[tokio::test]
async fn a_reported_click_returns_to_the_bottom_but_motion_does_not() {
    let (view, mut input, _wake) = scrolled(4, 3);
    let click = MouseEventKind::Down(crossterm::event::MouseButton::Left);
    view.runtime.scroll_up(3);
    view.mouse(wheel(click));
    assert_eq!(offset(&view), 3);
    assert!(sent(&mut input).is_empty());

    feed(&view, b"\x1b[?1003h\x1b[?1006h");
    view.mouse(wheel(MouseEventKind::Moved));
    assert_eq!(offset(&view), 3);
    assert_eq!(sent(&mut input), b"\x1b[<35;1;1M");

    view.mouse(wheel(click));
    assert_eq!(offset(&view), 0);
    assert_eq!(sent(&mut input), b"\x1b[<0;1;1M");
}

#[tokio::test]
async fn a_mouse_report_counts_as_sent_only_when_the_terminal_took_it() {
    let (view, mut input, _wake) = scrolled(4, 3);
    let click = wheel(MouseEventKind::Down(crossterm::event::MouseButton::Left));
    assert!(!view.mouse(click));

    feed(&view, b"\x1b[?1000h\x1b[?1006h");
    assert!(view.mouse(click));

    while view
        .runtime
        .try_send_bytes(Bytes::from_static(b"x"))
        .is_ok()
    {}
    assert!(!view.mouse(click));

    sent(&mut input);
    assert!(view.mouse(click));
}

#[tokio::test]
async fn plain_page_keys_scroll_a_page_and_swallow_the_release() {
    let (view, mut input, wake) = scrolled(4, 3);
    let none = KeyModifiers::NONE;

    view.key(page(KeyCode::PageUp, none, KeyEventKind::Press));
    view.key(page(KeyCode::PageUp, none, KeyEventKind::Repeat));
    view.key(page(KeyCode::PageUp, none, KeyEventKind::Release));
    assert_eq!(offset(&view), 8);
    assert_eq!(wake.take(), HashSet::from([scrolled_id()]));

    view.key(page(KeyCode::PageDown, none, KeyEventKind::Press));
    view.key(page(KeyCode::PageDown, none, KeyEventKind::Release));
    assert_eq!(offset(&view), 4);
    assert!(sent(&mut input).is_empty());
}

#[tokio::test]
async fn page_keys_reach_the_program_when_modified_or_owned_by_it() {
    let (view, mut input, _wake) = scrolled(4, 3);
    view.runtime.scroll_up(4);

    view.key(page(
        KeyCode::PageUp,
        KeyModifiers::SHIFT,
        KeyEventKind::Press,
    ));
    assert_eq!(offset(&view), 0);
    assert_eq!(sent(&mut input), b"\x1b[5;2~");

    feed(&view, b"\x1b[?1h");
    view.runtime.scroll_up(4);
    view.key(page(
        KeyCode::PageDown,
        KeyModifiers::NONE,
        KeyEventKind::Press,
    ));
    assert_eq!(offset(&view), 0);
    assert_eq!(sent(&mut input), b"\x1b[6~");
}

#[tokio::test]
async fn the_cursor_hides_while_the_view_is_scrolled_back() {
    let (mut view, _input, _wake) = scrolled(4, 1);
    feed(&view, b"\x1b[H");
    assert!(view.cursor().expect("cursor").visible);

    view.mouse(wheel(MouseEventKind::ScrollUp));
    assert!(!view.cursor().expect("cursor").visible);

    view.text("x");
    assert!(view.cursor().expect("cursor").visible);
}

#[tokio::test]
async fn a_scrolled_view_keeps_its_place_as_output_arrives_and_on_resize() {
    let (mut view, _input, _wake) = scrolled(4, 3);
    view.mouse(wheel(MouseEventKind::ScrollUp));
    let before = screen_text(&mut view);

    feed(&view, b"\r\nline-31\r\nline-32");
    assert_eq!(offset(&view), 5);
    view.invalidate();
    assert_eq!(screen_text(&mut view), before);

    view.resize(Geometry {
        cols: 24,
        rows: 4,
        cell_width_px: 0,
        cell_height_px: 0,
    });
    assert!(offset(&view) > 0);
}

#[cfg(unix)]
struct CoreView {
    view: View,
    wake: Wake,
    screen: Vec<String>,
}

#[cfg(unix)]
impl CoreView {
    async fn attach(test: &crate::hc::test_support::TestCore, script: &str, printed: &str) -> Self {
        let address = test
            .core
            .run(serde_json::json!({"argv": ["sh", "-c", script], "no_parent": true}))
            .expect("run");
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !test
                .core
                .call("terminal.read", serde_json::json!({"terminal": address}))
                .is_ok_and(|read| read.to_string().contains(printed))
            {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the terminal never printed {printed}"));
        let geometry = Geometry {
            cols: 40,
            rows: 5,
            cell_width_px: 0,
            cell_height_px: 0,
        };
        let wake = Wake::default();
        let view = View::attach(
            test.core.clone(),
            &address,
            Placing {
                geometry,
                shown: true,
            },
            PaneId::from_raw(7),
            &wake,
            &Events::default().sender(),
            Scrollback::from(&crate::config::Config::default()),
        )
        .expect("attach");
        let mut attached = Self {
            view,
            wake,
            screen: vec![String::new(); 5],
        };
        attached.until_shown(printed).await;
        attached
    }

    fn repaint(&mut self) {
        for row in self.view.draw() {
            self.screen[usize::from(row.y)] = text(&row).trim_end().to_owned();
        }
    }

    fn shows(&self, line: &str) -> bool {
        self.screen.iter().any(|row| row == line)
    }

    async fn until_shown(&mut self, line: &str) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                self.repaint();
                if self.shows(line) {
                    return;
                }
                self.wake.wait().await;
                self.wake.take();
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the view never showed {line}; saw {:?}", self.screen));
    }

    fn scroll_until(&mut self, step: impl Fn(&View), line: &str) -> Vec<String> {
        let mut seen = self.screen.clone();
        for _ in 0..200 {
            step(&self.view);
            self.repaint();
            seen.extend(self.screen.iter().cloned());
            if self.shows(line) {
                return seen;
            }
        }
        panic!("scrolling never showed {line}; saw {:?}", self.screen);
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_core_view_scrolls_back_to_what_its_terminal_printed_before_the_attach() {
    let Some(test) = crate::hc::test_support::TestCore::start() else {
        return;
    };
    let mut core = CoreView::attach(
        &test,
        "seq 1 100; printf '%060d\\n' 0; echo done; sleep 30",
        "done",
    )
    .await;
    let press = |code| page(code, KeyModifiers::NONE, KeyEventKind::Press);

    let seen = core.scroll_until(
        |view| {
            view.mouse(wheel(MouseEventKind::ScrollUp));
        },
        "1",
    );
    assert!(seen.contains(&"0".repeat(40)), "{seen:?}");
    assert!(seen.contains(&"0".repeat(20)), "{seen:?}");
    core.scroll_until(
        |view| {
            view.mouse(wheel(MouseEventKind::ScrollDown));
        },
        "done",
    );
    core.scroll_until(|view| view.key(press(KeyCode::PageUp)), "1");
    core.scroll_until(|view| view.key(press(KeyCode::PageDown)), "done");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_core_view_scrolls_back_through_output_printed_after_the_attach() {
    let Some(test) = crate::hc::test_support::TestCore::start() else {
        return;
    };
    let mut core = CoreView::attach(
        &test,
        "echo ready; read line; seq 1 100; echo end; sleep 30",
        "ready",
    )
    .await;

    core.view.text("go\r");
    core.until_shown("end").await;

    core.scroll_until(
        |view| {
            view.mouse(wheel(MouseEventKind::ScrollUp));
        },
        "1",
    );
}

#[tokio::test]
async fn the_scrollback_text_is_the_whole_history_unstyled_with_wrapped_lines_joined() {
    let (view, _input) = View::test_with_input(10, 3);
    feed(&view, b"first\r\n\x1b[31mred\x1b[0m\r\n");
    feed(&view, b"abcdefghijklmno\r\n");
    for line in 0..20 {
        feed(&view, format!("line {line}\r\n").as_bytes());
    }
    feed(&view, b"last");

    let lines: Vec<String> = (0..20).map(|line| format!("line {line}")).collect();
    let expected = format!("first\nred\nabcdefghijklmno\n{}\nlast", lines.join("\n"));
    assert_eq!(view.scrollback_text(), expected);
}
