use super::*;
use crate::core_client::tui::keys::Navigation;

fn three() -> Layout {
    spaces(&[
        (Some("main"), &[None]),
        (Some("other"), &[None, None]),
        (Some("third"), &[None]),
    ])
}

fn navigating(mut surface: Surface, layout: Layout) -> Surface {
    surface.update(layout);
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.act(KeybindAction::WorkspacePicker, false), None);
    assert!(surface.navigating());
    surface
}

#[tokio::test]
async fn the_workspace_picker_key_enters_navigate_mode_with_the_focused_workspace_selected() {
    let mut surface = plain(120, 30);
    surface.update(three());
    surface.act(KeybindAction::NextWorkspace, false);
    frame(&mut surface, &HashSet::new());

    assert_eq!(surface.act(KeybindAction::WorkspacePicker, false), None);

    assert!(surface.navigating());
    assert_eq!(surface.outer.selected(), Some("w1"));
    assert!(surface.needs_frame());
}

#[tokio::test]
async fn moving_the_selection_wraps_at_either_end_and_leaves_focus_alone() {
    let mut surface = navigating(plain(120, 30), three());

    surface.navigate(Navigation::Move(-1));
    assert_eq!(surface.outer.selected(), Some("w2"));
    surface.navigate(Navigation::Move(1));
    assert_eq!(surface.outer.selected(), Some("w0"));
    surface.navigate(Navigation::Move(1));
    assert_eq!(surface.outer.selected(), Some("w1"));
    assert_eq!(workspace(&surface), "w0");
    assert!(surface.navigating());
}

#[tokio::test]
async fn accepting_switches_to_the_selected_workspace_and_leaves_navigate_mode() {
    let mut surface = navigating(plain(120, 30), three());
    surface.navigate(Navigation::Move(1));

    let action = surface.navigate(Navigation::Accept);

    assert_eq!(action, Some(KeybindAction::SwitchWorkspace(1)));
    assert!(!surface.navigating());
    surface.act(KeybindAction::SwitchWorkspace(1), false);
    assert_eq!(workspace(&surface), "w1");
}

#[tokio::test]
async fn accepting_with_no_workspace_leaves_navigate_mode() {
    let mut surface = navigating(plain(120, 30), spaces(&[]));

    assert_eq!(surface.navigate(Navigation::Accept), None);
    assert!(!surface.navigating());
}

#[tokio::test]
async fn a_jump_to_a_workspace_or_tab_that_does_not_exist_stays_in_navigate_mode() {
    let mut surface = navigating(plain(120, 30), three());

    for missing in [
        KeybindAction::SwitchWorkspace(3),
        KeybindAction::SwitchTab(1),
    ] {
        assert_eq!(surface.navigate(Navigation::Jump(missing)), None);
        assert!(surface.navigating(), "{missing:?}");
    }

    let there = KeybindAction::SwitchWorkspace(2);
    assert_eq!(surface.navigate(Navigation::Jump(there)), Some(there));
    assert!(!surface.navigating());
}

#[tokio::test]
async fn closing_a_workspace_in_navigate_mode_closes_the_selected_one() {
    let mut surface = navigating(plain(120, 30), three());
    surface.navigate(Navigation::Move(1));

    assert_eq!(
        surface.act(KeybindAction::CloseWorkspace, false),
        Some(Operation::Close(vec!["t2".to_owned(), "t3".to_owned()]))
    );

    surface.leave_navigate();
    assert_eq!(
        surface.act(KeybindAction::CloseWorkspace, false),
        Some(Operation::Close(vec!["t1".to_owned()]))
    );
}

#[tokio::test]
async fn the_sidebar_marks_the_selected_workspace_apart_from_the_focused_one() {
    let palette = Palette::catppuccin();
    let selection = background(crate::ui::chrome::workspace_selection_background(&palette));
    let active = background(palette.active_row_bg);
    let mut surface = navigating(plain(120, 30), three());
    surface.navigate(Navigation::Move(1));
    frame(&mut surface, &HashSet::new());

    assert_eq!(cell(&surface, 1, 2).bg, active);
    assert_eq!(cell(&surface, 1, 3).bg, selection);
    assert_ne!(cell(&surface, 1, 4).bg, selection);

    surface.leave_navigate();
    frame(&mut surface, &HashSet::new());
    assert_ne!(cell(&surface, 1, 3).bg, selection);
}

