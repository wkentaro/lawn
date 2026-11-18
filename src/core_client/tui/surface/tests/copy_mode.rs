use super::*;
use crate::core_client::tui::copy_mode::CopyMode;
use crate::protocol::CellData;

fn copying(shown: &[u8]) -> (Surface, Rect, HashSet<PaneId>) {
    let (mut surface, inner, _input) = selecting(false, shown);
    surface.enter_copy_mode();
    assert!(surface.copy_mode());
    let dirty = HashSet::from([id_of(&surface, "t0")]);
    (surface, inner, dirty)
}

fn typed(surface: &mut Surface, commands: &str) -> Option<String> {
    commands
        .chars()
        .map(|command| {
            surface.copy_mode_key(&TerminalKey::new(
                KeyCode::Char(command),
                KeyModifiers::NONE,
            ))
        })
        .last()
        .flatten()
}

fn pressed(surface: &mut Surface, code: KeyCode, modifiers: KeyModifiers) -> Option<String> {
    surface.copy_mode_key(&TerminalKey::new(code, modifiers))
}

fn cursor(surface: &mut Surface, dirty: &HashSet<PaneId>) -> Option<(u16, u16)> {
    frame(surface, dirty);
    let (_, inner) = super::super::visible(&surface.views, id_of(surface, "t0"))?;
    let cursor = surface.overlays().copy.cursor;
    (!cursor.is_empty()).then(|| (cursor.x - inner.x, cursor.y - inner.y))
}

fn offset(surface: &Surface) -> usize {
    metrics(surface, "t0").offset_from_bottom
}

fn last_row(surface: &Surface) -> u16 {
    surface.host.rows - 1
}

fn cell_at(rows: &[PaneSurfacePatchRow], x: u16, y: u16) -> Option<CellData> {
    rows.iter()
        .find(|row| row.y == y && row.x <= x && x < row.x + row.cells.len() as u16)
        .map(|row| row.cells[usize::from(x - row.x)].clone())
}

fn row_text(rows: &[PaneSurfacePatchRow], y: u16) -> String {
    text(
        &rows
            .iter()
            .filter(|row| row.y == y)
            .cloned()
            .collect::<Vec<_>>(),
    )
}

#[tokio::test]
async fn copy_mode_starts_at_the_program_cursor_with_its_bar_on_the_last_row_of_the_panes() {
    let (mut surface, inner, dirty) = copying(b"hello\r\nworld");

    let (rows, hardware) = surface.frame(&dirty, never_attach, ignore_leaving);

    assert!(hardware.is_none());
    let accent = crate::core_client::tui::copy_mode::cursor_cell(&surface.palette).bg;
    let at = |x: u16, y: u16| cell_at(&rows, x, y).map(|cell| cell.bg);
    assert_eq!(at(inner.x + 5, inner.y + 1), Some(accent));
    assert_ne!(at(inner.x + 4, inner.y + 1), Some(accent));
    assert_eq!(cursor(&mut surface, &dirty), Some((5, 1)));

    typed(&mut surface, "h");
    surface.enter_copy_mode();
    assert_eq!(cursor(&mut surface, &dirty), Some((4, 1)));
}

#[tokio::test]
async fn the_bar_spans_the_last_row_of_the_pane_area_across_both_panes() {
    let (mut surface, _, dirty) = copying(b"hello");

    let rows = frame(&mut surface, &dirty);

    let width = surface.host.cols;
    let bar = crate::core_client::tui::copy_mode::bar(&surface.overlays().copy, &surface.palette);
    let y = last_row(&surface);
    let drawn: Vec<_> = (0..width).filter_map(|x| cell_at(&rows, x, y)).collect();
    assert_eq!(drawn, bar);
    assert!(row_text(&rows, y).starts_with(" COPY  h/j/k/l"));
}

#[tokio::test]
async fn copy_mode_does_not_start_in_a_pane_that_has_not_drawn() {
    let mut view = View::test(20, 5);
    view.test_feed(&lines(20));
    let inner = Rect::new(0, 0, 20, 5);

    assert!(CopyMode::enter(PaneId::from_raw(1), &view, inner).is_none());

    view.draw();
    assert!(CopyMode::enter(PaneId::from_raw(1), &view, inner).is_some());
}

#[tokio::test]
async fn copy_mode_does_not_start_while_the_program_holds_synchronized_output() {
    let (mut surface, _, _input) = selecting(false, b"hello");
    feed(&surface, "t0", b"\x1b[?2026h");

    surface.enter_copy_mode();
    assert!(!surface.copy_mode());

    feed(&surface, "t0", b"\x1b[?2026l");
    let dirty = HashSet::from([id_of(&surface, "t0")]);
    frame(&mut surface, &dirty);
    surface.enter_copy_mode();
    assert!(surface.copy_mode());
}

#[tokio::test]
async fn hjkl_and_the_arrows_move_the_cursor_and_stop_at_the_pane_edges() {
    let (mut surface, inner, dirty) = copying(b"hello\r\nworld");

    typed(&mut surface, "hk");
    assert_eq!(cursor(&mut surface, &dirty), Some((4, 0)));
    typed(&mut surface, "lj");
    assert_eq!(cursor(&mut surface, &dirty), Some((5, 1)));
    for (code, at) in [
        (KeyCode::Left, (4, 1)),
        (KeyCode::Up, (4, 0)),
        (KeyCode::Right, (5, 0)),
        (KeyCode::Down, (5, 1)),
    ] {
        pressed(&mut surface, code, KeyModifiers::NONE);
        assert_eq!(cursor(&mut surface, &dirty), Some(at), "{code:?}");
    }

    typed(&mut surface, &"l".repeat(usize::from(inner.width)));
    typed(&mut surface, &"j".repeat(usize::from(inner.height)));
    assert_eq!(
        cursor(&mut surface, &dirty),
        Some((inner.width - 1, inner.height - 1))
    );
    typed(&mut surface, &"hk".repeat(usize::from(inner.width)));
    assert_eq!(cursor(&mut surface, &dirty), Some((0, 0)));
    assert_eq!(offset(&surface), 0);
}

