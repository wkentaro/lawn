use super::*;

fn two_workspaces() -> Layout {
    spaces(&[(Some("main"), &[None, None]), (Some("other"), &[None])])
}

fn opened(layout: Layout) -> (Surface, Screen) {
    let mut surface = plain(64, 20);
    surface.update(layout, 0);
    frame(&mut surface, &HashSet::new());
    assert_eq!(tap(&mut surface, 58, 1), None);
    assert!(surface.switching());
    let screen = Screen::of(&mut surface);
    (surface, screen)
}

fn bold() -> u16 {
    styled(Style::default().add_modifier(Modifier::BOLD)).modifier
}

#[tokio::test]
async fn at_the_mobile_width_threshold_a_header_sits_above_the_panes_and_one_column_more_brings_back_the_sidebar(
) {
    let mut surface = plain(64, 20);
    surface.update(spaces(&[(None, &[None, None])]), 0);
    frame(&mut surface, &HashSet::new());

    assert_eq!(surface.areas.mobile_header, Rect::new(0, 0, 64, 2));
    assert_eq!(pane_area(&surface), Rect::new(0, 2, 64, 18));
    assert!(surface.areas.sidebar.is_empty() && surface.areas.tab_bar.is_empty());

    surface.resize(host(65, 20));
    frame(&mut surface, &HashSet::new());
    assert!(surface.areas.mobile_header.is_empty());
    assert_eq!(surface.areas.sidebar.width, 26);
    assert_eq!(surface.areas.tab_bar, Rect::new(26, 0, 39, 1));
}

#[tokio::test]
async fn the_header_names_the_focused_workspace_in_bold_with_its_tab_on_the_right_and_a_switch_button_last(
) {
    let palette = Palette::catppuccin();
    let mut surface = plain(64, 20);
    surface.update(spaces(&[(Some("main"), &[None])]), 0);
    frame(&mut surface, &HashSet::new());

    assert_eq!(
        line(&surface, 0),
        format!(" · main{}tab 1 │{}", " ".repeat(41), " ".repeat(9))
    );
    assert_eq!(
        line(&surface, 1),
        format!(" no agents{}│ switch  ", " ".repeat(44))
    );
    assert_eq!(cell(&surface, 3, 0).modifier, bold());
    assert_ne!(cell(&surface, 48, 0).modifier, bold());
    assert_eq!(cell(&surface, 56, 1).modifier, bold());
    assert_eq!(cell(&surface, 10, 1).bg, background(palette.panel_bg));
    for y in 0..2 {
        assert_eq!(cell(&surface, 53, y).bg, background(palette.panel_bg));
        for x in 54..64 {
            assert_eq!(cell(&surface, x, y).bg, background(palette.surface0));
        }
    }
}

#[tokio::test]
async fn with_several_tabs_the_header_counts_the_focused_tab_and_names_a_named_one() {
    let mut surface = plain(64, 20);
    surface.update(spaces(&[(Some("main"), &[None, None, Some("logs")])]), 0);
    surface.act(KeybindAction::NextTab, false);
    frame(&mut surface, &HashSet::new());
    assert_eq!(span(&surface, 0, 42..55), "tab 2 · 2/3 │");

    surface.act(KeybindAction::NextTab, false);
    frame(&mut surface, &HashSet::new());
    assert_eq!(span(&surface, 0, 39..55), "tab logs · 3/3 │");
}

#[tokio::test]
async fn without_a_workspace_the_header_says_so_and_keeps_its_switch_button() {
    let mut surface = plain(64, 20);
    surface.update(spaces(&[]), 0);
    frame(&mut surface, &HashSet::new());

    assert_eq!(span(&surface, 0, 0..15), " no workspace  ");
    assert_eq!(span(&surface, 1, 54..64), "│ switch  ");
}

#[tokio::test]
async fn a_press_anywhere_on_the_switch_button_opens_the_switcher_and_beside_it_does_not() {
    for (column, row, opens) in [(54, 0, true), (63, 1, true), (53, 1, false), (20, 0, false)] {
        let mut surface = plain(64, 20);
        surface.update(two_workspaces(), 0);
        frame(&mut surface, &HashSet::new());

        assert_eq!(tap(&mut surface, column, row), None);
        assert_eq!(surface.switching(), opens, "{column},{row}");
    }
}