#[tokio::test]
async fn a_collapsed_sidebar_marks_the_selected_workspace_number() {
    let palette = Palette::catppuccin();
    let selection = background(crate::ui::chrome::workspace_selection_background(&palette));
    let surface = dressed(120, 30, |config| config.ui.sidebar_start_collapsed = true);
    let mut surface = navigating(surface, three());
    surface.navigate(Navigation::Move(1));
    frame(&mut surface, &HashSet::new());

    assert!(line(&surface, 1).starts_with("2  │"));
    assert_eq!(cell(&surface, 0, 1).bg, selection);
    assert_eq!(
        cell(&surface, 0, 1).fg,
        styled(Style::default().fg(palette.overlay1)).fg
    );
    assert_eq!(cell(&surface, 0, 0).bg, background(palette.active_row_bg));
}

#[tokio::test]
async fn an_offscreen_selection_scrolls_the_workspace_list_into_view() {
    let mut surface = navigating(plain(120, 12), many(12));

    surface.navigate(Navigation::Move(-1));
    frame(&mut surface, &HashSet::new());

    let shown: Vec<String> = (0..12).map(|y| line(&surface, y)).collect();
    assert!(
        shown.iter().any(|row| row.starts_with(" ws11 ")),
        "{shown:#?}"
    );
    assert_eq!(workspace(&surface), "w0");
}

#[tokio::test]
async fn entering_navigate_mode_scrolls_the_focused_workspace_back_into_the_sidebar() {
    let mut surface = plain(120, 12);
    surface.update(many(12));
    frame(&mut surface, &HashSet::new());
    surface.act(KeybindAction::SwitchWorkspace(11), false);
    frame(&mut surface, &HashSet::new());
    for _ in 0..12 {
        wheel(&mut surface, false, 5, 3);
    }
    frame(&mut surface, &HashSet::new());
    let shown = |surface: &Surface| (0..12).any(|y| line(surface, y).starts_with(" ws11 "));
    assert!(!shown(&surface));

    surface.act(KeybindAction::WorkspacePicker, false);
    frame(&mut surface, &HashSet::new());

    assert!(shown(&surface));
}

#[tokio::test]
async fn a_collapsed_sidebar_scrolls_to_show_a_selection_past_its_last_row() {
    let surface = dressed(120, 6, |config| config.ui.sidebar_start_collapsed = true);
    let mut surface = navigating(surface, many(12));

    surface.navigate(Navigation::Move(-1));
    frame(&mut surface, &HashSet::new());

    let shown: Vec<String> = (0..6).map(|y| line(&surface, y)).collect();
    assert!(shown.iter().any(|row| row.starts_with("12 ")), "{shown:#?}");
}

#[tokio::test]
async fn the_navigate_bar_covers_the_bottom_tab_bar_or_the_last_pane_row_and_goes_on_leaving() {
    for position in [TabBarPositionConfig::Bottom, TabBarPositionConfig::Top] {
        let surface = dressed(120, 30, |config| config.ui.tab_bar_position = position);
        let mut surface = navigating(surface, three());
        let mut screen = Screen::of(&mut surface);

        assert_eq!(
            surface.outer.bar().map(|bar| bar.0),
            Some(Rect::new(26, 29, 94, 1)),
            "{position:?}"
        );
        assert_eq!(
            screen.span(29, 26..120).trim_end(),
            " NAVIGATE  esc back  ↑/↓ workspace  tab pane  ? keybinds"
        );
        assert!(line(&surface, 0).starts_with(" spaces"));

        surface.leave_navigate();
        screen.show(&mut surface, &HashSet::new());
        assert_eq!(screen.line(29), line(&surface, 29));
    }
}

#[tokio::test]
async fn the_cursor_is_hidden_on_the_row_under_the_navigate_bar() {
    let mut surface = dressed(120, 5, |config| {
        config.ui.tab_bar_position = TabBarPositionConfig::Top;
    });
    surface.update(spaces(&[(None, &[None])]));
    frame_attaching(&mut surface, &HashSet::new());
    let dirty = HashSet::from([id_of(&surface, "t1")]);
    surface.resize(host(120, 2));
    frame(&mut surface, &dirty);
    feed(&surface, "t1", b"\x1b[1;3H\x1b[?25h");
    frame(&mut surface, &dirty);
    let shown = surface.cursor().map(|cursor| (cursor.x, cursor.y));
    assert_eq!(shown, Some((28, 1)));

    surface.act(KeybindAction::WorkspacePicker, false);
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.cursor().map(|cursor| (cursor.x, cursor.y)), None);

    surface.leave_navigate();
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.cursor().map(|cursor| (cursor.x, cursor.y)), shown);
}

