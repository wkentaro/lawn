use ratatui::buffer::{Buffer, Cell};
use ratatui::style::{Modifier, Style};

use super::*;
use crate::core_client::tui::session::Operation;
use crate::core_client::tui::surface::Pointed;
use crate::input::TextCommit;
use crate::protocol::CursorState;
use crate::raw_input::RawInputEvent;
use crate::session_doc::{Arrangement, Destination, Direction};
use crate::ui::text_editor::TextEditor;

const COLS: u16 = 80;
const ROWS: u16 = 24;

fn one_pane() -> Surface {
    let mut surface = surface(COLS, ROWS);
    surface.update(row_of(&terminals(1)), 0);
    frame_attaching(&mut surface, &HashSet::new());
    surface
}

fn inner(surface: &Surface) -> Rect {
    super::super::visible(&surface.views, id_of(surface, "t0"))
        .expect("visible")
        .1
}

fn press(surface: &mut Surface, code: KeyCode) -> Option<Operation> {
    ran(surface.dialog_key(&TerminalKey::new(code, KeyModifiers::NONE)))
}

fn typed(surface: &mut Surface, text: &str) {
    for character in text.chars() {
        assert_eq!(press(surface, KeyCode::Char(character)), None);
    }
}

fn at(surface: &mut Surface, kind: MouseEventKind, column: u16, row: u16) -> Option<Pointed> {
    surface.mouse(mouse_at(kind, column, row), std::time::Instant::now())
}

fn left(surface: &mut Surface, column: u16, row: u16) -> Option<Pointed> {
    at(
        surface,
        MouseEventKind::Down(MouseButton::Left),
        column,
        row,
    )
}

fn dim() -> u16 {
    let mut cell = Cell::default();
    cell.set_style(Style::default().add_modifier(Modifier::DIM));
    CellData::from_ratatui_cell(&cell).modifier
}

fn closes(terminals: &[&str]) -> Option<Operation> {
    Some(Operation::Close(
        terminals
            .iter()
            .map(|terminal| (*terminal).to_owned())
            .collect(),
    ))
}

fn column_of(screen: &Screen, y: u16, text: &str) -> u16 {
    let line = screen.line(y);
    let byte = line.find(text).expect(text);
    line[..byte].chars().count() as u16
}

fn row_with(screen: &Screen, text: &str) -> u16 {
    (0..ROWS)
        .find(|y| screen.line(*y).contains(text))
        .expect(text)
}

struct Help {
    lines: Vec<String>,
    area: Rect,
    close: Rect,
    scrollbar: Rect,
    metrics: crate::pane::ScrollMetrics,
    max_scroll: usize,
    cursor: Option<CursorState>,
}