#[tokio::test]
async fn the_switcher_covers_the_screen_with_its_title_close_button_and_both_sections() {
    let (surface, screen) = opened(two_workspaces());

    assert_eq!(
        screen.line(0),
        format!(" switch{}│  close  ", " ".repeat(47))
    );
    assert_eq!(screen.line(1), format!("{}│    ×    ", " ".repeat(54)));
    assert_eq!(screen.line(2), "─".repeat(64));
    assert_eq!(
        screen.lines(3..14),
        [
            "  spaces",
            "   + new workspace",
            "   · main",
            "   tab 1 · 1/2",
            "   · other",
            "   tab 1",
            "  tabs",
            "   + new tab",
            "   tab 1",
            "   tab 2",
            "",
        ]
    );
    assert!(line(&surface, 0).starts_with(" · main "));
}

#[tokio::test]
async fn the_switcher_highlights_the_focused_workspace_and_the_focused_tab() {
    let palette = Palette::catppuccin();
    let (mut surface, mut screen) = opened(two_workspaces());
    let at = |screen: &Screen, x, y| screen.cell(x, y).bg;

    for y in [5, 6] {
        assert_eq!(at(&screen, 1, y), background(palette.surface0));
        assert_eq!(at(&screen, 63, y), background(palette.surface0));
    }
    assert_eq!(at(&screen, 1, 7), background(palette.panel_bg));
    assert_eq!(at(&screen, 1, 11), background(palette.surface_dim));
    assert_eq!(at(&screen, 1, 12), background(palette.panel_bg));
    assert_eq!(screen.cell(5, 5).modifier, bold());

    surface.act(KeybindAction::NextTab, false);
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(at(&screen, 1, 11), background(palette.panel_bg));
    assert_eq!(at(&screen, 1, 12), background(palette.surface_dim));
}

#[tokio::test]
async fn the_close_button_or_escape_closes_the_switcher_and_shows_the_panes_again() {
    let mut surface = plain(64, 20);
    surface.update(two_workspaces(), 0);
    let mut screen = Screen::of(&mut surface);
    let panes = screen.lines(0..20);

    for close in [true, false] {
        tap(&mut surface, 58, 1);
        screen.show(&mut surface, &HashSet::new());
        assert_ne!(screen.lines(0..20), panes);

        if close {
            assert_eq!(tap(&mut surface, 54, 1), None);
        } else {
            surface.close_switcher();
        }
        assert!(!surface.switching());
        assert!(!surface.relayout);
        screen.show(&mut surface, &HashSet::new());
        assert_eq!(screen.lines(0..20), panes);
    }
}

#[tokio::test]
async fn a_press_on_a_workspace_or_a_tab_in_the_switcher_focuses_it_and_closes_the_switcher() {
    let (mut surface, _) = opened(two_workspaces());

    tap(&mut surface, 5, 12);
    assert_eq!(tab(&surface), "tab2");
    assert!(!surface.switching());

    frame(&mut surface, &HashSet::new());
    tap(&mut surface, 58, 1);
    frame(&mut surface, &HashSet::new());
    tap(&mut surface, 40, 8);
    assert_eq!(workspace(&surface), "w1");
    assert!(!surface.switching());
}

#[tokio::test]
async fn the_create_rows_act_like_their_keys_and_close_the_switcher() {
    let (mut surface, _) = opened(two_workspaces());
    let workspace = tap(&mut surface, 5, 4);
    assert!(workspace.is_some());
    assert_eq!(
        destination(workspace),
        destination(surface.act(KeybindAction::NewWorkspace, false))
    );
    assert!(!surface.switching());

    frame(&mut surface, &HashSet::new());
    tap(&mut surface, 58, 1);
    frame(&mut surface, &HashSet::new());
    let tab = tap(&mut surface, 5, 10);
    assert!(tab.is_some());
    assert_eq!(
        destination(tab),
        destination(surface.act(KeybindAction::NewTab, false))
    );
    assert!(!surface.switching());

    let (mut surface, screen) = opened(spaces(&[]));
    assert_eq!(screen.lines(3..6), ["  spaces", "   + new workspace", ""]);
    let workspace = tap(&mut surface, 5, 4);
    assert!(workspace.is_some());
    assert_eq!(
        destination(workspace),
        destination(surface.act(KeybindAction::NewWorkspace, false))
    );
    assert!(!surface.switching());
}

