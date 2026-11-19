use ratatui::buffer::{Buffer, Cell};
use ratatui::style::{Modifier, Style};

use super::*;
use crate::core_client::tui::session::Operation;
use crate::core_client::tui::surface::Pointed;
use crate::input::TextCommit;
use crate::protocol::CursorState;
use crate::raw_input::RawInputEvent;
use crate::ui::text_editor::TextEditor;

const COLS: u16 = 80;
const ROWS: u16 = 24;

fn one_pane() -> Surface {
    let mut surface = surface(COLS, ROWS);
    surface.update(row_of(&terminals(1)));
    frame_attaching(&mut surface, &HashSet::new());
    surface
}

fn inner(surface: &Surface) -> Rect {
    super::super::visible(&surface.views, id_of(surface, "t0"))
        .expect("visible")
        .1
}

fn press(surface: &mut Surface, code: KeyCode) -> Option<Operation> {
    surface.dialog_key(&TerminalKey::new(code, KeyModifiers::NONE))
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
    surface.update(row_of(&terminals(2)));
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
