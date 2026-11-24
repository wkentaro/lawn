use super::*;
use crate::session_doc::{Arrangement, Destination, Direction};

fn two_spaces() -> Layout {
    serde_json::from_value(json!({
        "workspaces": [
            {"id": "w0", "name": "main", "tabs": [
                {"id": "tab0", "root": balanced(&["t0".to_owned(), "t1".to_owned()])},
                {"id": "tab1", "name": "logs", "root": {"kind": "pane", "terminal": "t2"}},
            ]},
            {"id": "w1", "name": "side", "tabs": [
                {"id": "tab2", "root": {"kind": "pane", "terminal": "t3"}},
            ]},
        ],
    }))
    .expect("layout")
}

fn spread_out() -> Surface {
    let mut surface = plain(120, 30);
    surface.update(two_spaces());
    frame(&mut surface, &HashSet::new());
    surface
}

fn pane(surface: &Surface, terminal: &str) -> Rect {
    surface
        .tiles
        .tiles
        .iter()
        .find(|tile| tile.terminal == terminal)
        .expect(terminal)
        .info
        .inner_rect
}

fn right(surface: &mut Surface, column: u16, row: u16) -> Option<Operation> {
    frame(surface, &HashSet::new());
    point(
        surface,
        MouseEventKind::Down(MouseButton::Right),
        column,
        row,
    )
}

fn key(surface: &mut Surface, code: KeyCode) -> Option<Operation> {
    ran(surface.dialog_key(&TerminalKey::new(code, KeyModifiers::NONE)))
}

fn listed(surface: &mut Surface, x: u16, y: u16) -> Vec<String> {
    let screen = Screen::of(surface);
    assert_eq!(screen.cell(x, y).symbol, "┌");
    (y + 1..)
        .take_while(|row| screen.cell(x, *row).symbol == "│")
        .map(|row| {
            let text = screen.span(row, x + 1..surface.host.cols);
            text.split('│').next().unwrap_or_default().trim().to_owned()
        })
        .collect()
}

fn highlighted(surface: &mut Surface, x: u16, y: u16) -> Vec<bool> {
    let screen = Screen::of(surface);
    let accent = background(Palette::catppuccin().accent);
    (y + 1..)
        .take_while(|row| screen.cell(x, *row).symbol == "│")
        .map(|row| screen.cell(x + 1, row).bg == accent)
        .collect()
}

fn choose(surface: &mut Surface, x: u16, y: u16, label: &str) -> Option<Operation> {
    let index = listed(surface, x, y)
        .iter()
        .position(|item| item == label)
        .expect(label);
    point(
        surface,
        MouseEventKind::Down(MouseButton::Left),
        x + 2,
        y + 1 + index as u16,
    )
}