#[tokio::test]
async fn the_wheel_scrolls_the_switcher_two_rows_a_step_and_stops_at_either_end() {
    let (mut surface, mut screen) = opened(many(12));
    let top = |screen: &Screen| screen.span(3, 1..64).trim_end().to_owned();
    assert_eq!(top(&screen), " spaces");

    wheel(&mut surface, true, 30, 10);
    assert!(surface.needs_frame());
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(top(&screen), "  · ws0");

    for _ in 0..5 {
        wheel(&mut surface, true, 30, 10);
    }
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(top(&screen), "  · ws5");
    assert_eq!(screen.span(19, 1..64).trim_end(), "  tab 1");
    wheel(&mut surface, true, 30, 10);
    assert!(!surface.needs_frame());

    wheel(&mut surface, false, 0, 0);
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(top(&screen), "  · ws4");
    for _ in 0..5 {
        wheel(&mut surface, false, 30, 10);
    }
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(top(&screen), " spaces");
    wheel(&mut surface, false, 30, 10);
    assert!(!surface.needs_frame());
    assert!(surface.switching());
}

#[tokio::test]
async fn the_switcher_shows_a_scrollbar_on_its_left_only_when_the_list_overflows() {
    let (_, screen) = opened(two_workspaces());
    assert!((3..20).all(|y| screen.cell(0, y).symbol == " "));

    let (mut surface, mut screen) = opened(many(12));
    let track: String = (3..20).map(|y| screen.cell(0, y).symbol.as_str()).collect();
    assert!(track.starts_with('▌') && track.ends_with('│'), "{track}");
    assert!(track.chars().all(|glyph| glyph == '▌' || glyph == '│'));

    for _ in 0..6 {
        wheel(&mut surface, true, 30, 10);
    }
    screen.show(&mut surface, &HashSet::new());
    let track: String = (3..20).map(|y| screen.cell(0, y).symbol.as_str()).collect();
    assert!(track.starts_with('│') && track.ends_with('▌'), "{track}");
}

#[tokio::test]
async fn opening_the_switcher_starts_at_the_top_of_the_list() {
    let mut surface = plain(64, 20);
    surface.update(many(12), 0);
    surface.act(KeybindAction::PreviousWorkspace, false);
    frame(&mut surface, &HashSet::new());
    assert_eq!(workspace(&surface), "w11");

    for _ in 0..2 {
        tap(&mut surface, 58, 1);
        let screen = Screen::of(&mut surface);
        assert_eq!(screen.span(3, 1..64).trim_end(), " spaces");
        wheel(&mut surface, true, 30, 10);
        frame(&mut surface, &HashSet::new());
        surface.close_switcher();
        frame(&mut surface, &HashSet::new());
    }
}

#[tokio::test]
async fn a_workspace_focused_while_the_switcher_is_open_scrolls_into_view() {
    let (mut surface, mut screen) = opened(many(12));

    surface.act(KeybindAction::PreviousWorkspace, false);
    screen.show(&mut surface, &HashSet::new());

    assert_eq!(workspace(&surface), "w11");
    assert!(surface.switching());
    assert_eq!(screen.span(18, 1..64).trim_end(), "  · ws11");
    assert_eq!(screen.span(19, 1..64).trim_end(), "  tab 1");

    surface.act(KeybindAction::NextWorkspace, false);
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(screen.span(3, 1..64).trim_end(), "  · ws0");
}