fn help(scroll: usize, query: &str, searching: bool) -> Help {
    let mut buffer = Buffer::empty(Rect::new(0, 0, COLS, ROWS));
    let drawn = crate::ui::dialog::render_help(
        &mut buffer,
        &TextEditor::from(query),
        searching,
        scroll,
        &keybinds(),
        &Palette::catppuccin(),
    )
    .expect("help");
    let area = drawn.area;
    Help {
        lines: (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect(),
        area,
        close: drawn.close,
        scrollbar: drawn.scrollbar,
        metrics: drawn.scroll_metrics,
        max_scroll: drawn.max_scroll,
        cursor: drawn.cursor,
    }
}

fn panel(screen: &Screen, area: Rect) -> Vec<String> {
    (area.y..area.bottom())
        .map(|y| screen.span(y, area.x..area.right()))
        .collect()
}

fn helping() -> (Surface, Screen) {
    let mut surface = one_pane();
    let mut screen = Screen::of(&mut surface);
    assert_eq!(surface.act(KeybindAction::Help, false), None);
    screen.show(&mut surface, &HashSet::new());
    (surface, screen)
}

#[tokio::test]
async fn a_close_herdr_would_confirm_asks_over_the_dimmed_screen_and_enter_runs_it() {
    let mut surface = one_pane();
    feed(&surface, "t0", b"hello");
    let mut screen = Screen::of(&mut surface);
    let pane = inner(&surface);
    assert_eq!(screen.cell(pane.x, pane.y).modifier & dim(), 0);

    assert_eq!(surface.act(KeybindAction::ClosePane, true), None);
    assert!(surface.dialog_open());
    screen.show(&mut surface, &HashSet::new());

    let title = row_with(&screen, " Close workspace?");
    assert!(screen.line(title + 1).contains(" workspace — 1 pane"));
    assert!(screen.line(title + 3).contains(" ↵ confirm "));
    assert_eq!(screen.span(pane.y, pane.x..pane.x + 5), "hello");
    assert_ne!(screen.cell(pane.x, pane.y).modifier & dim(), 0);
    assert_eq!(
        screen
            .cell(column_of(&screen, title, "Close"), title)
            .modifier
            & dim(),
        0
    );

    assert_eq!(press(&mut surface, KeyCode::Char('y')), None);
    assert!(surface.dialog_open());
    assert_eq!(press(&mut surface, KeyCode::Enter), closes(&["t0"]));
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn escape_answers_no_and_the_screen_comes_back_as_it_was() {
    let mut surface = one_pane();
    let mut screen = Screen::of(&mut surface);
    let before = screen.cells.clone();

    for action in [
        KeybindAction::ClosePane,
        KeybindAction::CloseTab,
        KeybindAction::CloseWorkspace,
    ] {
        assert_eq!(surface.act(action, true), None, "{action:?}");
        screen.show(&mut surface, &HashSet::new());
        assert_ne!(screen.cells, before, "{action:?}");

        assert_eq!(press(&mut surface, KeyCode::Esc), None);
        assert!(!surface.dialog_open());
        screen.show(&mut surface, &HashSet::new());
        assert!(screen.cells == before, "{action:?}");
    }
}

#[tokio::test]
async fn a_left_click_on_confirm_runs_the_close_and_a_left_click_anywhere_else_answers_no() {
    let mut surface = one_pane();
    let mut screen = Screen::of(&mut surface);
    surface.act(KeybindAction::CloseWorkspace, true);
    screen.show(&mut surface, &HashSet::new());
    let title = row_with(&screen, " Close workspace?");
    let buttons = title + 3;
    let confirm = column_of(&screen, buttons, "↵ confirm");
    let cancel = column_of(&screen, buttons, "esc cancel");

    for kind in [
        MouseEventKind::Moved,
        MouseEventKind::Down(MouseButton::Right),
        MouseEventKind::Down(MouseButton::Middle),
        MouseEventKind::Up(MouseButton::Left),
        MouseEventKind::Drag(MouseButton::Left),
        MouseEventKind::ScrollDown,
    ] {
        assert_eq!(at(&mut surface, kind, confirm, buttons), None, "{kind:?}");
        assert!(surface.dialog_open(), "{kind:?}");
    }

    for (column, row) in [(cancel, buttons), (confirm, title), (0, 0)] {
        assert_eq!(left(&mut surface, column, row), None, "{column},{row}");
        assert!(!surface.dialog_open(), "{column},{row}");
        surface.act(KeybindAction::CloseWorkspace, true);
        screen.show(&mut surface, &HashSet::new());
    }

    assert_eq!(
        left(&mut surface, confirm, buttons),
        closes(&["t0"]).map(Pointed::Run)
    );
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn the_help_key_lists_the_live_keybinds_and_escape_enter_or_a_question_mark_closes_it() {
    let mut surface = one_pane();
    surface.keybinds.keybinds.detach = crate::config::ActionKeybinds::direct("ctrl+q");
    let mut screen = Screen::of(&mut surface);

    for close in [KeyCode::Esc, KeyCode::Enter, KeyCode::Char('?')] {
        assert_eq!(surface.act(KeybindAction::Help, false), None);
        screen.show(&mut surface, &HashSet::new());
        let row = row_with(&screen, "detach");
        assert!(
            screen.line(row).contains("ctrl+q"),
            "{:?}",
            screen.line(row)
        );
        assert!(screen.line(1).contains("┌"));

        assert_eq!(press(&mut surface, close), None);
        assert!(!surface.dialog_open(), "{close:?}");
        screen.show(&mut surface, &HashSet::new());
    }
}

#[tokio::test]
async fn the_help_list_scrolls_by_line_and_page_and_to_either_end_and_stops_there() {
    let (mut surface, mut screen) = helping();
    let area = help(0, "", false).area;
    let max = help(0, "", false).max_scroll;
    assert!(max > 8);
    assert_eq!(panel(&screen, area), help(0, "", false).lines);

    for (code, scroll) in [
        (KeyCode::Char('j'), 1),
        (KeyCode::Down, 2),
        (KeyCode::Char('k'), 1),
        (KeyCode::Up, 0),
        (KeyCode::PageDown, 8),
        (KeyCode::PageUp, 0),
        (KeyCode::End, max),
        (KeyCode::Home, 0),
    ] {
        press(&mut surface, code);
        screen.show(&mut surface, &HashSet::new());
        assert_eq!(
            panel(&screen, area),
            help(scroll, "", false).lines,
            "{code:?}"
        );
    }

    press(&mut surface, KeyCode::Up);
    assert!(!surface.needs_frame());
    press(&mut surface, KeyCode::End);
    screen.show(&mut surface, &HashSet::new());
    for code in [KeyCode::Down, KeyCode::Char('j'), KeyCode::PageDown] {
        press(&mut surface, code);
        assert!(!surface.needs_frame(), "{code:?}");
    }
}

#[tokio::test]
async fn a_slash_searches_the_list_as_typed_escape_clears_the_search_and_enter_closes() {
    let (mut surface, mut screen) = helping();
    let area = help(0, "", false).area;

    typed(&mut surface, "/");
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(panel(&screen, area), help(0, "", true).lines);
    let next = TerminalKey::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
    assert_eq!(surface.dialog_key(&next), None);
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(panel(&screen, area), help(1, "", true).lines);

    typed(&mut surface, "zoomj");
    press(&mut surface, KeyCode::Backspace);
    screen.show(&mut surface, &HashSet::new());
    let searched = help(0, "zoom", true);
    assert_eq!(panel(&screen, area), searched.lines);
    assert!(searched.cursor.is_some());
    assert_eq!(surface.cursor(), searched.cursor);

    press(&mut surface, KeyCode::Esc);
    screen.show(&mut surface, &HashSet::new());
    assert!(surface.dialog_open());
    assert_eq!(panel(&screen, area), help(0, "", false).lines);
    assert_eq!(surface.cursor(), None);

    typed(&mut surface, "/z");
    assert_eq!(press(&mut surface, KeyCode::Enter), None);
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn text_and_pastes_reach_the_help_search_only_while_it_is_focused() {
    let (mut surface, mut screen) = helping();
    let area = help(0, "", false).area;
    let paste = TerminalKey::new(KeyCode::Char('v'), KeyModifiers::CONTROL);

    assert!(surface.dialog_input(&RawInputEvent::Text(TextCommit::new("zo"))));
    assert!(!surface.paste_shortcut(&paste, || unreachable!("no paste target")));
    assert!(!surface.needs_frame());

    typed(&mut surface, "/");
    assert!(surface.dialog_input(&RawInputEvent::Text(TextCommit::new("zo"))));
    assert!(surface.paste_shortcut(&paste, || Some("o".to_owned())));
    assert!(surface.dialog_input(&RawInputEvent::Paste("m".to_owned())));
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(panel(&screen, area), help(0, "zoom", true).lines);
}

#[tokio::test]
async fn the_help_wheel_scrolls_three_rows_and_its_scrollbar_jumps_on_a_press_and_follows_a_drag() {
    let (mut surface, mut screen) = helping();
    let top = help(0, "", false);
    let (area, track, metrics) = (top.area, top.scrollbar, top.metrics);
    let middle = area.x + area.width / 2;

    at(&mut surface, MouseEventKind::ScrollDown, middle, 10);
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(panel(&screen, area), help(3, "", false).lines);
    at(&mut surface, MouseEventKind::ScrollUp, 0, 0);
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(panel(&screen, area), top.lines);
    at(&mut surface, MouseEventKind::ScrollUp, middle, 10);
    assert!(!surface.needs_frame());

    let thumb = crate::ui::scrollbar_thumb(metrics, track).expect("thumb");
    let below = thumb.top + thumb.len + 1;
    left(&mut surface, track.x, below);
    screen.show(&mut surface, &HashSet::new());
    let jumped = metrics.max_offset_from_bottom
        - crate::ui::scrollbar_offset_from_row(metrics, track, below);
    assert!(jumped > 0);
    assert_eq!(panel(&screen, area), help(jumped, "", false).lines);

    press(&mut surface, KeyCode::Home);
    screen.show(&mut surface, &HashSet::new());
    left(&mut surface, track.x, thumb.top);
    assert!(!surface.needs_frame());
    at(
        &mut surface,
        MouseEventKind::Drag(MouseButton::Left),
        track.x,
        thumb.top + 3,
    );
    screen.show(&mut surface, &HashSet::new());
    let dragged = metrics.max_offset_from_bottom
        - crate::ui::scrollbar_offset_from_drag_row(metrics, track, thumb.top + 3, 0);
    assert!(dragged > 0);
    assert_eq!(panel(&screen, area), help(dragged, "", false).lines);

    at(
        &mut surface,
        MouseEventKind::Up(MouseButton::Left),
        track.x,
        thumb.top + 3,
    );
    at(
        &mut surface,
        MouseEventKind::Drag(MouseButton::Left),
        track.x,
        track.y,
    );
    assert!(!surface.needs_frame());
    assert!(surface.dialog_open());
}

#[tokio::test]
async fn the_help_close_button_leaves_the_search_first_then_closes_and_a_click_outside_closes() {
    let (mut surface, mut screen) = helping();
    let top = help(0, "", false);
    let (area, close) = (top.area, top.close);

    typed(&mut surface, "/zoom");
    assert_eq!(left(&mut surface, close.x, close.y), None);
    screen.show(&mut surface, &HashSet::new());
    assert!(surface.dialog_open());
    assert_eq!(panel(&screen, area), top.lines);

    left(&mut surface, close.right() - 1, close.y);
    assert!(!surface.dialog_open());

    surface.act(KeybindAction::Help, false);
    screen.show(&mut surface, &HashSet::new());
    left(&mut surface, area.x + 2, area.y + 5);
    left(&mut surface, area.x, area.y);
    assert!(surface.dialog_open());
    left(&mut surface, area.x.saturating_sub(1), area.y);
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn while_a_dialog_is_open_no_mouse_event_reaches_a_pane_and_the_cursor_is_the_dialogs() {
    let mut surface = one_pane();
    surface.forget(id_of(&surface, "t0"));
    let mut inputs = attach_with_input(
        &mut surface,
        b"\x1b[?1000h\x1b[?1003h\x1b[?1006h\x1b[1;3H\x1b[?25h",
    );
    frame(&mut surface, &HashSet::new());
    let focused = surface.focused_id();
    assert!(surface.cursor().is_some());
    surface.act(KeybindAction::Help, false);
    frame(&mut surface, &HashSet::new());
    inputs.iter_mut().for_each(|input| {
        reports(input);
    });
    let area = help(0, "", false).area;
    let (column, row) = (area.x + 2, area.y + 5);

    for kind in [
        MouseEventKind::Moved,
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Drag(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
        MouseEventKind::Down(MouseButton::Right),
        MouseEventKind::Up(MouseButton::Right),
        MouseEventKind::ScrollDown,
        MouseEventKind::ScrollUp,
    ] {
        assert_eq!(at(&mut surface, kind, column, row), None, "{kind:?}");
    }
    frame(&mut surface, &HashSet::new());

    for input in &mut inputs {
        assert_eq!(reports(input), b"");
    }
    assert_eq!(surface.focused_id(), focused);
    assert!(surface.gesture.is_none());
    assert!(surface.dialog_open());
    assert_eq!(surface.cursor(), None);

    typed(&mut surface, "/");
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.cursor(), help(0, "", true).cursor);
}

#[tokio::test]
async fn nothing_is_copyable_while_a_dialog_is_open_and_the_selection_is_again_once_it_closes() {
    let (mut surface, inner, _input) = selecting(false, b"hello world");
    let now = std::time::Instant::now();
    surface.mouse(click(inner.x, inner.y), now);
    surface.mouse(drag_to(inner.x + 4, inner.y), now);
    surface.mouse(release_at(inner.x + 4, inner.y), now);
    assert!(surface.copyable());

    surface.act(KeybindAction::Help, false);
    assert!(!surface.copyable());

    press(&mut surface, KeyCode::Esc);
    assert!(surface.copyable());
}

#[tokio::test]
async fn opening_a_dialog_releases_a_mouse_button_held_in_a_pane() {
    let mut surface = one_pane();
    surface.forget(id_of(&surface, "t0"));
    let mut input = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1006h").remove(0);
    frame(&mut surface, &HashSet::new());
    let pane = inner(&surface);
    left(&mut surface, pane.x + 1, pane.y + 1);
    assert_eq!(reports(&mut input), b"\x1b[<0;2;2M");

    surface.act(KeybindAction::Help, false);
    assert_eq!(reports(&mut input), b"\x1b[<0;2;2m");
    at(
        &mut surface,
        MouseEventKind::Up(MouseButton::Left),
        pane.x + 1,
        pane.y + 1,
    );
    assert_eq!(reports(&mut input), b"");
}

#[tokio::test]
async fn pane_output_under_an_open_dialog_stays_under_it_and_shows_once_it_closes() {
    let mut surface = one_pane();
    let mut screen = Screen::of(&mut surface);
    let pane = inner(&surface);
    surface.act(KeybindAction::CloseWorkspace, true);
    screen.show(&mut surface, &HashSet::new());

    feed(&surface, "t0", b"hello");
    let dirty = HashSet::from([id_of(&surface, "t0")]);
    assert!(frame(&mut surface, &dirty).is_empty());

    press(&mut surface, KeyCode::Esc);
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(screen.span(pane.y, pane.x..pane.x + 5), "hello");
    assert_eq!(screen.cell(pane.x, pane.y).modifier & dim(), 0);
}

#[tokio::test]
async fn a_dialog_is_redrawn_only_when_its_state_or_the_host_size_changes() {
    let (mut surface, _) = helping();
    let image = |surface: &Surface| {
        surface
            .dialog
            .as_ref()
            .and_then(|dialog| dialog.image())
            .map(|(area, cells)| (*area, cells.as_ptr()))
    };
    let cells = |surface: &Surface| {
        surface
            .dialog
            .as_ref()
            .and_then(|dialog| dialog.image())
            .map(|(_, cells)| cells.clone())
    };
    let drawn = image(&surface);
    let unscrolled = cells(&surface);

    assert!(frame(&mut surface, &HashSet::new()).is_empty());
    press(&mut surface, KeyCode::Home);
    at(&mut surface, MouseEventKind::Moved, 3, 3);
    assert!(!surface.needs_frame());
    surface.update(row_of(&terminals(2)), 0);
    frame_attaching(&mut surface, &HashSet::new());
    assert_eq!(image(&surface), drawn);

    press(&mut surface, KeyCode::Char('j'));
    assert!(surface.needs_frame());
    frame(&mut surface, &HashSet::new());
    assert_ne!(cells(&surface), unscrolled);

    surface.resize(host(100, 30));
    let screen = Screen::of(&mut surface);
    assert_eq!(
        image(&surface).map(|(area, _)| area),
        Some(Rect::new(0, 0, 100, 30))
    );
    assert_eq!(screen.span(4, 12..14), "┌─", "{:?}", screen.line(4));
}

#[tokio::test]
async fn a_dialog_opened_in_copy_mode_dims_the_copy_bar_under_it_and_copy_mode_resumes_once_it_closes(
) {
    let (mut surface, _, _input) = selecting(false, b"hello");
    surface.resize(host(COLS, ROWS));
    surface.enter_copy_mode();
    let mut screen = Screen::of(&mut surface);
    let bar = row_with(&screen, " COPY ");
    let before = screen.cells.clone();
    assert_eq!(screen.cell(1, bar).modifier & dim(), 0);

    surface.act(KeybindAction::Help, false);
    screen.show(&mut surface, &HashSet::new());
    assert!(screen.line(bar).contains(" COPY "));
    assert_ne!(screen.cell(1, bar).modifier & dim(), 0);
    press(&mut surface, KeyCode::Char('j'));
    press(&mut surface, KeyCode::Esc);

    assert!(surface.copy_mode());
    screen.show(&mut surface, &HashSet::new());
    assert!(screen.cells == before);
}

fn naming(arrangement: crate::session_doc::Arrangement) -> Option<Operation> {
    Some(Operation::Arrange(arrangement))
}

fn workspace_named(name: &str) -> Option<Operation> {
    naming(crate::session_doc::Arrangement::NameWorkspace {
        workspace: "w0".to_owned(),
        name: name.to_owned(),
    })
}

fn tab_named(name: &str) -> Option<Operation> {
    naming(crate::session_doc::Arrangement::NameTab {
        tab: "t0".to_owned(),
        name: name.to_owned(),
    })
}

fn clear_field(surface: &mut Surface) {
    let clear = TerminalKey::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(surface.dialog_key(&clear), None);
}

#[tokio::test]
async fn renaming_a_workspace_starts_from_its_label_and_enter_saves_the_trimmed_name() {
    let mut surface = one_pane();
    let mut screen = Screen::of(&mut surface);

    assert_eq!(surface.act(KeybindAction::RenameWorkspace, false), None);
    screen.show(&mut surface, &HashSet::new());
    let title = row_with(&screen, "rename workspace");
    assert!(screen.line(title + 2).contains(" workspace "));
    assert!(screen.line(title + 3).contains(" ↵ save "));

    typed(&mut surface, " 2 ");
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        workspace_named("workspace 2")
    );
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn a_blank_name_or_an_unnamed_tabs_own_label_saves_nothing() {
    let mut surface = one_pane();

    surface.act(KeybindAction::RenameTab, false);
    assert_eq!(press(&mut surface, KeyCode::Enter), None);
    assert!(!surface.dialog_open());

    surface.act(KeybindAction::RenameTab, false);
    clear_field(&mut surface);
    typed(&mut surface, " ");
    assert_eq!(press(&mut surface, KeyCode::Enter), None);

    surface.act(KeybindAction::RenameWorkspace, false);
    clear_field(&mut surface);
    assert_eq!(press(&mut surface, KeyCode::Enter), None);

    surface.act(KeybindAction::RenameTab, false);
    typed(&mut surface, "2");
    assert_eq!(press(&mut surface, KeyCode::Enter), tab_named("12"));
}

#[tokio::test]
async fn the_clear_button_empties_the_field_save_saves_and_a_click_anywhere_else_cancels() {
    let mut surface = one_pane();
    let mut screen = Screen::of(&mut surface);
    surface.act(KeybindAction::RenameTab, false);
    screen.show(&mut surface, &HashSet::new());
    let title = row_with(&screen, "rename tab");
    let buttons = title + 3;
    let clear = column_of(&screen, buttons, "^c clear");
    let save = column_of(&screen, buttons, "↵ save");

    assert_eq!(left(&mut surface, clear, buttons), None);
    assert!(surface.dialog_open());
    screen.show(&mut surface, &HashSet::new());
    assert!(!screen.line(title + 2).contains('1'));
    typed(&mut surface, "logs");
    assert_eq!(
        left(&mut surface, save, buttons),
        tab_named("logs").map(Pointed::Run)
    );
    assert!(!surface.dialog_open());

    surface.act(KeybindAction::RenameTab, false);
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(left(&mut surface, save, title), None);
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn text_and_pastes_go_into_the_rename_field() {
    let mut surface = one_pane();
    let paste = TerminalKey::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
    surface.act(KeybindAction::RenameWorkspace, false);

    assert!(surface.paste_shortcut(&paste, || Some(" main".to_owned())));
    assert!(surface.dialog_input(&RawInputEvent::Paste("\nsite".to_owned())));
    assert!(surface.dialog_input(&RawInputEvent::Text(TextCommit::new("!"))));
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        workspace_named("workspace main site!")
    );
}

#[tokio::test]
async fn a_drag_held_in_a_pane_when_a_dialog_opens_by_key_stops_reaching_the_pane() {
    let mut surface = one_pane();
    surface.forget(id_of(&surface, "t0"));
    let mut input = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1002h\x1b[?1006h").remove(0);
    frame(&mut surface, &HashSet::new());
    let pane = inner(&surface);
    left(&mut surface, pane.x + 1, pane.y + 1);
    at(
        &mut surface,
        MouseEventKind::Drag(MouseButton::Left),
        pane.x + 2,
        pane.y + 1,
    );
    reports(&mut input);

    surface.act(KeybindAction::Help, false);
    frame(&mut surface, &HashSet::new());
    assert_eq!(reports(&mut input), b"\x1b[<0;3;2m");
    at(
        &mut surface,
        MouseEventKind::Drag(MouseButton::Left),
        pane.x + 3,
        pane.y + 2,
    );
    at(
        &mut surface,
        MouseEventKind::Up(MouseButton::Left),
        pane.x + 3,
        pane.y + 2,
    );

    assert_eq!(reports(&mut input), b"");
}

#[tokio::test]
async fn a_click_on_a_dialog_closed_by_key_but_still_drawn_does_not_reach_the_pane_under_it() {
    let mut surface = one_pane();
    surface.forget(id_of(&surface, "t0"));
    let mut input = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1006h").remove(0);
    frame(&mut surface, &HashSet::new());
    surface.act(KeybindAction::Help, false);
    frame(&mut surface, &HashSet::new());
    reports(&mut input);
    let area = help(0, "", false).area;

    press(&mut surface, KeyCode::Esc);
    assert!(!surface.dialog_open());
    left(&mut surface, area.x + 2, area.y + 5);
    at(
        &mut surface,
        MouseEventKind::Up(MouseButton::Left),
        area.x + 2,
        area.y + 5,
    );
    at(
        &mut surface,
        MouseEventKind::ScrollDown,
        area.x + 2,
        area.y + 5,
    );

    assert_eq!(reports(&mut input), b"");
}

#[tokio::test]
async fn a_dialog_opened_and_closed_before_a_frame_releases_a_held_pane_button_once_and_the_next_click_works(
) {
    let mut surface = one_pane();
    surface.forget(id_of(&surface, "t0"));
    let mut input = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1006h").remove(0);
    frame(&mut surface, &HashSet::new());
    let pane = inner(&surface);
    left(&mut surface, pane.x + 1, pane.y + 1);

    surface.act(KeybindAction::Help, false);
    at(
        &mut surface,
        MouseEventKind::Up(MouseButton::Left),
        pane.x + 1,
        pane.y + 1,
    );
    press(&mut surface, KeyCode::Enter);
    assert!(!surface.dialog_open());
    frame(&mut surface, &HashSet::new());
    left(&mut surface, pane.x + 2, pane.y + 1);

    assert_eq!(reports(&mut input), b"\x1b[<0;2;2M\x1b[<0;2;2m\x1b[<0;3;2M");
}

fn pane_named(name: &str) -> Option<Operation> {
    Some(Operation::Name {
        terminal: "t0".to_owned(),
        name: name.to_owned(),
    })
}

fn heard_name(surface: &mut Surface, terminal: &str, name: Option<&str>) {
    surface.heard(
        Report::Name(terminal.to_owned(), name.map(str::to_owned)),
        std::time::Instant::now(),
    );
}

#[tokio::test]
async fn renaming_an_unnamed_pane_starts_empty_and_enter_saves_the_trimmed_name() {
    let mut surface = one_pane();
    let mut screen = Screen::of(&mut surface);

    assert_eq!(surface.act(KeybindAction::RenamePane, false), None);
    screen.show(&mut surface, &HashSet::new());
    let title = row_with(&screen, "rename pane");
    assert_eq!(
        screen.line(title + 2).trim_matches([' ', '│']),
        "",
        "{:?}",
        screen.line(title + 2)
    );

    typed(&mut surface, " build ");
    assert_eq!(press(&mut surface, KeyCode::Enter), pane_named("build"));
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn renaming_a_named_pane_starts_from_its_name_and_saving_it_blank_clears_it() {
    let mut surface = one_pane();
    heard_name(&mut surface, "t0", Some("build"));
    let mut screen = Screen::of(&mut surface);

    surface.act(KeybindAction::RenamePane, false);
    screen.show(&mut surface, &HashSet::new());
    let title = row_with(&screen, "rename pane");
    assert!(screen.line(title + 2).contains(" build "));
    typed(&mut surface, "s");
    assert_eq!(press(&mut surface, KeyCode::Enter), pane_named("builds"));

    surface.act(KeybindAction::RenamePane, false);
    clear_field(&mut surface);
    typed(&mut surface, " ");
    assert_eq!(press(&mut surface, KeyCode::Enter), pane_named(""));
}

#[tokio::test]
async fn escape_closes_the_pane_rename_without_saving() {
    let mut surface = one_pane();
    surface.act(KeybindAction::RenamePane, false);
    typed(&mut surface, "build");

    assert_eq!(press(&mut surface, KeyCode::Esc), None);
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn a_pane_name_saved_as_it_was_opened_sends_nothing_so_a_stale_field_cannot_undo_a_rename() {
    let mut surface = one_pane();
    surface.act(KeybindAction::RenamePane, false);
    assert_eq!(press(&mut surface, KeyCode::Enter), None);

    heard_name(&mut surface, "t0", Some("build"));
    surface.act(KeybindAction::RenamePane, false);
    typed(&mut surface, " ");
    assert_eq!(press(&mut surface, KeyCode::Enter), None);
}

#[tokio::test]
async fn a_pane_name_with_outer_spaces_saved_untouched_does_not_undo_a_newer_name() {
    let mut surface = one_pane();
    heard_name(&mut surface, "t0", Some("build "));
    surface.act(KeybindAction::RenamePane, false);
    heard_name(&mut surface, "t0", Some("test"));

    assert_eq!(press(&mut surface, KeyCode::Enter), None);
}

#[tokio::test]
async fn a_pane_name_the_field_shows_differently_saved_untouched_does_not_undo_a_newer_name() {
    let mut surface = one_pane();
    heard_name(&mut surface, "t0", Some("build\tlogs"));
    surface.act(KeybindAction::RenamePane, false);
    heard_name(&mut surface, "t0", Some("test"));

    assert_eq!(press(&mut surface, KeyCode::Enter), None);
}

#[tokio::test]
async fn a_pane_name_the_field_cannot_show_at_all_is_removed_by_saving_the_field_edited_to_empty() {
    let mut surface = one_pane();
    heard_name(&mut surface, "t0", Some("\u{7f}"));
    surface.act(KeybindAction::RenamePane, false);
    typed(&mut surface, " ");

    assert_eq!(press(&mut surface, KeyCode::Enter), pane_named(""));
}

#[tokio::test]
async fn an_untouched_empty_field_saved_after_a_newer_name_arrived_sends_nothing_and_keeps_it() {
    let mut surface = one_pane();
    heard_name(&mut surface, "t0", Some("\u{7f}"));
    surface.act(KeybindAction::RenamePane, false);
    heard_name(&mut surface, "t0", Some("build"));

    assert_eq!(press(&mut surface, KeyCode::Enter), None);
    assert_eq!(surface.session.name("t0"), Some("build"));
}

#[tokio::test]
async fn an_unnamed_pane_field_typed_in_and_emptied_again_sends_nothing_after_a_newer_name_arrived()
{
    let mut surface = one_pane();
    surface.act(KeybindAction::RenamePane, false);
    typed(&mut surface, "x");
    press(&mut surface, KeyCode::Backspace);
    heard_name(&mut surface, "t0", Some("build"));

    assert_eq!(press(&mut surface, KeyCode::Enter), None);
    assert_eq!(surface.session.name("t0"), Some("build"));
}

#[tokio::test]
async fn a_named_pane_field_edited_back_to_its_name_sends_nothing_after_a_newer_name_arrived() {
    let mut surface = one_pane();
    heard_name(&mut surface, "t0", Some("dev"));
    surface.act(KeybindAction::RenamePane, false);
    typed(&mut surface, "s");
    press(&mut surface, KeyCode::Backspace);
    heard_name(&mut surface, "t0", Some("build"));

    assert_eq!(press(&mut surface, KeyCode::Enter), None);
    assert_eq!(surface.session.name("t0"), Some("build"));
}

#[tokio::test]
async fn clearing_a_pane_name_of_only_spaces_removes_it_and_leaving_it_untouched_sends_nothing() {
    let mut surface = one_pane();
    heard_name(&mut surface, "t0", Some("  "));
    surface.act(KeybindAction::RenamePane, false);
    assert_eq!(press(&mut surface, KeyCode::Enter), None);

    surface.act(KeybindAction::RenamePane, false);
    clear_field(&mut surface);
    assert_eq!(press(&mut surface, KeyCode::Enter), pane_named(""));
}

#[tokio::test]
async fn a_named_workspace_or_tab_saved_unchanged_sends_nothing_and_an_unnamed_workspace_keeps_its_label(
) {
    let mut surface = one_pane();
    surface.act(KeybindAction::RenameWorkspace, false);
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        workspace_named("workspace")
    );

    surface.update(
        serde_json::from_value(json!({"workspaces": [{"id": "w0", "name": "main", "tabs": [
            {"id": "t0", "name": "logs", "root": {"kind": "pane", "terminal": "t0"}},
        ]}]}))
        .expect("layout"),
        0,
    );
    surface.act(KeybindAction::RenameWorkspace, false);
    assert_eq!(press(&mut surface, KeyCode::Enter), None);
    surface.act(KeybindAction::RenameTab, false);
    assert_eq!(press(&mut surface, KeyCode::Enter), None);
}

#[tokio::test]
async fn a_pane_rename_whose_pane_left_the_session_while_open_sends_nothing() {
    let mut surface = one_pane();
    surface.act(KeybindAction::RenamePane, false);
    typed(&mut surface, "build");

    surface.update(row_of(&["t1".to_owned()]), 0);

    assert!(surface.dialog_open());
    assert_eq!(press(&mut surface, KeyCode::Enter), None);
}

#[tokio::test]
async fn a_pane_field_edited_by_pasted_text_or_the_clear_button_is_saved() {
    let mut surface = one_pane();
    surface.act(KeybindAction::RenamePane, false);
    assert!(surface.dialog_input(&RawInputEvent::Paste("build".to_owned())));
    assert_eq!(press(&mut surface, KeyCode::Enter), pane_named("build"));

    heard_name(&mut surface, "t0", Some("build"));
    let mut screen = Screen::of(&mut surface);
    surface.act(KeybindAction::RenamePane, false);
    screen.show(&mut surface, &HashSet::new());
    let buttons = row_with(&screen, "rename pane") + 3;
    let clear = column_of(&screen, buttons, "^c clear");
    assert_eq!(left(&mut surface, clear, buttons), None);
    assert_eq!(press(&mut surface, KeyCode::Enter), pane_named(""));
}

#[tokio::test]
async fn a_dialog_stays_as_drawn_while_the_copy_cursor_moves_behind_it() {
    let (mut surface, _, _input) = selecting(false, b"hello");
    surface.resize(host(COLS, ROWS));
    surface.enter_copy_mode();
    surface.act(KeybindAction::Help, false);
    frame(&mut surface, &HashSet::new());
    let image = |surface: &Surface| {
        surface
            .dialog
            .as_ref()
            .and_then(|dialog| dialog.image())
            .map(|(_, cells)| cells.clone())
    };
    let drawn = image(&surface);
    let cursor = surface.painted.copy.cursor;

    feed(&surface, "t0", &b"line\r\n".repeat(60));
    let dirty = HashSet::from([id_of(&surface, "t0")]);
    frame(&mut surface, &dirty);

    assert_ne!(surface.painted.copy.cursor, cursor);
    assert!(image(&surface) == drawn);
}

fn spread() -> Layout {
    serde_json::from_value(json!({
        "next_id": 5,
        "workspaces": [
            {"id": "w0", "name": "main", "tabs": [
                {"id": "tab0", "root": balanced(&terminals(2))},
                {"id": "tab1", "name": "logs", "root": {"kind": "pane", "terminal": "t2"}},
            ]},
            {"id": "w1", "name": "side", "tabs": [
                {"id": "tab2", "name": "build", "root": balanced(&["t3".to_owned(), "t4".to_owned()])},
            ]},
        ],
    }))
    .expect("layout")
}

fn spread_surface() -> Surface {
    let mut surface = surface(COLS, ROWS);
    surface.update(spread(), 0);
    surface
}

fn moved(terminal: &str, to: Destination) -> Option<Operation> {
    Some(Operation::Arrange(Arrangement::MovePane {
        terminal: terminal.to_owned(),
        to,
        tab: None,
    }))
}

fn into(terminal: &str, tab: &str, pane: &str) -> Option<Operation> {
    Some(Operation::Arrange(Arrangement::MovePane {
        terminal: terminal.to_owned(),
        to: beside(pane),
        tab: Some(tab.to_owned()),
    }))
}

fn new_tab(workspace: &str) -> Destination {
    Destination::Tab {
        workspace: Some(workspace.to_owned()),
        name: None,
    }
}

fn beside(terminal: &str) -> Destination {
    Destination::Split {
        beside: terminal.to_owned(),
        direction: Direction::Right,
        ratio: 0.5,
    }
}

fn down(surface: &mut Surface, times: usize) {
    for _ in 0..times {
        assert_eq!(press(surface, KeyCode::Down), None);
    }
}

fn picking(surface: &mut Surface) -> Screen {
    let mut screen = Screen::of(surface);
    assert_eq!(surface.act(KeybindAction::MovePane, false), None);
    assert!(surface.dialog_open());
    screen.show(surface, &HashSet::new());
    screen
}

#[tokio::test]
async fn the_move_key_lists_each_workspace_with_its_other_tabs_then_a_new_workspace_under_the_panes_title(
) {
    let mut surface = spread_surface();
    let screen = picking(&mut surface);

    let title = row_with(&screen, " move terminal · 1 ");
    let rows: Vec<u16> = ["main", "└─ logs", "side", "└─ build", "+ New workspace"]
        .into_iter()
        .map(|label| row_with(&screen, label))
        .collect();
    assert!(title < rows[0], "{rows:?}");
    assert!(
        rows.windows(2).all(|pair| pair[0] + 1 == pair[1]),
        "{rows:?}"
    );
    assert!(!screen
        .lines(1..ROWS)
        .iter()
        .any(|line| line.contains("─ 1")));
}

#[tokio::test]
async fn enter_moves_the_pane_to_a_new_tab_into_a_tab_beside_its_focused_pane_or_to_a_new_workspace(
) {
    let rows = [
        (0, moved("t0", new_tab("w0"))),
        (1, into("t0", "tab1", "t2")),
        (2, moved("t0", new_tab("w1"))),
        (3, into("t0", "tab2", "t3")),
        (4, moved("t0", Destination::Workspace { name: None })),
        (6, moved("t0", Destination::Workspace { name: None })),
    ];
    for (steps, expected) in rows {
        let mut surface = spread_surface();
        picking(&mut surface);
        down(&mut surface, steps);
        assert_eq!(press(&mut surface, KeyCode::Enter), expected, "{steps}");
        assert!(!surface.dialog_open());
    }
}

#[tokio::test]
async fn the_picker_steps_with_the_arrows_control_n_and_p_and_pages_and_stops_at_either_end() {
    let mut surface = spread_surface();
    picking(&mut surface);
    let control = |character| TerminalKey::new(KeyCode::Char(character), KeyModifiers::CONTROL);
    assert_eq!(press(&mut surface, KeyCode::Up), None);
    assert_eq!(surface.dialog_key(&control('n')), None);
    assert_eq!(surface.dialog_key(&control('n')), None);
    assert_eq!(surface.dialog_key(&control('p')), None);
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        into("t0", "tab1", "t2")
    );

    picking(&mut surface);
    assert_eq!(press(&mut surface, KeyCode::PageDown), None);
    assert_eq!(press(&mut surface, KeyCode::PageUp), None);
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        moved("t0", new_tab("w0"))
    );
}

#[tokio::test]
async fn a_tab_row_splits_beside_the_pane_that_tab_last_focused() {
    let mut surface = spread_surface();
    Screen::of(&mut surface);
    surface.act(KeybindAction::SwitchWorkspace(1), false);
    surface.act(KeybindAction::FocusPaneRight, false);
    assert_eq!(surface.session.focused(), Some("t4"));
    surface.act(KeybindAction::SwitchWorkspace(0), false);

    picking(&mut surface);
    down(&mut surface, 3);
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        into("t0", "tab2", "t4")
    );
}

#[tokio::test]
async fn a_named_pane_titles_the_picker_and_names_its_new_workspace() {
    let mut surface = spread_surface();
    heard_name(&mut surface, "t0", Some("build"));
    let screen = picking(&mut surface);
    row_with(&screen, " move build · 1 ");

    down(&mut surface, 4);
    let name = Some("build".to_owned());
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        moved("t0", Destination::Workspace { name })
    );
}

#[tokio::test]
async fn escape_or_the_move_key_closes_the_picker_and_another_prefixed_key_keeps_it() {
    let mut surface = spread_surface();
    let prefix = TerminalKey::new(surface.keybinds.prefix[0].0, surface.keybinds.prefix[0].1);
    picking(&mut surface);
    assert_eq!(press(&mut surface, KeyCode::Esc), None);
    assert!(!surface.dialog_open());

    picking(&mut surface);
    assert_eq!(surface.dialog_key(&prefix), None);
    assert_eq!(press(&mut surface, KeyCode::Char('x')), None);
    assert!(surface.dialog_open());
    assert_eq!(surface.dialog_key(&prefix), None);
    assert_eq!(press(&mut surface, KeyCode::Char('m')), None);
    assert!(!surface.dialog_open());

    picking(&mut surface);
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        moved("t0", new_tab("w0"))
    );
}

#[tokio::test]
async fn a_search_matches_only_the_workspace_and_tab_names_not_the_separator_drawn_between_them() {
    let mut surface = spread_surface();
    let mut screen = picking(&mut surface);
    typed(&mut surface, "/");
    screen.show(&mut surface, &HashSet::new());
    assert!(!screen
        .lines(1..ROWS)
        .iter()
        .any(|line| line.contains("logs") || line.contains("build")));
    assert_ne!(
        press(&mut surface, KeyCode::Enter),
        into("t0", "tab1", "t2")
    );

    let mut layout = spread();
    layout.workspaces[0].tabs[1].name = Some("logs/err".to_owned());
    surface.update(layout, 0);
    picking(&mut surface);
    typed(&mut surface, "/");
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        into("t0", "tab1", "t2")
    );
}

#[tokio::test]
async fn typing_keeps_the_rows_that_match_with_their_workspace_and_selects_the_first_matching_tab()
{
    let mut surface = spread_surface();
    let mut screen = picking(&mut surface);
    typed(&mut surface, "side bu");
    screen.show(&mut surface, &HashSet::new());
    row_with(&screen, "side");
    row_with(&screen, "└─ build");
    row_with(&screen, "+ New workspace");
    assert!(!screen
        .lines(1..ROWS)
        .iter()
        .any(|line| line.contains("logs")));
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        into("t0", "tab2", "t3")
    );

    picking(&mut surface);
    typed(&mut surface, "side");
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        into("t0", "tab2", "t3")
    );
}

