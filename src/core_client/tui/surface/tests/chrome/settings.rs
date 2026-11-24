use super::*;
use crate::config::{ConfigEdit, StatusIndicatorStyle, ToastDelivery};

fn opened(configure: impl FnOnce(&mut Config)) -> (Surface, Screen) {
    opened_in(30, configure)
}

fn opened_in(rows: u16, configure: impl FnOnce(&mut Config)) -> (Surface, Screen) {
    let mut config = Config::default();
    configure(&mut config);
    let mut surface = Surface::new(
        host(120, rows),
        Chrome::new(&config, unheard()),
        Settings::from(&config),
    );
    surface.update(spaces(&[(Some("main"), &[None])]), 0);
    frame_attaching(&mut surface, &HashSet::new());
    surface.act(KeybindAction::Settings, false);
    let screen = Screen::of(&mut surface);
    (surface, screen)
}

fn typed(surface: &mut Surface, code: KeyCode) -> Option<Pointed> {
    surface.dialog_key(&TerminalKey::new(code, KeyModifiers::NONE))
}

fn held(surface: &mut Surface, code: KeyCode) -> Option<Pointed> {
    let mut key = TerminalKey::new(code, KeyModifiers::NONE);
    key.kind = crossterm::event::KeyEventKind::Repeat;
    surface.dialog_key(&key)
}

fn spot(screen: &Screen, text: &str) -> (u16, u16) {
    let width = text.chars().count() as u16;
    (0..30)
        .find_map(|y| {
            (0..120 - width)
                .find(|x| screen.span(y, *x..*x + width) == text)
                .map(|x| (x, y))
        })
        .expect(text)
}

fn press(surface: &mut Surface, (x, y): (u16, u16)) -> Option<Pointed> {
    surface.mouse(
        mouse_at(MouseEventKind::Down(MouseButton::Left), x, y),
        std::time::Instant::now(),
    )
}

fn shown(surface: &mut Surface, screen: &mut Screen) {
    screen.show(surface, &HashSet::new());
}

fn chrome_in(surface: &Surface, palette: Palette) -> bool {
    cell(surface, 1, 2).bg == background(palette.active_row_bg)
}

fn marked(screen: &Screen, label: &str) -> bool {
    let marker = format!("▸ {label}");
    screen
        .lines(0..30)
        .iter()
        .any(|line| line.contains(&marker))
}

fn saved(edit: ConfigEdit<'static>) -> Option<Pointed> {
    Some(Pointed::Save(edit))
}

#[tokio::test]
async fn the_settings_key_opens_every_tab_but_integrations() {
    let (_surface, screen) = opened(|_| {});

    let (_, y) = spot(&screen, " theme ");
    let tabs = screen.line(y);
    assert!(
        tabs.contains(" theme   indicators   sound   toasts "),
        "{tabs}"
    );
    assert!(!tabs.contains("integrations"), "{tabs}");
}

#[tokio::test]
async fn the_dialog_opens_on_the_configured_theme_and_marks_it() {
    let (_surface, screen) = opened(|config| config.theme.name = Some("gruvbox".to_owned()));

    let (_, y) = spot(&screen, "gruvbox ✓");
    assert!(screen.line(y).contains(" ▸ gruvbox ✓"));
}

#[tokio::test]
async fn moving_through_the_themes_repaints_the_chrome_in_each_and_escape_restores_the_configured_one(
) {
    let (mut surface, mut screen) = opened(|_| {});
    assert!(chrome_in(&surface, Palette::catppuccin()));

    assert_eq!(typed(&mut surface, KeyCode::Down), None);
    shown(&mut surface, &mut screen);
    assert!(chrome_in(&surface, Palette::catppuccin_latte()));

    assert_eq!(typed(&mut surface, KeyCode::Esc), None);
    shown(&mut surface, &mut screen);
    assert!(!surface.dialog_open());
    assert!(chrome_in(&surface, Palette::catppuccin()));
}

#[tokio::test]
async fn a_click_on_a_theme_previews_it_and_a_click_off_the_dialog_cancels_the_preview() {
    let (mut surface, mut screen) = opened(|_| {});

    assert_eq!(press(&mut surface, spot(&screen, "dracula")), None);
    shown(&mut surface, &mut screen);
    assert!(surface.dialog_open());
    assert!(chrome_in(&surface, Palette::dracula()));

    assert_eq!(press(&mut surface, (0, 0)), None);
    shown(&mut surface, &mut screen);
    assert!(!surface.dialog_open());
    assert!(chrome_in(&surface, Palette::catppuccin()));
}