#[tokio::test]
async fn while_the_switcher_is_open_no_mouse_event_reaches_a_pane_and_nothing_is_copyable() {
    let mut surface = plain(64, 20);
    let mut layout = spaces(&[(None, &[None])]);
    layout.workspaces[0].tabs[0].root =
        serde_json::from_value(balanced(&["t1".to_owned(), "t9".to_owned()])).expect("root");
    surface.update(layout, 0);
    let mut inputs = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1003h\x1b[?1006h");
    frame(&mut surface, &HashSet::new());
    let focused = surface.focused_id();
    let right = pane_inner(&surface, "t9");
    assert!(right.y <= 16 && right.bottom() > 17 && right.width > 3);
    tap(&mut surface, 58, 1);
    frame(&mut surface, &HashSet::new());
    inputs.iter_mut().for_each(|input| {
        reports(input);
    });

    for (kind, column, row) in [
        (MouseEventKind::Moved, right.x, 16),
        (MouseEventKind::Down(MouseButton::Left), right.x, 16),
        (MouseEventKind::Drag(MouseButton::Left), right.x + 3, 17),
        (MouseEventKind::Up(MouseButton::Left), right.x + 3, 17),
        (MouseEventKind::Down(MouseButton::Right), right.x, 16),
        (MouseEventKind::Up(MouseButton::Right), right.x, 16),
        (MouseEventKind::Down(MouseButton::Middle), right.x, 16),
        (MouseEventKind::ScrollDown, right.x, 16),
        (MouseEventKind::ScrollUp, right.x, 16),
    ] {
        assert_eq!(point(&mut surface, kind, column, row), None);
    }

    for input in &mut inputs {
        assert_eq!(reports(input), b"");
    }
    assert_eq!(surface.focused_id(), focused);
    assert!(surface.gesture.is_none() && surface.select.shown().is_none());
    assert!(surface.switching());
    assert!(!surface.copyable());
}

#[tokio::test]
async fn the_cursor_is_hidden_while_the_switcher_is_open_and_returns_once_it_closes() {
    let mut surface = plain(64, 20);
    surface.update(spaces(&[(None, &[None])]), 0);
    frame_attaching(&mut surface, &HashSet::new());
    feed(&surface, "t1", b"\x1b[1;3H\x1b[?25h");
    let dirty = HashSet::from([id_of(&surface, "t1")]);
    frame(&mut surface, &dirty);
    let shown = surface.cursor().map(|cursor| (cursor.x, cursor.y));
    assert!(shown.is_some());

    tap(&mut surface, 58, 1);
    frame(&mut surface, &HashSet::new());
    assert!(surface.cursor().is_none());

    surface.close_switcher();
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.cursor().map(|cursor| (cursor.x, cursor.y)), shown);
}

#[tokio::test]
async fn pane_output_while_the_switcher_is_open_stays_under_it_and_shows_once_it_closes() {
    let mut surface = plain(64, 20);
    surface.update(spaces(&[(None, &[None])]), 0);
    let mut screen = Screen::of(&mut surface);
    let inner = pane_inner(&surface, "t1");
    tap(&mut surface, 58, 1);
    screen.show(&mut surface, &HashSet::new());

    feed(&surface, "t1", b"hello");
    let dirty = HashSet::from([id_of(&surface, "t1")]);
    assert!(frame(&mut surface, &dirty).is_empty());

    surface.close_switcher();
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(screen.span(inner.y, inner.x..inner.x + 5), "hello");
}

#[tokio::test]
async fn the_switcher_is_not_redrawn_when_nothing_it_shows_changed() {
    let (mut surface, _) = opened(two_workspaces());

    assert!(frame(&mut surface, &HashSet::new()).is_empty());
    point(&mut surface, MouseEventKind::Moved, 30, 10);
    wheel(&mut surface, true, 30, 10);
    assert!(!surface.needs_frame());

    let built = surface.outer.cover().map(|cover| cover.1.as_ptr());
    surface.update(two_workspaces(), 0);
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.outer.cover().map(|cover| cover.1.as_ptr()), built);

    let mut renamed = two_workspaces();
    renamed.workspaces[1].name = Some("renamed".to_owned());
    surface.update(renamed, 0);
    frame(&mut surface, &HashSet::new());
    assert_ne!(surface.outer.cover().map(|cover| cover.1.as_ptr()), built);
}

