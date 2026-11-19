use ratatui::style::{Color, Modifier, Style};

use super::*;
use crate::config::{SidebarCollapsedModeConfig, TabBarPositionConfig, TabBarRightEntryConfig};
use crate::core_client::tui::session::Operation;
use crate::core_client::tui::surface::Pointed;
use crate::core_client::tui::terminals::Report;
use crate::protocol::CellData;

fn dressed(cols: u16, rows: u16, configure: impl FnOnce(&mut Config)) -> Surface {
    let mut config = Config::default();
    configure(&mut config);
    around(cols, rows, Chrome::new(&config))
}

fn around(cols: u16, rows: u16, outer: Chrome) -> Surface {
    Surface::new(
        host(cols, rows),
        PaneChrome {
            borders: PaneBordersConfig::Auto,
            gaps: false,
            outer_borders: true,
        },
        outer,
        Palette::catppuccin(),
        true,
        Copying {
            on_select: true,
            toast: Some(ToastClipboardPosition::BottomCenter),
        },
    )
}

fn plain(cols: u16, rows: u16) -> Surface {
    dressed(cols, rows, |_| {})
}

fn spaces(workspaces: &[(Option<&str>, &[Option<&str>])]) -> Layout {
    let mut next = 0;
    let workspaces: Vec<Value> = workspaces
        .iter()
        .enumerate()
        .map(|(index, (name, tabs))| {
            let tabs: Vec<Value> = tabs
                .iter()
                .map(|tab| {
                    next += 1;
                    json!({
                        "id": format!("tab{next}"),
                        "name": tab,
                        "root": {"kind": "pane", "terminal": format!("t{next}")},
                    })
                })
                .collect();
            json!({"id": format!("w{index}"), "name": name, "tabs": tabs})
        })
        .collect();
    serde_json::from_value(json!({ "workspaces": workspaces })).expect("layout")
}

fn line(surface: &Surface, y: u16) -> String {
    span(surface, y, 0..surface.canvas_cols)
}

fn span(surface: &Surface, y: u16, xs: std::ops::Range<u16>) -> String {
    xs.map(|x| cell(surface, x, y).symbol.as_str()).collect()
}

fn find(surface: &Surface, y: u16, label: &str) -> u16 {
    let width = label.chars().count() as u16;
    (0..surface.canvas_cols.saturating_sub(width))
        .find(|x| span(surface, y, *x..*x + width) == label)
        .expect("label")
}

fn cell(surface: &Surface, x: u16, y: u16) -> &CellData {
    &surface.canvas[usize::from(y) * usize::from(surface.canvas_cols) + usize::from(x)]
}

fn styled(style: Style) -> CellData {
    let mut cell = ratatui::buffer::Cell::default();
    cell.set_style(style);
    CellData::from_ratatui_cell(&cell)
}

fn background(color: Color) -> u32 {
    styled(Style::default().bg(color)).bg
}

fn pane_area(surface: &Surface) -> Rect {
    surface.areas.pane_surface
}

#[tokio::test]
async fn the_sidebar_takes_its_default_width_and_the_tab_bar_the_top_row() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(Some("main"), &[None])]));
    frame(&mut surface, &HashSet::new());

    assert_eq!(surface.areas.sidebar, Rect::new(0, 0, 26, 30));
    assert_eq!(surface.areas.tab_bar, Rect::new(26, 0, 94, 1));
    assert_eq!(pane_area(&surface), Rect::new(26, 1, 94, 29));
    assert_eq!(surface.tiles.tiles[0].info.rect, Rect::new(26, 1, 94, 29));
}

#[tokio::test]
async fn the_sidebar_width_is_clamped_to_its_configured_bounds() {
    for (width, expected) in [(100, 36), (4, 18)] {
        let mut surface = dressed(120, 30, |config| config.ui.sidebar_width = width);
        surface.update(spaces(&[(None, &[None])]));
        frame(&mut surface, &HashSet::new());

        assert_eq!(surface.areas.sidebar.width, expected, "{width}");
    }
}

#[tokio::test]
async fn a_collapsed_sidebar_is_four_columns_when_compact_and_gone_when_hidden() {
    for (mode, expected) in [
        (SidebarCollapsedModeConfig::Compact, 4),
        (SidebarCollapsedModeConfig::Hidden, 0),
    ] {
        let mut surface = dressed(120, 30, |config| {
            config.ui.sidebar_start_collapsed = true;
            config.ui.sidebar_collapsed_mode = mode;
        });
        surface.update(spaces(&[(None, &[None])]));
        frame(&mut surface, &HashSet::new());

        assert_eq!(surface.areas.sidebar.width, expected);
        assert_eq!(pane_area(&surface).x, expected);
    }
}

#[tokio::test]
async fn a_single_tab_hides_the_tab_bar_only_when_configured() {
    for (tabs, hidden) in [(&[None][..], true), (&[None, None][..], false)] {
        let mut surface = dressed(120, 30, |config| {
            config.ui.hide_tab_bar_when_single_tab = true;
        });
        surface.update(spaces(&[(None, tabs)]));
        frame(&mut surface, &HashSet::new());

        assert_eq!(surface.areas.tab_bar.is_empty(), hidden);
        assert_eq!(pane_area(&surface).y, u16::from(!hidden));
    }
}

#[tokio::test]
async fn a_bottom_tab_bar_takes_the_last_row_below_the_panes() {
    let mut surface = dressed(120, 30, |config| {
        config.ui.tab_bar_position = TabBarPositionConfig::Bottom;
    });
    surface.update(spaces(&[(None, &[None])]));
    frame(&mut surface, &HashSet::new());

    assert_eq!(surface.areas.tab_bar, Rect::new(26, 29, 94, 1));
    assert_eq!(pane_area(&surface), Rect::new(26, 0, 94, 29));
    assert!(span(&surface, 29, 26..120).contains(" 1 "));
}

#[tokio::test]
async fn the_copy_mode_bar_covers_a_bottom_tab_bar_and_sits_above_a_top_one() {
    for position in [TabBarPositionConfig::Bottom, TabBarPositionConfig::Top] {
        let row = 29;
        let mut surface = dressed(120, 30, |config| config.ui.tab_bar_position = position);
        surface.update(spaces(&[(None, &[None])]));
        frame_attaching(&mut surface, &HashSet::new());
        feed(&surface, "t1", b"hello");
        let dirty = HashSet::from([id_of(&surface, "t1")]);
        frame(&mut surface, &dirty);
        surface.enter_copy_mode();

        let rows = frame(&mut surface, &dirty);

        let bar = surface.overlays().copy.bar;
        assert_eq!(bar, Rect::new(26, row, 94, 1), "{position:?}");
        let covered = match position {
            TabBarPositionConfig::Bottom => surface.areas.tab_bar,
            TabBarPositionConfig::Top => Rect::new(26, pane_area(&surface).bottom() - 1, 94, 1),
        };
        assert_eq!(bar, covered);
        let drawn: String = rows
            .iter()
            .filter(|drawn| drawn.y == row)
            .flat_map(|drawn| drawn.cells[26..].iter().map(|cell| cell.symbol.as_str()))
            .collect();
        assert!(drawn.starts_with(" COPY"), "{drawn:?}");
        assert!(line(&surface, 0).starts_with(" spaces"));
        surface.copy_mode_key(&TerminalKey::new(KeyCode::Char('q'), KeyModifiers::NONE));
        let rows = frame(&mut surface, &dirty);
        let restored: String = rows
            .iter()
            .filter(|drawn| drawn.y == row)
            .flat_map(|drawn| drawn.cells.iter().map(|cell| cell.symbol.as_str()))
            .collect();
        assert_eq!(restored, line(&surface, row));
    }
}