#[tokio::test]
async fn line_motions_reach_the_line_start_its_first_word_and_its_last_character() {
    let (mut surface, _, dirty) = copying(b"   two words  \x1b[1D");

    for (commands, col) in [("0", 0), ("^", 3), ("$", 11), ("0", 0)] {
        typed(&mut surface, commands);
        assert_eq!(cursor(&mut surface, &dirty), Some((col, 0)), "{commands}");
    }
    pressed(&mut surface, KeyCode::End, KeyModifiers::NONE);
    assert_eq!(cursor(&mut surface, &dirty), Some((11, 0)));
    pressed(&mut surface, KeyCode::Home, KeyModifiers::NONE);
    assert_eq!(cursor(&mut surface, &dirty), Some((0, 0)));
}

#[tokio::test]
async fn word_motions_move_by_words_and_by_blank_separated_words() {
    let (mut surface, _, dirty) = copying(b"foo.bar baz");
    typed(&mut surface, "0");

    for (commands, col) in [
        ("w", 3),
        ("w", 4),
        ("b", 3),
        ("e", 6),
        ("0W", 8),
        ("B", 0),
        ("E", 6),
    ] {
        typed(&mut surface, commands);
        assert_eq!(cursor(&mut surface, &dirty), Some((col, 0)), "{commands}");
    }
}

#[tokio::test]
async fn paragraph_motions_jump_to_the_blank_lines_around_a_paragraph() {
    let (mut surface, _, dirty) = copying(b"one\r\n\r\ntwo\r\nthree");

    typed(&mut surface, "{");
    assert_eq!(cursor(&mut surface, &dirty), Some((5, 1)));
    typed(&mut surface, "}");
    assert_eq!(cursor(&mut surface, &dirty), Some((5, 4)));
}

#[tokio::test]
async fn pages_and_half_pages_move_the_cursor_and_the_view_together() {
    let (mut surface, inner, dirty) = copying(&lines(40));
    let bottom = inner.height - 1;
    let half = usize::from(inner.height / 2);
    let page = usize::from(inner.height - 2);

    for (code, modifiers, scrolled) in [
        (KeyCode::Char('u'), KeyModifiers::CONTROL, half),
        (KeyCode::Char('d'), KeyModifiers::CONTROL, 0),
        (KeyCode::Char('b'), KeyModifiers::CONTROL, page),
        (KeyCode::Char('f'), KeyModifiers::CONTROL, 0),
        (KeyCode::PageUp, KeyModifiers::NONE, page),
        (KeyCode::PageDown, KeyModifiers::NONE, 0),
    ] {
        pressed(&mut surface, code, modifiers);
        assert_eq!(cursor(&mut surface, &dirty), Some((7, bottom)), "{code:?}");
        assert_eq!(offset(&surface), scrolled, "{code:?}");
    }
}

#[tokio::test]
async fn g_goes_to_the_first_line_of_history_and_shift_g_to_the_last() {
    let (mut surface, inner, dirty) = copying(&lines(40));
    let max = metrics(&surface, "t0").max_offset_from_bottom;

    typed(&mut surface, "g");
    assert_eq!(cursor(&mut surface, &dirty), Some((7, 0)));
    assert_eq!(offset(&surface), max);

    typed(&mut surface, "G");
    assert_eq!(cursor(&mut surface, &dirty), Some((7, inner.height - 1)));
    assert_eq!(offset(&surface), 0);
}

#[tokio::test]
async fn the_view_follows_the_cursor_past_the_top_and_the_bottom() {
    let (mut surface, inner, dirty) = copying(&lines(40));
    let bottom = inner.height - 1;

    typed(&mut surface, &"k".repeat(usize::from(bottom)));
    assert_eq!(cursor(&mut surface, &dirty), Some((7, 0)));
    assert_eq!(offset(&surface), 0);

    typed(&mut surface, "kk");
    assert_eq!(cursor(&mut surface, &dirty), Some((7, 0)));
    assert_eq!(offset(&surface), 2);

    typed(&mut surface, &"j".repeat(usize::from(bottom) + 1));
    assert_eq!(cursor(&mut surface, &dirty), Some((7, bottom)));
    assert_eq!(offset(&surface), 1);
}

#[tokio::test]
async fn v_selects_from_its_anchor_to_the_cursor_and_y_copies_it_and_leaves() {
    let (mut surface, _, _) = copying(b"hello world\r\nnext");

    typed(&mut surface, "kv");
    assert!(surface.select.shown().is_none());
    typed(&mut surface, "0");
    surface.deselect();
    assert_eq!(shown_text(&surface).as_deref(), Some("hello"));
    assert!(!surface.copyable());
    assert!(surface.overlays().copy.selecting);

    assert_eq!(typed(&mut surface, "y").as_deref(), Some("hello"));
    assert!(!surface.copy_mode());
    assert!(surface.select.shown().is_none());
}