#[tokio::test]
async fn in_copy_mode_a_press_on_the_switch_button_does_not_open_the_switcher() {
    let mut surface = plain(64, 20);
    surface.update(spaces(&[(None, &[None])]), 0);
    frame_attaching(&mut surface, &HashSet::new());
    feed(&surface, "t1", b"hello");
    let dirty = HashSet::from([id_of(&surface, "t1")]);
    frame(&mut surface, &dirty);
    surface.enter_copy_mode();
    frame(&mut surface, &dirty);

    tap(&mut surface, 58, 1);
    assert!(!surface.switching());
    assert!(surface.copy_mode());

    surface.copy_mode_key(&TerminalKey::new(KeyCode::Char('q'), KeyModifiers::NONE));
    frame(&mut surface, &dirty);
    tap(&mut surface, 58, 1);
    assert!(surface.switching());
}

#[tokio::test]
async fn widening_past_the_threshold_closes_the_switcher_and_narrowing_again_does_not_reopen_it() {
    let (mut surface, _) = opened(two_workspaces());

    surface.resize(host(50, 20));
    let screen = Screen::of(&mut surface);
    assert!(surface.switching());
    assert_eq!(screen.span(0, 40..50), "│  close  ");

    surface.resize(host(120, 20));
    let screen = Screen::of(&mut surface);
    assert!(!surface.switching());
    assert!(surface.outer.cover().is_none());
    assert_eq!(surface.areas.sidebar.width, 26);
    assert!(screen.line(2).starts_with(" main "), "{:?}", screen.line(2));
    let canvas: Vec<String> = (0..20)
        .map(|y| line(&surface, y).trim_end().to_owned())
        .collect();
    assert_eq!(screen.lines(0..20), canvas);

    for (column, row) in [(40, 1), (5, 4), (5, 10)] {
        tap(&mut surface, column, row);
        assert!(!surface.switching());
        assert_eq!(workspace(&surface), "w0");
    }

    surface.resize(host(64, 20));
    let screen = Screen::of(&mut surface);
    assert!(!surface.switching());
    assert!(screen.line(0).starts_with(" · main "));
}

#[tokio::test]
async fn on_one_row_the_header_is_one_row_and_on_two_rows_the_switcher_draws_only_its_rule() {
    let mut surface = plain(64, 1);
    surface.update(two_workspaces(), 0);
    let screen = Screen::of(&mut surface);
    assert_eq!(surface.areas.mobile_header, Rect::new(0, 0, 64, 1));
    assert_eq!(screen.span(0, 54..64), "│ switch  ");
    assert!(screen.line(0).starts_with(" · main "));

    let mut surface = plain(64, 2);
    surface.update(two_workspaces(), 0);
    frame(&mut surface, &HashSet::new());
    tap(&mut surface, 58, 1);
    let screen = Screen::of(&mut surface);
    assert_eq!(screen.line(0), "─".repeat(64));
    assert_eq!(screen.line(1).trim_end(), "");

    tap(&mut surface, 58, 0);
    tap(&mut surface, 5, 1);
    assert!(surface.switching());
    assert_eq!(workspace(&surface), "w0");
}

#[tokio::test]
async fn the_header_and_the_switcher_name_an_unnamed_workspace_after_its_folder() {
    let mut surface = plain(64, 20);
    surface.update(spaces(&[(None, &[None])]), 0);
    moved(&mut surface, "t1", "/srv/app");
    frame(&mut surface, &HashSet::new());
    assert!(line(&surface, 0).starts_with(" · app "));

    tap(&mut surface, 58, 1);
    let screen = Screen::of(&mut surface);

    assert_eq!(screen.line(5).trim_end(), "   · app");
}

fn header(surface: &mut Surface) -> Screen {
    frame(surface, &HashSet::new());
    Screen::of(surface)
}

fn switcher(surface: &mut Surface) -> Screen {
    frame(surface, &HashSet::new());
    tap(surface, 58, 1);
    assert!(surface.switching());
    Screen::of(surface)
}

