use super::*;
use crate::core_client::tui::surface::reloaded;

fn config(toml: &str) -> Config {
    toml::from_str(toml).expect("config")
}

fn one_space() -> Surface {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(Some("main"), &[None]), (Some("other"), &[None])]));
    frame_attaching(&mut surface, &HashSet::new());
    surface
}

fn row_with(screen: &Screen, text: &str) -> String {
    screen
        .lines(0..30)
        .into_iter()
        .find(|line| line.contains(text))
        .expect(text)
}

#[tokio::test]
async fn a_reload_moves_the_sidebar_to_its_new_width_and_lays_the_panes_out_beside_it() {
    let mut surface = one_space();

    surface.configure(&config("[ui]\nsidebar_width = 30"));
    frame(&mut surface, &HashSet::new());

    assert_eq!(surface.areas.sidebar.width, 30);
    assert_eq!(surface.tiles.tiles[0].info.rect.x, 30);
    assert_eq!(cell(&surface, 29, 5).symbol, "│");
}

#[tokio::test]
async fn a_reload_keeps_a_dragged_sidebar_width_inside_the_new_bounds() {
    let mut surface = one_space();
    drag_divider(&mut surface, 25, 30);
    assert_eq!(surface.areas.sidebar.width, 31);

    surface.configure(&config("[ui]\nsidebar_width = 20"));
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.areas.sidebar.width, 31);

    surface.configure(&config("[ui]\nsidebar_max_width = 28"));
    frame(&mut surface, &HashSet::new());
    assert_eq!(surface.areas.sidebar.width, 28);
}

#[tokio::test]
async fn a_reload_during_a_divider_drag_leaves_the_drag_going() {
    let mut surface = one_space();
    point(&mut surface, MouseEventKind::Down(MouseButton::Left), 25, 5);
    point(&mut surface, MouseEventKind::Drag(MouseButton::Left), 30, 5);

    surface.configure(&Config::default());
    point(&mut surface, MouseEventKind::Drag(MouseButton::Left), 33, 5);
    frame(&mut surface, &HashSet::new());

    assert_eq!(surface.areas.sidebar.width, 34);
}

#[tokio::test]
async fn a_reload_paints_the_chrome_in_the_new_theme() {
    let mut surface = one_space();

    surface.configure(&config("[theme]\nname = \"gruvbox\""));
    frame(&mut surface, &HashSet::new());

    assert_eq!(
        cell(&surface, 1, 2).bg,
        background(Palette::gruvbox().active_row_bg)
    );
}

#[tokio::test]
async fn the_navigate_bar_opened_after_a_reload_shows_the_new_help_key() {
    let mut surface = one_space();
    let mut screen = Screen::of(&mut surface);

    surface.configure(&config("[keys]\nhelp = \"prefix+h\""));
    surface.act(KeybindAction::WorkspacePicker, false);
    screen.show(&mut surface, &HashSet::new());

    assert!(row_with(&screen, " keybinds").contains("h keybinds"));
}

#[tokio::test]
async fn a_reload_keeps_the_help_dialog_open_and_lists_the_new_keys() {
    let mut surface = one_space();
    let mut screen = Screen::of(&mut surface);
    surface.act(KeybindAction::Help, false);
    screen.show(&mut surface, &HashSet::new());
    assert!(!row_with(&screen, "detach").contains("ctrl+q"));

    surface.configure(&config("[keys]\ndetach = \"ctrl+q\""));
    screen.show(&mut surface, &HashSet::new());

    assert!(surface.dialog_open());
    assert!(row_with(&screen, "detach").contains("ctrl+q"));
}

#[tokio::test]
async fn a_reload_gives_the_open_panes_and_the_next_attach_the_new_wheel_step() {
    let mut surface = one_space();
    assert!(!surface.views.is_empty());

    surface.configure(&config("[ui]\nmouse_scroll_lines = 7"));

    assert!(surface
        .views
        .values()
        .all(|shown| shown.view.wheel_lines() == 7));
    assert_eq!(surface.scrollback().wheel_lines, 7);
}

#[tokio::test]
async fn a_reload_that_turns_on_close_confirmation_asks_before_closing_a_workspace() {
    let mut surface = one_space();
    assert!(surface
        .act(KeybindAction::CloseWorkspace, surface.confirms_close())
        .is_some());

    surface.configure(&config("[ui]\nconfirm_close = true"));

    assert_eq!(
        surface.act(KeybindAction::CloseWorkspace, surface.confirms_close()),
        None
    );
    assert!(surface.dialog_open());
}