#[tokio::test]
async fn shift_v_selects_whole_lines_and_enter_copies_them() {
    let (mut surface, _, _) = copying(b"first\r\nsecond\r\nthird");

    typed(&mut surface, "Vk");

    assert_eq!(
        pressed(&mut surface, KeyCode::Enter, KeyModifiers::NONE).as_deref(),
        Some("second\nthird")
    );
    assert!(!surface.copy_mode());
}

#[tokio::test]
async fn y_without_a_moved_selection_copies_nothing_and_leaves() {
    let (mut surface, _, _) = copying(b"hello");

    assert_eq!(typed(&mut surface, "vy"), None);
    assert!(!surface.copy_mode());
}

#[tokio::test]
async fn escape_first_drops_the_selection_then_leaves_without_copying() {
    let (mut surface, _, _) = copying(b"hello");
    typed(&mut surface, "vh");

    assert_eq!(
        pressed(&mut surface, KeyCode::Esc, KeyModifiers::NONE),
        None
    );
    assert!(surface.copy_mode());
    assert!(surface.select.shown().is_none());

    assert_eq!(
        pressed(&mut surface, KeyCode::Esc, KeyModifiers::NONE),
        None
    );
    assert!(!surface.copy_mode());
}

#[tokio::test]
async fn q_leaves_without_copying_the_selection() {
    let (mut surface, _, _) = copying(b"hello");

    assert_eq!(typed(&mut surface, "vhq"), None);
    assert!(!surface.copy_mode());
    assert!(surface.select.shown().is_none());
}

#[tokio::test]
async fn leaving_puts_the_view_back_where_copy_mode_started() {
    let (mut surface, inner, _input) = selecting(false, &lines(40));
    let dirty = HashSet::from([id_of(&surface, "t0")]);
    let now = std::time::Instant::now();
    surface.mouse(mouse_at(MouseEventKind::ScrollUp, inner.x, inner.y), now);
    let entry = offset(&surface);
    assert!(entry > 0);
    frame(&mut surface, &dirty);
    surface.enter_copy_mode();

    typed(&mut surface, "g");
    assert_ne!(offset(&surface), entry);
    typed(&mut surface, "q");

    assert_eq!(offset(&surface), entry);
}

#[tokio::test]
async fn at_the_bottom_the_view_follows_output_and_the_next_motion_returns_to_the_cursor() {
    let (mut surface, inner, dirty) = copying(&lines(40));
    typed(&mut surface, &"k".repeat(usize::from(inner.height - 1)));
    assert_eq!(cursor(&mut surface, &dirty), Some((7, 0)));

    feed(&surface, "t0", b"\r\nnew-1");
    assert_eq!(cursor(&mut surface, &dirty), None);
    assert_eq!(offset(&surface), 0);

    typed(&mut surface, "l");
    assert_eq!(cursor(&mut surface, &dirty), Some((8, 0)));
    assert_eq!(offset(&surface), 1);
    assert_eq!(typed(&mut surface, "hv0y").as_deref(), Some("line-33"));
}

#[tokio::test]
async fn a_scrolled_back_view_and_its_cursor_mark_stay_put_during_output() {
    let (mut surface, inner, dirty) = copying(&lines(40));
    typed(&mut surface, &"k".repeat(usize::from(inner.height)));
    assert_eq!(cursor(&mut surface, &dirty), Some((7, 0)));
    assert_eq!(offset(&surface), 1);

    feed(&surface, "t0", b"\r\nnew-1\r\nnew-2");
    assert_eq!(cursor(&mut surface, &dirty), Some((7, 0)));
    assert_eq!(offset(&surface), 3);
    assert_eq!(typed(&mut surface, "v0y").as_deref(), Some("line-32"));
}

#[tokio::test]
async fn moving_focus_away_parks_copy_mode_and_coming_back_resumes_it() {
    let (mut surface, _, dirty) = copying(b"hello");
    typed(&mut surface, "hvh");
    assert_eq!(shown_text(&surface).as_deref(), Some("lo"));

    surface.act(KeybindAction::FocusPaneRight, true);
    assert!(!surface.copy_mode());
    assert!(surface.select.shown().is_none());
    assert_eq!(
        pressed(&mut surface, KeyCode::Char('h'), KeyModifiers::NONE),
        None
    );
    frame(&mut surface, &dirty);
    assert!(surface.overlays().copy == Default::default());
    assert!(surface.cursor().is_some());

    surface.act(KeybindAction::FocusPaneLeft, true);
    assert!(surface.copy_mode());
    assert_eq!(cursor(&mut surface, &dirty), Some((3, 0)));
    assert_eq!(shown_text(&surface).as_deref(), Some("lo"));
    assert!(surface.overlays().copy.selecting);
}

#[tokio::test]
async fn a_parked_copy_mode_leaves_mouse_selection_in_the_other_pane_alone() {
    let (mut surface, _, _) = copying(b"hello");
    surface.act(KeybindAction::FocusPaneRight, true);
    let (_, right) = super::super::visible(&surface.views, id_of(&surface, "t1")).expect("visible");

    mouse(&mut surface, click(right.x, right.y));
    mouse(&mut surface, drag_to(right.x + 4, right.y));
    mouse(&mut surface, release_at(right.x + 4, right.y));
    let both = HashSet::from([id_of(&surface, "t0"), id_of(&surface, "t1")]);
    frame(&mut surface, &both);
    assert_eq!(shown_text(&surface).as_deref(), Some("hello"));
    assert!(surface.copyable());

    surface.deselect();
    assert!(surface.select.shown().is_none());
}

