use std::time::{Duration, Instant};

use super::*;
use crate::config::{SoundConfig, ToastDelivery, ToastHerdrPosition};
use crate::core_client::tui::gate::Gate;
use crate::core_client::tui::notices::{Effect, Notifying};
use crate::core_client::tui::surface::Arrival;
use crate::core_client::tui::terminals::Report;
use crate::pane::core_status;
use crate::sound::Sound;

fn notifying(configure: impl FnOnce(&mut Notifying)) -> Surface {
    notifying_on(100, 30, configure)
}

fn notifying_on(cols: u16, rows: u16, configure: impl FnOnce(&mut Notifying)) -> Surface {
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
        host(cols, rows),
        Chrome::new(&config),
        Settings {
            chrome: PaneChrome {
                borders: PaneBordersConfig::Auto,
                gaps: false,
                outer_borders: true,
            },
            palette: Palette::catppuccin(),
            scrollbars: true,
            copying: Copying {
                on_select: true,
                toast: Some(ToastClipboardPosition::BottomRight),
            },
            keybinds: keybinds(),
            confirm_close: false,
            scrollback: SCROLLBACK,
        },
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
    Report::Snapshot {
        folders: Default::default(),
        statuses: states
            .iter()
            .map(|(terminal, state)| ((*terminal).to_owned(), status(state)))
            .collect(),
    }
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
    assert_eq!(surface.due(due), [Effect::Sound(Sound::Request)]);
    assert!(surface.notice.is_some());

    let mut surface = notifying(|notifying| notifying.delay = Duration::from_secs(1));
    says(&mut surface, "t3", &["blocked", "working"], now);
    assert_eq!(surface.due(due), []);
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

        surface.due(expiry - Duration::from_millis(1));
        assert!(!surface.needs_frame());
        surface.due(expiry);
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
    surface.due(later);

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

    let opened: Vec<String> =
        std::iter::from_fn(|| surface.notices.open(&surface.session, now)).collect();

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

    surface.due(now + Duration::from_secs(5));
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
    surface.due(later);

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

    assert_eq!(surface.due(now + Duration::from_secs(1)), []);
    assert!(surface.notice.is_none());
}

#[tokio::test]
async fn a_finish_waiting_out_the_delay_is_dropped_when_another_agent_comes_and_goes() {
    let mut surface = notifying(|notifying| notifying.delay = Duration::from_secs(1));
    let now = Instant::now();
    finished(&mut surface, "t3", now);
    let other = core_status(&json!({"state": "idle", "app": "codex"}));
    surface.heard(Report::Status("t3".to_owned(), other), now);
    says(&mut surface, "t3", &["idle"], now);

    assert_eq!(surface.due(now + Duration::from_secs(1)), []);
    assert!(surface.notice.is_none());
}

fn nothing_arriving() -> tokio::sync::mpsc::UnboundedReceiver<Arrival> {
    tokio::sync::mpsc::unbounded_channel().1
}

fn with_another_tab() -> Layout {
    let mut layout = paired();
    layout.workspaces[1].tabs.push(
        serde_json::from_value(json!({"id": "tab4", "root": {"kind": "pane", "terminal": "t6"}}))
            .expect("tab"),
    );
    layout
}

#[tokio::test]
async fn a_block_heard_before_the_layout_that_places_its_pane_notifies_when_that_layout_arrives() {
    let mut surface = herdr();
    let now = Instant::now();

    assert_eq!(says(&mut surface, "t6", &["blocked"], now), []);
    assert!(surface.notice.is_none());

    assert_eq!(
        surface.arranged(with_another_tab(), &mut nothing_arriving(), now),
        [Effect::Sound(Sound::Request)]
    );
    assert!(showing(&mut surface, "other · 2 · 2"));
    assert_eq!(
        surface.arranged(with_another_tab(), &mut nothing_arriving(), now),
        []
    );
}