#[tokio::test]
async fn the_cursor_stays_above_the_navigate_bar() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None])]));
    frame_attaching(&mut surface, &HashSet::new());
    feed(&surface, "t1", b"\x1b[1;3H\x1b[?25h");
    let dirty = HashSet::from([id_of(&surface, "t1")]);
    frame(&mut surface, &dirty);
    let shown = surface.cursor().map(|cursor| (cursor.x, cursor.y));
    assert!(shown.is_some());

    surface.act(KeybindAction::WorkspacePicker, false);
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.cursor().map(|cursor| (cursor.x, cursor.y)), shown);
}

#[tokio::test]
async fn the_navigate_bar_is_not_redrawn_when_nothing_it_shows_changed() {
    let mut surface = navigating(plain(120, 30), three());
    frame(&mut surface, &HashSet::new());
    let built = surface.outer.bar().map(|bar| bar.1.as_ptr());

    assert!(frame(&mut surface, &HashSet::new()).is_empty());
    surface.update(three());
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.outer.bar().map(|bar| bar.1.as_ptr()), built);

    surface.navigate(Navigation::Move(1));
    assert!(surface.needs_frame());
}

#[tokio::test]
async fn tabs_under_the_navigate_bar_take_no_clicks_or_wheel() {
    let surface = dressed(120, 30, |config| {
        config.ui.tab_bar_position = TabBarPositionConfig::Bottom;
    });
    let mut surface = navigating(surface, spaces(&[(None, &[None, None])]));
    let second = find(&surface, 29, " 2 ") + 1;
    frame(&mut surface, &HashSet::new());

    tap(&mut surface, second, 29);
    assert_eq!(tab(&surface), "tab1");
    wheel(&mut surface, true, second, 29);
    assert_eq!(tab(&surface), "tab1");
    assert!(surface.navigating());

    surface.leave_navigate();
    frame(&mut surface, &HashSet::new());
    tap(&mut surface, second, 29);
    assert_eq!(tab(&surface), "tab2");
}

#[tokio::test]
async fn closing_a_workspace_with_confirm_on_asks_about_the_selected_one_and_ends_navigate_mode() {
    let mut surface = navigating(plain(120, 30), three());
    surface.navigate(Navigation::Move(1));

    assert_eq!(surface.act(KeybindAction::CloseWorkspace, true), None);

    assert!(surface.dialog_open());
    assert!(!surface.navigating());
    let screen = Screen::of(&mut surface);
    assert!(
        (0..30).any(|y| screen.line(y).contains(" other — 2 panes")),
        "{:#?}",
        screen.lines(0..30)
    );
    assert_eq!(
        ran(surface.dialog_key(&TerminalKey::new(KeyCode::Enter, KeyModifiers::NONE))),
        Some(Operation::Close(vec!["t2".to_owned(), "t3".to_owned()]))
    );
    assert!(!surface.navigating());
}

#[tokio::test]
async fn renaming_a_workspace_in_navigate_mode_renames_the_selected_one_and_ends_navigate_mode() {
    let mut surface = navigating(plain(120, 30), three());
    surface.navigate(Navigation::Move(1));

    assert_eq!(surface.act(KeybindAction::RenameWorkspace, false), None);

    assert!(surface.dialog_open());
    assert!(!surface.navigating());
    assert_eq!(
        surface.dialog_key(&TerminalKey::new(KeyCode::Char('!'), KeyModifiers::NONE)),
        None
    );
    assert_eq!(
        ran(surface.dialog_key(&TerminalKey::new(KeyCode::Enter, KeyModifiers::NONE))),
        Some(Operation::Arrange(
            crate::session_doc::Arrangement::NameWorkspace {
                workspace: "w1".to_owned(),
                name: "other!".to_owned(),
            }
        ))
    );
}

#[tokio::test]
async fn the_help_key_in_navigate_mode_opens_help_and_closing_it_returns_to_the_panes() {
    let mut surface = navigating(plain(120, 30), three());

    assert_eq!(surface.act(KeybindAction::Help, false), None);
    assert!(surface.dialog_open());
    assert!(!surface.navigating());

    surface.dialog_key(&TerminalKey::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!surface.dialog_open());
    assert!(!surface.navigating());
}

#[tokio::test]
async fn the_resize_mode_key_in_navigate_mode_ends_navigate_mode_and_the_resize_bar_takes_its_row()
{
    let mut surface = navigating(plain(120, 30), three());
    frame(&mut surface, &HashSet::new());

    assert_eq!(surface.act(KeybindAction::EnterResizeMode, false), None);
    assert!(surface.resizing());
    assert!(!surface.navigating());
    let screen = Screen::of(&mut surface);
    assert_eq!(
        screen.span(29, 26..120).trim_end(),
        " RESIZE   h/l width  j/k height  esc done"
    );
}