#[tokio::test]
async fn the_sidebar_lists_every_workspace_under_its_header_and_above_its_footer() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(Some("main"), &[None]), (None, &[None])]));
    frame(&mut surface, &HashSet::new());

    assert!(line(&surface, 0).starts_with(" spaces "));
    assert!(line(&surface, 2).starts_with(" main "));
    assert!(line(&surface, 3).starts_with(" workspace "));
    assert!(line(&surface, 28).starts_with(" new "));
    assert_eq!(span(&surface, 29, 24..26), "«│");
    assert_eq!(cell(&surface, 25, 10).symbol, "│");
}

#[tokio::test]
async fn the_focused_workspace_row_takes_the_active_background() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(Some("main"), &[None]), (Some("other"), &[None])]));
    frame(&mut surface, &HashSet::new());
    let active = background(Palette::catppuccin().active_row_bg);

    assert_eq!(cell(&surface, 1, 2).bg, active);
    assert_ne!(cell(&surface, 1, 3).bg, active);

    surface.act(KeybindAction::NextWorkspace, false);
    frame(&mut surface, &HashSet::new());

    assert_ne!(cell(&surface, 1, 2).bg, active);
    assert_eq!(cell(&surface, 1, 3).bg, active);
}

#[tokio::test]
async fn a_collapsed_compact_sidebar_numbers_the_workspaces_and_offers_to_expand() {
    let mut surface = dressed(120, 30, |config| config.ui.sidebar_start_collapsed = true);
    surface.update(spaces(&[(Some("main"), &[None]), (Some("other"), &[None])]));
    frame(&mut surface, &HashSet::new());

    assert!(line(&surface, 0).starts_with("1  │"));
    assert!(line(&surface, 1).starts_with("2  │"));
    assert!(line(&surface, 29).starts_with(" » │"));
    let palette = Palette::catppuccin();
    assert_eq!(
        (cell(&surface, 0, 0).fg, cell(&surface, 0, 0).bg),
        (
            styled(Style::default().fg(palette.text)).fg,
            background(palette.active_row_bg)
        )
    );
}

#[tokio::test]
async fn the_toggle_key_collapses_and_expands_the_sidebar() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None])]));
    frame(&mut surface, &HashSet::new());

    assert_eq!(surface.act(KeybindAction::ToggleSidebar, false), None);
    assert!(surface.needs_frame());
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.areas.sidebar.width, 4);
    assert_eq!(surface.tiles.tiles[0].info.rect.x, 4);

    surface.act(KeybindAction::ToggleSidebar, false);
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.areas.sidebar.width, 26);
}

#[tokio::test]
async fn a_long_workspace_list_scrolls_to_reveal_the_focused_workspace() {
    let names: Vec<String> = (0..12).map(|index| format!("ws{index}")).collect();
    let workspaces: Vec<(Option<&str>, &[Option<&str>])> = names
        .iter()
        .map(|name| (Some(name.as_str()), &[None][..]))
        .collect();
    let mut surface = plain(120, 12);
    surface.update(spaces(&workspaces));
    frame(&mut surface, &HashSet::new());
    assert!(line(&surface, 2).starts_with(" ws0 "));
    assert_eq!(cell(&surface, 24, 2).symbol, "▕");

    surface.act(KeybindAction::SwitchWorkspace(11), false);
    frame(&mut surface, &HashSet::new());

    let shown: Vec<String> = (0..12).map(|y| line(&surface, y)).collect();
    assert!(
        shown.iter().any(|row| row.starts_with(" ws11 ")),
        "{shown:#?}"
    );
    assert!(
        !shown.iter().any(|row| row.starts_with(" ws0 ")),
        "{shown:#?}"
    );
}

#[tokio::test]
async fn the_tab_bar_names_tabs_by_number_or_name_and_offers_a_new_one() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None, Some("logs"), None])]));
    frame(&mut surface, &HashSet::new());

    let bar = span(&surface, 0, 26..120);
    let labels: Vec<&str> = bar.split_whitespace().collect();
    assert_eq!(&labels[..4], ["1", "logs", "3", "+"]);
}

#[tokio::test]
async fn the_focused_tab_is_bold_and_underlined_and_a_named_tab_brighter() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None, Some("logs"), None])]));
    frame(&mut surface, &HashSet::new());
    let palette = Palette::catppuccin();
    let at = |label: &str| cell(&surface, find(&surface, 0, label), 0).clone();

    let focused = at("1");
    assert_eq!((focused.fg, focused.bg, focused.modifier), {
        let expected = styled(
            Style::default()
                .fg(palette.text)
                .bg(palette.panel_bg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        );
        (expected.fg, expected.bg, expected.modifier)
    });
    assert_eq!(
        at("logs").fg,
        styled(Style::default().fg(palette.overlay1)).fg
    );
    assert_eq!(at("3").fg, styled(Style::default().fg(palette.overlay0)).fg);
}

#[tokio::test]
async fn overflowing_tabs_scroll_to_keep_the_focused_tab_in_view() {
    let tabs = [None; 20];
    let mut surface = plain(90, 10);
    surface.update(spaces(&[(None, &tabs)]));
    frame(&mut surface, &HashSet::new());
    let bar = span(&surface, 0, 26..90);
    assert!(bar.starts_with(" < "), "{bar:?}");
    assert!(bar.ends_with(" >  + "), "{bar:?}");
    assert!(bar.contains('…'));

    surface.act(KeybindAction::SwitchTab(19), false);
    frame(&mut surface, &HashSet::new());

    let bar = span(&surface, 0, 26..90);
    assert!(bar.contains(" 20 "), "{bar:?}");
    assert!(!bar.contains(" 1 "), "{bar:?}");

    surface.resize(host(70, 10));
    frame(&mut surface, &HashSet::new());
    assert!(span(&surface, 0, 26..70).contains(" 20 "));
}