#[tokio::test]
async fn a_destination_removed_while_the_picker_is_open_keeps_the_picker_open_and_neither_enter_nor_its_footer_action_sends_anything_before_or_after_the_repaint(
) {
    for repaint in [true, false] {
        let mut surface = spread_surface();
        let mut screen = picking(&mut surface);
        down(&mut surface, 2);
        screen.show(&mut surface, &HashSet::new());
        let action = row_with(&screen, " New tab Enter");
        let new_tab = column_of(&screen, action, "New tab");
        let mut layout = spread();
        layout.workspaces.pop();
        surface.update(layout, 0);
        if repaint {
            screen.show(&mut surface, &HashSet::new());
            assert!(!screen
                .lines(1..ROWS)
                .iter()
                .any(|line| line.contains("side")));
        }

        assert_eq!(press(&mut surface, KeyCode::Enter), None, "{repaint}");
        assert!(surface.dialog_open(), "{repaint}");
        assert_eq!(left(&mut surface, new_tab, action), None, "{repaint}");
        assert!(surface.dialog_open(), "{repaint}");
    }
}

#[tokio::test]
async fn a_pane_parked_or_gone_while_the_picker_is_open_is_not_moved() {
    for parked in [true, false] {
        let mut surface = spread_surface();
        picking(&mut surface);
        down(&mut surface, 2);
        let mut layout = spread();
        if parked {
            layout.parked.insert("t0".to_owned());
        } else {
            layout.workspaces[0].tabs[0].root = json_pane("t1");
        }
        surface.update(layout, 0);
        assert_eq!(press(&mut surface, KeyCode::Enter), None, "{parked}");
    }
}