#[tokio::test]
async fn focusing_a_hidden_parked_pane_directly_brings_back_its_selection() {
    let (mut surface, _, dirty) = copying(b"hello");
    let pane = id_of(&surface, "t0");
    typed(&mut surface, "hvh");
    surface.act(KeybindAction::FocusPaneRight, true);
    surface.act(KeybindAction::Zoom, true);
    frame(&mut surface, &HashSet::new());
    assert!(super::super::visible(&surface.views, pane).is_none());

    surface.act(KeybindAction::LastPane, true);
    frame(&mut surface, &dirty);

    assert!(surface.copy_mode());
    assert_eq!(shown_text(&surface).as_deref(), Some("lo"));
    assert_eq!(cursor(&mut surface, &dirty), Some((3, 0)));
    assert_eq!(typed(&mut surface, "y").as_deref(), Some("lo"));
}

#[tokio::test]
async fn a_key_before_the_next_frame_keeps_a_hidden_parked_pane_in_copy_mode() {
    let (mut surface, _, dirty) = copying(b"hello");
    let pane = id_of(&surface, "t0");
    typed(&mut surface, "hvh");
    surface.act(KeybindAction::FocusPaneRight, true);
    surface.act(KeybindAction::Zoom, true);
    frame(&mut surface, &HashSet::new());
    assert!(super::super::visible(&surface.views, pane).is_none());

    surface.act(KeybindAction::LastPane, true);
    assert_eq!(typed(&mut surface, "h"), None);

    assert!(surface.copy_mode());
    frame(&mut surface, &dirty);
    assert_eq!(typed(&mut surface, "y").as_deref(), Some("lo"));
}

#[tokio::test]
async fn a_yank_before_the_next_frame_copies_the_selection_a_parked_pane_kept() {
    let (mut surface, _, _) = copying(b"hello");
    typed(&mut surface, "hvh");
    surface.act(KeybindAction::FocusPaneRight, true);
    frame(&mut surface, &HashSet::new());
    assert!(surface.select.shown().is_none());

    surface.act(KeybindAction::LastPane, true);

    assert_eq!(typed(&mut surface, "y").as_deref(), Some("lo"));
    assert!(!surface.copy_mode());
}

#[tokio::test]
async fn a_word_motion_stops_at_the_last_column_a_narrow_pane_shows() {
    let mut surface = surface_with_scrollbars(6, 10, false);
    surface.update(row_of(&terminals(2)));
    for (_, id, geometry) in requests(&mut surface) {
        let view = View::test(geometry.cols.max(crate::pane::MIN_PANE_COLS), geometry.rows);
        view.test_feed(b"a bc\r");
        surface.attached(id, geometry, Ok(view));
    }
    let dirty = HashSet::new();
    frame(&mut surface, &dirty);
    surface.enter_copy_mode();
    assert_eq!(cursor(&mut surface, &dirty), Some((0, 0)));

    typed(&mut surface, "w");

    assert_eq!(cursor(&mut surface, &dirty), Some((1, 0)));
}

#[tokio::test]
async fn a_mouse_selection_takes_over_from_the_keyboard_one() {
    let (mut surface, inner, dirty) = copying(b"hello world");
    typed(&mut surface, "0vl");
    assert_eq!(shown_text(&surface).as_deref(), Some("he"));

    mouse(&mut surface, click(inner.x + 6, inner.y));
    mouse(&mut surface, drag_to(inner.x + 10, inner.y));
    mouse(&mut surface, release_at(inner.x + 10, inner.y));
    frame(&mut surface, &dirty);
    assert_eq!(shown_text(&surface).as_deref(), Some("world"));

    typed(&mut surface, "l");
    frame(&mut surface, &dirty);
    assert_eq!(shown_text(&surface).as_deref(), Some("world"));
    assert!(surface.overlays().copy.selecting);

    assert_eq!(
        pressed(&mut surface, KeyCode::Esc, KeyModifiers::NONE),
        None
    );
    assert!(surface.copy_mode());
    assert!(surface.select.shown().is_none());
    frame(&mut surface, &dirty);
    assert!(surface.select.shown().is_none());
    assert!(!surface.overlays().copy.selecting);
}

#[tokio::test]
async fn a_keyboard_selection_takes_over_from_the_mouse_one() {
    let (mut surface, inner, dirty) = copying(b"hello world");
    mouse(&mut surface, click(inner.x + 6, inner.y));
    mouse(&mut surface, drag_to(inner.x + 10, inner.y));
    mouse(&mut surface, release_at(inner.x + 10, inner.y));
    frame(&mut surface, &dirty);
    assert!(surface.overlays().copy.selecting);

    assert_eq!(
        pressed(&mut surface, KeyCode::Esc, KeyModifiers::NONE),
        None
    );
    assert!(surface.copy_mode());
    assert!(surface.select.shown().is_none());

    mouse(&mut surface, click(inner.x + 6, inner.y));
    mouse(&mut surface, drag_to(inner.x + 10, inner.y));
    mouse(&mut surface, release_at(inner.x + 10, inner.y));
    typed(&mut surface, "0vl");
    assert_eq!(shown_text(&surface).as_deref(), Some("he"));
    assert_eq!(typed(&mut surface, "y").as_deref(), Some("he"));
}