#[tokio::test]
async fn a_reload_takes_the_pane_borders_scrollbars_and_copy_settings() {
    let mut surface = one_space();

    surface.configure(&config(
        "[ui]\npane_borders = \"off\"\npane_gaps = true\npane_outer_borders = false\n\
         pane_scrollbars = false\ncopy_on_select = false\n\
         [ui.toast.clipboard]\nenabled = false",
    ));

    assert_eq!(
        surface.chrome,
        PaneChrome {
            borders: PaneBordersConfig::Off,
            gaps: true,
            outer_borders: false,
        }
    );
    assert!(!surface.scrollbars);
    assert!(!surface.select.copies_on_select());
    assert_eq!(surface.toast_at, None);
}

#[test]
fn a_reload_keeps_each_section_that_failed_to_parse_from_the_running_config() {
    let mut running = config(
        "[theme]\nname = \"nord\"\n[keys]\nhelp = \"prefix+h\"\n\
         [ui]\nsidebar_width = 31\n[advanced]\nscrollback_limit_bytes = 1234",
    );
    let next = config(
        "[theme]\nname = \"dracula\"\n[keys]\nhelp = \"prefix+j\"\n\
         [ui]\nsidebar_width = 22\n[advanced]\nscrollback_limit_bytes = 5678",
    );
    let invalid = ["theme", "keys", "ui", "advanced"].map(str::to_owned);

    let (kept, _) = reloaded(&mut running, next, &invalid);

    assert_eq!(kept.theme.name.as_deref(), Some("nord"));
    assert_eq!(
        kept.keybinds().help.prefix_rhs_label().as_deref(),
        Some("h")
    );
    assert_eq!(kept.ui.sidebar_width, 31);
    assert_eq!(kept.advanced.scrollback_limit_bytes, 1234);

    let (taken, _) = reloaded(&mut running, config("[ui]\nsidebar_width = 22"), &[]);
    assert_eq!(taken.ui.sidebar_width, 22);
}

#[test]
fn a_reload_with_inverted_sidebar_bounds_keeps_the_whole_running_ui() {
    let mut running = config("[ui]\nconfirm_close = true\nsidebar_width = 30");
    let next =
        config("[ui]\nsidebar_min_width = 40\nsidebar_max_width = 20\nconfirm_close = false");

    let (kept, diagnostics) = reloaded(&mut running, next, &[]);

    assert!(kept.ui.confirm_close);
    assert_eq!(kept.ui.sidebar_width, 30);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
}

#[test]
fn a_reload_whose_keys_leave_no_prefix_keeps_the_running_keys() {
    let mut running = config("[keys]\nprefix = \"ctrl+a\"\nhelp = \"prefix+h\"");
    let prefix = running.prefix_keys();
    let next = config("[keys]\nprefix = \"\"\nhelp = \"prefix+j\"");

    let (kept, diagnostics) = reloaded(&mut running, next, &[]);

    assert_eq!(kept.prefix_keys(), prefix);
    assert_eq!(
        kept.keybinds().help.prefix_rhs_label().as_deref(),
        Some("h")
    );
    assert!(!diagnostics.is_empty());
}

#[tokio::test]
async fn a_pane_whose_attach_was_in_flight_during_a_reload_takes_the_new_wheel_step() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(Some("main"), &[None])]));
    let pending = requests(&mut surface);
    let (_, id, geometry) = pending[0].clone();

    surface.configure(&config("[ui]\nmouse_scroll_lines = 7"));
    surface.attached(id, geometry, Ok(View::test(geometry.cols, geometry.rows)));

    assert_eq!(surface.views[&id].view.wheel_lines(), 7);
}

#[tokio::test]
async fn every_reload_takes_a_visible_toast_away_so_none_stays_at_its_old_place_or_colors() {
    for toml in [
        "[ui]\nsidebar_width = 34\n[ui.toast.clipboard]\nenabled = false",
        "[ui.toast.clipboard]\nposition = \"top-right\"",
        "[theme]\nname = \"tokyo-night\"",
    ] {
        let mut surface = one_space();
        surface.copied(std::time::Instant::now());
        frame(&mut surface, &HashSet::new());
        assert!(!surface.overlays().toast.is_empty());

        surface.configure(&config(toml));
        let screen = Screen::of(&mut surface);

        assert!(surface.overlays().toast.is_empty(), "{toml}");
        assert!(
            !screen
                .lines(0..30)
                .iter()
                .any(|line| line.contains("copied to clipboard")),
            "{toml}"
        );
    }
}