fn json_pane(terminal: &str) -> crate::session_doc::Node {
    crate::session_doc::Node::Pane {
        terminal: terminal.to_owned(),
    }
}

#[tokio::test]
async fn a_lone_pane_moves_to_a_new_workspace_and_leaves_a_new_tab_where_it_already_is_to_the_edit()
{
    let mut surface = one_pane();
    picking(&mut surface);
    let Some(Operation::Arrange(arrangement)) = press(&mut surface, KeyCode::Enter) else {
        panic!("no move");
    };
    let mut layout = row_of(&terminals(1));
    let before = layout.clone();
    assert!(layout.arrange(&arrangement));
    assert_eq!(layout, before);

    picking(&mut surface);
    down(&mut surface, 1);
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        moved("t0", Destination::Workspace { name: None })
    );
}

#[tokio::test]
async fn a_pane_alone_in_an_earlier_tab_moves_to_a_new_tab_at_the_end_of_its_workspace() {
    let mut surface = spread_surface();
    Screen::of(&mut surface);
    surface.act(KeybindAction::SwitchWorkspace(1), false);
    let mut layout = spread();
    layout.workspaces[1].tabs.push(crate::session_doc::Tab {
        id: "tab3".to_owned(),
        name: None,
        root: json_pane("t5"),
    });
    layout.workspaces[1].tabs[0].root = json_pane("t3");
    layout.workspaces[0].tabs[1].root = json_pane("t4");
    surface.update(layout, 0);
    assert_eq!(surface.session.focused(), Some("t3"));

    picking(&mut surface);
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        moved("t3", new_tab("w1"))
    );
}