#[tokio::test]
async fn erased_history_drops_the_selection_and_clamps_the_cursor() {
    let (mut surface, inner, dirty) = copying(&lines(40));
    typed(&mut surface, "vkk");
    assert!(surface.select.shown().is_some());

    feed(&surface, "t0", b"\x1b[3J");
    frame(&mut surface, &dirty);

    assert!(surface.copy_mode());
    assert!(surface.select.shown().is_none());
    assert!(!surface.overlays().copy.selecting);
    assert_eq!(cursor(&mut surface, &dirty), Some((7, inner.height - 1)));
}

#[tokio::test]
async fn a_hidden_parked_pane_keeps_its_copy_mode() {
    let (mut surface, _, dirty) = copying(b"hello");
    let pane = id_of(&surface, "t0");
    typed(&mut surface, "h");
    surface.act(KeybindAction::FocusPaneRight, true);
    surface.act(KeybindAction::Zoom, true);
    frame(&mut surface, &HashSet::new());
    assert!(super::super::visible(&surface.views, pane).is_none());

    surface.act(KeybindAction::Zoom, true);
    frame(&mut surface, &HashSet::new());
    surface.act(KeybindAction::FocusPaneLeft, true);

    assert!(surface.copy_mode());
    assert_eq!(cursor(&mut surface, &dirty), Some((4, 0)));
}

#[tokio::test]
async fn entering_copy_mode_in_another_pane_ends_the_parked_one() {
    let (mut surface, _, _) = copying(&lines(40));
    typed(&mut surface, "g");
    assert!(offset(&surface) > 0);
    surface.act(KeybindAction::FocusPaneRight, true);

    surface.enter_copy_mode();
    assert!(surface.copy_mode());
    assert_eq!(offset(&surface), 0);

    surface.act(KeybindAction::FocusPaneLeft, true);
    assert!(!surface.copy_mode());
}

#[tokio::test]
async fn a_detached_pane_ends_copy_mode() {
    let (mut surface, _, _) = copying(b"hello");

    surface.forget(id_of(&surface, "t0"));

    assert!(!surface.copy_mode());
}

#[tokio::test]
async fn a_new_pane_size_keeps_copy_mode_drops_its_selection_and_clamps_the_cursor() {
    let (mut surface, _, dirty) = copying(b"hello");
    typed(&mut surface, &format!("v{}", "l".repeat(40)));
    assert!(surface.select.shown().is_some());

    surface.resize(host(30, 10));
    frame(&mut surface, &dirty);

    assert!(surface.copy_mode());
    assert!(surface.select.shown().is_none());
    assert!(!surface.overlays().copy.selecting);
    let (_, inner) = super::super::visible(&surface.views, id_of(&surface, "t0")).expect("visible");
    assert_eq!(cursor(&mut surface, &dirty), Some((inner.width - 1, 0)));
    typed(&mut surface, "h");
    assert_eq!(cursor(&mut surface, &dirty), Some((inner.width - 2, 0)));
}

#[tokio::test]
async fn switching_screens_keeps_copy_mode_drops_its_selection_and_clamps_the_cursor() {
    for by_key in [false, true] {
        let (mut surface, inner, dirty) = copying(&lines(40));
        typed(&mut surface, "vk");
        assert!(surface.select.shown().is_some());

        feed(&surface, "t0", b"\x1b[?1049h");
        if by_key {
            assert_eq!(typed(&mut surface, "l"), None);
        } else {
            frame(&mut surface, &dirty);
        }

        assert!(surface.copy_mode(), "{by_key}");
        assert!(surface.select.shown().is_none(), "{by_key}");
        assert!(!surface.overlays().copy.selecting, "{by_key}");
        let col = if by_key { 8 } else { 7 };
        assert_eq!(
            cursor(&mut surface, &dirty),
            Some((col, inner.height - 1)),
            "{by_key}"
        );
    }
}

#[tokio::test]
async fn the_cursor_and_bar_stay_on_their_rows_while_the_program_holds_synchronized_output() {
    let (mut surface, _, dirty) = copying(&lines(40));
    feed(&surface, "t0", b"\x1b[?2026h");

    typed(&mut surface, "g");
    let rows = frame(&mut surface, &dirty);
    assert_eq!(cursor(&mut surface, &dirty), None);
    assert!(row_text(&rows, last_row(&surface)).starts_with(" COPY "));
    assert!(rows.iter().all(|row| row.y == last_row(&surface)));

    feed(&surface, "t0", b"\x1b[?2026l");
    assert_eq!(cursor(&mut surface, &dirty), Some((7, 0)));
}

const FOUND: &[u8] = b"alpha one\r\nbeta two\r\nalpha three\r\ngamma";

fn bar_text(surface: &Surface) -> String {
    crate::core_client::tui::copy_mode::bar(&surface.overlays().copy, &surface.palette)
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>()
        .trim_end()
        .to_owned()
}

fn searched(surface: &mut Surface, prompt: &str) {
    typed(surface, prompt);
    pressed(surface, KeyCode::Enter, KeyModifiers::NONE);
}

fn wide(shown: &[u8]) -> (Surface, Rect, HashSet<PaneId>) {
    let (mut surface, _, dirty) = copying(shown);
    surface.resize(host(120, 10));
    frame(&mut surface, &dirty);
    let (_, inner) = super::super::visible(&surface.views, id_of(&surface, "t0")).expect("visible");
    (surface, inner, dirty)
}

