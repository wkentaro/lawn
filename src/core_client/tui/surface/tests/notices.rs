use std::time::{Duration, Instant};

use super::*;
use crate::config::{SoundConfig, ToastDelivery, ToastHerdrPosition};
use crate::core_client::tui::notices::{Effect, Notifying};
use crate::core_client::tui::terminals::Report;
use crate::pane::core_status;
use crate::sound::Sound;

fn notifying(configure: impl FnOnce(&mut Notifying)) -> Surface {
    let mut notifying = Notifying {
        delivery: ToastDelivery::Herdr,
        delay: Duration::ZERO,
        position: ToastHerdrPosition::BottomRight,
        sound: SoundConfig::default(),
    };
    configure(&mut notifying);
    let mut config = Config::default();
    config.ui.sidebar_start_collapsed = true;
    let mut surface = Surface::new(
        host(100, 30),
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
            toast: Some(ToastClipboardPosition::BottomRight),
        },
        keybinds(),
    )
    .notifying(notifying);
    surface.update(paired());
    frame(&mut surface, &HashSet::new());
    surface
}

fn herdr() -> Surface {
    notifying(|_| {})
}

fn paired() -> Layout {
    let pair = |id: &str, first: &str, second: &str| {
        json!({"id": id, "root": {
            "kind": "split", "direction": "right", "ratio": 0.5,
            "first": {"kind": "pane", "terminal": first},
            "second": {"kind": "pane", "terminal": second},
        }})
    };
    serde_json::from_value(json!({"workspaces": [
        {"id": "w0", "name": "main", "tabs": [pair("tab1", "t1", "t2"), pair("tab2", "t3", "t4")]},
        {"id": "w1", "name": "other", "tabs": [{"id": "tab3", "root": {"kind": "pane", "terminal": "t5"}}]},
    ]}))
    .expect("layout")
}

fn status(state: &str) -> crate::pane::CoreStatus {
    core_status(&json!({"state": state, "app": "claude"}))
}

fn says(surface: &mut Surface, terminal: &str, states: &[&str], now: Instant) -> Vec<Effect> {
    states
        .iter()
        .flat_map(|state| surface.heard(Report::Status(terminal.to_owned(), status(state)), now))
        .collect()
}

fn finished(surface: &mut Surface, terminal: &str, now: Instant) -> Vec<Effect> {
    says(surface, terminal, &["working", "done"], now)
}

fn screen(surface: &mut Surface) -> Vec<String> {
    let rows = frame(surface, &HashSet::new());
    let mut cells = surface.canvas.clone();
    crate::protocol::surface_delta::apply_rows(&mut cells, surface.canvas_cols, &rows);
    cells
        .chunks(usize::from(surface.canvas_cols))
        .map(|row| row.iter().map(|cell| cell.symbol.as_str()).collect())
        .collect()
}

fn showing(surface: &mut Surface, text: &str) -> bool {
    screen(surface).iter().any(|line| line.contains(text))
}

fn notice(surface: &Surface) -> Rect {
    surface
        .notice
        .as_ref()
        .map_or_else(Rect::default, |notice| notice.0)
}

#[tokio::test]
async fn a_block_in_another_tab_draws_a_toast_and_plays_the_request_sound() {
    let mut surface = herdr();

    let effects = says(&mut surface, "t3", &["blocked"], Instant::now());

    assert_eq!(effects, [Effect::Sound(Sound::Request)]);
    assert!(surface.needs_frame());
    let lines = screen(&mut surface);
    assert!(lines
        .iter()
        .any(|line| line.contains("● claude needs attention")));
    assert!(lines.iter().any(|line| line.contains("main · 1 · 2")));
}

#[tokio::test]
async fn a_block_in_the_focused_tab_plays_the_request_sound_without_a_toast() {
    let mut surface = herdr();

    let effects = says(&mut surface, "t2", &["blocked"], Instant::now());

    assert_eq!(effects, [Effect::Sound(Sound::Request)]);
    assert!(surface.notice.is_none());
}