fn retyped(surface: &mut Surface, text: &str) -> Option<Operation> {
    surface.dialog_key(&TerminalKey::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    for character in text.chars() {
        assert_eq!(key(surface, KeyCode::Char(character)), None);
    }
    key(surface, KeyCode::Enter)
}

fn panes(terminals: &[&str]) -> Option<Operation> {
    Some(Operation::Close(
        terminals
            .iter()
            .map(|terminal| (*terminal).to_owned())
            .collect(),
    ))
}

const PANE_ITEMS: [&str; 8] = [
    "Park",
    "Rename pane",
    "Swap with focused pane",
    "Move to...",
    "Split right",
    "Split down",
    "Zoom",
    "Close pane",
];

#[tokio::test]
async fn a_right_click_on_a_workspace_row_offers_rename_and_close_for_that_workspace() {
    let mut surface = spread_out();
    let x = find(&surface, 3, "side");
    assert_eq!(right(&mut surface, x, 3), None);
    assert_eq!(listed(&mut surface, x, 3), ["Rename", "Close"]);

    assert_eq!(choose(&mut surface, x, 3, "Rename"), None);
    assert_eq!(
        retyped(&mut surface, "far"),
        Some(Operation::Arrange(Arrangement::NameWorkspace {
            workspace: "w1".to_owned(),
            name: "far".to_owned(),
        }))
    );

    assert_eq!(right(&mut surface, x, 3), None);
    assert_eq!(choose(&mut surface, x, 3, "Close"), panes(&["t3"]));
    assert_eq!(workspace(&surface), "w0");
}

#[tokio::test]
async fn closing_from_a_menu_asks_first_exactly_when_the_close_key_would() {
    let mut surface = spread_out().confirming_close(true);
    let x = find(&surface, 3, "side");
    right(&mut surface, x, 3);
    assert_eq!(choose(&mut surface, x, 3, "Close"), None);
    assert!(asked(&mut surface));
    assert_eq!(key(&mut surface, KeyCode::Enter), panes(&["t3"]));

    let target = pane(&surface, "t1");
    right(&mut surface, target.x + 1, target.y + 1);
    assert_eq!(
        choose(&mut surface, target.x + 1, target.y + 1, "Close pane"),
        panes(&["t1"])
    );
}

#[tokio::test]
async fn a_right_click_on_a_tab_offers_new_rename_and_close_and_focuses_that_tab_first() {
    let mut surface = spread_out();
    let x = find(&surface, 0, "logs");
    assert_eq!(right(&mut surface, x, 0), None);
    assert_eq!(listed(&mut surface, x, 0), ["New tab", "Rename", "Close"]);
    assert_eq!(tab(&surface), "tab0");

    assert_eq!(choose(&mut surface, x, 0, "Close"), panes(&["t2"]));
    assert_eq!(tab(&surface), "tab1");

    let first = find(&surface, 0, " 1 ") + 1;
    right(&mut surface, first, 0);
    assert_eq!(
        destination(choose(&mut surface, first, 0, "New tab")),
        Some(Destination::Tab {
            workspace: Some("w0".to_owned()),
            name: None,
        })
    );
    assert_eq!(tab(&surface), "tab0");

    right(&mut surface, x, 0);
    assert_eq!(choose(&mut surface, x, 0, "Rename"), None);
    assert_eq!(
        retyped(&mut surface, "out"),
        Some(Operation::Arrange(Arrangement::NameTab {
            tab: "tab1".to_owned(),
            name: "out".to_owned(),
        }))
    );
}

#[tokio::test]
async fn a_right_click_on_a_pane_lists_its_items_with_swap_only_off_the_focused_pane_and_clear_only_when_named(
) {
    let mut surface = spread_out();
    let other = pane(&surface, "t1");
    right(&mut surface, other.x + 1, other.y + 1);
    assert_eq!(listed(&mut surface, other.x + 1, other.y + 1), PANE_ITEMS);
    key(&mut surface, KeyCode::Esc);

    named(&mut surface, "t1", Some("db"));
    right(&mut surface, other.x + 1, other.y + 1);
    let mut items = PANE_ITEMS.to_vec();
    items.insert(2, "Clear pane name");
    assert_eq!(listed(&mut surface, other.x + 1, other.y + 1), items);
    key(&mut surface, KeyCode::Esc);

    let focused = pane(&surface, "t0");
    right(&mut surface, focused.x + 1, focused.y + 1);
    let mut items = PANE_ITEMS.to_vec();
    items.remove(2);
    assert_eq!(listed(&mut surface, focused.x + 1, focused.y + 1), items);
}

#[tokio::test]
async fn pane_items_act_on_the_clicked_pane_and_leave_the_focus_where_it_was() {
    let mut surface = spread_out();
    let other = pane(&surface, "t1");
    let (x, y) = (other.x + 1, other.y + 1);
    let mut run = |label: &str| {
        right(&mut surface, x, y);
        let operation = choose(&mut surface, x, y, label);
        assert_eq!(surface.session.focused(), Some("t0"), "{label}");
        operation
    };
    assert_eq!(run("Park"), Some(Operation::Park("t1".to_owned())));
    assert_eq!(
        destination(run("Split right")),
        Some(Destination::Split {
            beside: "t1".to_owned(),
            direction: Direction::Right,
            ratio: 0.5,
        })
    );
    assert_eq!(
        destination(run("Split down")),
        Some(Destination::Split {
            beside: "t1".to_owned(),
            direction: Direction::Down,
            ratio: 0.5,
        })
    );
    assert_eq!(run("Close pane"), panes(&["t1"]));
    assert_eq!(
        run("Swap with focused pane"),
        Some(Operation::Arrange(Arrangement::SwapWith {
            terminal: "t0".to_owned(),
            with: "t1".to_owned(),
        }))
    );

    assert_eq!(run("Rename pane"), None);
    assert_eq!(
        retyped(&mut surface, "db"),
        Some(Operation::Name {
            terminal: "t1".to_owned(),
            name: "db".to_owned(),
        })
    );
    named(&mut surface, "t1", Some("db"));
    right(&mut surface, x, y);
    assert_eq!(
        choose(&mut surface, x, y, "Clear pane name"),
        Some(Operation::Name {
            terminal: "t1".to_owned(),
            name: String::new(),
        })
    );

    right(&mut surface, x, y);
    assert_eq!(choose(&mut surface, x, y, "Move to..."), None);
    assert!(Screen::of(&mut surface)
        .lines(0..30)
        .iter()
        .any(|line| line.contains(" move db ")));
}

#[tokio::test]
async fn zoom_from_a_pane_menu_focuses_and_zooms_the_clicked_pane() {
    let mut surface = spread_out();
    let other = pane(&surface, "t1");
    right(&mut surface, other.x + 1, other.y + 1);
    assert_eq!(choose(&mut surface, other.x + 1, other.y + 1, "Zoom"), None);
    assert_eq!(surface.session.focused(), Some("t1"));
    assert!(surface.session.zoomed());
}

#[tokio::test]
async fn up_and_down_move_the_highlight_within_the_menu_and_enter_picks_it() {
    let mut surface = spread_out();
    let x = find(&surface, 0, "logs");
    right(&mut surface, x, 0);
    assert_eq!(highlighted(&mut surface, x, 0), [true, false, false]);
    assert_eq!(key(&mut surface, KeyCode::Up), None);
    assert_eq!(highlighted(&mut surface, x, 0), [true, false, false]);
    assert_eq!(key(&mut surface, KeyCode::Char('j')), None);
    assert_eq!(highlighted(&mut surface, x, 0), [true, false, false]);
    for _ in 0..3 {
        assert_eq!(key(&mut surface, KeyCode::Down), None);
    }
    assert_eq!(highlighted(&mut surface, x, 0), [false, false, true]);
    assert_eq!(key(&mut surface, KeyCode::Char('j')), None);
    assert_eq!(key(&mut surface, KeyCode::Up), None);
    assert_eq!(highlighted(&mut surface, x, 0), [false, true, false]);
    assert_eq!(key(&mut surface, KeyCode::Enter), None);
    assert!(Screen::of(&mut surface)
        .lines(0..30)
        .iter()
        .any(|line| line.contains("rename tab")));
}

#[tokio::test]
async fn escape_or_a_click_off_the_items_closes_the_menu_and_another_right_click_leaves_it_open() {
    let mut surface = spread_out();
    let x = find(&surface, 0, "logs");
    right(&mut surface, x, 0);
    assert_eq!(key(&mut surface, KeyCode::Esc), None);
    assert!(!surface.dialog_open());

    right(&mut surface, x, 0);
    let target = pane(&surface, "t1");
    assert_eq!(right(&mut surface, target.x + 1, target.y + 1), None);
    assert!(surface.dialog_open());
    assert_eq!(listed(&mut surface, x, 0), ["New tab", "Rename", "Close"]);
    assert_eq!(
        point(&mut surface, MouseEventKind::Down(MouseButton::Left), x, 1),
        None
    );
    assert!(!surface.dialog_open());
    assert_eq!(tab(&surface), "tab0");
}

#[tokio::test]
async fn pointing_at_an_item_highlights_it_for_enter() {
    let mut surface = spread_out();
    let x = find(&surface, 0, "logs");
    right(&mut surface, x, 0);
    Screen::of(&mut surface);
    point(&mut surface, MouseEventKind::Moved, x + 2, 3);
    assert_eq!(highlighted(&mut surface, x, 0), [false, false, true]);
    point(&mut surface, MouseEventKind::Moved, x + 30, 10);
    assert_eq!(highlighted(&mut surface, x, 0), [false, false, true]);
    assert_eq!(key(&mut surface, KeyCode::Enter), panes(&["t2"]));
}

#[tokio::test]
async fn a_click_acts_on_the_item_drawn_under_it_even_after_the_items_change_unseen() {
    let mut surface = spread_out();
    named(&mut surface, "t1", Some("db"));
    let other = pane(&surface, "t1");
    let (x, y) = (other.x + 1, other.y + 1);
    right(&mut surface, x, y);
    let items = listed(&mut surface, x, y);
    let row =
        |label: &str| y + 1 + items.iter().position(|item| item == label).expect(label) as u16;
    named(&mut surface, "t1", None);
    let swap = Some(Operation::Arrange(Arrangement::SwapWith {
        terminal: "t0".to_owned(),
        with: "t1".to_owned(),
    }));
    assert_eq!(
        point(
            &mut surface,
            MouseEventKind::Down(MouseButton::Left),
            x + 2,
            row("Swap with focused pane")
        ),
        swap
    );

    named(&mut surface, "t1", Some("db"));
    right(&mut surface, x, y);
    listed(&mut surface, x, y);
    named(&mut surface, "t1", None);
    assert_eq!(
        point(
            &mut surface,
            MouseEventKind::Down(MouseButton::Left),
            x + 2,
            row("Clear pane name")
        ),
        None
    );
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn a_name_report_while_a_pane_menu_is_open_redraws_its_items() {
    let mut surface = spread_out();
    let other = pane(&surface, "t1");
    let (x, y) = (other.x + 1, other.y + 1);
    right(&mut surface, x, y);
    let mut screen = Screen::of(&mut surface);
    named(&mut surface, "t1", Some("db"));
    assert!(surface.needs_frame());
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(screen.span(y + 3, x + 1..x + 16), "Clear pane name");
}

#[tokio::test]
async fn items_on_a_target_that_left_the_session_do_nothing() {
    let mut surface = spread_out();
    let x = find(&surface, 3, "side");
    let other = pane(&surface, "t1");
    let gone: Layout = serde_json::from_value(json!({
        "workspaces": [
            {"id": "w0", "name": "main", "tabs": [
                {"id": "tab0", "root": {"kind": "pane", "terminal": "t0"}},
            ]},
        ],
    }))
    .expect("layout");
    for label in ["Rename", "Close"] {
        right(&mut surface, x, 3);
        listed(&mut surface, x, 3);
        surface.update(gone.clone());
        let index = u16::from(label == "Close");
        assert_eq!(
            point(
                &mut surface,
                MouseEventKind::Down(MouseButton::Left),
                x + 2,
                4 + index
            ),
            None,
            "{label}"
        );
        assert!(!surface.dialog_open(), "{label}");
        surface.update(two_spaces());
        frame(&mut surface, &HashSet::new());
    }
    for label in PANE_ITEMS {
        right(&mut surface, other.x + 1, other.y + 1);
        let items = listed(&mut surface, other.x + 1, other.y + 1);
        let row = other.y + 2 + items.iter().position(|item| item == label).expect(label) as u16;
        surface.update(gone.clone());
        assert_eq!(
            point(
                &mut surface,
                MouseEventKind::Down(MouseButton::Left),
                other.x + 3,
                row
            ),
            None,
            "{label}"
        );
        assert!(!surface.dialog_open(), "{label}");
        assert!(!surface.session.zoomed(), "{label}");
        surface.update(two_spaces());
        frame(&mut surface, &HashSet::new());
    }
}

#[tokio::test]
async fn the_menu_opens_at_the_click_over_the_undimmed_screen_on_a_pane_border_too_and_on_bare_chrome_never(
) {
    let mut surface = spread_out();
    assert_eq!(right(&mut surface, 2, 0), None);
    assert!(!surface.dialog_open());
    let x = find(&surface, 0, "logs");
    right(&mut surface, x, 0);
    let screen = Screen::of(&mut surface);
    assert_eq!(screen.cell(x, 0).symbol, "┌");
    let behind = pane(&surface, "t1");
    assert_eq!(
        screen
            .cell(behind.right() - 1, behind.bottom() - 1)
            .modifier
            & Modifier::DIM.bits(),
        0
    );
    key(&mut surface, KeyCode::Esc);

    right(&mut surface, behind.x + 1, behind.y - 1);
    assert_eq!(listed(&mut surface, behind.x + 1, behind.y - 1), PANE_ITEMS);
}

#[tokio::test]
async fn a_right_click_on_a_pane_that_reports_the_mouse_opens_the_menu_and_sends_the_pane_nothing()
{
    let mut surface = plain(120, 30);
    surface.update(two_spaces());
    let mut inputs = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1006h");
    frame(&mut surface, &HashSet::new());
    for input in inputs.iter_mut() {
        reports(input);
    }
    let other = pane(&surface, "t1");
    right(&mut surface, other.x + 1, other.y + 1);
    point(
        &mut surface,
        MouseEventKind::Up(MouseButton::Right),
        other.x + 1,
        other.y + 1,
    );
    assert!(surface.dialog_open());
    for input in inputs.iter_mut() {
        assert_eq!(reports(input), b"");
    }
}

fn with_alt(surface: &mut Surface, kind: MouseEventKind, column: u16, row: u16) {
    let mut event = mouse_at(kind, column, row);
    event.modifiers = KeyModifiers::ALT;
    assert!(surface.mouse(event, std::time::Instant::now()).is_none());
}

#[tokio::test]
async fn a_right_click_with_the_passthrough_modifier_goes_to_a_pane_that_reports_the_mouse_without_it(
) {
    let mut surface = plain(120, 30);
    surface.configure(
        &toml::from_str("[ui]\nright_click_passthrough_modifier = \"alt\"").expect("config"),
    );
    surface.update(two_spaces());
    let mut inputs = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1002h\x1b[?1006h");
    frame(&mut surface, &HashSet::new());
    for input in inputs.iter_mut() {
        reports(input);
    }
    let other = pane(&surface, "t1");
    let (x, y) = (other.x + 1, other.y + 1);
    assert_ne!(surface.session.focused(), Some("t1"));

    frame(&mut surface, &HashSet::new());
    with_alt(&mut surface, MouseEventKind::Down(MouseButton::Right), x, y);
    with_alt(
        &mut surface,
        MouseEventKind::Drag(MouseButton::Right),
        x + 1,
        y,
    );
    with_alt(
        &mut surface,
        MouseEventKind::Up(MouseButton::Right),
        x + 1,
        y,
    );

    assert!(!surface.dialog_open());
    assert_eq!(surface.session.focused(), Some("t1"));
    let sent: Vec<Vec<u8>> = inputs.iter_mut().map(reports).collect();
    assert_eq!(
        sent.iter()
            .filter(|bytes| !bytes.is_empty())
            .collect::<Vec<_>>(),
        [b"\x1b[<2;2;2M\x1b[<34;3;2M\x1b[<2;3;2m"]
    );

    right(&mut surface, x, y);
    assert!(surface.dialog_open());
    assert!(inputs.iter_mut().all(|input| reports(input).is_empty()));
}

#[tokio::test]
async fn a_right_click_with_the_passthrough_modifier_opens_the_menu_on_a_pane_that_reports_no_mouse(
) {
    let mut surface = plain(120, 30);
    surface.configure(
        &toml::from_str("[ui]\nright_click_passthrough_modifier = \"alt\"").expect("config"),
    );
    surface.update(two_spaces());
    let mut inputs = attach_with_input(&mut surface, b"");
    frame(&mut surface, &HashSet::new());
    let other = pane(&surface, "t1");

    with_alt(
        &mut surface,
        MouseEventKind::Down(MouseButton::Right),
        other.x + 1,
        other.y + 1,
    );

    assert!(surface.dialog_open());
    assert!(inputs.iter_mut().all(|input| reports(input).is_empty()));
}

#[tokio::test]
async fn a_right_click_with_the_passthrough_modifier_waits_for_a_frame_after_a_resize() {
    let mut surface = plain(120, 30);
    surface.configure(
        &toml::from_str("[ui]\nright_click_passthrough_modifier = \"alt\"").expect("config"),
    );
    surface.update(two_spaces());
    let mut inputs = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1006h");
    frame(&mut surface, &HashSet::new());
    for input in inputs.iter_mut() {
        reports(input);
    }
    let other = pane(&surface, "t1");

    surface.resize(host(100, 30));
    with_alt(
        &mut surface,
        MouseEventKind::Down(MouseButton::Right),
        other.x + 1,
        other.y + 1,
    );

    assert_ne!(surface.session.focused(), Some("t1"));
    assert!(inputs.iter_mut().all(|input| reports(input).is_empty()));
}

#[tokio::test]
async fn a_right_click_with_the_passthrough_modifier_on_the_scrollbar_opens_the_menu() {
    let shown = [lines(40), b"\x1b[?1000h\x1b[?1006h".to_vec()].concat();
    let (mut surface, _, mut input) = selecting(true, &shown);
    surface.right_click_passthrough = Some(KeyModifiers::ALT);
    let track = track(&surface, "t0");
    reports(&mut input);

    with_alt(
        &mut surface,
        MouseEventKind::Down(MouseButton::Right),
        track.x,
        track.y + 3,
    );

    assert!(surface.dialog_open());
    assert!(reports(&mut input).is_empty());
}

#[tokio::test]
async fn a_right_click_with_the_passthrough_modifier_goes_to_the_pane_in_copy_and_resize_mode() {
    for resizing in [false, true] {
        let mut surface = plain(120, 30);
        surface.right_click_passthrough = Some(KeyModifiers::ALT);
        surface.update(two_spaces());
        let mut inputs = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1006h");
        match resizing {
            true => {
                surface.act(KeybindAction::EnterResizeMode, false);
            }
            false => surface.enter_copy_mode(),
        }
        frame(&mut surface, &HashSet::new());
        for input in inputs.iter_mut() {
            reports(input);
        }
        let other = pane(&surface, "t1");

        with_alt(
            &mut surface,
            MouseEventKind::Down(MouseButton::Right),
            other.x + 1,
            other.y + 1,
        );

        assert_eq!(surface.session.focused(), Some("t1"), "{resizing}");
        let sent: Vec<u8> = inputs.iter_mut().flat_map(reports).collect();
        assert_eq!(sent, b"\x1b[<2;2;2M", "{resizing}");
    }
}

#[tokio::test]
async fn a_right_click_with_the_passthrough_modifier_on_a_mode_bar_reaches_no_pane() {
    let mut surface = surface_with_scrollbars(20, 10, false);
    surface.right_click_passthrough = Some(KeyModifiers::ALT);
    surface.update(row_of(&terminals(1)));
    let mut input = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1006h").remove(0);
    surface.act(KeybindAction::EnterResizeMode, false);
    frame(&mut surface, &HashSet::new());
    reports(&mut input);
    with_alt(&mut surface, MouseEventKind::Down(MouseButton::Right), 3, 9);
    assert!(reports(&mut input).is_empty());

    surface.resize_key(&TerminalKey::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!surface.resizing());
    with_alt(&mut surface, MouseEventKind::Down(MouseButton::Right), 3, 9);
    assert!(reports(&mut input).is_empty());

    surface.enter_copy_mode();
    frame(&mut surface, &HashSet::new());
    with_alt(&mut surface, MouseEventKind::Down(MouseButton::Right), 3, 9);
    assert!(reports(&mut input).is_empty());
    assert!(!surface.dialog_open());

    let mut surface = plain(120, 30);
    surface.right_click_passthrough = Some(KeyModifiers::ALT);
    surface.update(spaces(&[(None, &[None])]));
    let mut input = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1006h").remove(0);
    surface.act(KeybindAction::WorkspacePicker, false);
    frame(&mut surface, &HashSet::new());
    reports(&mut input);
    for leave in [false, true] {
        if leave {
            surface.leave_navigate();
        }
        with_alt(
            &mut surface,
            MouseEventKind::Down(MouseButton::Right),
            40,
            29,
        );
        assert!(reports(&mut input).is_empty(), "{leave}");
        assert!(!surface.dialog_open(), "{leave}");
    }
    frame(&mut surface, &HashSet::new());
    with_alt(
        &mut surface,
        MouseEventKind::Down(MouseButton::Right),
        40,
        29,
    );
    assert!(!reports(&mut input).is_empty());
}

fn asked(surface: &mut Surface) -> bool {
    Screen::of(surface)
        .lines(0..30)
        .iter()
        .any(|line| line.contains("Close workspace?"))
}

fn held(surface: &mut Surface) -> Option<Operation> {
    ran(surface.dialog_key(
        &TerminalKey::new(KeyCode::Enter, KeyModifiers::NONE).with_kind(KeyEventKind::Repeat),
    ))
}

fn parked(terminal: &str) -> Layout {
    let mut layout = two_spaces();
    layout.parked.insert(terminal.to_owned());
    layout
}

#[tokio::test]
async fn a_held_enter_opens_the_close_confirmation_from_the_menu_and_never_answers_it() {
    let mut surface = spread_out().confirming_close(true);
    let x = find(&surface, 3, "side");
    right(&mut surface, x, 3);
    frame(&mut surface, &HashSet::new());
    key(&mut surface, KeyCode::Down);
    assert_eq!(key(&mut surface, KeyCode::Enter), None);
    assert_eq!(held(&mut surface), None);
    assert_eq!(held(&mut surface), None);
    assert!(asked(&mut surface));
    assert_eq!(typed(&mut surface, b"\x1b[13;1:3u"), None);
    assert_eq!(key(&mut surface, KeyCode::Enter), panes(&["t3"]));
}

fn typed(surface: &mut Surface, bytes: &[u8]) -> Option<Operation> {
    let events = crate::raw_input::parse_framed_input(bytes);
    let [crate::raw_input::RawInputEvent::Key(key)] = events.as_slice() else {
        panic!("{events:?}");
    };
    ran(surface.dialog_key(key))
}

fn closing() -> Surface {
    let mut surface = spread_out().confirming_close(true);
    let x = find(&surface, 3, "side");
    right(&mut surface, x, 3);
    frame(&mut surface, &HashSet::new());
    key(&mut surface, KeyCode::Down);
    surface
}

#[tokio::test]
async fn an_enter_the_host_sent_as_a_bare_return_answers_a_confirmation_one_opened_only_after_another_key(
) {
    let mut surface = closing();

    assert_eq!(typed(&mut surface, b"\r"), None);
    assert_eq!(typed(&mut surface, b"\r"), None);
    assert_eq!(typed(&mut surface, b"\r"), None);
    assert!(asked(&mut surface));

    assert_eq!(key(&mut surface, KeyCode::Down), None);
    assert!(asked(&mut surface));
    assert_eq!(typed(&mut surface, b"\r"), panes(&["t3"]));
}

#[tokio::test]
async fn an_enter_a_host_without_the_kitty_protocol_sent_as_an_escape_sequence_is_held_back_the_same_way(
) {
    for enter in [&b"\x1b[27;2;13~"[..], b"\x1b[13;2u"] {
        let mut surface = closing();

        assert_eq!(typed(&mut surface, enter), None);
        assert_eq!(typed(&mut surface, enter), None);
        assert!(asked(&mut surface));

        assert_eq!(key(&mut surface, KeyCode::Down), None);
        assert_eq!(typed(&mut surface, enter), panes(&["t3"]));
    }
}

#[tokio::test]
async fn an_enter_answers_a_confirmation_enter_opened_once_that_enter_is_released() {
    for opener in [&b"\r"[..], b"\x1b[13u"] {
        let mut surface = closing();

        assert_eq!(typed(&mut surface, opener), None);
        assert_eq!(typed(&mut surface, b"\x1b[13u"), None);
        assert_eq!(typed(&mut surface, b"\x1b[13;1:1u"), None);
        assert_eq!(typed(&mut surface, b"\x1b[13;1:2u"), None);
        assert_eq!(typed(&mut surface, b"\x1b[57441;2u"), None);
        assert_eq!(typed(&mut surface, b"\x1b[57441;1:3u"), None);
        assert_eq!(typed(&mut surface, b"\x1b[120;1:2u"), None);
        assert_eq!(typed(&mut surface, b"\x1b[57358u"), None);
        assert_eq!(typed(&mut surface, b"\x1b[13u"), None);
        assert!(asked(&mut surface));

        assert_eq!(typed(&mut surface, b"\x1b[13;1:3u"), None);
        assert!(asked(&mut surface));
        assert_eq!(typed(&mut surface, b"\x1b[13u"), panes(&["t3"]));
    }
}

#[tokio::test]
async fn the_release_of_the_other_enter_key_does_not_let_a_held_enter_answer() {
    let main: [&[u8]; 2] = [b"\x1b[13u", b"\x1b[13;1:3u"];
    let keypad: [&[u8]; 2] = [b"\x1b[57414u", b"\x1b[57414;1:3u"];
    for ([press, release], [_, other]) in [(main, keypad), (keypad, main)] {
        let mut surface = closing();

        assert_eq!(typed(&mut surface, press), None);
        assert_eq!(typed(&mut surface, other), None);
        assert_eq!(typed(&mut surface, press), None);
        assert!(asked(&mut surface));

        assert_eq!(typed(&mut surface, release), None);
        assert_eq!(typed(&mut surface, press), panes(&["t3"]));
    }
}

#[tokio::test]
async fn an_enter_key_pressed_while_the_other_is_held_is_held_too() {
    let mut surface = closing();

    assert_eq!(typed(&mut surface, b"\x1b[13u"), None);
    assert_eq!(typed(&mut surface, b"\x1b[57414u"), None);
    assert_eq!(typed(&mut surface, b"\x1b[13;1:3u"), None);
    assert_eq!(typed(&mut surface, b"\x1b[57414u"), None);
    assert!(asked(&mut surface));

    assert_eq!(typed(&mut surface, b"\x1b[57414;1:3u"), None);
    assert_eq!(typed(&mut surface, b"\x1b[57414u"), panes(&["t3"]));
}

#[tokio::test]
async fn a_release_the_host_sent_as_it_lost_focus_does_not_let_a_held_enter_answer() {
    let mut surface = closing();

    assert_eq!(typed(&mut surface, b"\x1b[13u"), None);
    assert_eq!(typed(&mut surface, b"\x1b[13;1:3u"), None);
    surface.host_focus(false);
    surface.host_focus(true);
    assert_eq!(typed(&mut surface, b"\x1b[13u"), None);
    assert!(asked(&mut surface));

    assert_eq!(typed(&mut surface, b"\x1b[13;1:3u"), None);
    assert_eq!(typed(&mut surface, b"\x1b[13u"), panes(&["t3"]));
}

#[tokio::test]
async fn a_kitty_host_is_asked_for_every_key_from_the_menu_until_the_confirmation_closes() {
    let mut surface = spread_out().confirming_close(true).on_a_kitty_host(true);
    assert!(!surface.wants_all_keys());
    let x = find(&surface, 3, "side");
    right(&mut surface, x, 3);
    frame(&mut surface, &HashSet::new());
    assert!(surface.wants_all_keys());

    key(&mut surface, KeyCode::Down);
    typed(&mut surface, b"\x1b[13u");
    assert!(asked(&mut surface));
    assert!(surface.wants_all_keys());

    key(&mut surface, KeyCode::Esc);
    assert!(!surface.wants_all_keys());
    assert!(!closing().wants_all_keys());
    surface.act(KeybindAction::RenameWorkspace, false);
    assert!(!surface.wants_all_keys());
    let mut surface = spread_out().on_a_kitty_host(true);
    launch(&mut surface);
    assert!(surface.dialog_open());
    assert!(!surface.wants_all_keys());
}

#[tokio::test]
async fn enter_picks_only_an_item_the_menu_drew_and_down_stops_at_the_last_drawn_one() {
    let mut surface = plain(120, 8);
    surface.update(two_spaces());
    frame(&mut surface, &HashSet::new());
    let other = pane(&surface, "t1");
    let (x, y) = (other.x + 1, other.y + 1);
    right(&mut surface, x, y);
    let shown = listed(&mut surface, x, 0);
    assert!(shown.len() < PANE_ITEMS.len(), "{shown:?}");
    for _ in 0..PANE_ITEMS.len() {
        key(&mut surface, KeyCode::Down);
    }
    let mut last = vec![false; shown.len()];
    last[shown.len() - 1] = true;
    assert_eq!(highlighted(&mut surface, x, 0), last);
    assert_ne!(key(&mut surface, KeyCode::Enter), panes(&["t1"]));
}

#[tokio::test]
async fn down_and_enter_typed_ahead_of_a_frame_pick_the_item_they_reach() {
    let mut surface = spread_out();
    let x = find(&surface, 0, "logs");
    right(&mut surface, x, 0);
    frame(&mut surface, &HashSet::new());
    key(&mut surface, KeyCode::Down);
    key(&mut surface, KeyCode::Down);
    assert_eq!(key(&mut surface, KeyCode::Enter), panes(&["t2"]));
}

#[tokio::test]
async fn a_menu_closes_when_its_target_leaves_the_screen_and_stays_while_it_remains() {
    let mut surface = spread_out();
    let other = pane(&surface, "t1");
    right(&mut surface, other.x + 1, other.y + 1);
    surface.update(two_spaces());
    assert!(surface.dialog_open());
    surface.update(parked("t1"));
    assert!(!surface.dialog_open());

    surface.update(two_spaces());
    let x = find(&surface, 0, "logs");
    right(&mut surface, x, 0);
    let moved: Layout = serde_json::from_value(json!({
        "workspaces": [
            {"id": "w0", "name": "main", "tabs": [
                {"id": "tab0", "root": {"kind": "pane", "terminal": "t1"}},
                {"id": "tab1", "name": "logs", "root": {"kind": "pane", "terminal": "t2"}},
            ]},
            {"id": "w1", "name": "side", "tabs": [
                {"id": "tab2", "root": balanced(&["t3".to_owned(), "t0".to_owned()])},
            ]},
        ],
    }))
    .expect("layout");
    surface.update(moved);
    assert_eq!(workspace(&surface), "w1");
    assert!(!surface.dialog_open());
}

fn starting(surface: &mut Surface) -> u64 {
    let Some(Operation::Start(request, _)) = surface.act(KeybindAction::SplitVertical, true) else {
        panic!("a split starts");
    };
    request
}

#[tokio::test]
async fn a_pane_menu_closes_when_a_creation_reveals_another_tab() {
    let mut surface = spread_out();
    let request = starting(&mut surface);
    let other = pane(&surface, "t1");
    right(&mut surface, other.x + 1, other.y + 1);
    surface.started(request, Some("t2".to_owned()));
    assert_eq!(tab(&surface), "tab1");
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn swap_acts_with_the_pane_the_menu_showed_as_focused_or_not_at_all() {
    let mut surface = plain(120, 30);
    let mut layout = two_spaces();
    layout.workspaces[0].tabs[0].root = serde_json::from_value(balanced(&[
        "t0".to_owned(),
        "t1".to_owned(),
        "t4".to_owned(),
    ]))
    .expect("root");
    surface.update(layout);
    let request = starting(&mut surface);
    frame(&mut surface, &HashSet::new());
    let other = pane(&surface, "t1");
    let (x, y) = (other.x + 1, other.y + 1);
    right(&mut surface, x, y);
    let row = y
        + 1
        + listed(&mut surface, x, y)
            .iter()
            .position(|item| item == "Swap with focused pane")
            .expect("swap") as u16;
    surface.session.focus("t4");
    assert_eq!(
        point(
            &mut surface,
            MouseEventKind::Down(MouseButton::Left),
            x + 2,
            row
        ),
        None
    );

    surface.session.focus("t0");
    right(&mut surface, x, y);
    surface.started(request, Some("t4".to_owned()));
    assert!(surface.needs_frame());
}

#[tokio::test]
async fn a_zoomed_pane_offers_no_move() {
    let mut surface = spread_out();
    surface.act(KeybindAction::Zoom, false);
    frame(&mut surface, &HashSet::new());
    let zoomed = pane(&surface, "t0");
    right(&mut surface, zoomed.x + 1, zoomed.y + 1);
    let mut items = PANE_ITEMS.to_vec();
    items.retain(|item| !["Swap with focused pane", "Move to..."].contains(item));
    assert_eq!(listed(&mut surface, zoomed.x + 1, zoomed.y + 1), items);
}

#[tokio::test]
async fn a_right_click_in_copy_or_resize_mode_opens_no_menu() {
    let mut surface = spread_out();
    let x = find(&surface, 0, "logs");
    let other = pane(&surface, "t1");
    surface.enter_copy_mode();
    assert!(surface.copy_mode());
    right(&mut surface, x, 0);
    right(&mut surface, other.x + 1, other.y + 1);
    assert!(!surface.dialog_open());

    let mut surface = spread_out();
    surface.act(KeybindAction::EnterResizeMode, false);
    right(&mut surface, x, 0);
    right(&mut surface, other.x + 1, other.y + 1);
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn a_right_press_while_a_tab_is_held_reaches_neither_a_menu_nor_the_pane() {
    let mut surface = plain(120, 30);
    surface.update(two_spaces());
    let mut inputs = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1006h");
    frame(&mut surface, &HashSet::new());
    for input in inputs.iter_mut() {
        reports(input);
    }
    let x = find(&surface, 0, "logs");
    let other = pane(&surface, "t1");
    point(&mut surface, MouseEventKind::Down(MouseButton::Left), x, 0);
    point(
        &mut surface,
        MouseEventKind::Drag(MouseButton::Left),
        other.x + 1,
        other.y + 1,
    );
    point(
        &mut surface,
        MouseEventKind::Down(MouseButton::Right),
        other.x + 1,
        other.y + 1,
    );
    assert!(!surface.dialog_open());
    for input in inputs.iter_mut() {
        assert_eq!(reports(input), b"");
    }
}

#[tokio::test]
async fn a_split_from_a_pane_menu_starts_in_the_reported_folder_of_the_clicked_pane() {
    let mut surface = spread_out();
    let now = std::time::Instant::now();
    surface.heard(
        Report::Snapshot {
            folders: [("t0".to_owned(), "/srv/focused".into())]
                .into_iter()
                .collect(),
            statuses: Vec::new(),
        },
        now,
    );
    surface.heard(Report::Folder("t1".to_owned(), "/srv/clicked".into()), now);
    let other = pane(&surface, "t1");
    right(&mut surface, other.x + 1, other.y + 1);
    let split = choose(&mut surface, other.x + 1, other.y + 1, "Split right").expect("split");
    assert_eq!(surface.folder(&split), Some("/srv/clicked".into()));
    assert_eq!(
        surface.folder(&Operation::Park("t1".to_owned())),
        Some("/srv/focused".into())
    );
}

#[tokio::test]
async fn a_folder_report_leaves_an_open_menu_as_drawn() {
    let mut surface = spread_out();
    let other = pane(&surface, "t1");
    right(&mut surface, other.x + 1, other.y + 1);
    frame(&mut surface, &HashSet::new());
    surface.heard(
        Report::Folder("t2".to_owned(), "/srv/hidden".into()),
        std::time::Instant::now(),
    );
    assert!(!surface.needs_frame());
}

#[tokio::test]
async fn an_item_a_shorter_screen_no_longer_draws_is_not_picked_by_enter() {
    let mut surface = spread_out();
    let other = pane(&surface, "t1");
    right(&mut surface, other.x + 1, other.y + 1);
    frame(&mut surface, &HashSet::new());
    for _ in 0..PANE_ITEMS.len() {
        key(&mut surface, KeyCode::Down);
    }
    surface.resize(host(120, 8));
    frame(&mut surface, &HashSet::new());
    assert!(surface.dialog_open());
    assert_eq!(key(&mut surface, KeyCode::Enter), None);
}

#[tokio::test]
async fn a_right_press_on_a_target_an_unpainted_update_took_away_opens_no_menu() {
    let mut surface = spread_out();
    let other = pane(&surface, "t1");
    surface.update(parked("t1"));
    point(
        &mut surface,
        MouseEventKind::Down(MouseButton::Right),
        other.x + 1,
        other.y + 1,
    );
    assert!(!surface.dialog_open());

    let mut surface = spread_out();
    let x = find(&surface, 0, "logs");
    let mut layout = two_spaces();
    let logs = layout.workspaces[0].tabs.remove(1);
    layout.workspaces[1].tabs.push(logs);
    surface.update(layout);
    point(&mut surface, MouseEventKind::Down(MouseButton::Right), x, 0);
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn enter_picks_nothing_when_the_menu_is_too_narrow_to_draw_a_label() {
    let mut surface = spread_out();
    let other = pane(&surface, "t1");
    right(&mut surface, other.x + 1, other.y + 1);
    frame(&mut surface, &HashSet::new());
    surface.resize(host(2, 30));
    frame(&mut surface, &HashSet::new());
    assert!(surface.dialog_open());
    assert_eq!(key(&mut surface, KeyCode::Enter), None);
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn a_right_press_on_the_old_tile_of_a_pane_a_zoom_hid_opens_no_menu() {
    let mut surface = spread_out();
    let other = pane(&surface, "t1");
    surface.act(KeybindAction::Zoom, false);
    point(
        &mut surface,
        MouseEventKind::Down(MouseButton::Right),
        other.x + 1,
        other.y + 1,
    );
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn an_open_pane_menu_closes_when_a_zoom_hides_its_pane() {
    let mut surface = spread_out();
    let other = pane(&surface, "t1");
    right(&mut surface, other.x + 1, other.y + 1);
    surface.act(KeybindAction::Zoom, false);
    assert!(!surface.dialog_open());

    let focused = pane(&surface, "t0");
    surface.act(KeybindAction::Zoom, false);
    right(&mut surface, focused.x + 1, focused.y + 1);
    surface.act(KeybindAction::Zoom, false);
    assert!(surface.dialog_open());
}

fn launcher(surface: &Surface) -> (u16, u16) {
    let footer = surface.host.rows - 2;
    (find(surface, footer, "menu"), footer)
}

fn launch(surface: &mut Surface) -> Option<Pointed> {
    let (x, y) = launcher(surface);
    launch_at(surface, x, y)
}

fn launch_at(surface: &mut Surface, x: u16, y: u16) -> Option<Pointed> {
    surface.mouse(
        mouse_at(MouseEventKind::Down(MouseButton::Left), x, y),
        std::time::Instant::now(),
    )
}

fn launched(surface: &mut Surface) -> Vec<String> {
    let (x, y) = launcher(surface);
    listed(surface, x + 4 - 17, y - 6)
}

fn pick(surface: &mut Surface, label: &str) -> Option<Pointed> {
    let (x, y) = launcher(surface);
    let (left, top) = (x + 4 - 17, y - 6);
    let index = launched(surface)
        .iter()
        .position(|item| item == label)
        .expect(label);
    surface.mouse(
        mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            left + 2,
            top + 1 + index as u16,
        ),
        std::time::Instant::now(),
    )
}

#[tokio::test]
async fn the_menu_launcher_in_the_sidebar_footer_lists_settings_keybinds_reload_config_and_detach()
{
    let mut surface = spread_out();
    assert!(line(&surface, 28).starts_with(" new "));
    let (x, y) = launcher(&surface);
    assert_eq!(x + 4, surface.areas.sidebar.width - 1);
    for kind in [MouseEventKind::Moved, MouseEventKind::Up(MouseButton::Left)] {
        point(&mut surface, kind, x, y);
        assert!(!surface.dialog_open(), "{kind:?}");
    }

    assert_eq!(launch(&mut surface), None);

    assert!(surface.dialog_open());
    assert_eq!(
        launched(&mut surface),
        ["settings", "keybinds", "reload config", "detach"]
    );
}

#[tokio::test]
async fn each_global_item_hands_its_action_to_the_client_and_closes_the_menu() {
    for (label, action) in [
        ("settings", KeybindAction::Settings),
        ("keybinds", KeybindAction::Help),
        ("reload config", KeybindAction::ReloadConfig),
        ("detach", KeybindAction::Detach),
    ] {
        let mut surface = spread_out();
        launch(&mut surface);

        assert_eq!(pick(&mut surface, label), Some(Pointed::Act(action)));
        assert!(!surface.dialog_open(), "{label}");
    }
}

#[tokio::test]
async fn keys_move_through_the_global_menu_and_a_held_enter_picks_nothing() {
    let mut surface = spread_out();
    launch(&mut surface);
    launched(&mut surface);

    assert_eq!(key(&mut surface, KeyCode::Down), None);
    assert_eq!(key(&mut surface, KeyCode::Down), None);
    assert_eq!(held(&mut surface), None);
    assert!(surface.dialog_open());
    assert_eq!(
        surface.dialog_key(&TerminalKey::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(Pointed::Act(KeybindAction::ReloadConfig))
    );
}

#[tokio::test]
async fn clicking_the_launcher_while_the_menu_is_open_closes_it() {
    let mut surface = spread_out();
    launch(&mut surface);
    launched(&mut surface);

    assert_eq!(launch(&mut surface), None);

    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn the_launcher_opens_nothing_in_copy_or_resize_mode() {
    let mut surface = spread_out();
    surface.enter_copy_mode();
    assert_eq!(launch(&mut surface), None);
    assert!(!surface.dialog_open());

    let mut surface = spread_out();
    surface.act(KeybindAction::EnterResizeMode, false);
    assert_eq!(launch(&mut surface), None);
    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn j_and_k_move_through_the_global_menu_like_the_arrows() {
    let mut surface = spread_out();
    launch(&mut surface);
    launched(&mut surface);

    assert_eq!(key(&mut surface, KeyCode::Char('j')), None);
    assert_eq!(key(&mut surface, KeyCode::Char('j')), None);
    assert_eq!(key(&mut surface, KeyCode::Char('j')), None);
    assert_eq!(key(&mut surface, KeyCode::Char('k')), None);
    Screen::of(&mut surface);

    assert_eq!(
        surface.dialog_key(&TerminalKey::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(Pointed::Act(KeybindAction::ReloadConfig))
    );
}

#[tokio::test]
async fn a_shorter_terminal_redraws_the_open_menu_above_the_launcher_it_moved() {
    let mut surface = spread_out();
    launch(&mut surface);
    launched(&mut surface);

    surface.resize(host(120, 25));
    frame(&mut surface, &HashSet::new());

    assert_eq!(launcher(&surface).1, 23);
    assert_eq!(
        launched(&mut surface),
        ["settings", "keybinds", "reload config", "detach"]
    );
}

#[tokio::test]
async fn a_reload_that_widens_the_sidebar_redraws_the_open_menu_above_the_moved_launcher() {
    let mut surface = spread_out();
    launch(&mut surface);
    let before = launcher(&surface);
    let mut screen = Screen::of(&mut surface);

    surface.configure(&toml::from_str("[ui]\nsidebar_width = 34").expect("config"));
    screen.show(&mut surface, &HashSet::new());

    let (x, y) = launcher(&surface);
    assert_ne!((x, y), before);
    let (left, top) = (x + 4 - 17, y - 6);
    assert_eq!(screen.cell(left, top).symbol, "┌");
    assert!(screen.span(top + 4, left..left + 17).contains("detach"));
}

#[tokio::test]
async fn the_open_menu_closes_when_the_launcher_goes_away() {
    let mut surface = spread_out();
    launch(&mut surface);
    launched(&mut surface);

    surface.resize(host(64, 30));
    Screen::of(&mut surface);

    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn on_a_short_terminal_a_second_press_on_the_launcher_closes_the_menu_over_it() {
    let mut surface = plain(120, 5);
    surface.update(two_spaces());
    frame(&mut surface, &HashSet::new());
    launch(&mut surface);
    let (_, y) = launcher(&surface);
    assert!(Screen::of(&mut surface)
        .span(y, 0..120)
        .contains("reload config"));

    assert_eq!(launch(&mut surface), None);

    assert!(!surface.dialog_open());
}

#[tokio::test]
async fn a_press_on_the_launcher_clears_the_pane_selection() {
    let mut surface = plain(120, 30);
    surface.select = crate::core_client::tui::select::Select::new(false);
    surface.update(spaces(&[(Some("main"), &[None])]));
    attach_with_input(&mut surface, b"hello world");
    frame(&mut surface, &HashSet::new());
    let id = surface.session.focused_id().expect("focused");
    let (_, inner) =
        crate::core_client::tui::surface::visible(&surface.views, id).expect("visible");
    point(
        &mut surface,
        MouseEventKind::Down(MouseButton::Left),
        inner.x,
        inner.y,
    );
    point(
        &mut surface,
        MouseEventKind::Drag(MouseButton::Left),
        inner.x + 4,
        inner.y,
    );
    point(
        &mut surface,
        MouseEventKind::Up(MouseButton::Left),
        inner.x + 4,
        inner.y,
    );
    assert!(surface.select.shown().is_some());

    launch(&mut surface);

    assert!(surface.dialog_open());
    assert!(surface.select.shown().is_none());
}

#[tokio::test]
async fn in_copy_or_resize_mode_a_press_on_the_launcher_over_new_does_nothing() {
    for resizing in [false, true] {
        let mut surface = dressed(120, 30, |config| {
            config.ui.sidebar_width = 8;
            config.ui.sidebar_min_width = 8;
            config.ui.sidebar_max_width = 8;
        });
        surface.update(two_spaces());
        frame(&mut surface, &HashSet::new());
        match resizing {
            true => {
                surface.act(KeybindAction::EnterResizeMode, false);
            }
            false => surface.enter_copy_mode(),
        }
        let (x, y) = launcher(&surface);
        let workspaces = surface.session.workspaces().len();

        for column in x..x + 2 {
            assert_eq!(launch_at(&mut surface, column, y), None, "{resizing}");
        }

        assert!(!surface.dialog_open());
        assert_eq!(surface.session.workspaces().len(), workspaces);
    }
}

#[tokio::test]
async fn a_global_menu_squeezed_to_its_leading_space_picks_nothing() {
    let mut surface = dressed(120, 10, |config| config.ui.mobile_width_threshold = 0);
    surface.update(two_spaces());
    frame(&mut surface, &HashSet::new());
    launch(&mut surface);
    Screen::of(&mut surface);

    surface.resize(host(3, 10));
    Screen::of(&mut surface);
    assert!(surface.dialog_open());

    assert_eq!(key(&mut surface, KeyCode::Down), None);
    assert_eq!(key(&mut surface, KeyCode::Down), None);
    assert_eq!(
        surface.dialog_key(&TerminalKey::new(KeyCode::Enter, KeyModifiers::NONE)),
        None
    );
}

#[tokio::test]
async fn a_context_menu_squeezed_to_one_column_still_picks_the_label_it_drew() {
    let mut surface = dressed(120, 10, |config| config.ui.mobile_width_threshold = 0);
    surface.update(two_spaces());
    frame(&mut surface, &HashSet::new());
    let x = find(&surface, 0, "logs");
    right(&mut surface, x, 0);
    Screen::of(&mut surface);

    surface.resize(host(3, 10));
    let screen = Screen::of(&mut surface);
    assert!(surface.dialog_open());
    assert_eq!(screen.cell(1, 1).symbol, "N");

    assert!(key(&mut surface, KeyCode::Enter).is_some());
}