#[tokio::test]
async fn the_tab_bar_shows_text_and_hostname_entries_and_zoom_while_zoomed() {
    let mut surface = dressed(400, 30, |config| {
        config.ui.tab_bar_right = vec![
            TabBarRightEntryConfig::Zoom,
            TabBarRightEntryConfig::Text {
                text: "hi\u{1b}there".to_owned(),
            },
            TabBarRightEntryConfig::Command {
                command: "date".to_owned(),
                interval_seconds: 5,
                timeout_seconds: 2,
            },
            TabBarRightEntryConfig::Hostname,
        ];
        config.ui.tab_bar_right_separator = " | ".to_owned();
    });
    surface.update(spaces(&[(None, &[None])]));
    frame(&mut surface, &HashSet::new());
    let hostname = crate::app::tab_bar_status::sanitize_status_text(
        crate::platform::hostname().as_deref().unwrap_or_default(),
    );
    let right = match &hostname {
        Some(hostname) => format!("hithere | {hostname}"),
        None => "hithere".to_owned(),
    };
    assert!(
        line(&surface, 0).ends_with(&right),
        "{:?}",
        line(&surface, 0)
    );

    surface.act(KeybindAction::Zoom, false);
    frame(&mut surface, &HashSet::new());

    let bar = line(&surface, 0);
    assert!(bar.ends_with(&format!("ZOOM | {right}")), "{bar:?}");
    assert!(bar.contains(" 1 Z "), "{bar:?}");
    assert_eq!(
        cell(&surface, find(&surface, 0, "ZOOM"), 0).bg,
        background(Palette::catppuccin().accent)
    );
}

#[tokio::test]
async fn a_datetime_entry_wakes_the_loop_when_its_text_can_next_change() {
    for (format, period) in [("%H:%M:%S", 1.0), ("%H:%M", 60.0)] {
        let surface = dressed(120, 30, |config| {
            config.ui.tab_bar_right = vec![TabBarRightEntryConfig::Datetime {
                format: format.to_owned(),
            }];
        });

        let due = surface.deadline().expect("deadline");

        let wait = due
            .saturating_duration_since(std::time::Instant::now())
            .as_secs_f64();
        let into = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs_f64()
            % period;
        let early = wait - (period - into);
        assert!(
            early.abs() < 0.25 || (early + period).abs() < 0.25,
            "{format}: {wait}"
        );
    }
}

#[tokio::test]
async fn an_unchanged_datetime_does_not_redraw_the_tab_bar() {
    let mut surface = dressed(120, 30, |config| {
        config.ui.tab_bar_right = vec![TabBarRightEntryConfig::Datetime {
            format: "%Y".to_owned(),
        }];
    });
    surface.update(spaces(&[(None, &[None])]));
    frame(&mut surface, &HashSet::new());
    let due = surface.deadline().expect("deadline");

    surface.tick(due);

    assert!(!surface.needs_frame());
    assert!(surface.deadline().expect("deadline") > due);
}

#[tokio::test]
async fn the_chrome_is_not_redrawn_when_nothing_it_shows_changed() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(Some("main"), &[None, None])]));
    frame_attaching(&mut surface, &HashSet::new());
    let built = surface.outer.rows().as_ptr();

    assert!(frame(&mut surface, &HashSet::new()).is_empty());
    feed(&surface, "t1", b"output");
    let dirty = HashSet::from([id_of(&surface, "t1")]);
    let rows = frame(&mut surface, &dirty);
    assert!(
        rows.iter().all(|row| row.x >= 26 && row.y >= 1),
        "chrome rows"
    );

    surface.update(spaces(&[(Some("main"), &[None, None])]));
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.outer.rows().as_ptr(), built);

    surface.update(spaces(&[(Some("renamed"), &[None, None])]));
    frame(&mut surface, &HashSet::new());
    assert_ne!(surface.outer.rows().as_ptr(), built);
    assert!(line(&surface, 2).starts_with(" renamed "));
}

#[tokio::test]
async fn a_press_on_the_chrome_reaches_no_pane_and_starts_no_selection() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None])]));
    frame_attaching(&mut surface, &HashSet::new());
    feed(&surface, "t1", b"hello world");
    let dirty = HashSet::from([id_of(&surface, "t1")]);
    frame(&mut surface, &dirty);

    for (column, row) in [(5, 5), (60, 0)] {
        mouse(&mut surface, click(column, row));
        mouse(&mut surface, drag_to(80, 3));
        mouse(
            &mut surface,
            mouse_at(MouseEventKind::Up(MouseButton::Left), 80, 3),
        );

        assert!(surface.select.shown().is_none(), "{column},{row}");
        assert!(surface.gesture.is_none());
    }
}

#[tokio::test]
async fn the_copy_toast_sits_inside_the_pane_area() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None])]));
    frame(&mut surface, &HashSet::new());

    surface.copied(std::time::Instant::now());
    let toast = surface.overlays().toast;

    let panes = pane_area(&surface);
    assert!(!toast.is_empty());
    assert_eq!(toast.intersection(panes), toast);

    surface.act(KeybindAction::ToggleSidebar, false);
    frame(&mut surface, &HashSet::new());
    let moved = surface.overlays().toast;
    assert_eq!(moved.intersection(pane_area(&surface)), moved);
    assert_ne!(moved, toast);
}

#[tokio::test]
async fn a_changed_datetime_redraws_the_chrome_without_the_panes() {
    let mut surface = dressed(120, 30, |config| {
        config.ui.tab_bar_right = vec![TabBarRightEntryConfig::Datetime {
            format: "%S".to_owned(),
        }];
    });
    surface.update(spaces(&[(None, &[None])]));
    frame_attaching(&mut surface, &HashSet::new());
    let before = line(&surface, 0);
    let due = surface.deadline().expect("deadline");
    std::thread::sleep(
        due.saturating_duration_since(std::time::Instant::now())
            + std::time::Duration::from_millis(20),
    );

    surface.tick(std::time::Instant::now());
    assert!(surface.needs_frame());
    let rows = frame(&mut surface, &HashSet::new());

    assert!(rows.iter().any(|row| row.y == 0 && row.x == 26));
    assert!(rows.iter().all(|row| row.x < 26 || row.y == 0), "pane rows");
    assert_ne!(line(&surface, 0), before);
}

fn cursor_in_a_one_row_pane_area(output: &[u8]) -> Option<(u16, u16)> {
    let mut surface = dressed(120, 5, |config| {
        config.ui.tab_bar_position = TabBarPositionConfig::Bottom;
    });
    surface.update(spaces(&[(None, &[None])]));
    frame_attaching(&mut surface, &HashSet::new());
    let dirty = HashSet::from([id_of(&surface, "t1")]);
    surface.resize(host(120, 2));
    frame(&mut surface, &dirty);
    assert_eq!(pane_area(&surface), Rect::new(26, 0, 94, 1));

    feed(&surface, "t1", output);
    frame(&mut surface, &dirty);
    surface.cursor().map(|cursor| (cursor.x, cursor.y))
}

#[tokio::test]
async fn a_cursor_below_a_one_row_pane_area_is_not_shown_on_the_bottom_tab_bar() {
    assert_eq!(
        cursor_in_a_one_row_pane_area(b"\x1b[1;3H\x1b[?25h"),
        Some((28, 0))
    );
    assert_eq!(cursor_in_a_one_row_pane_area(b"\x1b[2;1H\x1b[?25h"), None);
}

fn point(surface: &mut Surface, kind: MouseEventKind, column: u16, row: u16) -> Option<Operation> {
    point_at(surface, kind, column, row, std::time::Instant::now())
}