fn started(config: &Config, tabs: usize, focus: &[usize]) -> Surface {
    let mut surface = Surface::new(host(90, 30), Chrome::new(config), Settings::from(config));
    surface.update(spaces(&[(Some("main"), &vec![None; tabs])]));
    frame(&mut surface, &HashSet::new());
    for tab in focus {
        surface.act(KeybindAction::SwitchTab(*tab), false);
        frame(&mut surface, &HashSet::new());
    }
    surface
}

fn strip(surface: &Surface) -> String {
    span(surface, 0, 0..90)
}

#[tokio::test]
async fn after_a_reload_the_tab_strip_shows_the_focused_tab_as_a_fresh_start_would() {
    let status = "[ui]\ntab_bar_right = [{ type = \"text\", text = \"twelve-chars-\" }, \
                  { type = \"text\", text = \"twelve-chars-\" }]\n";
    let next = config(&format!(
        "{status}tab_bar_right_separator = \"elevenchars\""
    ));
    let mut reloaded = started(&config(status), 4, &[1]);

    reloaded.configure(&next);
    frame(&mut reloaded, &HashSet::new());

    let fresh = started(&next, 4, &[1]);
    assert_eq!(strip(&reloaded), strip(&fresh));
    assert!(strip(&fresh).contains(" 2 "), "{}", strip(&fresh));
}

#[test]
fn a_reload_reports_retired_sidebar_tokens_as_the_legacy_reload_does() {
    let mut running = Config::default();
    let next = config("[ui.sidebar.spaces]\nrows = [[\"workspace\"], [\"branch\"]]");

    let (_, diagnostics) = reloaded(&mut running, next, &[]);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("branch")),
        "{diagnostics:?}"
    );
}

#[test]
fn a_reload_reports_a_tab_strip_entry_it_hides_as_the_legacy_reload_does() {
    let mut running = Config::default();
    let next = config("[ui]\ntab_bar_right = [{ type = \"datetime\", format = \"\" }]");

    let (_, diagnostics) = reloaded(&mut running, next, &[]);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("tab_bar_right[0]")),
        "{diagnostics:?}"
    );
}

#[tokio::test]
async fn a_reload_that_changes_nothing_leaves_a_scrolled_sidebar_where_it_was() {
    let mut surface = plain(120, 12);
    surface.update(many(12));
    frame(&mut surface, &HashSet::new());
    wheel(&mut surface, true, 5, 4);
    frame(&mut surface, &HashSet::new());
    assert!(
        line(&surface, 2).starts_with(" ws1 "),
        "{:?}",
        line(&surface, 2)
    );

    surface.configure(&Config::default());
    frame(&mut surface, &HashSet::new());

    assert!(
        line(&surface, 2).starts_with(" ws1 "),
        "{:?}",
        line(&surface, 2)
    );
}

#[tokio::test]
async fn a_tab_drag_released_after_a_reload_in_the_same_batch_moves_the_tab() {
    let mut surface = plain(120, 30);
    surface.update(spaces(&[(None, &[None, None, None])]));
    frame(&mut surface, &HashSet::new());
    let (first, plus) = (find(&surface, 0, " 1 ") + 1, find(&surface, 0, " + ") + 1);
    point(
        &mut surface,
        MouseEventKind::Down(MouseButton::Left),
        first,
        0,
    );
    point(
        &mut surface,
        MouseEventKind::Drag(MouseButton::Left),
        plus,
        0,
    );
    frame(&mut surface, &HashSet::new());

    surface.configure(&config("[ui]\nmouse_scroll_lines = 7"));

    assert_eq!(
        point(&mut surface, MouseEventKind::Up(MouseButton::Left), plus, 0),
        Some(Operation::Arrange(
            crate::session_doc::Arrangement::MoveTab {
                tab: "tab1".to_owned(),
                to: crate::session_doc::Place::End,
            }
        ))
    );
}