#[tokio::test]
async fn enter_saves_the_selected_theme_and_the_reload_after_it_closes_the_dialog_in_that_theme() {
    let (mut surface, mut screen) = opened(|_| {});
    typed(&mut surface, KeyCode::Down);
    shown(&mut surface, &mut screen);

    assert_eq!(
        typed(&mut surface, KeyCode::Enter),
        saved(ConfigEdit::Theme("catppuccin-latte"))
    );
    assert!(surface.dialog_open());

    surface.configure(&toml::from_str("[theme]\nname = \"catppuccin-latte\"").expect("config"));
    surface.saved(true);
    shown(&mut surface, &mut screen);
    assert!(!surface.dialog_open());
    assert!(chrome_in(&surface, Palette::catppuccin_latte()));
}

#[tokio::test]
async fn a_refused_save_restores_the_configured_theme_and_selects_it_again() {
    let (mut surface, mut screen) = opened(|_| {});
    typed(&mut surface, KeyCode::Down);
    shown(&mut surface, &mut screen);
    typed(&mut surface, KeyCode::Enter);

    surface.saved(false);
    shown(&mut surface, &mut screen);

    assert!(surface.dialog_open());
    assert!(chrome_in(&surface, Palette::catppuccin()));
    typed(&mut surface, KeyCode::Tab);
    typed(&mut surface, KeyCode::BackTab);
    shown(&mut surface, &mut screen);
    assert_eq!(
        typed(&mut surface, KeyCode::Enter),
        saved(ConfigEdit::Theme("catppuccin"))
    );
}

#[tokio::test]
async fn a_click_on_an_indicator_style_saves_it_at_once_and_the_dialog_shows_it_after_the_reload() {
    let (mut surface, mut screen) = opened(|_| {});
    typed(&mut surface, KeyCode::Tab);
    shown(&mut surface, &mut screen);

    assert_eq!(
        press(&mut surface, spot(&screen, "distinct symbols  ×")),
        saved(ConfigEdit::StatusIndicators(StatusIndicatorStyle::Symbols))
    );
    surface.configure(&toml::from_str("[ui]\nstatus_indicators = \"symbols\"").expect("config"));
    surface.saved(true);
    assert!(surface.dialog_open());

    typed(&mut surface, KeyCode::BackTab);
    typed(&mut surface, KeyCode::Tab);
    shown(&mut surface, &mut screen);
    assert_eq!(
        typed(&mut surface, KeyCode::Enter),
        saved(ConfigEdit::StatusIndicators(StatusIndicatorStyle::Symbols))
    );
}

#[tokio::test]
async fn a_tab_click_shows_that_section_on_its_configured_choice() {
    let (mut surface, mut screen) =
        opened(|config| config.ui.status_indicators = StatusIndicatorStyle::Symbols);

    assert_eq!(press(&mut surface, spot(&screen, " indicators ")), None);
    shown(&mut surface, &mut screen);
    assert_eq!(
        typed(&mut surface, KeyCode::Enter),
        saved(ConfigEdit::StatusIndicators(StatusIndicatorStyle::Symbols))
    );
}

#[tokio::test]
async fn back_tab_from_the_first_section_wraps_to_the_last() {
    let (mut surface, mut screen) =
        opened(|config| config.ui.toast.delivery = ToastDelivery::Terminal);

    typed(&mut surface, KeyCode::BackTab);
    shown(&mut surface, &mut screen);

    assert_eq!(
        typed(&mut surface, KeyCode::Char(' ')),
        saved(ConfigEdit::ToastDelivery(ToastDelivery::Terminal))
    );
}

#[tokio::test]
async fn a_click_on_a_sound_choice_saves_it_at_once_and_the_tab_reopens_on_it() {
    let (mut surface, mut screen) = opened(|config| config.ui.sound.enabled = true);
    press(&mut surface, spot(&screen, " sound "));
    shown(&mut surface, &mut screen);
    assert!(marked(&screen, "on"));

    assert_eq!(
        press(&mut surface, spot(&screen, "   off")),
        saved(ConfigEdit::Sound(false))
    );
    surface.configure(&toml::from_str("[ui.sound]\nenabled = false").expect("config"));
    surface.saved(true);
    assert!(surface.dialog_open());

    typed(&mut surface, KeyCode::BackTab);
    typed(&mut surface, KeyCode::Tab);
    shown(&mut surface, &mut screen);
    assert!(marked(&screen, "off"));
    typed(&mut surface, KeyCode::Up);
    shown(&mut surface, &mut screen);
    assert_eq!(
        typed(&mut surface, KeyCode::Enter),
        saved(ConfigEdit::Sound(true))
    );
}