#[tokio::test]
async fn navigate_mode_over_copy_mode_hides_the_copy_cursor_until_it_ends() {
    let mut surface = plain(120, 30);
    surface.update(three());
    frame_attaching(&mut surface, &HashSet::new());
    surface.enter_copy_mode();
    let cursor = surface.overlays().copy.cursor;
    assert!(!cursor.is_empty());

    surface.act(KeybindAction::WorkspacePicker, false);
    assert!(surface.copy_mode());
    assert!(surface.overlays().copy.cursor.is_empty());

    surface.leave_navigate();
    assert_eq!(surface.overlays().copy.cursor, cursor);
}

#[tokio::test]
async fn moving_up_then_down_before_a_frame_keeps_the_final_selection_in_view() {
    let mut surface = navigating(plain(120, 12), many(12));

    surface.navigate(Navigation::Move(-1));
    surface.navigate(Navigation::Move(1));
    frame(&mut surface, &HashSet::new());

    assert_eq!(surface.outer.selected(), Some("w0"));
    assert!(
        line(&surface, 2).starts_with(" ws0 "),
        "{:?}",
        line(&surface, 2)
    );
}

#[tokio::test]
async fn widening_with_the_last_workspace_selected_shows_it_in_the_sidebar() {
    let mut surface = plain(64, 12);
    surface.update(many(12));
    frame(&mut surface, &HashSet::new());
    tap(&mut surface, 58, 1);
    for _ in 0..11 {
        surface.navigate(Navigation::Move(1));
    }
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.outer.selected(), Some("w11"));

    surface.resize(host(120, 12));
    frame(&mut surface, &HashSet::new());

    let shown: Vec<String> = (0..12).map(|y| line(&surface, y)).collect();
    assert!(
        shown.iter().any(|row| row.starts_with(" ws11 ")),
        "{shown:#?}"
    );
}

#[tokio::test]
async fn a_selected_workspace_closed_by_an_update_vanishes_until_a_move_selects_another() {
    let mut surface = navigating(plain(120, 30), three());
    surface.navigate(Navigation::Move(1));
    assert!(!surface.selection_vanished());

    let mut closed = three();
    closed.workspaces.remove(1);
    surface.update(closed);
    frame(&mut surface, &HashSet::new());
    assert!(surface.navigating());
    assert!(surface.selection_vanished());
    assert_eq!(surface.navigate(Navigation::Accept), None);
    assert!(surface.navigating());

    surface.navigate(Navigation::Move(-1));
    assert!(!surface.selection_vanished());
    assert_eq!(surface.outer.selected(), Some("w2"));
}

#[tokio::test]
async fn a_resize_while_navigating_brings_a_selection_scrolled_out_of_the_sidebar_back_into_view() {
    let mut surface = navigating(plain(120, 12), many(12));
    surface.navigate(Navigation::Move(-1));
    frame(&mut surface, &HashSet::new());
    for _ in 0..12 {
        wheel(&mut surface, false, 2, 5);
    }
    frame(&mut surface, &HashSet::new());
    let shown =
        |surface: &Surface, rows: u16| (0..rows).any(|y| line(surface, y).starts_with(" ws11 "));
    assert!(!shown(&surface, 12));

    surface.resize(host(120, 14));
    frame(&mut surface, &HashSet::new());

    assert!(shown(&surface, 14));
}

fn compact(rows: u16) -> Surface {
    dressed(120, rows, |config| config.ui.sidebar_start_collapsed = true)
}

fn compact_rows(surface: &Surface, rows: u16) -> Vec<String> {
    (0..rows)
        .map(|y| line(surface, y).chars().take(2).collect::<String>())
        .collect()
}

fn shown_in(surface: &Surface, rows: u16, name: &str) -> bool {
    (0..rows).any(|y| line(surface, y).starts_with(&format!(" {name} ")))
}