#[tokio::test]
async fn slash_and_question_mark_open_a_search_prompt_on_the_bar() {
    let (mut surface, _, dirty) = copying(FOUND);

    typed(&mut surface, "/");
    assert_eq!(bar_text(&surface), " COPY  /");
    let bar = crate::core_client::tui::copy_mode::bar(&surface.overlays().copy, &surface.palette);
    let mut caret = ratatui::buffer::Cell::default();
    caret
        .set_fg(surface.palette.panel_bg)
        .set_bg(surface.palette.text);
    let caret = CellData::from_ratatui_cell(&caret);
    assert_eq!((bar[8].fg, bar[8].bg), (caret.fg, caret.bg));
    pressed(&mut surface, KeyCode::Esc, KeyModifiers::NONE);
    assert!(surface.copy_mode());
    assert!(bar_text(&surface).starts_with(" COPY  h/j/k/l"));

    typed(&mut surface, "?");
    assert_eq!(bar_text(&surface), " COPY  ?");
    surface.resize(host(60, 10));
    frame(&mut surface, &dirty);
    assert_eq!(
        bar_text(&surface),
        format!(" COPY  ?{}  enter search  esc cancel", " ".repeat(26))
    );
}

#[tokio::test]
async fn the_prompt_takes_typed_text_editing_keys_and_paste_and_none_reach_the_program() {
    let (mut surface, _, mut input) = selecting(false, FOUND);
    surface.enter_copy_mode();
    while input.try_recv().is_ok() {}

    typed(&mut surface, "/yq/n");
    assert!(surface.copy_mode());
    pressed(&mut surface, KeyCode::Char('u'), KeyModifiers::CONTROL);
    let text = crate::raw_input::RawInputEvent::Text(crate::input::TextCommit::new("nee"));
    assert!(surface.copy_mode_input(&text));
    assert!(surface.copy_mode_input(&crate::raw_input::RawInputEvent::Paste("dleX".into())));
    pressed(&mut surface, KeyCode::Backspace, KeyModifiers::NONE);

    assert_eq!(bar_text(&surface), " COPY  /needle");
    assert!(input.try_recv().is_err());
}

#[tokio::test]
async fn enter_searches_forward_from_the_cursor_and_wraps_to_the_first_match() {
    let (mut surface, _, dirty) = wide(FOUND);
    assert_eq!(cursor(&mut surface, &dirty), Some((5, 3)));

    searched(&mut surface, "/alpha");

    assert_eq!(cursor(&mut surface, &dirty), Some((0, 0)));
    assert!(bar_text(&surface).contains("n/N repeat 1/2  "));
}

#[tokio::test]
async fn n_repeats_in_the_search_direction_and_shift_n_against_it_both_wrapping() {
    let (mut surface, _, dirty) = wide(b"alpha\r\nalpha\r\nalpha\r\nend");
    searched(&mut surface, "/alpha");
    assert_eq!(cursor(&mut surface, &dirty), Some((0, 0)));

    for (commands, row) in [("n", 1), ("n", 2), ("n", 0), ("N", 2), ("N", 1)] {
        typed(&mut surface, commands);
        assert_eq!(
            cursor(&mut surface, &dirty),
            Some((0, row)),
            "/ then {commands}"
        );
    }

    searched(&mut surface, "?alpha");
    assert_eq!(cursor(&mut surface, &dirty), Some((0, 0)));
    for (commands, row) in [("n", 2), ("N", 0), ("n", 2), ("N", 0)] {
        typed(&mut surface, commands);
        assert_eq!(
            cursor(&mut surface, &dirty),
            Some((0, row)),
            "? then {commands}"
        );
    }
    assert!(bar_text(&surface).contains("n/N repeat 1/3  "));
}

#[tokio::test]
async fn a_search_without_a_match_keeps_the_cursor_and_counts_none() {
    let (mut surface, _, dirty) = wide(FOUND);

    searched(&mut surface, "/zzz");

    assert_eq!(cursor(&mut surface, &dirty), Some((5, 3)));
    typed(&mut surface, "nN");
    assert_eq!(cursor(&mut surface, &dirty), Some((5, 3)));
    assert!(bar_text(&surface).contains("n/N repeat 0/0  "));
}

#[tokio::test]
async fn escape_cancels_the_prompt_then_clears_the_search_then_leaves() {
    let (mut surface, _, dirty) = wide(FOUND);
    searched(&mut surface, "/alpha");

    typed(&mut surface, "/beta");
    pressed(&mut surface, KeyCode::Esc, KeyModifiers::NONE);
    assert!(bar_text(&surface).contains("n/N repeat 1/2  "));
    searched(&mut surface, "/");
    assert!(bar_text(&surface).contains("n/N repeat 1/2  "));
    assert_eq!(cursor(&mut surface, &dirty), Some((0, 0)));

    pressed(&mut surface, KeyCode::Esc, KeyModifiers::NONE);
    assert!(surface.copy_mode());
    assert!(bar_text(&surface).contains("n/N repeat  v/space"));
    assert!(surface.overlays().copy.current.is_empty());
    typed(&mut surface, "n");
    assert_eq!(cursor(&mut surface, &dirty), Some((0, 0)));

    pressed(&mut surface, KeyCode::Esc, KeyModifiers::NONE);
    assert!(!surface.copy_mode());
}

#[tokio::test]
async fn the_bar_lists_search_and_repeat_and_counts_the_current_match() {
    let (mut surface, _, _) = wide(FOUND);
    assert_eq!(
        bar_text(&surface),
        " COPY  h/j/k/l w/b/e { } move  / ? search  n/N repeat  v/space select  y/enter copy  q/esc exit"
    );

    searched(&mut surface, "/alpha");

    assert_eq!(
        bar_text(&surface),
        " COPY  h/j/k/l w/b/e { } move  / ? search  n/N repeat 1/2  v/space select  y/enter copy  esc clear  q exit"
    );
}