#[tokio::test]
async fn a_finish_in_another_workspace_draws_a_toast_and_plays_the_done_sound() {
    let mut surface = herdr();

    let effects = finished(&mut surface, "t5", Instant::now());

    assert_eq!(effects, [Effect::Sound(Sound::Done)]);
    let lines = screen(&mut surface);
    assert!(lines.iter().any(|line| line.contains("● claude finished")));
    assert!(lines.iter().any(|line| line.contains("other · 2")));
}

#[tokio::test]
async fn a_finish_in_the_focused_tab_while_the_host_has_focus_notifies_nothing() {
    let mut surface = notifying(|notifying| notifying.delivery = ToastDelivery::Terminal);

    assert_eq!(finished(&mut surface, "t2", Instant::now()), []);
}

#[tokio::test]
async fn a_finish_in_the_focused_tab_while_the_host_lacks_focus_sounds_and_tells_the_host_but_draws_no_toast(
) {
    let now = Instant::now();
    for (delivery, told) in [
        (ToastDelivery::Herdr, None),
        (
            ToastDelivery::Terminal,
            Some(Effect::Terminal {
                title: "claude finished".to_owned(),
                body: "main · 1 · 1".to_owned(),
            }),
        ),
    ] {
        let mut surface = notifying(|notifying| notifying.delivery = delivery);
        surface.host_focus(false);

        let effects = finished(&mut surface, "t2", now);

        assert_eq!(
            effects,
            [Some(Effect::Sound(Sound::Done)), told]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
        );
        assert!(surface.notice.is_none());
    }
}

#[tokio::test]
async fn system_delivery_asks_for_a_system_notification_with_the_agent_and_its_place() {
    let mut surface = notifying(|notifying| notifying.delivery = ToastDelivery::System);

    let effects = says(&mut surface, "t3", &["blocked"], Instant::now());

    assert_eq!(
        effects,
        [
            Effect::Sound(Sound::Request),
            Effect::System {
                title: "claude needs attention".to_owned(),
                body: "main · 1 · 2".to_owned(),
            },
        ]
    );
    assert!(surface.notice.is_none());
}

#[tokio::test]
async fn with_toasts_off_only_the_sound_plays() {
    let mut surface = notifying(|notifying| notifying.delivery = ToastDelivery::Off);

    assert_eq!(
        finished(&mut surface, "t3", Instant::now()),
        [Effect::Sound(Sound::Done)]
    );
    assert!(surface.notice.is_none());
}

#[tokio::test]
async fn an_agent_muted_in_the_sound_settings_toasts_without_a_sound() {
    let mut surface = notifying(|notifying| {
        notifying.sound =
            serde_json::from_value(json!({"agents": {"claude": "off"}})).expect("sound");
    });

    assert_eq!(finished(&mut surface, "t3", Instant::now()), []);
    assert!(surface.notice.is_some());
}

#[tokio::test]
async fn going_idle_without_a_finish_notifies_nothing() {
    let mut surface = herdr();

    assert_eq!(
        says(&mut surface, "t3", &["working", "idle"], Instant::now()),
        []
    );
    assert!(surface.notice.is_none());
}

#[tokio::test]
async fn a_report_that_changes_nothing_notifies_nothing() {
    let mut surface = herdr();
    let now = Instant::now();
    says(&mut surface, "t3", &["blocked"], now);
    screen(&mut surface);
    surface.open_notice(now);

    assert_eq!(says(&mut surface, "t3", &["blocked"], now), []);
    assert!(surface.notice.is_none());
}