#[tokio::test]
async fn a_zoomed_pane_is_not_moved() {
    let mut surface = spread_surface();
    Screen::of(&mut surface);
    surface.act(KeybindAction::Zoom, false);
    assert!(surface.session.zoomed());
    picking(&mut surface);
    down(&mut surface, 2);
    assert_eq!(press(&mut surface, KeyCode::Enter), None);
}

#[tokio::test]
async fn focus_follows_the_moved_pane_to_its_new_place() {
    let mut surface = spread_surface();
    picking(&mut surface);
    down(&mut surface, 2);
    let Some(Operation::Arrange(arrangement)) = press(&mut surface, KeyCode::Enter) else {
        panic!("no move");
    };
    let mut layout = spread();
    assert!(layout.arrange(&arrangement));
    surface.update(layout, 0);

    assert_eq!(surface.session.focused(), Some("t0"));
    assert_eq!(surface.session.focused_tab(), Some("tab5"));
}

#[tokio::test]
async fn a_click_selects_a_row_its_action_moves_there_the_wheel_steps_and_outside_closes() {
    let mut surface = spread_surface();
    let mut screen = picking(&mut surface);
    let logs = row_with(&screen, "└─ logs");
    assert_eq!(
        left(&mut surface, column_of(&screen, logs, "logs"), logs),
        None
    );
    assert!(surface.dialog_open());
    screen.show(&mut surface, &HashSet::new());
    let action = row_with(&screen, " Split right Enter");
    let split = column_of(&screen, action, "Split right");
    assert_eq!(
        left(&mut surface, split, action),
        into("t0", "tab1", "t2").map(Pointed::Run)
    );
    assert!(!surface.dialog_open());

    let mut screen = picking(&mut surface);
    assert_eq!(at(&mut surface, MouseEventKind::ScrollDown, 1, 1), None);
    screen.show(&mut surface, &HashSet::new());
    let action = row_with(&screen, " Split right Enter");
    assert_eq!(
        left(
            &mut surface,
            column_of(&screen, action, "Split right"),
            action
        ),
        into("t0", "tab2", "t3").map(Pointed::Run)
    );

    picking(&mut surface);
    assert_eq!(left(&mut surface, 0, 0), None);
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn a_list_longer_than_the_picker_scrolls_to_keep_the_selection_in_view() {
    let mut surface = spread_surface();
    let workspaces: Vec<Value> = (0..12)
        .map(|index| {
            json!({"id": format!("w{index}"), "name": format!("ws{index}"), "tabs": [
                {"id": format!("tab{index}"), "root": {"kind": "pane", "terminal": format!("t{index}")}},
            ]})
        })
        .collect();
    surface.update(
        serde_json::from_value(json!({"workspaces": workspaces})).expect("layout"),
        0,
    );
    let mut screen = picking(&mut surface);
    assert!(!screen
        .lines(1..ROWS)
        .iter()
        .any(|line| line.contains("+ New workspace")));

    for _ in 0..4 {
        assert_eq!(press(&mut surface, KeyCode::PageDown), None);
    }
    screen.show(&mut surface, &HashSet::new());
    row_with(&screen, "+ New workspace");
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        moved("t0", Destination::Workspace { name: None })
    );
}

