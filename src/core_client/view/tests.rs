use super::*;
use crate::core_client::screen::Screen;

fn view(cols: u16, rows: u16) -> View {
    let (_events_tx, events) = mpsc::channel(1);
    View::new(
        TerminalRuntime::test_with_screen_bytes(cols, rows, b""),
        events,
        Arc::new(Notify::new()),
        Arc::new(RenderSignal::new()),
    )
}

fn feed(view: &View, bytes: &[u8]) {
    view.runtime.test_process_pty_bytes(bytes);
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

    view.resize(30, 6);
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
    let mut view = View::attach(test.core.clone(), &address, 40, 5).expect("attach");
    let mut screen = vec![String::new(); 5];

    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !screen.iter().any(|row| row.contains("lawn-view-hello")) {
            view.output().await;
            for row in view.draw() {
                screen[usize::from(row.y)] = text(&row);
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the view never showed the output; saw {screen:?}"));
    tokio::time::timeout(std::time::Duration::from_secs(10), view.ended())
        .await
        .expect("the view sees the program end");
}