#[tokio::test]
async fn a_status_without_a_known_agent_or_for_a_parked_pane_notifies_nothing() {
    let mut surface = herdr();
    let mut layout = paired();
    layout.parked.insert("t4".to_owned());
    surface.update(layout);
    let now = Instant::now();

    let unknown = surface.heard(
        Report::Status("t3".to_owned(), core_status(&json!({"state": "blocked"}))),
        now,
    );
    let parked = says(&mut surface, "t4", &["blocked"], now);

    assert_eq!((unknown, parked), (vec![], vec![]));
    assert!(surface.notice.is_none());
}

fn snapshot(states: &[(&str, &str)]) -> Report {
    Report::Statuses(
        states
            .iter()
            .map(|(terminal, state)| ((*terminal).to_owned(), status(state)))
            .collect(),
    )
}

#[tokio::test]
async fn the_first_status_snapshot_notifies_nothing() {
    let mut surface = herdr();

    let effects = surface.heard(snapshot(&[("t3", "blocked")]), Instant::now());

    assert_eq!(effects, []);
    assert!(surface.notice.is_none());
}

#[tokio::test]
async fn a_later_snapshot_notifies_only_the_states_that_changed() {
    let mut surface = herdr();
    let now = Instant::now();
    surface.heard(snapshot(&[("t3", "blocked"), ("t4", "working")]), now);

    let effects = surface.heard(snapshot(&[("t3", "blocked"), ("t4", "done")]), now);

    assert_eq!(effects, [Effect::Sound(Sound::Done)]);
    assert!(showing(&mut surface, "claude finished"));
}

#[tokio::test]
async fn a_notice_waits_for_the_configured_delay_and_drops_once_its_state_has_passed() {
    let mut surface = notifying(|notifying| notifying.delay = Duration::from_secs(1));
    let now = Instant::now();
    let due = now + Duration::from_secs(1);

    assert_eq!(says(&mut surface, "t3", &["blocked"], now), []);
    assert_eq!(surface.deadline(), Some(due));
    assert_eq!(surface.tick(due), [Effect::Sound(Sound::Request)]);
    assert!(surface.notice.is_some());

    let mut surface = notifying(|notifying| notifying.delay = Duration::from_secs(1));
    says(&mut surface, "t3", &["blocked", "working"], now);
    assert_eq!(surface.tick(due), []);
    assert!(surface.notice.is_none());
}

#[tokio::test]
async fn a_toast_for_attention_lasts_eight_seconds_and_one_for_a_finish_five() {
    for (states, lasts) in [(&["blocked"][..], 8), (&["working", "done"][..], 5)] {
        let mut surface = herdr();
        let now = Instant::now();
        says(&mut surface, "t3", states, now);
        screen(&mut surface);
        let expiry = now + Duration::from_secs(lasts);
        assert_eq!(surface.deadline(), Some(expiry));

        surface.tick(expiry - Duration::from_millis(1));
        assert!(!surface.needs_frame());
        surface.tick(expiry);
        assert!(surface.needs_frame());
        assert!(!showing(&mut surface, "claude"));
        assert_eq!(surface.deadline(), None);
    }
}

#[tokio::test]
async fn a_second_toast_waits_until_the_first_expires_and_then_takes_its_place() {
    let mut surface = herdr();
    let now = Instant::now();
    says(&mut surface, "t3", &["blocked"], now);
    says(&mut surface, "t5", &["blocked"], now);
    let first = notice(&surface);
    assert!(showing(&mut surface, "main · 1 · 2"));

    let later = now + Duration::from_secs(8);
    surface.tick(later);

    assert_eq!(notice(&surface), first);
    assert!(surface.needs_frame());
    assert!(showing(&mut surface, "other · 2"));
    assert_eq!(surface.deadline(), Some(later + Duration::from_secs(8)));
}

#[tokio::test]
async fn a_new_notice_about_the_same_pane_replaces_its_toast() {
    let mut surface = herdr();
    let now = Instant::now();
    says(&mut surface, "t3", &["blocked"], now);
    assert!(showing(&mut surface, "claude needs attention"));

    finished(&mut surface, "t3", now);

    assert!(surface.needs_frame());
    let lines = screen(&mut surface);
    assert!(lines.iter().any(|line| line.contains("claude finished")));
    assert!(!lines.iter().any(|line| line.contains("needs attention")));
}