#[tokio::test]
async fn matches_in_view_are_highlighted_and_the_current_one_is_marked() {
    let (mut surface, inner, dirty) = copying(FOUND);
    let current = crate::core_client::tui::copy_mode::cursor_cell(&surface.palette);
    let other = crate::core_client::tui::copy_mode::match_cell(&surface.palette);
    searched(&mut surface, "/alpha");

    let rows = frame(&mut surface, &dirty);

    let at = |x: u16, y: u16| cell_at(&rows, inner.x + x, inner.y + y).expect("painted");
    for x in 0..5 {
        assert_eq!(
            (at(x, 0).bg, at(x, 0).modifier),
            (current.bg, current.modifier)
        );
        assert_eq!(at(x, 2).bg, other.bg);
    }
    assert_ne!(at(5, 0).bg, current.bg);
    assert_ne!(at(5, 2).bg, other.bg);

    typed(&mut surface, "n");
    let rows = frame(&mut surface, &dirty);
    let at = |x: u16, y: u16| cell_at(&rows, inner.x + x, inner.y + y).expect("painted");
    assert_eq!(at(1, 0).bg, other.bg);
    assert_eq!(at(1, 2).bg, current.bg);
}

#[tokio::test]
async fn highlights_stay_on_the_drawn_rows_while_the_program_holds_synchronized_output() {
    let (mut surface, _, dirty) = wide(&lines(40));
    searched(&mut surface, "?line-39");
    frame(&mut surface, &dirty);
    let held = surface.overlays().copy.current;
    assert!(!held.is_empty());

    feed(&surface, "t0", b"\x1b[?2026h");
    typed(&mut surface, "g");
    frame(&mut surface, &dirty);
    assert_eq!(surface.overlays().copy.current, held);

    feed(&surface, "t0", b"\x1b[?2026l");
    frame(&mut surface, &dirty);
    assert!(surface.overlays().copy.current.is_empty());
    assert!(bar_text(&surface).contains("n/N repeat 0/0  "));
}

#[tokio::test]
async fn new_output_drops_the_matches_from_the_frame_that_draws_it() {
    let (mut surface, inner, dirty) = wide(FOUND);
    let current = crate::core_client::tui::copy_mode::cursor_cell(&surface.palette);
    searched(&mut surface, "/alpha");
    frame(&mut surface, &dirty);

    feed(&surface, "t0", b"\r\nalpha four");
    assert!(!surface.overlays().copy.current.is_empty());
    let rows = frame(&mut surface, &dirty);

    let cell = cell_at(&rows, inner.x + 1, inner.y).expect("repainted");
    assert_ne!(cell.bg, current.bg);
    assert!(surface.overlays().copy.matches.is_empty());
    assert!(bar_text(&surface).contains("n/N repeat 0/0  "));

    typed(&mut surface, "n");
    assert_eq!(cursor(&mut surface, &dirty), Some((0, 2)));
    assert!(bar_text(&surface).contains("n/N repeat 2/3  "));
}

#[tokio::test]
async fn a_new_size_or_screen_drops_the_matches() {
    for switch in [false, true] {
        let (mut surface, _, dirty) = wide(FOUND);
        searched(&mut surface, "/alpha");
        frame(&mut surface, &dirty);

        if switch {
            feed(&surface, "t0", b"\x1b[?1049h");
        } else {
            surface.resize(host(100, 10));
        }
        frame(&mut surface, &dirty);

        assert!(surface.overlays().copy.current.is_empty(), "{switch}");
        assert!(surface.overlays().copy.matches.is_empty(), "{switch}");
        assert!(bar_text(&surface).contains("repeat 0/0"), "{switch}");
    }
}

#[tokio::test]
async fn y_copies_the_current_match_when_nothing_is_selected() {
    for commands in ["y", "vy"] {
        let (mut surface, _, _) = copying(FOUND);
        searched(&mut surface, "/beta");

        assert_eq!(
            typed(&mut surface, commands).as_deref(),
            Some("beta"),
            "{commands}"
        );
        assert!(!surface.copy_mode());
    }
}

#[tokio::test]
async fn a_selection_wins_over_the_match_when_yanking() {
    let (mut surface, _, _) = copying(FOUND);
    searched(&mut surface, "/beta");

    assert_eq!(typed(&mut surface, "vly").as_deref(), Some("be"));
}

#[tokio::test]
async fn a_match_is_not_copied_once_the_content_it_came_from_changed() {
    let (mut surface, _, _) = copying(FOUND);
    searched(&mut surface, "/beta");

    feed(&surface, "t0", b"\r\nmore");

    assert_eq!(typed(&mut surface, "y"), None);
    assert!(!surface.copy_mode());
}

#[tokio::test]
async fn a_search_jump_keeps_the_match_off_the_last_row() {
    let (mut surface, inner, dirty) = copying(&lines(40));
    typed(&mut surface, "g");

    searched(&mut surface, "/line-20");

    assert_eq!(cursor(&mut surface, &dirty), Some((0, inner.height - 2)));
}