#[tokio::test]
async fn at_120_columns_the_picker_is_a_list_column_wide_and_its_rows_action_and_outside_still_hit()
{
    let mut surface = surface(120, ROWS);
    surface.update(spread(), 0);
    let mut screen = picking(&mut surface);
    let top = row_with(&screen, " move terminal · 1 ");
    let left_edge = column_of(&screen, top, " move ") - 2;
    let line: Vec<char> = screen.line(top).chars().collect();
    assert_eq!(line[usize::from(left_edge)], '┌');
    let right_edge = (left_edge..120)
        .find(|x| line[usize::from(*x)] == '┐')
        .expect("corner");
    assert_eq!(right_edge - left_edge + 1, 53);
    assert!(!screen
        .lines(1..ROWS)
        .iter()
        .any(|line| line.contains("Preview")));
    row_with(&screen, " 2 tabs│");

    let side = row_with(&screen, "side");
    assert_eq!(
        left(&mut surface, column_of(&screen, side, "side"), side),
        None
    );
    screen.show(&mut surface, &HashSet::new());
    let action = row_with(&screen, " New tab Enter");
    assert_eq!(
        left(&mut surface, column_of(&screen, action, "New tab"), action),
        moved("t0", new_tab("w1")).map(Pointed::Run)
    );

    picking(&mut surface);
    assert_eq!(left(&mut surface, right_edge + 2, side), None);
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn a_click_picks_the_row_drawn_under_it_even_after_the_rows_change_unseen() {
    let mut surface = spread_surface();
    let screen = picking(&mut surface);
    let logs = row_with(&screen, "└─ logs");
    let mut layout = spread();
    layout.workspaces[0].tabs.pop();
    surface.update(layout, 0);

    assert_eq!(
        left(&mut surface, column_of(&screen, logs, "logs"), logs),
        None
    );
    assert_eq!(press(&mut surface, KeyCode::Enter), None);
    assert!(surface.dialog_open());
}

#[tokio::test]
async fn a_picker_too_small_to_draw_its_rows_does_not_move_on_enter() {
    let mut surface = surface(COLS, 7);
    surface.update(spread(), 0);
    let screen = picking(&mut surface);
    row_with(&screen, "Enlarge terminal");

    assert_eq!(press(&mut surface, KeyCode::Enter), None);
    assert!(surface.dialog_open());
}

#[tokio::test]
async fn pointer_motion_over_the_picker_copies_nothing_and_a_click_copies_it_once() {
    let mut surface = spread_surface();
    picking(&mut surface);
    let copies = |surface: &Surface| surface.dialog.as_ref().map(|modal| modal.copies());
    let before = copies(&surface);
    for column in 0..20 {
        assert_eq!(at(&mut surface, MouseEventKind::Moved, column, 3), None);
    }
    assert_eq!(copies(&surface), before);
    assert_eq!(at(&mut surface, MouseEventKind::ScrollDown, 1, 1), None);
    assert_eq!(copies(&surface), before.map(|copies| copies + 1));
}

#[tokio::test]
async fn a_click_after_scrolling_picks_the_row_drawn_under_it() {
    let mut surface = spread_surface();
    let workspaces: Vec<Value> = (0..12)
        .map(|index| {
            json!({"id": format!("w{index}"), "name": format!("ws{index}"), "tabs": [
                {"id": format!("tab{index}"), "name": format!("logs{index}"),
                 "root": {"kind": "pane", "terminal": format!("t{index}")}},
            ]})
        })
        .collect();
    surface.update(
        serde_json::from_value(json!({"workspaces": workspaces})).expect("layout"),
        0,
    );
    let mut screen = picking(&mut surface);
    let first = row_with(&screen, " /") + 2;
    let unscrolled = screen.line(first);
    for _ in 0..4 {
        assert_eq!(press(&mut surface, KeyCode::PageDown), None);
    }
    screen.show(&mut surface, &HashSet::new());
    let line = screen.line(first);
    assert_ne!(line, unscrolled);
    let row = line.split('│').nth(1).expect("row").trim_start();
    let label = row.split("  ").next().expect("label").trim().to_owned();
    let index = label.trim_start_matches("└─ ").trim_start_matches("├─ ");
    let index = index
        .trim_start_matches("ws")
        .trim_start_matches("logs")
        .to_owned();
    let expected = if label.starts_with("ws") {
        moved("t0", new_tab(&format!("w{index}")))
    } else {
        into("t0", &format!("tab{index}"), &format!("t{index}"))
    };

    let column = column_of(&screen, first, &label);
    assert_eq!(left(&mut surface, column, first), None);
    assert_eq!(press(&mut surface, KeyCode::Enter), expected, "{label}");
}

#[tokio::test]
async fn a_tab_renamed_out_of_the_search_before_repaint_is_not_swapped_for_another_row() {
    for click in [false, true] {
        let mut surface = spread_surface();
        let mut screen = picking(&mut surface);
        typed(&mut surface, "side bu");
        screen.show(&mut surface, &HashSet::new());
        let action = row_with(&screen, " Split right Enter");
        let mut layout = spread();
        layout.workspaces[1].tabs[0].name = Some("deploy".to_owned());
        surface.update(layout, 0);

        let sent = if click {
            let column = column_of(&screen, action, "Split right");
            left(&mut surface, column, action).and_then(|pointed| match pointed {
                Pointed::Run(operation) => Some(operation),
                _ => None,
            })
        } else {
            press(&mut surface, KeyCode::Enter)
        };
        assert_eq!(sent, None, "{click}");
    }
}

#[tokio::test]
async fn a_picker_shrunk_until_it_draws_no_rows_does_not_move_on_enter_before_or_after_the_repaint()
{
    let mut surface = spread_surface();
    picking(&mut surface);
    surface.resize(host(COLS, 4));
    assert_eq!(press(&mut surface, KeyCode::Enter), None);
    assert!(surface.dialog_open());

    Screen::of(&mut surface).show(&mut surface, &HashSet::new());
    assert_eq!(press(&mut surface, KeyCode::Enter), None);
    assert!(surface.dialog_open());
}

#[tokio::test]
async fn a_held_prefix_repeating_once_still_lets_the_move_key_close_the_picker() {
    let mut surface = spread_surface();
    let prefix = TerminalKey::new(surface.keybinds.prefix[0].0, surface.keybinds.prefix[0].1);
    picking(&mut surface);
    assert_eq!(surface.dialog_key(&prefix), None);
    assert_eq!(
        surface.dialog_key(&prefix.clone().with_kind(KeyEventKind::Repeat)),
        None
    );
    assert_eq!(press(&mut surface, KeyCode::Char('m')), None);
    assert!(!surface.dialog_open());
}

fn unnamed_side() -> Surface {
    let mut layout = spread();
    layout.workspaces[1].name = None;
    let mut surface = surface(COLS, ROWS);
    surface.update(layout, 0);
    hears(&mut surface, folders("/srv/app"));
    surface
}

fn folders(folder: &str) -> Report {
    Report::Snapshot {
        folders: [("t3".to_owned(), folder.into())].into_iter().collect(),
        statuses: Vec::new(),
    }
}

fn hears(surface: &mut Surface, report: Report) {
    surface.heard(report, std::time::Instant::now());
}

#[tokio::test]
async fn a_folder_report_relabels_the_picker_rows_and_search_and_keeps_the_selection() {
    let mut surface = unnamed_side();
    let mut screen = picking(&mut surface);
    row_with(&screen, "app");
    down(&mut surface, 3);

    hears(
        &mut surface,
        Report::Folder("t3".to_owned(), "/srv/app/src".into()),
    );
    screen.show(&mut surface, &HashSet::new());
    row_with(&screen, "src");
    assert!(!screen
        .lines(1..ROWS)
        .iter()
        .any(|line| line.contains("app")));
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        into("t0", "tab2", "t3")
    );

    picking(&mut surface);
    typed(&mut surface, "src");
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        into("t0", "tab2", "t3")
    );
}