#[tokio::test]
async fn a_change_heard_before_its_layout_waits_out_the_delay_from_when_the_layout_arrives() {
    let mut surface = notifying(|notifying| notifying.delay = Duration::from_secs(1));
    let now = Instant::now();
    let later = now + Duration::from_secs(5);
    finished(&mut surface, "t6", now);

    assert_eq!(
        surface.arranged(with_another_tab(), &mut nothing_arriving(), later),
        []
    );
    assert_eq!(surface.deadline(), Some(later + Duration::from_secs(1)));
    assert_eq!(
        surface.due(later + Duration::from_secs(1)),
        [Effect::Sound(Sound::Done)]
    );
    assert!(showing(&mut surface, "claude finished"));
}

#[tokio::test]
async fn changes_kept_for_a_layout_are_judged_in_the_order_they_arrived_and_the_cap_drops_the_oldest(
) {
    let mut surface = herdr();
    let now = Instant::now();
    let terminals: Vec<String> = (10..20).map(|index| format!("t{index}")).collect();
    for terminal in &terminals {
        says(&mut surface, terminal, &["blocked"], now);
    }
    says(&mut surface, "t10", &["working", "blocked"], now);
    let mut layout = paired();
    layout.workspaces[1].tabs[0].root =
        serde_json::from_value(super::balanced(&terminals)).expect("node");

    surface.arranged(layout, &mut nothing_arriving(), now);

    let opened: Vec<String> =
        std::iter::from_fn(|| surface.notices.open(&surface.session, now)).collect();
    assert_eq!(
        opened,
        [&terminals[1..2], &terminals[3..], &terminals[..1]].concat()
    );
}

#[tokio::test]
async fn a_fresh_snapshot_is_applied_whole_before_a_notice_due_then_is_judged() {
    let mut surface = notifying(|notifying| notifying.delay = Duration::from_secs(1));
    let now = Instant::now();
    says(&mut surface, "t3", &["blocked"], now);
    let missed = json!({"method": "missed", "params": {"terminals": [
        {"address": "t3", "cwd": "/srv/app", "status": {"state": "working", "app": "claude"}},
    ]}});

    let effects: Vec<Effect> = crate::core_client::tui::terminals::reported(&missed)
        .into_iter()
        .flat_map(|report| surface.heard(report, now + Duration::from_secs(1)))
        .collect();

    assert_eq!(effects, []);
    assert!(surface.notice.is_none());
}

#[tokio::test]
async fn reports_already_queued_are_applied_before_a_due_notice_is_delivered() {
    let mut surface = notifying(|notifying| notifying.delay = Duration::from_secs(1));
    let now = Instant::now();
    says(&mut surface, "t3", &["blocked"], now);
    let (queue, mut reports) = tokio::sync::mpsc::unbounded_channel();
    for report in [
        Report::Folder("t1".to_owned(), "/srv/app".into()),
        Report::Status("t3".to_owned(), status("working")),
    ] {
        queue.send(report.into()).expect("queued");
    }

    assert_eq!(
        surface
            .caught_up(
                &mut Gate::default(),
                &mut reports,
                now + Duration::from_secs(1),
            )
            .1,
        []
    );
    assert!(surface.notice.is_none());
}

#[tokio::test]
async fn a_snapshot_after_a_reconnect_makes_its_changes_in_the_order_the_core_lists_its_terminals()
{
    let mut surface = herdr();
    let now = Instant::now();
    let terminals: Vec<String> = (10..20).rev().map(|index| format!("t{index}")).collect();
    let mut layout = paired();
    layout.workspaces[1].tabs[0].root =
        serde_json::from_value(super::balanced(&terminals)).expect("node");
    surface.arranged(layout, &mut nothing_arriving(), now);
    let missed = |state: &str| {
        json!({"method": "missed", "params": {"terminals": terminals
            .iter()
            .map(|terminal| json!({
                "address": terminal, "cwd": "/", "status": {"state": state, "app": "claude"},
            }))
            .collect::<Vec<_>>()}})
    };

    for state in ["working", "blocked"] {
        let report =
            crate::core_client::tui::terminals::reported(&missed(state)).expect("a snapshot");
        surface.heard(report, now);
    }

    let opened: Vec<String> =
        std::iter::from_fn(|| surface.notices.open(&surface.session, now)).collect();
    assert_eq!(opened, [&terminals[..1], &terminals[2..]].concat());
}