#[tokio::test]
async fn at_most_eight_toasts_wait_and_the_oldest_waiting_one_drops() {
    let mut surface = herdr();
    let mut layout = paired();
    let terminals: Vec<String> = (10..20).map(|index| format!("t{index}")).collect();
    layout.workspaces[1].tabs[0].root =
        serde_json::from_value(super::balanced(&terminals)).expect("node");
    surface.update(layout);
    let now = Instant::now();
    for terminal in &terminals {
        says(&mut surface, terminal, &["blocked"], now);
    }

    let opened: Vec<String> = std::iter::from_fn(|| surface.notices.open(now)).collect();

    assert_eq!(opened, [&terminals[..1], &terminals[2..]].concat());
}

#[tokio::test]
async fn a_press_on_the_toast_focuses_its_pane_and_dismisses_it() {
    let mut surface = herdr();
    says(&mut surface, "t5", &["blocked"], Instant::now());
    screen(&mut surface);
    let rect = notice(&surface);

    assert_eq!(
        surface.mouse(click(rect.x + 1, rect.y + 1), Instant::now()),
        None
    );

    assert_eq!(surface.session.focused(), Some("t5"));
    assert!(surface.notice.is_none());
    assert!(!showing(&mut surface, "claude needs attention"));
}

#[tokio::test]
async fn open_notification_target_focuses_the_pane_of_the_toast_and_does_nothing_without_one() {
    let mut surface = herdr();
    let now = Instant::now();
    surface.open_notice(now);
    assert_eq!(surface.session.focused(), Some("t1"));

    finished(&mut surface, "t4", now);
    screen(&mut surface);
    surface.open_notice(now);

    assert_eq!(surface.session.focused(), Some("t4"));
    assert!(surface.notice.is_none());
}

#[tokio::test]
async fn the_toast_is_a_bordered_card_in_the_configured_corner_of_the_screen() {
    for (position, corner) in [
        (ToastHerdrPosition::BottomRight, Rect::new(72, 26, 28, 4)),
        (ToastHerdrPosition::TopLeft, Rect::new(0, 0, 28, 4)),
    ] {
        let mut surface = notifying(|notifying| notifying.position = position);
        says(&mut surface, "t3", &["blocked"], Instant::now());

        let lines = screen(&mut surface);

        assert_eq!(notice(&surface), corner);
        let top: Vec<char> = lines[usize::from(corner.y)].chars().collect();
        let x = usize::from(corner.x);
        assert_eq!(top[x], '┌');
        assert_eq!(top[x + usize::from(corner.width) - 1], '┐');
    }
}

#[tokio::test]
async fn on_a_narrow_screen_the_toast_is_a_banner_on_the_bottom_row() {
    let mut surface = herdr();
    surface.resize(host(40, 20));
    screen(&mut surface);
    says(&mut surface, "t3", &["blocked"], Instant::now());

    let lines = screen(&mut surface);

    assert_eq!(notice(&surface), Rect::new(0, 19, 40, 1));
    assert!(
        lines[19].starts_with(" ● claude waiting · main · 1 · 2"),
        "{}",
        lines[19]
    );
}

#[tokio::test]
async fn the_copied_toast_moves_above_a_notice_it_would_cover() {
    let mut surface = herdr();
    let now = Instant::now();
    surface.copied(now);
    let alone = surface
        .toast
        .as_ref()
        .map(|toast| toast.image.0)
        .expect("toast");

    says(&mut surface, "t3", &["blocked"], now);

    let card = notice(&surface);
    let toast = surface
        .toast
        .as_ref()
        .map(|toast| toast.image.0)
        .expect("toast");
    assert_eq!(toast.y, alone.y - card.height);
    assert!(toast.bottom() <= card.y);
}