fn point_at(
    surface: &mut Surface,
    kind: MouseEventKind,
    column: u16,
    row: u16,
    now: std::time::Instant,
) -> Option<Operation> {
    match surface.mouse(mouse_at(kind, column, row), now) {
        Some(Pointed::Act(action)) => surface.act(action, false),
        Some(Pointed::Copied(text)) => unreachable!("{text}"),
        None => None,
    }
}

fn tap(surface: &mut Surface, column: u16, row: u16) -> Option<Operation> {
    let pressed = point(
        surface,
        MouseEventKind::Down(MouseButton::Left),
        column,
        row,
    );
    let released = point(surface, MouseEventKind::Up(MouseButton::Left), column, row);
    pressed.or(released)
}

fn wheel(surface: &mut Surface, down: bool, column: u16, row: u16) {
    let kind = if down {
        MouseEventKind::ScrollDown
    } else {
        MouseEventKind::ScrollUp
    };
    point(surface, kind, column, row);
}

fn workspace(surface: &Surface) -> &str {
    surface
        .session
        .focused_workspace()
        .map_or("", |workspace| workspace.id.as_str())
}

fn tab(surface: &Surface) -> &str {
    surface.session.focused_tab().unwrap_or_default()
}

fn many(count: usize) -> Layout {
    let names: Vec<String> = (0..count).map(|index| format!("ws{index}")).collect();
    let workspaces: Vec<(Option<&str>, &[Option<&str>])> = names
        .iter()
        .map(|name| (Some(name.as_str()), &[None][..]))
        .collect();
    spaces(&workspaces)
}

fn thumb_rows(surface: &Surface, x: u16) -> Vec<u16> {
    let thumb = styled(Style::default().fg(Palette::catppuccin().overlay0)).fg;
    (0..surface.host.rows)
        .filter(|y| cell(surface, x, *y).symbol == "▕" && cell(surface, x, *y).fg == thumb)
        .collect()
}

fn pane_inner(surface: &Surface, terminal: &str) -> Rect {
    super::super::visible(&surface.views, id_of(surface, terminal))
        .expect("visible")
        .1
}

#[tokio::test]
async fn a_workspace_row_takes_focus_when_released_where_it_was_pressed() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(Some("main"), &[None]), (Some("other"), &[None])]));
    frame(&mut surface, &HashSet::new());

    let pressed = point(&mut surface, MouseEventKind::Down(MouseButton::Left), 5, 3);
    assert_eq!(pressed, None);
    assert_eq!(workspace(&surface), "w0");

    point(&mut surface, MouseEventKind::Up(MouseButton::Left), 9, 3);
    assert_eq!(workspace(&surface), "w1");
}

#[tokio::test]
async fn a_press_released_off_its_row_or_tab_focuses_nothing() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[
        (Some("main"), &[None, None]),
        (Some("other"), &[None]),
    ]));
    frame(&mut surface, &HashSet::new());
    let second = find(&surface, 0, " 2 ");

    point(&mut surface, MouseEventKind::Down(MouseButton::Left), 5, 3);
    point(&mut surface, MouseEventKind::Drag(MouseButton::Left), 5, 2);
    point(&mut surface, MouseEventKind::Up(MouseButton::Left), 5, 2);
    point(
        &mut surface,
        MouseEventKind::Down(MouseButton::Left),
        second + 1,
        0,
    );
    point(&mut surface, MouseEventKind::Up(MouseButton::Left), 60, 5);

    assert_eq!((workspace(&surface), tab(&surface)), ("w0", "tab1"));
}

#[tokio::test]
async fn a_row_that_replaced_the_pressed_one_takes_no_focus() {
    let mut surface = plain(120, 30);
    surface.update(many(3));
    frame(&mut surface, &HashSet::new());

    point(&mut surface, MouseEventKind::Down(MouseButton::Left), 5, 3);
    let mut closed = many(3);
    closed.workspaces.remove(1);
    surface.update(closed);
    frame(&mut surface, &HashSet::new());
    point(&mut surface, MouseEventKind::Up(MouseButton::Left), 5, 3);

    assert_eq!(workspace(&surface), "w0");
}

#[tokio::test]
async fn a_tab_that_replaced_the_pressed_one_takes_no_focus() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None, None, None])]));
    frame(&mut surface, &HashSet::new());
    let second = find(&surface, 0, " 2 ") + 1;

    point(
        &mut surface,
        MouseEventKind::Down(MouseButton::Left),
        second,
        0,
    );
    let mut closed = spaces(&[(None, &[None, None, None])]);
    closed.workspaces[0].tabs.remove(1);
    surface.update(closed);
    frame(&mut surface, &HashSet::new());
    point(
        &mut surface,
        MouseEventKind::Up(MouseButton::Left),
        second,
        0,
    );

    assert_eq!(tab(&surface), "tab1");
}

#[tokio::test]
async fn a_tab_takes_focus_when_released_where_it_was_pressed() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None, None, Some("logs")])]));
    frame(&mut surface, &HashSet::new());

    let (logs, second) = (find(&surface, 0, "logs"), find(&surface, 0, " 2 ") + 1);

    tap(&mut surface, logs, 0);
    assert_eq!(tab(&surface), "tab3");
    tap(&mut surface, second, 0);
    assert_eq!(tab(&surface), "tab2");
}

#[tokio::test]
async fn new_and_plus_start_a_workspace_and_a_tab_like_their_keys() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None])]));
    frame(&mut surface, &HashSet::new());
    let plus = find(&surface, 0, " + ") + 1;

    let workspace = tap(&mut surface, 2, 28);
    let tab = tap(&mut surface, plus, 0);

    assert!(workspace.is_some() && tab.is_some());
    assert_eq!(workspace, surface.act(KeybindAction::NewWorkspace, false));
    assert_eq!(tab, surface.act(KeybindAction::NewTab, false));
    assert_eq!(tap(&mut surface, 6, 28), None);
}

#[tokio::test]
async fn the_sidebar_glyphs_collapse_and_expand_it() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None])]));
    frame(&mut surface, &HashSet::new());

    assert_eq!(tap(&mut surface, 24, 29), None);
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.areas.sidebar.width, 4);

    assert_eq!(cell(&surface, 1, 29).symbol, "»");
    tap(&mut surface, 1, 29);
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.areas.sidebar.width, 26);
}

#[tokio::test]
async fn the_tab_scroll_buttons_move_the_strip_and_a_redraw_keeps_it() {
    let tabs = [None; 20];
    let mut surface = plain(90, 10);
    surface.update(spaces(&[(Some("main"), &tabs)]));
    frame(&mut surface, &HashSet::new());
    let right = find(&surface, 0, " > ") + 1;
    assert!(span(&surface, 0, 26..90).contains(" 1 "));

    tap(&mut surface, right, 0);
    assert!(surface.needs_frame());
    frame(&mut surface, &HashSet::new());
    let bar = span(&surface, 0, 26..90);
    assert!(!bar.contains(" 1 ") && bar.contains(" 2 "), "{bar:?}");

    surface.update(spaces(&[(Some("renamed"), &tabs)]));
    frame(&mut surface, &HashSet::new());
    assert!(line(&surface, 2).starts_with(" renamed "));
    assert!(!span(&surface, 0, 26..90).contains(" 1 "));

    tap(&mut surface, 27, 0);
    frame(&mut surface, &HashSet::new());
    assert!(span(&surface, 0, 26..90).contains(" 1 "));
    assert_eq!(tab(&surface), "tab1");
}