#[tokio::test]
async fn a_notice_names_the_tab_as_it_is_called_when_the_notice_comes_due() {
    let mut surface = notifying(|notifying| notifying.delay = Duration::from_secs(1));
    let now = Instant::now();
    says(&mut surface, "t3", &["blocked"], now);
    let mut layout = paired();
    layout.workspaces[0].tabs[1].name = Some("build".to_owned());
    surface.arranged(layout, &mut nothing_arriving(), now);

    surface.due(now + Duration::from_secs(1));

    assert!(showing(&mut surface, "main · 1 · build"));
}

#[tokio::test]
async fn a_notice_whose_pane_is_parked_during_the_delay_is_dropped() {
    let mut surface = notifying(|notifying| notifying.delay = Duration::from_secs(1));
    let now = Instant::now();
    says(&mut surface, "t3", &["blocked"], now);
    let mut layout = paired();
    layout.parked.insert("t3".to_owned());
    surface.arranged(layout, &mut nothing_arriving(), now);

    assert_eq!(surface.due(now + Duration::from_secs(1)), []);
    assert!(surface.notice.is_none());
}

#[tokio::test]
async fn a_layout_at_the_deadline_applies_the_reports_already_queued_before_a_notice_is_delivered()
{
    let mut surface = notifying(|notifying| notifying.delay = Duration::from_secs(1));
    let now = Instant::now();
    says(&mut surface, "t3", &["blocked"], now);
    let (queue, mut reports) = tokio::sync::mpsc::unbounded_channel();
    queue
        .send(Report::Status("t3".to_owned(), status("working")).into())
        .expect("queued");

    assert_eq!(
        surface.arranged(paired(), &mut reports, now + Duration::from_secs(1)),
        []
    );
    assert!(surface.notice.is_none());
}

#[tokio::test]
async fn a_layout_already_queued_that_parks_a_pane_is_applied_before_its_due_notice_is_delivered() {
    let mut surface = notifying(|notifying| notifying.delay = Duration::from_secs(1));
    let now = Instant::now();
    says(&mut surface, "t3", &["blocked"], now);
    let mut layout = paired();
    layout.parked.insert("t3".to_owned());
    let (queue, mut layouts) = tokio::sync::mpsc::unbounded_channel();
    queue
        .send(crate::session_doc::Loaded { layout, version: 2 }.into())
        .expect("queued");

    assert_eq!(
        surface
            .caught_up(
                &mut Gate::default(),
                &mut layouts,
                now + Duration::from_secs(1),
            )
            .1,
        []
    );
    assert!(surface.notice.is_none());
}

#[tokio::test]
async fn a_block_while_its_pane_is_parked_notifies_nothing_after_the_pane_returns() {
    let mut surface = notifying(|notifying| notifying.delay = Duration::from_secs(1));
    let now = Instant::now();
    let mut parked = paired();
    parked.parked.insert("t3".to_owned());
    surface.arranged(parked, &mut nothing_arriving(), now);
    says(&mut surface, "t3", &["blocked"], now);

    surface.arranged(paired(), &mut nothing_arriving(), now);

    assert_eq!(surface.due(now + Duration::from_secs(1)), []);
    assert!(surface.notice.is_none());
}

#[tokio::test]
async fn a_press_on_the_toast_in_resize_mode_opens_nothing() {
    let mut surface = herdr();
    says(&mut surface, "t5", &["blocked"], Instant::now());
    screen(&mut surface);
    surface.act(KeybindAction::EnterResizeMode, false);
    screen(&mut surface);
    let rect = notice(&surface);

    surface.mouse(click(rect.x + 1, rect.y + 1), Instant::now());

    assert_ne!(surface.session.focused(), Some("t5"));
    assert!(surface.notice.is_some());
}