#[tokio::test]
async fn a_toast_under_an_open_dialog_appears_is_replaced_and_expires_on_time() {
    let mut surface = herdr();
    assert_eq!(surface.act(KeybindAction::CloseWorkspace, true), None);
    assert!(surface.dialog_open());
    assert!(showing(&mut surface, "Close workspace?"));
    let now = Instant::now();

    says(&mut surface, "t3", &["blocked"], now);
    assert!(surface.needs_frame());
    assert!(showing(&mut surface, "claude needs attention"));

    finished(&mut surface, "t3", now);
    let lines = screen(&mut surface);
    assert!(lines.iter().any(|line| line.contains("claude finished")));
    assert!(!lines.iter().any(|line| line.contains("needs attention")));

    surface.tick(now + Duration::from_secs(5));
    let lines = screen(&mut surface);
    assert!(!lines.iter().any(|line| line.contains("claude")));
    assert!(lines.iter().any(|line| line.contains("Close workspace?")));
}

#[tokio::test]
async fn a_press_or_the_key_opens_nothing_until_the_toast_that_took_over_is_drawn() {
    let mut surface = herdr();
    let now = Instant::now();
    says(&mut surface, "t3", &["blocked"], now);
    says(&mut surface, "t5", &["blocked"], now);
    screen(&mut surface);
    let rect = notice(&surface);
    let later = now + Duration::from_secs(8);
    surface.tick(later);

    assert_eq!(surface.mouse(click(rect.x + 1, rect.y + 1), later), None);
    surface.open_notice(later);
    assert_eq!(surface.session.focused(), Some("t1"));
    assert!(showing(&mut surface, "other · 2"));

    surface.open_notice(later);
    assert_eq!(surface.session.focused(), Some("t5"));
}

#[tokio::test]
async fn on_a_narrow_screen_the_copy_search_prompt_stays_above_the_banner() {
    let mut surface = herdr();
    surface.resize(host(40, 20));
    screen(&mut surface);
    feed(&surface, "t1", b"hello");
    screen(&mut surface);
    surface.enter_copy_mode();
    surface.copy_mode_key(&TerminalKey::new(KeyCode::Char('/'), KeyModifiers::NONE));
    let prompt = screen(&mut surface);
    let bar = surface.painted.copy.bar;
    assert!(!bar.is_empty());

    says(&mut surface, "t3", &["blocked"], Instant::now());
    let lines = screen(&mut surface);

    assert_eq!(notice(&surface).y, bar.y);
    let row = usize::from(bar.y);
    assert_eq!(lines[row], prompt[row]);
}

#[tokio::test]
async fn a_copied_toast_pushed_past_a_short_screen_by_a_notice_is_cut_at_the_pane_area() {
    let mut surface = notifying(|notifying| notifying.position = ToastHerdrPosition::TopRight);
    surface.toast_at = Some(ToastClipboardPosition::TopRight);
    let now = Instant::now();
    says(&mut surface, "t3", &["blocked"], now);
    surface.copied(now);

    surface.resize(host(100, 5));
    screen(&mut surface);

    let toast = surface
        .toast
        .as_ref()
        .map(|toast| toast.image.0)
        .expect("toast");
    assert_eq!(toast.intersection(surface.areas.pane_surface), toast);
}

#[tokio::test]
async fn a_finish_waiting_out_the_delay_is_dropped_once_another_agent_runs_in_its_pane() {
    let mut surface = notifying(|notifying| notifying.delay = Duration::from_secs(1));
    let now = Instant::now();
    finished(&mut surface, "t3", now);
    let other = core_status(&json!({"state": "idle", "app": "codex"}));
    surface.heard(Report::Status("t3".to_owned(), other), now);

    assert_eq!(surface.tick(now + Duration::from_secs(1)), []);
    assert!(surface.notice.is_none());
}