#[tokio::test]
async fn the_wheel_over_the_tabs_goes_to_the_previous_or_next_tab_and_wraps() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None, None, None])]));
    frame(&mut surface, &HashSet::new());
    let first = find(&surface, 0, " 1 ") + 1;
    let plus = find(&surface, 0, " + ") + 1;

    wheel(&mut surface, true, first, 0);
    assert_eq!(tab(&surface), "tab2");
    wheel(&mut surface, false, plus, 0);
    assert_eq!(tab(&surface), "tab1");
    wheel(&mut surface, false, first, 0);
    assert_eq!(tab(&surface), "tab3");
    wheel(&mut surface, true, first, 0);
    assert_eq!(tab(&surface), "tab1");

    wheel(&mut surface, true, 100, 0);
    assert_eq!(tab(&surface), "tab1");
}

#[tokio::test]
async fn the_wheel_scrolls_the_workspace_list_and_redraws_keep_it_until_focus_moves() {
    let mut surface = plain(120, 12);
    surface.update(many(12));
    frame(&mut surface, &HashSet::new());

    wheel(&mut surface, true, 5, 4);
    wheel(&mut surface, true, 5, 4);
    wheel(&mut surface, false, 5, 4);
    assert!(surface.needs_frame());
    frame(&mut surface, &HashSet::new());
    assert!(
        line(&surface, 2).starts_with(" ws1 "),
        "{:?}",
        line(&surface, 2)
    );
    assert_eq!(workspace(&surface), "w0");

    let mut renamed = many(12);
    renamed.workspaces[11].name = Some("last".to_owned());
    surface.update(renamed);
    frame(&mut surface, &HashSet::new());
    assert!(line(&surface, 2).starts_with(" ws1 "));

    surface.act(KeybindAction::NextWorkspace, false);
    surface.act(KeybindAction::PreviousWorkspace, false);
    frame(&mut surface, &HashSet::new());
    assert!(line(&surface, 2).starts_with(" ws1 "));
    surface.act(KeybindAction::NextWorkspace, false);
    frame(&mut surface, &HashSet::new());
    surface.act(KeybindAction::PreviousWorkspace, false);
    frame(&mut surface, &HashSet::new());
    assert!(line(&surface, 2).starts_with(" ws0 "));
}

#[tokio::test]
async fn a_workspace_closed_above_the_focused_one_keeps_a_list_scrolled_by_hand() {
    let mut surface = plain(120, 12);
    surface.update(many(12));
    surface.act(KeybindAction::PreviousWorkspace, false);
    frame(&mut surface, &HashSet::new());
    assert_eq!(workspace(&surface), "w11");
    assert!(!line(&surface, 2).starts_with(" ws0 "));

    for _ in 0..12 {
        wheel(&mut surface, false, 5, 4);
    }
    frame(&mut surface, &HashSet::new());
    assert!(line(&surface, 2).starts_with(" ws0 "));

    let mut closed = many(12);
    closed.workspaces.remove(1);
    surface.update(closed);
    frame(&mut surface, &HashSet::new());

    assert_eq!(workspace(&surface), "w11");
    assert!(
        line(&surface, 2).starts_with(" ws0 "),
        "{:?}",
        line(&surface, 2)
    );
    assert!(line(&surface, 3).starts_with(" ws2 "));
}

#[tokio::test]
async fn the_list_scrollbar_jumps_on_a_track_press_and_follows_a_thumb_drag() {
    let mut surface = plain(120, 12);
    surface.update(many(12));
    frame(&mut surface, &HashSet::new());
    assert_eq!(thumb_rows(&surface, 24).first(), Some(&2));

    tap(&mut surface, 24, 9);
    frame(&mut surface, &HashSet::new());
    let shown: Vec<String> = (2..10).map(|y| line(&surface, y)).collect();
    assert!(
        shown.iter().any(|row| row.starts_with(" ws11 ")),
        "{shown:#?}"
    );
    let thumb = thumb_rows(&surface, 24);
    assert_eq!(thumb.last(), Some(&9));

    point(
        &mut surface,
        MouseEventKind::Down(MouseButton::Left),
        24,
        thumb[0],
    );
    point(&mut surface, MouseEventKind::Drag(MouseButton::Left), 60, 1);
    frame(&mut surface, &HashSet::new());
    assert!(line(&surface, 2).starts_with(" ws0 "));
    point(&mut surface, MouseEventKind::Up(MouseButton::Left), 60, 1);

    point(&mut surface, MouseEventKind::Drag(MouseButton::Left), 24, 9);
    assert!(!surface.needs_frame());
}

#[tokio::test]
async fn a_click_on_a_collapsed_workspace_number_focuses_it() {
    let mut surface = dressed(120, 30, |config| config.ui.sidebar_start_collapsed = true);
    surface.update(spaces(&[(Some("main"), &[None]), (Some("other"), &[None])]));
    frame(&mut surface, &HashSet::new());

    tap(&mut surface, 2, 1);
    assert_eq!(workspace(&surface), "w1");
    tap(&mut surface, 0, 0);
    assert_eq!(workspace(&surface), "w0");
}

#[tokio::test]
async fn chrome_events_reach_no_pane_and_move_no_focus() {
    let mut surface = plain(120, 30);
    let mut layout = spaces(&[(None, &[None, None])]);
    layout.workspaces[0].tabs[0].root =
        serde_json::from_value(balanced(&["t1".to_owned(), "t9".to_owned()])).expect("root");
    surface.update(layout);
    let mut inputs = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1003h\x1b[?1006h");
    frame(&mut surface, &HashSet::new());
    let focused = surface.focused_id();
    let right = pane_inner(&surface, "t9");
    inputs.iter_mut().for_each(|input| {
        reports(input);
    });

    for (kind, column, row) in [
        (MouseEventKind::Moved, 5, 2),
        (MouseEventKind::Down(MouseButton::Left), 5, 2),
        (MouseEventKind::Drag(MouseButton::Left), right.x, right.y),
        (MouseEventKind::Up(MouseButton::Left), right.x, right.y),
        (MouseEventKind::Down(MouseButton::Right), 100, 0),
        (MouseEventKind::Up(MouseButton::Right), right.x, right.y),
        (MouseEventKind::ScrollDown, 5, 10),
        (MouseEventKind::ScrollUp, 100, 0),
    ] {
        point(&mut surface, kind, column, row);
    }

    for input in &mut inputs {
        assert_eq!(reports(input), b"");
    }
    assert_eq!(surface.focused_id(), focused);
    assert!(surface.gesture.is_none() && surface.select.shown().is_none());
}

#[tokio::test]
async fn a_left_press_on_the_chrome_clears_the_selection_and_the_wheel_does_not() {
    let mut surface = plain(120, 30);
    surface.select = super::super::Select::new(false);
    surface.update(spaces(&[(None, &[None])]));
    attach_with_input(&mut surface, b"hello world");
    frame(&mut surface, &HashSet::new());
    let inner = pane_inner(&surface, "t1");
    let select = |surface: &mut Surface| {
        mouse(surface, click(inner.x, inner.y));
        mouse(surface, drag_to(inner.x + 4, inner.y));
        mouse(surface, release_at(inner.x + 4, inner.y));
        assert!(surface.select.shown().is_some());
    };

    select(&mut surface);
    wheel(&mut surface, true, 5, 5);
    wheel(&mut surface, true, 100, 0);
    assert!(surface.select.shown().is_some());

    tap(&mut surface, 5, 10);
    assert!(surface.select.shown().is_none());
}

#[tokio::test]
async fn a_pane_selection_dragged_over_the_chrome_keeps_selecting_and_copies_on_release() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None, None])]));
    attach_with_input(&mut surface, b"hello world");
    frame(&mut surface, &HashSet::new());
    let inner = pane_inner(&surface, "t1");

    mouse(&mut surface, click(inner.x + 4, inner.y));
    mouse(&mut surface, drag_to(10, inner.y));
    mouse(&mut surface, mouse_at(MouseEventKind::ScrollDown, 30, 0));
    let copied = mouse(&mut surface, release_at(10, inner.y));

    assert_eq!(copied.as_deref(), Some("hello"));
    assert_eq!(tab(&surface), "tab1");
}