#[tokio::test]
async fn a_queued_block_is_judged_against_the_layout_queued_before_it() {
    let judged = |unparked_first: bool| {
        let mut surface = herdr();
        let now = Instant::now();
        let mut parked = paired();
        parked.parked.insert("t3".to_owned());
        surface.arranged(parked, &mut nothing_arriving(), now);
        let block = Arrival::from(Report::Status("t3".to_owned(), status("blocked")));
        let unpark = Arrival::from(crate::session_doc::Loaded {
            layout: paired(),
            version: 2,
        });
        let (queue, mut arrivals) = tokio::sync::mpsc::unbounded_channel();
        let queued = if unparked_first {
            [unpark, block]
        } else {
            [block, unpark]
        };
        for arrival in queued {
            queue.send(arrival).expect("queued");
        }

        surface.caught_up(&mut Gate::default(), &mut arrivals, now);

        surface.notice.is_some()
    };

    assert!(!judged(false));
    assert!(judged(true));
}

#[tokio::test]
async fn a_press_on_a_banner_still_under_the_resize_bar_opens_nothing_after_the_mode_ends() {
    let mut surface = notifying_on(40, 20, |_| {});
    says(&mut surface, "t3", &["blocked"], Instant::now());
    surface.act(KeybindAction::EnterResizeMode, false);
    screen(&mut surface);
    let banner = notice(&surface);
    assert!(surface.painted.resize.intersects(banner));
    let row = surface.painted.resize.y;
    let column = surface.painted.resize.intersection(banner).x;

    surface.resize_key(&TerminalKey::new(KeyCode::Esc, KeyModifiers::NONE));
    surface.mouse(click(column, row), Instant::now());

    assert_ne!(surface.session.focused(), Some("t3"));
    assert!(surface.notice.is_some());
}

#[tokio::test]
async fn a_press_on_a_banner_still_under_the_copy_bar_opens_nothing_after_copy_mode_ends() {
    let mut surface = notifying_on(40, 20, |_| {});
    feed(&surface, "t1", b"hello");
    screen(&mut surface);
    surface.enter_copy_mode();
    surface.copy_mode_key(&TerminalKey::new(KeyCode::Char('/'), KeyModifiers::NONE));
    says(&mut surface, "t3", &["blocked"], Instant::now());
    screen(&mut surface);
    let bar = surface.painted.copy.bar;
    let under = bar.intersection(notice(&surface));
    assert!(!under.is_empty());

    for key in [KeyCode::Esc, KeyCode::Char('q')] {
        surface.copy_mode_key(&TerminalKey::new(key, KeyModifiers::NONE));
    }
    assert!(!surface.copy_mode());
    surface.mouse(click(under.x, under.y), Instant::now());

    assert_ne!(surface.session.focused(), Some("t3"));
    assert!(surface.notice.is_some());
}

#[tokio::test]
async fn a_press_on_a_banner_still_under_the_switcher_opens_nothing_after_it_closes() {
    let mut surface = notifying_on(40, 20, |_| {});
    says(&mut surface, "t3", &["blocked"], Instant::now());
    screen(&mut surface);
    surface.mouse(click(34, 1), Instant::now());
    assert!(surface.outer.switching());
    screen(&mut surface);
    let under = surface.painted.cover.intersection(notice(&surface));
    assert!(!under.is_empty());

    surface.outer.leave();
    surface.mouse(click(under.x, under.y), Instant::now());

    assert_ne!(surface.session.focused(), Some("t3"));
    assert!(surface.notice.is_some());
}

#[tokio::test]
async fn a_press_on_the_toast_over_a_pane_border_opens_it_without_starting_a_drag() {
    let mut surface = herdr();
    let mut layout = paired();
    layout.workspaces[0].tabs[0].root = serde_json::from_value(json!({
        "kind": "split", "direction": "down", "ratio": 0.9,
        "first": {"kind": "pane", "terminal": "t1"},
        "second": {"kind": "pane", "terminal": "t2"},
    }))
    .expect("node");
    surface.update(layout);
    screen(&mut surface);
    says(&mut surface, "t3", &["blocked"], Instant::now());
    screen(&mut surface);
    let rect = notice(&surface);
    let border = (rect.y..rect.bottom())
        .flat_map(|y| (rect.x..rect.right()).map(move |x| ratatui::layout::Position::new(x, y)))
        .find(|point| surface.split_at(*point).is_some())
        .expect("a border under the toast");

    surface.mouse(click(border.x, border.y), Instant::now());

    assert!(surface.splitting.is_none());
    assert_eq!(surface.session.focused(), Some("t3"));
}