#[tokio::test]
async fn a_pane_renamed_while_the_picker_is_open_retitles_it() {
    let mut surface = spread_surface();
    let mut screen = picking(&mut surface);
    row_with(&screen, " move terminal · 1 ");
    heard_name(&mut surface, "t0", Some("build"));
    screen.show(&mut surface, &HashSet::new());
    row_with(&screen, " move build · 1 ");
}

fn status(state: &str) -> crate::pane::CoreStatus {
    crate::pane::core_status(&json!({"state": state, "app": "claude"}))
}

#[tokio::test]
async fn a_resize_drops_an_open_dialogs_frame_and_hits_while_session_changes_only_ask_the_picker_to_repaint(
) {
    let changes: [fn(&mut Surface); 5] = [
        |surface| {
            let mut layout = spread();
            layout.workspaces[1].name = Some("other".to_owned());
            surface.update(layout, 0);
        },
        |surface| hears(surface, folders("/srv/web")),
        |surface| {
            hears(
                surface,
                Report::Folder("t3".to_owned(), "/srv/app/src".into()),
            )
        },
        |surface| {
            hears(
                surface,
                Report::Names([("t3".to_owned(), "web".to_owned())].into_iter().collect()),
            )
        },
        |surface| heard_name(surface, "t3", Some("api")),
    ];
    for (index, change) in changes.iter().enumerate() {
        let mut surface = unnamed_side();
        picking(&mut surface);
        let painted = |surface: &Surface| {
            surface
                .dialog
                .as_ref()
                .map(|dialog| (dialog.image().is_some(), dialog.cursor().is_some()))
        };
        assert_eq!(painted(&surface), Some((true, true)), "{index}");
        assert!(!surface.needs_frame(), "{index}");
        change(&mut surface);
        assert_eq!(painted(&surface), Some((true, true)), "{index}");
        assert!(surface.needs_frame(), "{index}");
    }

    let statuses: [fn(&mut Surface); 2] = [
        |surface| hears(surface, Report::Removed("t3".to_owned())),
        |surface| hears(surface, Report::Status("t3".to_owned(), status("blocked"))),
    ];
    for (index, report) in statuses.iter().enumerate() {
        let mut surface = unnamed_side();
        picking(&mut surface);
        report(&mut surface);
        let host = surface.host_area();
        let outdated = surface.dialog.as_ref().map(|dialog| dialog.stale(host));
        assert_eq!(outdated, Some(false), "status {index}");
    }

    let (mut surface, _) = helping();
    let expected = help(0, "", false);
    surface.resize(host(COLS + 2, ROWS));
    assert_eq!(
        surface
            .dialog
            .as_ref()
            .map(|dialog| (dialog.image().is_some(), dialog.cursor().is_some())),
        Some((false, false))
    );
    assert_eq!(left(&mut surface, expected.close.x, expected.close.y), None);
    assert!(surface.dialog_open());
}