#[tokio::test]
async fn a_press_on_the_chrome_keeps_copy_mode_and_its_keyboard_selection() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(Some("main"), &[None]), (Some("other"), &[None])]));
    frame_attaching(&mut surface, &HashSet::new());
    feed(&surface, "t1", b"hello");
    let dirty = HashSet::from([id_of(&surface, "t1")]);
    frame(&mut surface, &dirty);
    surface.enter_copy_mode();
    for key in ['h', 'v', 'h'] {
        surface.copy_mode_key(&TerminalKey::new(KeyCode::Char(key), KeyModifiers::NONE));
    }
    assert_eq!(shown_text(&surface).as_deref(), Some("lo"));

    tap(&mut surface, 5, 10);
    frame(&mut surface, &dirty);
    assert!(surface.copy_mode());
    assert_eq!(shown_text(&surface).as_deref(), Some("lo"));

    tap(&mut surface, 5, 3);
    frame_attaching(&mut surface, &HashSet::new());
    assert_eq!(workspace(&surface), "w1");
    assert!(!surface.copy_mode());

    tap(&mut surface, 5, 2);
    assert_eq!(workspace(&surface), "w0");
    assert!(surface.copy_mode());
}

#[tokio::test]
async fn tabs_under_the_copy_mode_bar_take_no_clicks_or_wheel() {
    let mut surface = dressed(120, 30, |config| {
        config.ui.tab_bar_position = TabBarPositionConfig::Bottom;
    });
    surface.update(spaces(&[(None, &[None, None])]));
    frame_attaching(&mut surface, &HashSet::new());
    feed(&surface, "t1", b"hello");
    let dirty = HashSet::from([id_of(&surface, "t1")]);
    frame(&mut surface, &dirty);
    let second = find(&surface, 29, " 2 ") + 1;
    surface.enter_copy_mode();
    frame(&mut surface, &dirty);
    assert_eq!(surface.painted.copy.bar, surface.areas.tab_bar);

    tap(&mut surface, second, 29);
    assert_eq!(tab(&surface), "tab1");
    wheel(&mut surface, true, second, 29);
    assert_eq!(tab(&surface), "tab1");

    surface.copy_mode_key(&TerminalKey::new(KeyCode::Char('q'), KeyModifiers::NONE));
    frame(&mut surface, &dirty);
    tap(&mut surface, second, 29);
    assert_eq!(tab(&surface), "tab2");
}

struct Scratch(std::path::PathBuf);

impl std::ops::Deref for Scratch {
    type Target = std::path::Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("lawn-sidebar-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch directory");
    Scratch(dir)
}

fn remembered(path: &std::path::Path, configure: impl FnOnce(&mut Config)) -> Surface {
    let mut config = Config::default();
    configure(&mut config);
    let mut surface = around(
        120,
        30,
        Chrome::new(&config).remembering(path.to_path_buf()),
    );
    surface.update(spaces(&[(None, &[None])]));
    frame(&mut surface, &HashSet::new());
    surface
}

fn saved(path: &std::path::Path) -> Option<(Option<u16>, Option<bool>)> {
    crate::client::preferences::load(path)
        .map(|saved| (saved.sidebar_width, saved.sidebar_collapsed))
}

fn drag_divider(surface: &mut Surface, from: u16, to: u16) {
    point(surface, MouseEventKind::Down(MouseButton::Left), from, 5);
    point(surface, MouseEventKind::Drag(MouseButton::Left), to, 5);
    point(surface, MouseEventKind::Up(MouseButton::Left), to, 5);
    frame(surface, &HashSet::new());
}

#[tokio::test]
async fn dragging_the_sidebar_divider_resizes_the_sidebar_within_its_bounds_and_lays_out_the_panes()
{
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None])]));
    frame(&mut surface, &HashSet::new());
    assert_eq!(cell(&surface, 25, 5).symbol, "│");

    point(&mut surface, MouseEventKind::Down(MouseButton::Left), 25, 5);
    for (column, width) in [(30, 31), (2, 18), (100, 36), (21, 22)] {
        point(
            &mut surface,
            MouseEventKind::Drag(MouseButton::Left),
            column,
            5,
        );
        assert!(surface.needs_frame(), "{column}");
        frame(&mut surface, &HashSet::new());

        assert_eq!(surface.areas.sidebar.width, width, "{column}");
        assert_eq!(pane_area(&surface).x, width);
        assert_eq!(surface.tiles.tiles[0].info.rect.x, width);
        assert_eq!(cell(&surface, width - 1, 5).symbol, "│");
    }
    point(&mut surface, MouseEventKind::Up(MouseButton::Left), 21, 5);
    point(&mut surface, MouseEventKind::Drag(MouseButton::Left), 40, 5);
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.areas.sidebar.width, 22);
}

#[tokio::test]
async fn a_divider_drag_over_a_pane_and_below_the_screen_keeps_resizing_and_reaches_no_pane() {
    let mut surface = plain(120, 30);
    let mut layout = spaces(&[(None, &[None])]);
    layout.workspaces[0].tabs[0].root =
        serde_json::from_value(balanced(&["t1".to_owned(), "t9".to_owned()])).expect("root");
    surface.update(layout);
    let mut inputs = attach_with_input(&mut surface, b"\x1b[?1000h\x1b[?1003h\x1b[?1006h");
    frame(&mut surface, &HashSet::new());
    let focused = surface.focused_id();
    let right = pane_inner(&surface, "t9");
    inputs.iter_mut().for_each(|input| {
        reports(input);
    });

    point(&mut surface, MouseEventKind::Down(MouseButton::Left), 25, 5);
    point(
        &mut surface,
        MouseEventKind::Drag(MouseButton::Left),
        right.x,
        right.y,
    );
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.areas.sidebar.width, 36);
    point(
        &mut surface,
        MouseEventKind::Drag(MouseButton::Left),
        29,
        200,
    );
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.areas.sidebar.width, 30);
    let right = pane_inner(&surface, "t9");
    point(
        &mut surface,
        MouseEventKind::Up(MouseButton::Left),
        right.x,
        right.y,
    );

    for input in &mut inputs {
        assert_eq!(reports(input), b"");
    }
    assert_eq!(surface.focused_id(), focused);
    assert!(surface.gesture.is_none() && surface.select.shown().is_none());
}