#[tokio::test]
async fn a_waiting_toast_whose_pane_is_parked_is_skipped_when_the_shown_one_expires() {
    let mut surface = herdr();
    let now = Instant::now();
    says(&mut surface, "t3", &["blocked"], now);
    says(&mut surface, "t5", &["blocked"], now);
    let mut layout = paired();
    layout.parked.insert("t5".to_owned());
    surface.arranged(layout, &mut nothing_arriving(), now);

    surface.due(now + Duration::from_secs(8));

    assert!(surface.notice.is_none());
    assert!(!showing(&mut surface, "claude needs attention"));
}

fn reporting_pane(surface: &mut Surface) -> (tokio::sync::mpsc::Receiver<bytes::Bytes>, Rect) {
    surface.forget(id_of(surface, "t2"));
    let mut input = attach_with_input(surface, b"\x1b[?1000h\x1b[?1006h").remove(0);
    frame(surface, &HashSet::new());
    let pane = surface
        .tiles
        .tiles
        .iter()
        .find(|tile| tile.info.id == id_of(surface, "t2"))
        .map(|tile| tile.info.inner_rect)
        .unwrap_or_default();
    reports(&mut input);
    (input, pane)
}

#[tokio::test]
async fn a_press_on_the_toast_in_navigate_mode_opens_nothing_and_acts_as_it_would_without_a_toast()
{
    let pressed = |blocked: bool, at: Option<Rect>| {
        let mut surface = herdr();
        if blocked {
            says(&mut surface, "t5", &["blocked"], Instant::now());
        }
        let (mut input, pane) = reporting_pane(&mut surface);
        surface.act(KeybindAction::WorkspacePicker, false);
        assert!(surface.navigating());
        screen(&mut surface);
        let rect = at.unwrap_or_else(|| notice(&surface));
        assert!(pane.contains(ratatui::layout::Position::new(rect.x + 1, rect.y + 1)));

        surface.mouse(click(rect.x + 1, rect.y + 1), Instant::now());

        let after = (
            surface.session.focused().map(str::to_owned),
            surface.navigating(),
            reports(&mut input),
        );
        (rect, surface.notice.is_some(), after)
    };

    let (rect, shown, with_toast) = pressed(true, None);
    let (_, _, without) = pressed(false, Some(rect));

    assert!(shown);
    assert_ne!(with_toast.0.as_deref(), Some("t5"));
    assert_eq!(with_toast, without);
}

#[tokio::test]
async fn a_reloaded_config_changes_how_the_next_notice_is_delivered() {
    let mut surface = notifying(|notifying| notifying.delivery = ToastDelivery::Off);
    let mut config = Config::default();
    config.ui.sidebar_start_collapsed = true;
    config.ui.toast.delivery = ToastDelivery::System;
    config.ui.toast.delay_seconds = 0;

    surface.configure(&config);

    assert_eq!(
        says(&mut surface, "t3", &["blocked"], Instant::now()),
        [
            Effect::Sound(Sound::Request),
            Effect::System {
                title: "claude needs attention".to_owned(),
                body: "main · 1 · 2".to_owned(),
            },
        ]
    );
}

#[tokio::test]
async fn a_reloaded_config_moves_the_shown_toast_and_shows_no_queued_one_once_toasts_are_off() {
    let mut surface = herdr();
    let now = Instant::now();
    says(&mut surface, "t3", &["blocked"], now);
    says(&mut surface, "t5", &["blocked"], now);
    assert!(showing(&mut surface, "main · 1 · 2"));
    let first = notice(&surface);
    let mut config = Config::default();
    config.ui.sidebar_start_collapsed = true;
    config.ui.toast.delivery = ToastDelivery::Herdr;
    config.ui.toast.herdr.position = ToastHerdrPosition::TopLeft;

    surface.configure(&config);

    assert!(showing(&mut surface, "main · 1 · 2"));
    let moved = notice(&surface);
    assert_ne!((moved.x, moved.y), (first.x, first.y));

    config.ui.toast.delivery = ToastDelivery::Off;
    surface.configure(&config);
    surface.due(now + Duration::from_secs(8));

    assert!(!showing(&mut surface, "other · 2"));
    assert!(surface.notice.is_none());
}