#[tokio::test]
async fn the_wheel_scrolls_the_collapsed_list_one_row_at_a_time_within_what_was_drawn() {
    let mut surface = compact(6);
    surface.update(many(12));
    frame(&mut surface, &HashSet::new());
    let first = compact_rows(&surface, 1);

    wheel(&mut surface, false, 0, 1);
    frame(&mut surface, &HashSet::new());
    assert_eq!(compact_rows(&surface, 1), first);

    wheel(&mut surface, true, 0, 1);
    frame(&mut surface, &HashSet::new());
    assert_eq!(compact_rows(&surface, 1), ["2 "]);

    for _ in 0..20 {
        wheel(&mut surface, true, 0, 1);
    }
    frame(&mut surface, &HashSet::new());
    let bottom = compact_rows(&surface, 6);
    assert!(bottom.contains(&"12".to_owned()), "{bottom:?}");
    let top: usize = bottom[0].trim().parse().expect("number");

    for _ in 0..3 {
        wheel(&mut surface, true, 0, 1);
    }
    wheel(&mut surface, false, 0, 1);
    frame(&mut surface, &HashSet::new());
    let index: usize = compact_rows(&surface, 1)[0].trim().parse().expect("number");
    assert_eq!(index, top - 1);

    tap(&mut surface, 0, 0);
    assert_eq!(workspace(&surface), format!("w{}", index - 1));
}

#[tokio::test]
async fn escape_leaves_the_collapsed_list_where_navigate_scrolled_it_and_the_wheel_brings_back_the_first_workspace(
) {
    let mut surface = navigating(compact(6), many(12));
    surface.navigate(Navigation::Move(-1));
    frame(&mut surface, &HashSet::new());
    assert_ne!(compact_rows(&surface, 1), ["1 "]);

    surface.leave_navigate();
    frame(&mut surface, &HashSet::new());
    assert_ne!(compact_rows(&surface, 1), ["1 "]);

    for _ in 0..12 {
        wheel(&mut surface, false, 0, 1);
    }
    frame(&mut surface, &HashSet::new());
    assert_eq!(compact_rows(&surface, 1), ["1 "]);
    tap(&mut surface, 0, 0);
    assert_eq!(workspace(&surface), "w0");
}

#[tokio::test]
async fn expanding_the_sidebar_in_navigate_mode_reveals_the_selection() {
    let mut surface = navigating(compact(6), many(12));
    for _ in 0..4 {
        surface.navigate(Navigation::Move(1));
    }
    frame(&mut surface, &HashSet::new());
    assert!(compact_rows(&surface, 6).contains(&"5 ".to_owned()));

    surface.act(KeybindAction::ToggleSidebar, false);
    frame(&mut surface, &HashSet::new());

    assert!(surface.navigating());
    assert!(shown_in(&surface, 6, "ws4"));
}

#[tokio::test]
async fn collapsing_the_sidebar_in_navigate_mode_reveals_the_selection() {
    let mut surface = navigating(plain(120, 6), many(12));
    surface.navigate(Navigation::Move(-1));
    frame(&mut surface, &HashSet::new());
    for _ in 0..12 {
        wheel(&mut surface, false, 2, 3);
    }
    frame(&mut surface, &HashSet::new());
    assert!(!shown_in(&surface, 6, "ws11"));

    surface.act(KeybindAction::ToggleSidebar, false);
    frame(&mut surface, &HashSet::new());

    assert!(surface.navigating());
    assert!(compact_rows(&surface, 6).contains(&"12".to_owned()));
}

#[tokio::test]
async fn a_resize_while_navigating_reveals_the_selection_in_the_collapsed_list() {
    let mut surface = navigating(compact(20), many(12));
    surface.navigate(Navigation::Move(-1));
    frame(&mut surface, &HashSet::new());

    surface.resize(host(120, 6));
    frame(&mut surface, &HashSet::new());

    assert!(compact_rows(&surface, 6).contains(&"12".to_owned()));
}

#[tokio::test]
async fn with_the_selection_vanished_a_press_on_the_panes_leaves_navigate_mode() {
    let mut surface = navigating(plain(120, 30), three());
    tap(&mut surface, 60, 15);
    assert!(surface.navigating());

    surface.navigate(Navigation::Move(1));
    let mut closed = three();
    closed.workspaces.remove(1);
    surface.update(closed);
    frame(&mut surface, &HashSet::new());
    assert!(surface.selection_vanished());

    tap(&mut surface, 60, 15);
    assert!(!surface.navigating());
}

#[tokio::test]
async fn each_navigate_session_has_its_own_identity_and_none_is_reported_outside_navigate() {
    let mut surface = navigating(plain(120, 30), three());
    let first = surface.navigate_session();
    surface.leave_navigate();
    assert_eq!(surface.navigate_session(), None);

    surface.act(KeybindAction::WorkspacePicker, false);

    assert!(first.is_some());
    assert_ne!(surface.navigate_session(), first);
    assert!(surface.navigate_session().is_some());
}