#[tokio::test]
async fn a_parked_copy_mode_keeps_its_highlights_prompt_and_query_and_shows_no_bar() {
    let (mut surface, _, dirty) = wide(FOUND);
    searched(&mut surface, "/alpha");
    typed(&mut surface, "/al");

    surface.act(KeybindAction::FocusPaneRight, true);
    frame(&mut surface, &dirty);
    let parked = surface.overlays().copy;
    assert!(!parked.current.is_empty());
    assert!(parked.bar.is_empty() && parked.cursor.is_empty());
    let text = crate::raw_input::RawInputEvent::Text(crate::input::TextCommit::new("x"));
    assert!(!surface.copy_mode_input(&text));
    assert_eq!(typed(&mut surface, "p"), None);

    surface.act(KeybindAction::FocusPaneLeft, true);
    frame(&mut surface, &dirty);
    assert!(bar_text(&surface).starts_with(" COPY  /al  "));
    pressed(&mut surface, KeyCode::Esc, KeyModifiers::NONE);
    assert!(bar_text(&surface).contains("n/N repeat 1/2  "));
    assert!(!surface.overlays().copy.current.is_empty());
}

#[tokio::test]
async fn a_search_waits_for_the_frame_that_shows_new_output() {
    let (mut surface, _, dirty) = wide(FOUND);
    feed(&surface, "t0", b"\r\nalpha four");

    searched(&mut surface, "/alpha");
    assert!(bar_text(&surface).contains("n/N repeat  v/space"));
    assert!(surface.overlays().copy.current.is_empty());

    frame(&mut surface, &dirty);
    searched(&mut surface, "/alpha");
    assert_eq!(cursor(&mut surface, &dirty), Some((0, 4)));
    assert!(bar_text(&surface).contains("n/N repeat 3/3  "));
}

#[tokio::test]
async fn n_after_new_output_searches_from_the_cursor_and_not_the_old_match() {
    let (mut surface, _, dirty) = wide(b"ab ab");
    searched(&mut surface, "/ab");
    assert_eq!(cursor(&mut surface, &dirty), Some((0, 0)));

    feed(&surface, "t0", b"\x1b[1;1Hxab ab");
    frame(&mut surface, &dirty);
    typed(&mut surface, "n");

    assert_eq!(cursor(&mut surface, &dirty), Some((1, 0)));
}

#[tokio::test]
async fn a_new_pane_width_drops_the_matches_while_the_terminal_keeps_its_size() {
    let mut surface = surface_with_scrollbars(8, 10, false);
    surface.update(row_of(&terminals(2)));
    for (_, id, geometry) in requests(&mut surface) {
        let view = View::test(geometry.cols.max(crate::pane::MIN_PANE_COLS), geometry.rows);
        view.test_feed(b"ab ab\r");
        surface.attached(id, geometry, Ok(view));
    }
    let dirty = HashSet::from([id_of(&surface, "t0")]);
    frame(&mut surface, &dirty);
    surface.enter_copy_mode();
    searched(&mut surface, "/ab");
    frame(&mut surface, &dirty);
    assert!(!surface.overlays().copy.current.is_empty());
    let size = surface.views[&id_of(&surface, "t0")].view.size();

    surface.resize(host(10, 10));
    frame(&mut surface, &dirty);

    assert_eq!(surface.views[&id_of(&surface, "t0")].view.size(), size);
    assert!(surface.overlays().copy.current.is_empty());
}

#[tokio::test]
async fn n_moves_on_from_a_match_that_starts_past_the_columns_a_narrow_pane_shows() {
    let mut surface = surface_with_scrollbars(6, 10, false);
    surface.update(row_of(&terminals(2)));
    for (_, id, geometry) in requests(&mut surface) {
        let view = View::test(geometry.cols.max(crate::pane::MIN_PANE_COLS), geometry.rows);
        view.test_feed(b"xxa\r\nxxa\r");
        surface.attached(id, geometry, Ok(view));
    }
    let dirty = HashSet::new();
    frame(&mut surface, &dirty);
    surface.enter_copy_mode();
    typed(&mut surface, "k");

    searched(&mut surface, "/a");
    assert_eq!(cursor(&mut surface, &dirty), Some((1, 0)));

    typed(&mut surface, "n");
    assert_eq!(cursor(&mut surface, &dirty), Some((1, 1)));
}

#[tokio::test]
async fn prompt_keys_before_the_next_frame_reach_a_prompt_whose_pane_was_hidden() {
    let (mut surface, _, dirty) = wide(FOUND);
    let pane = id_of(&surface, "t0");
    typed(&mut surface, "/al");
    surface.act(KeybindAction::FocusPaneRight, true);
    surface.act(KeybindAction::Zoom, true);
    frame(&mut surface, &HashSet::new());
    assert!(super::super::visible(&surface.views, pane).is_none());

    surface.act(KeybindAction::LastPane, true);
    typed(&mut surface, "pha");

    frame(&mut surface, &dirty);
    assert!(bar_text(&surface).starts_with(" COPY  /alpha  "));
}

#[tokio::test]
async fn the_paste_shortcut_puts_the_clipboard_into_the_prompt_and_only_there() {
    let (mut surface, _, mut input) = selecting(false, FOUND);
    surface.enter_copy_mode();
    while input.try_recv().is_ok() {}
    let paste = TerminalKey::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
    let plain = TerminalKey::new(KeyCode::Char('v'), KeyModifiers::NONE);

    assert!(!surface.paste_shortcut(&paste, || panic!("read outside a prompt")));

    typed(&mut surface, "/");
    assert!(!surface.paste_shortcut(&plain, || panic!("read for a plain key")));
    assert!(surface.paste_shortcut(&paste, || Some("needle".to_owned())));
    assert!(surface.paste_shortcut(&paste, || None));

    assert_eq!(bar_text(&surface), " COPY  /needle");
    assert!(input.try_recv().is_err());
}