#[tokio::test]
async fn the_divider_does_not_resize_a_collapsed_sidebar_or_one_hidden_at_the_mobile_width() {
    let mut surface = dressed(120, 30, |config| config.ui.sidebar_start_collapsed = true);
    surface.update(spaces(&[(None, &[None])]));
    frame(&mut surface, &HashSet::new());
    drag_divider(&mut surface, 3, 30);
    assert_eq!(surface.areas.sidebar.width, 4);
    surface.act(KeybindAction::ToggleSidebar, false);
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.areas.sidebar.width, 26);

    let mut surface = dressed(64, 20, |config| config.ui.mobile_width_threshold = 64);
    surface.update(spaces(&[(None, &[None])]));
    frame(&mut surface, &HashSet::new());
    drag_divider(&mut surface, 25, 30);
    surface.resize(host(120, 20));
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.areas.sidebar.width, 26);
}

fn click_divider(surface: &mut Surface, column: u16, now: std::time::Instant) {
    point_at(
        surface,
        MouseEventKind::Down(MouseButton::Left),
        column,
        5,
        now,
    );
    point_at(
        surface,
        MouseEventKind::Up(MouseButton::Left),
        column,
        5,
        now,
    );
}

#[tokio::test]
async fn a_double_click_on_the_divider_restores_the_configured_width() {
    let mut surface = dressed(120, 30, |config| config.ui.sidebar_width = 24);
    surface.update(spaces(&[(None, &[None])]));
    frame(&mut surface, &HashSet::new());
    drag_divider(&mut surface, 23, 30);
    assert_eq!(surface.areas.sidebar.width, 31);
    let start = std::time::Instant::now() + std::time::Duration::from_secs(5);

    click_divider(&mut surface, 30, start);
    click_divider(
        &mut surface,
        30,
        start + std::time::Duration::from_millis(350),
    );
    frame(&mut surface, &HashSet::new());

    assert_eq!(surface.areas.sidebar.width, 24);
}

#[tokio::test]
async fn a_second_divider_press_after_350_ms_starts_a_drag_instead_of_resetting() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None])]));
    frame(&mut surface, &HashSet::new());
    drag_divider(&mut surface, 25, 30);
    let start = std::time::Instant::now() + std::time::Duration::from_secs(5);

    click_divider(&mut surface, 30, start);
    let later = start + std::time::Duration::from_millis(351);
    point_at(
        &mut surface,
        MouseEventKind::Down(MouseButton::Left),
        30,
        5,
        later,
    );
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.areas.sidebar.width, 31);
    point_at(
        &mut surface,
        MouseEventKind::Drag(MouseButton::Left),
        19,
        5,
        later,
    );
    frame(&mut surface, &HashSet::new());

    assert_eq!(surface.areas.sidebar.width, 20);
}

#[tokio::test]
async fn toggling_the_sidebar_by_key_or_click_saves_only_the_collapse_state() {
    let dir = scratch("toggle");
    let path = dir.join("preferences.json");
    let mut surface = remembered(&path, |_| {});

    surface.act(KeybindAction::ToggleSidebar, false);
    assert_eq!(saved(&path), Some((None, Some(true))));
    frame(&mut surface, &HashSet::new());
    tap(&mut surface, 1, 29);
    assert_eq!(saved(&path), Some((None, Some(false))));
}

#[tokio::test]
async fn a_right_press_during_a_divider_drag_leaves_the_drag_going() {
    let dir = scratch("right-press");
    let path = dir.join("preferences.json");
    let mut surface = remembered(&path, |_| {});

    point(&mut surface, MouseEventKind::Down(MouseButton::Left), 25, 5);
    point(&mut surface, MouseEventKind::Drag(MouseButton::Left), 30, 5);
    point(
        &mut surface,
        MouseEventKind::Down(MouseButton::Right),
        30,
        5,
    );
    point(&mut surface, MouseEventKind::Up(MouseButton::Right), 30, 5);
    point(&mut surface, MouseEventKind::Drag(MouseButton::Left), 32, 5);
    point(&mut surface, MouseEventKind::Up(MouseButton::Left), 32, 5);
    frame(&mut surface, &HashSet::new());

    assert_eq!(pane_area(&surface).x, 33);
    assert_eq!(saved(&path), Some((Some(33), None)));
}

#[tokio::test]
async fn a_save_keeps_what_another_client_saved_after_the_start() {
    let dir = scratch("other-client");
    let path = dir.join("preferences.json");
    let mut surface = remembered(&path, |_| {});
    std::fs::write(&path, r#"{"sidebar_section_split":0.25}"#).expect("write");

    surface.act(KeybindAction::ToggleSidebar, false);

    let saved = crate::client::preferences::load(&path).expect("saved");
    assert_eq!(saved.sidebar_collapsed, Some(true));
    assert_eq!(saved.sidebar_section_split, Some(0.25));
}

#[tokio::test]
async fn a_divider_drag_saves_its_width_on_release_and_not_while_dragging() {
    let dir = scratch("drag");
    let path = dir.join("preferences.json");
    let mut surface = remembered(&path, |_| {});

    point(&mut surface, MouseEventKind::Down(MouseButton::Left), 25, 5);
    for column in [30, 33] {
        point(
            &mut surface,
            MouseEventKind::Drag(MouseButton::Left),
            column,
            5,
        );
        frame(&mut surface, &HashSet::new());
        assert_eq!(saved(&path), None);
    }
    point(&mut surface, MouseEventKind::Up(MouseButton::Left), 33, 5);

    assert_eq!(saved(&path), Some((Some(34), None)));
    let later = std::time::Instant::now() + std::time::Duration::from_secs(5);
    for (from, to, width, at) in [(33, 100, 36, 0), (35, 2, 18, 1)] {
        let at = later + std::time::Duration::from_secs(at);
        point_at(
            &mut surface,
            MouseEventKind::Down(MouseButton::Left),
            from,
            5,
            at,
        );
        point_at(
            &mut surface,
            MouseEventKind::Drag(MouseButton::Left),
            to,
            5,
            at,
        );
        point_at(
            &mut surface,
            MouseEventKind::Up(MouseButton::Left),
            to,
            5,
            at,
        );
        frame(&mut surface, &HashSet::new());
        assert_eq!(saved(&path), Some((Some(width), None)), "{to}");
    }
}

#[tokio::test]
async fn a_double_click_reset_saves_that_the_width_is_no_longer_set_by_hand() {
    let dir = scratch("reset");
    let path = dir.join("preferences.json");
    let mut surface = remembered(&path, |_| {});
    drag_divider(&mut surface, 25, 30);
    surface.act(KeybindAction::ToggleSidebar, false);
    surface.act(KeybindAction::ToggleSidebar, false);
    frame(&mut surface, &HashSet::new());
    assert_eq!(saved(&path), Some((Some(31), Some(false))));
    let start = std::time::Instant::now() + std::time::Duration::from_secs(5);

    click_divider(&mut surface, 30, start);
    click_divider(
        &mut surface,
        30,
        start + std::time::Duration::from_millis(100),
    );

    assert_eq!(saved(&path), Some((None, Some(false))));
}

#[tokio::test]
async fn a_restart_restores_the_saved_width_and_collapse_state_over_config() {
    let dir = scratch("restart");
    let path = dir.join("preferences.json");
    let mut surface = remembered(&path, |_| {});
    drag_divider(&mut surface, 25, 30);
    surface.act(KeybindAction::ToggleSidebar, false);

    let mut surface = remembered(&path, |config| config.ui.sidebar_width = 20);
    assert_eq!(surface.areas.sidebar.width, 4);
    surface.act(KeybindAction::ToggleSidebar, false);
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.areas.sidebar.width, 31);
    assert_eq!(saved(&path), Some((Some(31), Some(false))));
}