#[tokio::test]
async fn two_help_scroll_keys_in_one_batch_scroll_two_rows() {
    let (mut surface, mut screen) = helping();
    press(&mut surface, KeyCode::Char('j'));
    press(&mut surface, KeyCode::Char('j'));
    screen.show(&mut surface, &HashSet::new());
    let expected = help(2, "", false);
    assert_eq!(panel(&screen, expected.area), expected.lines);
}

#[tokio::test]
async fn a_help_scroll_key_between_a_resize_and_the_next_frame_scrolls_from_where_the_list_was() {
    let (mut surface, mut screen) = helping();
    press(&mut surface, KeyCode::Char('j'));
    press(&mut surface, KeyCode::Char('j'));
    screen.show(&mut surface, &HashSet::new());
    surface.resize(host(COLS + 2, ROWS));
    surface.resize(host(COLS, ROWS));
    press(&mut surface, KeyCode::Char('j'));
    screen.show(&mut surface, &HashSet::new());
    let expected = help(3, "", false);
    assert_eq!(panel(&screen, expected.area), expected.lines);
}

#[tokio::test]
async fn two_wheel_events_in_one_batch_move_the_picker_two_steps() {
    let mut surface = spread_surface();
    picking(&mut surface);
    assert_eq!(at(&mut surface, MouseEventKind::ScrollDown, 1, 1), None);
    assert_eq!(at(&mut surface, MouseEventKind::ScrollDown, 1, 1), None);
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        moved("t0", Destination::Workspace { name: None })
    );
}

#[tokio::test]
async fn a_tab_list_narrower_than_a_rows_connector_and_first_letter_lists_nothing_and_enter_stays()
{
    for (cols, lists) in [(11, false), (12, true)] {
        let mut surface = surface(cols, ROWS);
        surface.update(spread(), 0);
        let screen = picking(&mut surface);
        let listed = screen
            .lines(1..ROWS)
            .iter()
            .any(|line| line.contains("└─ l"));
        assert_eq!(listed, lists, "{cols}");
        assert_eq!(
            press(&mut surface, KeyCode::Enter).is_some(),
            lists,
            "{cols}"
        );
        assert_eq!(surface.dialog_open(), !lists, "{cols}");
    }
}

#[tokio::test]
async fn a_scrollbar_grab_does_not_outlive_a_resize() {
    let (mut surface, _) = helping();
    let top = help(0, "", false);
    let thumb = crate::ui::scrollbar_thumb(top.metrics, top.scrollbar).expect("thumb");
    left(&mut surface, top.scrollbar.x, thumb.top);
    surface.resize(host(COLS, ROWS));
    let mut screen = Screen::of(&mut surface);
    screen.show(&mut surface, &HashSet::new());
    at(
        &mut surface,
        MouseEventKind::Drag(MouseButton::Left),
        top.scrollbar.x,
        thumb.top + 3,
    );
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(panel(&screen, top.area), top.lines);
}

#[tokio::test]
async fn a_click_before_a_dialogs_first_paint_acts_on_nothing_and_leaves_it_open() {
    let opens: [fn(&mut Surface); 3] = [
        |surface| assert_eq!(surface.act(KeybindAction::ClosePane, true), None),
        |surface| assert_eq!(surface.act(KeybindAction::RenameTab, false), None),
        |surface| assert_eq!(surface.act(KeybindAction::Help, false), None),
    ];
    for (index, open) in opens.iter().enumerate() {
        let mut surface = one_pane();
        open(&mut surface);
        assert!(surface.dialog_open(), "{index}");
        assert_eq!(left(&mut surface, 0, 0), None, "{index}");
        assert!(surface.dialog_open(), "{index}");
    }
}

#[tokio::test]
async fn a_tab_whose_wide_first_glyph_finds_no_room_beside_its_connector_is_not_picked_by_enter() {
    let mut surface = surface(12, 8);
    surface.update(
        serde_json::from_value(json!({"workspaces": [
            {"id": "w0", "name": "main", "tabs": [
                {"id": "tab0", "root": balanced(&terminals(2))},
                {"id": "tab1", "name": "開発", "root": {"kind": "pane", "terminal": "t2"}},
                {"id": "tab2", "name": "logs", "root": {"kind": "pane", "terminal": "t3"}},
            ]},
            {"id": "w1", "name": "side", "tabs": [
                {"id": "tab3", "name": "build", "root": {"kind": "pane", "terminal": "t4"}},
            ]},
        ]}))
        .expect("layout"),
        0,
    );
    let mut screen = picking(&mut surface);
    down(&mut surface, 1);
    screen.show(&mut surface, &HashSet::new());
    assert!(screen.lines(0..8).iter().any(|line| line.contains("├─")));
    assert!(!screen.lines(0..8).iter().any(|line| line.contains('開')));

    assert_eq!(press(&mut surface, KeyCode::Enter), None);
    assert!(surface.dialog_open());

    down(&mut surface, 1);
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(
        press(&mut surface, KeyCode::Enter),
        into("t0", "tab2", "t3")
    );
}

#[tokio::test]
async fn a_footer_too_narrow_to_draw_any_of_the_action_takes_no_click() {
    let mut surface = surface(17, ROWS);
    surface.update(spread(), 0);
    let screen = picking(&mut surface);
    let footer = row_with(&screen, "Esc close");
    let inner = column_of(&screen, footer, "Esc close") - 2;
    assert_eq!(screen.span(footer, inner - 1..inner + 2), "│  ");

    assert_eq!(left(&mut surface, inner, footer), None);
    assert!(surface.dialog_open());
}

#[tokio::test]
async fn a_tab_name_too_long_to_measure_beside_a_wide_first_glyph_is_not_picked_by_enter() {
    let mut surface = surface(12, 8);
    let name = format!("開{}", "a".repeat(65_533));
    surface.update(
        serde_json::from_value(json!({"workspaces": [
            {"id": "w0", "name": "main", "tabs": [
                {"id": "tab0", "root": balanced(&terminals(2))},
                {"id": "tab1", "name": name, "root": {"kind": "pane", "terminal": "t2"}},
                {"id": "tab2", "name": "logs", "root": {"kind": "pane", "terminal": "t3"}},
            ]},
            {"id": "w1", "name": "side", "tabs": [
                {"id": "tab3", "name": "build", "root": {"kind": "pane", "terminal": "t4"}},
            ]},
        ]}))
        .expect("layout"),
        0,
    );
    let mut screen = picking(&mut surface);
    down(&mut surface, 1);
    screen.show(&mut surface, &HashSet::new());
    assert!(!screen.lines(0..8).iter().any(|line| line.contains('開')));

    assert_eq!(press(&mut surface, KeyCode::Enter), None);
    assert!(surface.dialog_open());
}