#[tokio::test]
async fn the_toasts_tab_opens_on_the_configured_delivery_and_saves_the_one_moved_to() {
    let (mut surface, mut screen) =
        opened(|config| config.ui.toast.delivery = ToastDelivery::Herdr);
    press(&mut surface, spot(&screen, " toasts "));
    shown(&mut surface, &mut screen);
    assert!(marked(&screen, "inside herdr"));

    typed(&mut surface, KeyCode::Down);
    typed(&mut surface, KeyCode::Down);
    typed(&mut surface, KeyCode::Down);
    shown(&mut surface, &mut screen);

    assert_eq!(
        typed(&mut surface, KeyCode::Enter),
        saved(ConfigEdit::ToastDelivery(ToastDelivery::System))
    );
    typed(&mut surface, KeyCode::Up);
    shown(&mut surface, &mut screen);
    assert_eq!(
        typed(&mut surface, KeyCode::Enter),
        saved(ConfigEdit::ToastDelivery(ToastDelivery::Terminal))
    );
}

#[tokio::test]
async fn a_short_screen_still_draws_every_toast_delivery() {
    let (mut surface, mut screen) = opened_in(18, |_| {});
    typed(&mut surface, KeyCode::BackTab);
    typed(&mut surface, KeyCode::Down);
    typed(&mut surface, KeyCode::Down);
    typed(&mut surface, KeyCode::Down);
    shown(&mut surface, &mut screen);

    assert!(screen
        .lines(0..18)
        .iter()
        .any(|line| line.contains("   off")));
    assert_eq!(
        typed(&mut surface, KeyCode::Enter),
        saved(ConfigEdit::ToastDelivery(ToastDelivery::System))
    );
}

#[tokio::test]
async fn a_screen_too_short_for_every_choice_keeps_the_selected_one_drawn() {
    for rows in [15, 16, 17] {
        let (mut surface, mut screen) = opened_in(rows, |_| {});
        typed(&mut surface, KeyCode::BackTab);
        for delivery in [
            ToastDelivery::Off,
            ToastDelivery::Herdr,
            ToastDelivery::Terminal,
            ToastDelivery::System,
        ] {
            shown(&mut surface, &mut screen);
            assert_eq!(
                typed(&mut surface, KeyCode::Enter),
                saved(ConfigEdit::ToastDelivery(delivery)),
                "{rows} rows"
            );
            typed(&mut surface, KeyCode::Down);
        }
    }
}

#[tokio::test]
async fn enter_saves_only_a_choice_the_last_frame_drew_and_a_held_enter_or_space_saves_nothing() {
    let (mut surface, mut screen) = opened(|_| {});

    typed(&mut surface, KeyCode::Tab);
    assert_eq!(typed(&mut surface, KeyCode::Enter), None);

    shown(&mut surface, &mut screen);
    assert_eq!(held(&mut surface, KeyCode::Enter), None);
    assert_eq!(held(&mut surface, KeyCode::Char(' ')), None);
    assert_eq!(
        typed(&mut surface, KeyCode::Enter),
        saved(ConfigEdit::StatusIndicators(StatusIndicatorStyle::Dots))
    );
}

#[tokio::test]
async fn returning_to_the_theme_tab_keeps_the_previewed_theme_selected_and_painted() {
    let (mut surface, mut screen) = opened(|_| {});
    typed(&mut surface, KeyCode::Down);
    typed(&mut surface, KeyCode::Tab);
    typed(&mut surface, KeyCode::BackTab);
    shown(&mut surface, &mut screen);

    assert!(marked(&screen, "catppuccin-latte"));
    assert!(chrome_in(&surface, Palette::catppuccin_latte()));
    assert!(screen
        .lines(0..30)
        .iter()
        .any(|line| line.contains("   catppuccin ✓")));
}

#[tokio::test]
async fn a_click_on_a_theme_row_after_a_tab_not_yet_drawn_paints_the_theme_it_selects() {
    let (mut surface, mut screen) = opened(|_| {});
    let dracula = spot(&screen, "dracula");

    typed(&mut surface, KeyCode::Tab);
    assert_eq!(press(&mut surface, dracula), None);
    shown(&mut surface, &mut screen);

    assert!(marked(&screen, "dracula"));
    assert!(chrome_in(&surface, Palette::dracula()));
}

#[tokio::test]
async fn a_click_on_the_selected_theme_paints_it_while_auto_switch_paints_another() {
    let (mut surface, mut screen) = opened(|config| {
        config.theme.name = Some("dracula".to_owned());
        config.theme.auto_switch = true;
        config.theme.dark_name = Some("nord".to_owned());
    });
    assert!(chrome_in(&surface, Palette::nord()));
    assert!(marked(&screen, "dracula"));

    assert_eq!(press(&mut surface, spot(&screen, "dracula")), None);
    shown(&mut surface, &mut screen);

    assert!(chrome_in(&surface, Palette::dracula()));
}