#[tokio::test]
async fn a_reload_that_changes_nothing_leaves_a_scrolled_tab_strip_where_it_was() {
    let mut surface = plain(90, 10);
    surface.update(spaces(&[(None, &[None; 20])]));
    frame(&mut surface, &HashSet::new());
    let right = find(&surface, 0, " > ") + 1;
    tap(&mut surface, right, 0);
    frame(&mut surface, &HashSet::new());
    let scrolled = span(&surface, 0, 26..90);
    assert!(!scrolled.contains(" 1 "), "{scrolled:?}");

    surface.configure(&Config::default());
    frame(&mut surface, &HashSet::new());

    assert_eq!(span(&surface, 0, 26..90), scrolled);
}

fn scrolled_with(status: &str) -> (Surface, String) {
    scrolled_at(90, status)
}

fn scrolled_at(cols: u16, status: &str) -> (Surface, String) {
    let config = config(status);
    let mut surface = Surface::new(
        host(cols, 10),
        Chrome::new(&config),
        Settings::from(&config),
    );
    surface.update(spaces(&[(None, &[None; 20])]));
    frame(&mut surface, &HashSet::new());
    let right = find(&surface, 0, " > ") + 1;
    tap(&mut surface, right, 0);
    frame(&mut surface, &HashSet::new());
    let scrolled = span(&surface, 0, 26..cols);
    assert!(!scrolled.contains(" 1 "), "{scrolled:?}");
    (surface, scrolled)
}

fn status(text: &str, separator: &str) -> String {
    format!(
        "[ui]\ntab_bar_right = [{{ type = \"text\", text = \"{text}\" }}, \
         {{ type = \"text\", text = \"{text}\" }}]\ntab_bar_right_separator = \"{separator}\""
    )
}

#[tokio::test]
async fn a_reload_whose_status_takes_the_same_space_leaves_a_scrolled_tab_strip_alone() {
    let (mut surface, scrolled) = scrolled_with(&status("abcd", "xx"));

    surface.configure(&config(&status("abc", "xxxx")));
    frame(&mut surface, &HashSet::new());

    let strip = span(&surface, 0, 26..90);
    assert!(!strip.contains(" 1 "), "{strip:?}");
    assert!(strip.ends_with("abcxxxxabc"), "{strip:?}");
    assert_ne!(strip, scrolled);
}

#[tokio::test]
async fn a_reload_whose_status_grows_by_one_cell_brings_the_focused_tab_back_as_any_status_change_does(
) {
    let (mut surface, _) = scrolled_with(&status("abcd", "xx"));

    surface.configure(&config(&status("abcd", "xxx")));
    frame(&mut surface, &HashSet::new());

    let strip = span(&surface, 0, 26..90);
    assert!(strip.contains(" 1 "), "{strip:?}");
}

#[tokio::test]
async fn a_reload_that_changes_a_status_too_wide_to_show_leaves_a_scrolled_tab_strip_alone() {
    let wide = |separator| status(&"w".repeat(24), separator);
    let (mut surface, scrolled) = scrolled_with(&wide("xx"));

    surface.configure(&config(&wide("xxx")));
    frame(&mut surface, &HashSet::new());

    assert_eq!(span(&surface, 0, 26..90), scrolled);
}

#[tokio::test]
async fn a_reload_that_shows_or_hides_the_status_brings_the_focused_tab_back() {
    let (shown, hidden) = (status("abcd", "xx"), status(&"w".repeat(24), "xx"));
    for (from, to) in [(&shown, &hidden), (&hidden, &shown)] {
        let (mut surface, _) = scrolled_with(from);

        surface.configure(&config(to));
        frame(&mut surface, &HashSet::new());

        let strip = span(&surface, 0, 26..90);
        assert!(strip.contains(" 1 "), "{strip:?}");
    }
}

#[tokio::test]
async fn a_wider_tab_bar_whose_status_takes_the_extra_space_leaves_a_scrolled_tab_strip_alone() {
    let label = "w".repeat(13);
    let (mut surface, scrolled) = scrolled_at(70, &status(&label, "xx"));
    assert!(!scrolled.contains(&label), "{scrolled:?}");

    surface.resize(host(99, 10));
    frame(&mut surface, &HashSet::new());

    let strip = span(&surface, 0, 26..99);
    assert!(strip.ends_with(&format!("{label}xx{label}")), "{strip:?}");
    assert!(strip.starts_with(&scrolled), "{strip:?}");
}
