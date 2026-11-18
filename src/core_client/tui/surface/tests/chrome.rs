use ratatui::style::{Color, Modifier, Style};

use super::*;
use crate::config::{SidebarCollapsedModeConfig, TabBarPositionConfig, TabBarRightEntryConfig};
use crate::protocol::CellData;

fn dressed(cols: u16, rows: u16, configure: impl FnOnce(&mut Config)) -> Surface {
    let mut config = Config::default();
    configure(&mut config);
    Surface::new(
        host(cols, rows),
        PaneChrome {
            borders: PaneBordersConfig::Auto,
            gaps: false,
            outer_borders: true,
        },
        Chrome::new(&config),
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
async fn at_the_mobile_width_threshold_panes_fill_the_whole_screen() {
    let mut surface = dressed(64, 20, |config| config.ui.mobile_width_threshold = 64);
    surface.update(spaces(&[(None, &[None, None])]));
    frame(&mut surface, &HashSet::new());

    assert_eq!(pane_area(&surface), Rect::new(0, 0, 64, 20));
    assert!(surface.areas.sidebar.is_empty() && surface.areas.tab_bar.is_empty());
    assert!(surface.outer.rows().is_empty());
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