#[tokio::test]
async fn the_header_marks_the_workspace_with_blocked_over_an_unseen_finish_over_working_over_idle()
{
    let mut surface = symbols(64, 20);
    surface.update(paired(), 0);
    let mark = |surface: &mut Surface| header(surface).span(0, 0..3);

    said(&mut surface, "t1", "idle");
    assert_eq!(mark(&mut surface), " ○ ");
    said(&mut surface, "t2", "working");
    assert_eq!(mark(&mut surface), " ◐ ");
    said(&mut surface, "t3", "working");
    said(&mut surface, "t3", "done");
    assert_eq!(mark(&mut surface), " ✓ ");
    said(&mut surface, "t4", "blocked");
    assert_eq!(mark(&mut surface), " × ");
}

#[tokio::test]
async fn the_second_header_row_counts_every_agent_by_state_most_pressing_first() {
    let mut surface = symbols(64, 20);
    surface.update(paired(), 0);

    for (terminal, state) in [("t1", "working"), ("t2", "blocked"), ("t3", "working")] {
        said(&mut surface, terminal, state);
    }
    said(&mut surface, "t3", "done");
    said(&mut surface, "t4", "idle");
    said(&mut surface, "t5", "idle");

    assert_eq!(
        header(&mut surface).span(1, 0..54).trim_end(),
        " × 1 blocked · ✓ 1 done · ◐ 1 working · ○ 2 idle"
    );
}

#[tokio::test]
async fn with_dots_the_summary_marks_only_blocked_and_done() {
    let mut surface = plain(64, 20);
    surface.update(paired(), 0);

    for (terminal, state) in [("t1", "working"), ("t2", "blocked"), ("t3", "working")] {
        said(&mut surface, terminal, state);
    }
    said(&mut surface, "t3", "done");
    said(&mut surface, "t4", "idle");

    assert_eq!(
        header(&mut surface).span(1, 0..54).trim_end(),
        " ◉ 1 blocked · ● 1 done · 1 working · 1 idle"
    );
}

#[tokio::test]
async fn the_second_header_row_says_when_there_are_no_agents_or_all_are_idle() {
    let mut surface = plain(64, 20);
    surface.update(paired(), 0);
    assert_eq!(header(&mut surface).span(1, 0..54).trim_end(), " no agents");

    said(&mut surface, "t5", "idle");
    assert_eq!(header(&mut surface).span(1, 0..54).trim_end(), " all idle");
}

#[tokio::test]
async fn a_summary_too_wide_for_the_header_stops_at_an_ellipsis() {
    let mut surface = symbols(30, 20);
    surface.update(paired(), 0);

    said(&mut surface, "t1", "blocked");
    said(&mut surface, "t2", "working");

    assert_eq!(header(&mut surface).span(1, 0..19), " × 1 blocked …     ");
}

#[tokio::test]
async fn the_switch_button_carries_a_red_mark_while_any_agent_anywhere_is_blocked() {
    let palette = Palette::catppuccin();
    let mut surface = symbols(64, 20);
    surface.update(paired(), 0);

    said(&mut surface, "t5", "blocked");
    let screen = header(&mut surface);
    assert_eq!(screen.cell(63, 0).symbol, "×");
    assert_eq!(
        screen.cell(63, 0).fg,
        styled(Style::default().fg(palette.red)).fg
    );

    said(&mut surface, "t5", "working");
    assert_eq!(header(&mut surface).cell(63, 0).symbol, " ");
}

#[tokio::test]
async fn the_blocked_mark_stays_on_a_switch_button_too_narrow_for_its_label() {
    let mut surface = symbols(7, 1);
    surface.update(paired(), 0);

    said(&mut surface, "t5", "blocked");
    assert_eq!(header(&mut surface).span(0, 0..7), "│switc×");
}

#[tokio::test]
async fn the_switcher_lists_every_agent_first_with_its_workspace_tab_state_and_name() {
    let mut surface = symbols(64, 20);
    surface.update(paired(), 0);
    said(&mut surface, "t2", "working");
    surface.heard(Report::Status(
        "t5".to_owned(),
        crate::pane::core_status(&json!({"state": "blocked", "app": "mytool"})),
    ));

    let screen = switcher(&mut surface);

    assert_eq!(
        screen.lines(3..9),
        [
            "  agents",
            "   ◐ main",
            "   1 · working · claude",
            "   × other",
            "   blocked · agent",
            "  spaces",
        ]
    );
}