#[tokio::test]
async fn a_missing_or_corrupt_preferences_file_leaves_the_sidebar_to_config() {
    let dir = scratch("fallback");
    let corrupt = dir.join("corrupt.json");
    std::fs::write(&corrupt, b"{ not json").expect("corrupt preferences");
    for path in [dir.join("missing.json"), corrupt] {
        let surface = remembered(&path, |config| {
            config.ui.sidebar_width = 30;
            config.ui.sidebar_start_collapsed = false;
        });

        assert_eq!(surface.areas.sidebar.width, 30, "{}", path.display());
    }
}

#[tokio::test]
async fn a_failed_save_leaves_the_sidebar_working() {
    let dir = scratch("unwritable");
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, b"").expect("blocker file");
    let mut surface = remembered(&blocker.join("preferences.json"), |_| {});

    surface.act(KeybindAction::ToggleSidebar, false);
    frame(&mut surface, &HashSet::new());

    assert_eq!(surface.areas.sidebar.width, 4);
}

#[test]
fn each_session_keeps_its_own_preferences_file_and_the_default_session_shares_herdrs() {
    use crate::core_client::tui::chrome::preferences_path;

    assert_eq!(preferences_path("one"), preferences_path("one"));
    assert_ne!(preferences_path("one"), preferences_path("two"));
    assert_eq!(
        preferences_path(crate::session::DEFAULT_SESSION_NAME),
        crate::client::preferences::path_for_local_endpoint(
            &crate::session::client_socket_path_for(None)
        )
    );
}

#[tokio::test]
async fn a_still_press_on_the_divider_of_an_oversized_configured_width_leaves_it_to_config() {
    let dir = scratch("oversized");
    let path = dir.join("preferences.json");
    let mut surface = remembered(&path, |config| config.ui.sidebar_width = 100);
    assert_eq!(surface.areas.sidebar.width, 36);

    drag_divider(&mut surface, 35, 35);

    assert_eq!(saved(&path), Some((None, None)));
}

fn moved(surface: &mut Surface, terminal: &str, folder: &str) {
    surface.heard(Report::Folder(terminal.to_owned(), folder.into()));
}

fn first_row(surface: &mut Surface) -> String {
    frame(surface, &HashSet::new());
    line(surface, 2)
}

#[tokio::test]
async fn an_unnamed_workspace_reads_as_the_folder_of_its_first_pane_and_follows_it() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None])]));
    frame(&mut surface, &HashSet::new());

    surface.heard(Report::Snapshot(
        [("t1".to_owned(), "/srv/app".into())].into_iter().collect(),
    ));
    assert!(surface.needs_frame());
    assert!(first_row(&mut surface).starts_with(" app "));

    moved(&mut surface, "t1", "/srv/app/src");
    assert!(surface.needs_frame());
    assert!(first_row(&mut surface).starts_with(" src "));
}

#[tokio::test]
async fn a_fresh_snapshot_forgets_the_folders_it_no_longer_lists() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None])]));
    moved(&mut surface, "t1", "/srv/app");
    assert!(first_row(&mut surface).starts_with(" app "));

    surface.heard(Report::Snapshot(Default::default()));

    assert!(surface.needs_frame());
    assert!(first_row(&mut surface).starts_with(" workspace "));
}

#[tokio::test]
async fn a_named_workspace_keeps_its_name_wherever_its_first_pane_goes() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(Some("main"), &[None])]));
    frame(&mut surface, &HashSet::new());

    moved(&mut surface, "t1", "/srv/app");

    assert!(!surface.needs_frame());
    surface.resize(surface.host);
    assert!(first_row(&mut surface).starts_with(" main "));
}

#[tokio::test]
async fn the_home_folder_reads_as_a_tilde() {
    std::env::set_var("HOME", "/home/lawn-test");
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None])]));

    moved(&mut surface, "t1", "/home/lawn-test");

    assert!(first_row(&mut surface).starts_with(" ~ "));
}

#[tokio::test]
async fn only_the_first_pane_of_the_first_tab_names_the_workspace() {
    let mut surface = plain(120, 30);
    surface.update(
        serde_json::from_value(json!({"workspaces": [{"id": "w0", "tabs": [
            {"id": "tab1", "root": {
                "kind": "split", "direction": "right", "ratio": 0.5,
                "first": {"kind": "pane", "terminal": "t1"},
                "second": {"kind": "pane", "terminal": "t2"},
            }},
            {"id": "tab2", "root": {"kind": "pane", "terminal": "t3"}},
        ]}]}))
        .expect("layout"),
    );
    frame(&mut surface, &HashSet::new());

    moved(&mut surface, "t2", "/srv/split");
    moved(&mut surface, "t3", "/srv/second");
    assert!(!surface.needs_frame());
    surface.resize(surface.host);
    assert!(first_row(&mut surface).starts_with(" workspace "));

    moved(&mut surface, "t1", "/srv/first");
    assert!(first_row(&mut surface).starts_with(" first "));
}

#[tokio::test]
async fn a_terminal_that_names_no_workspace_draws_no_frame() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None])]));
    frame(&mut surface, &HashSet::new());

    moved(&mut surface, "t9", "/srv/app");

    assert!(!surface.needs_frame());
    surface.resize(surface.host);
    assert!(first_row(&mut surface).starts_with(" workspace "));
}

#[tokio::test]
async fn a_move_to_another_folder_of_the_same_name_draws_no_frame() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None])]));
    moved(&mut surface, "t1", "/srv/app");
    frame(&mut surface, &HashSet::new());

    moved(&mut surface, "t1", "/opt/app");

    assert!(!surface.needs_frame());
}

mod mobile;