#[tokio::test]
async fn the_switcher_marks_each_workspace_with_its_most_pressing_agent() {
    let mut surface = symbols(64, 20);
    surface.update(paired(), 0);
    said(&mut surface, "t1", "idle");
    said(&mut surface, "t5", "blocked");

    let screen = switcher(&mut surface);

    assert_eq!(
        screen.lines(8..13),
        [
            "  spaces",
            "   + new workspace",
            "   ○ main",
            "   tab 1 · 1/2",
            "   × other"
        ]
    );
}

#[tokio::test]
async fn the_focused_agent_takes_the_dim_background_in_the_switcher() {
    let palette = Palette::catppuccin();
    let mut surface = symbols(64, 20);
    surface.update(paired(), 0);
    said(&mut surface, "t1", "working");
    said(&mut surface, "t5", "working");

    let screen = switcher(&mut surface);

    for y in [4, 5] {
        assert_eq!(screen.cell(1, y).bg, background(palette.surface_dim));
    }
    assert_eq!(screen.cell(1, 6).bg, background(palette.panel_bg));
}

#[tokio::test]
async fn a_press_on_an_agent_focuses_its_pane_and_closes_the_switcher() {
    let mut surface = symbols(64, 20);
    surface.update(paired(), 0);
    said(&mut surface, "t1", "working");
    said(&mut surface, "t4", "working");
    switcher(&mut surface);

    tap(&mut surface, 10, 7);

    assert!(!surface.switching());
    assert_eq!(tab(&surface), "tab2");
    assert_eq!(surface.session.focused(), Some("t4"));
}

#[tokio::test]
async fn with_priority_sort_the_most_pressing_and_then_the_latest_agent_comes_first() {
    let mut surface = dressed(64, 30, |config| {
        config.ui.status_indicators = crate::config::StatusIndicatorStyle::Symbols;
        config.ui.agent_panel_sort = crate::config::AgentPanelSortConfig::Priority;
    });
    surface.update(paired(), 0);
    for (terminal, state) in [
        ("t1", "idle"),
        ("t2", "working"),
        ("t5", "blocked"),
        ("t4", "working"),
        ("t2", "working"),
    ] {
        said(&mut surface, terminal, state);
    }

    let screen = switcher(&mut surface);
    let names: Vec<String> = (4..12)
        .step_by(2)
        .map(|y| screen.line(y).trim_end().to_owned())
        .collect();
    let details: Vec<String> = (5..13)
        .step_by(2)
        .map(|y| screen.line(y).trim_end().to_owned())
        .collect();

    assert_eq!(names, ["   × other", "   ◐ main", "   ◐ main", "   ○ main"]);
    assert_eq!(
        details,
        [
            "   blocked · claude",
            "   2 · working · claude",
            "   1 · working · claude",
            "   1 · idle · claude",
        ]
    );
}

#[tokio::test]
async fn a_dialog_over_the_switcher_hides_it_and_the_switcher_returns_once_the_dialog_closes() {
    let (mut surface, mut screen) = opened(two_workspaces());
    let switcher = screen.lines(0..20);

    surface.act(KeybindAction::CloseWorkspace, true);
    assert!(surface.dialog_open());
    screen.show(&mut surface, &HashSet::new());
    assert!(switcher[0].starts_with(" switch"));
    assert!(
        screen.line(0).starts_with(" · main "),
        "{:?}",
        screen.line(0)
    );
    assert!(screen.line(7).contains("┌"), "{:?}", screen.line(7));
    assert!(
        screen.line(8).contains(" Close workspace?"),
        "{:?}",
        screen.line(8)
    );

    surface.dialog_key(&TerminalKey::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(surface.switching());
    screen.show(&mut surface, &HashSet::new());
    assert_eq!(screen.lines(0..20), switcher);
}
