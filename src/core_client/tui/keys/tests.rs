use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers, ModifierKeyCode};

use super::{Keys, Mode, Navigation, Routed};
use crate::config::{ActionKeybinds, Config, Keybinds, LiveKeybindConfig};
use crate::input::{KeybindAction, KeybindMatch, TerminalKey};
use crate::layout::PaneId;

#[derive(Debug, PartialEq)]
enum Outcome {
    Pane(PaneId),
    Text(PaneId, String),
    Action(KeybindAction),
    Command,
    Copy,
    CopyMode,
    Dialog,
    Dismiss,
    Navigate(Navigation),
}

fn left() -> PaneId {
    PaneId::from_raw(1)
}

fn right() -> PaneId {
    PaneId::from_raw(2)
}

fn keys_with(keybinds: Keybinds, erase_is_ctrl_h: bool) -> Keys {
    Keys::new(
        LiveKeybindConfig {
            prefix: Config::default().prefix_keys(),
            keybinds,
        },
        erase_is_ctrl_h,
        true,
    )
}

fn keys() -> Keys {
    keys_with(
        Keybinds {
            next_tab: ActionKeybinds::direct("ctrl+n"),
            ..Config::default().keybinds()
        },
        false,
    )
}

fn outcomes(routed: Vec<Routed>) -> Vec<Outcome> {
    routed
        .into_iter()
        .map(|routed| match routed {
            Routed::Key(pane, _) => Outcome::Pane(pane),
            Routed::Text(pane, text) => Outcome::Text(pane, text),
            Routed::Binding(KeybindMatch::Action(action)) => Outcome::Action(action),
            Routed::Binding(KeybindMatch::Command(_)) => Outcome::Command,
            Routed::Copy => Outcome::Copy,
            Routed::CopyMode(_) => Outcome::CopyMode,
            Routed::Dialog(_) => Outcome::Dialog,
            Routed::Dismiss => Outcome::Dismiss,
            Routed::Navigate(step) => Outcome::Navigate(step),
        })
        .collect()
}

fn press_in(keys: &mut Keys, key: &TerminalKey, pane: PaneId) -> Vec<Outcome> {
    outcomes(keys.route_host(key.clone(), Some(pane), false, Mode::Panes, |_| None))
}

fn press(keys: &mut Keys, key: &TerminalKey) -> Vec<Outcome> {
    press_in(keys, key, left())
}

fn key(code: char, modifiers: KeyModifiers) -> TerminalKey {
    TerminalKey::new(KeyCode::Char(code), modifiers)
}

fn prefix() -> TerminalKey {
    key('b', KeyModifiers::CONTROL)
}

fn host_key(bytes: &[u8]) -> TerminalKey {
    match crate::raw_input::parse_framed_input(bytes).pop() {
        Some(crate::raw_input::RawInputEvent::Key(key)) => key,
        other => panic!("{bytes:?} is not a key: {other:?}"),
    }
}

#[test]
fn a_direct_binding_runs_without_the_prefix() {
    let mut keys = keys();
    assert_eq!(
        press(&mut keys, &key('n', KeyModifiers::CONTROL)),
        [Outcome::Action(KeybindAction::NextTab)]
    );
    assert_eq!(
        press(&mut keys, &key('z', KeyModifiers::NONE)),
        [Outcome::Pane(left())]
    );
}

#[test]
fn the_prefix_then_a_binding_runs_it_and_returns_to_the_pane() {
    let mut keys = keys();
    assert_eq!(press(&mut keys, &prefix()), []);
    assert_eq!(
        press(&mut keys, &key('z', KeyModifiers::NONE)),
        [Outcome::Action(KeybindAction::Zoom)]
    );
    assert_eq!(
        press(&mut keys, &key('z', KeyModifiers::NONE)),
        [Outcome::Pane(left())]
    );
}

#[test]
fn the_prefix_twice_sends_the_prefix_to_the_pane() {
    let mut keys = keys();
    press(&mut keys, &prefix());
    assert_eq!(press(&mut keys, &prefix()), [Outcome::Pane(left())]);
    assert_eq!(
        press(&mut keys, &key('z', KeyModifiers::NONE)),
        [Outcome::Pane(left())]
    );
}

#[test]
fn escape_cancels_the_prefix() {
    let mut keys = keys();
    press(&mut keys, &prefix());
    assert_eq!(
        press(
            &mut keys,
            &TerminalKey::new(KeyCode::Esc, KeyModifiers::NONE)
        ),
        []
    );
    assert_eq!(
        press(&mut keys, &key('z', KeyModifiers::NONE)),
        [Outcome::Pane(left())]
    );
}

#[test]
fn an_unbound_key_after_the_prefix_only_leaves_it() {
    let mut keys = keys();
    press(&mut keys, &prefix());
    assert_eq!(press(&mut keys, &key('y', KeyModifiers::NONE)), []);
    assert_eq!(
        press(&mut keys, &key('y', KeyModifiers::NONE)),
        [Outcome::Pane(left())]
    );
}

#[test]
fn a_modifier_after_the_prefix_keeps_it() {
    let mut keys = keys();
    press(&mut keys, &prefix());
    let shift = TerminalKey::new(
        KeyCode::Modifier(ModifierKeyCode::LeftShift),
        KeyModifiers::SHIFT,
    );
    assert_eq!(press(&mut keys, &shift), []);
    assert_eq!(
        press(&mut keys, &key('z', KeyModifiers::NONE)),
        [Outcome::Action(KeybindAction::Zoom)]
    );
}

#[test]
fn a_held_direct_binding_repeats_its_action_and_keeps_its_release() {
    let mut keys = keys();
    let next = key('n', KeyModifiers::CONTROL);
    press(&mut keys, &next);
    assert_eq!(
        press(&mut keys, &next.clone().with_kind(KeyEventKind::Repeat)),
        [Outcome::Action(KeybindAction::NextTab)]
    );
    assert_eq!(press(&mut keys, &next.with_kind(KeyEventKind::Release)), []);
}

#[test]
fn a_held_prefix_binding_does_not_repeat_and_keeps_its_release() {
    let mut keys = keys();
    let zoom = key('z', KeyModifiers::NONE);
    press(&mut keys, &prefix());
    press(&mut keys, &zoom);
    assert_eq!(
        press(&mut keys, &zoom.clone().with_kind(KeyEventKind::Repeat)),
        []
    );
    assert_eq!(press(&mut keys, &zoom.with_kind(KeyEventKind::Release)), []);
}

#[test]
fn a_typed_key_and_its_release_reach_the_pane() {
    let mut keys = keys();
    let typed = key('a', KeyModifiers::NONE);
    assert_eq!(press(&mut keys, &typed), [Outcome::Pane(left())]);
    assert_eq!(
        press(&mut keys, &typed.with_kind(KeyEventKind::Release)),
        [Outcome::Pane(left())]
    );
}

#[test]
fn a_release_without_its_press_is_dropped() {
    let mut keys = keys();
    assert_eq!(
        press(
            &mut keys,
            &key('a', KeyModifiers::NONE).with_kind(KeyEventKind::Release)
        ),
        []
    );
}

#[test]
fn repeats_and_the_release_of_a_held_key_stay_with_the_pane_that_got_the_press() {
    let mut keys = keys();
    let up = TerminalKey::new(KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(press_in(&mut keys, &up, left()), [Outcome::Pane(left())]);
    press(&mut keys, &prefix());
    assert_eq!(
        press(&mut keys, &key('l', KeyModifiers::NONE)),
        [Outcome::Action(KeybindAction::FocusPaneRight)]
    );

    assert_eq!(
        press_in(
            &mut keys,
            &up.clone().with_kind(KeyEventKind::Repeat),
            right()
        ),
        [Outcome::Pane(left())]
    );
    assert_eq!(
        press_in(&mut keys, &up.with_kind(KeyEventKind::Release), right()),
        [Outcome::Pane(left())]
    );
}

#[test]
fn a_shifted_prefix_binding_keeps_its_release_reported_unshifted() {
    let mut keys = keys();
    press(&mut keys, &prefix());
    let pressed = host_key(b"D");
    let released = host_key(b"\x1b[100:68;2:3u");
    assert_eq!(pressed.code, KeyCode::Char('D'));
    assert_eq!(released.code, KeyCode::Char('d'));

    assert!(matches!(
        press(&mut keys, &pressed)[..],
        [Outcome::Action(_) | Outcome::Command]
    ));
    assert_eq!(press(&mut keys, &released), []);
}

#[test]
fn backspace_from_a_host_erasing_with_ctrl_h_is_not_a_ctrl_h_binding() {
    let keybinds = Keybinds {
        focus_pane_left: ActionKeybinds::direct("ctrl+h"),
        ..Config::default().keybinds()
    };
    let erase = key('h', KeyModifiers::CONTROL).with_vt_bytes(vec![0x08]);

    let mut keys = keys_with(keybinds.clone(), true);
    let routed = keys.route_host(erase.clone(), Some(left()), false, Mode::Panes, |_| None);
    assert!(matches!(&routed[..], [Routed::Key(_, key)] if key.code == KeyCode::Backspace));

    let mut keys = keys_with(keybinds, false);
    assert_eq!(
        press(&mut keys, &erase),
        [Outcome::Action(KeybindAction::FocusPaneLeft)]
    );
}

#[test]
fn committed_text_reaches_the_pane_unless_the_prefix_is_pending() {
    let mut keys = keys();
    let committed = key('n', KeyModifiers::CONTROL);
    assert_eq!(
        outcomes(
            keys.route_host(committed.clone(), Some(left()), false, Mode::Panes, |_| {
                Some("n")
            })
        ),
        [Outcome::Text(left(), "n".to_owned())]
    );

    press(&mut keys, &prefix());
    assert_eq!(
        outcomes(keys.route_host(
            key('z', KeyModifiers::NONE),
            Some(left()),
            false,
            Mode::Panes,
            |_| Some("z")
        )),
        [Outcome::Action(KeybindAction::Zoom)]
    );
    assert_eq!(
        press(&mut keys, &committed),
        [Outcome::Action(KeybindAction::NextTab)]
    );
}

#[test]
fn committed_text_completing_a_printable_prefix_reaches_the_pane_as_a_key() {
    let mut keys = Keys::new(
        LiveKeybindConfig {
            prefix: ["ctrl+b", "x"]
                .into_iter()
                .filter_map(crate::config::parse_key_combo)
                .collect(),
            keybinds: Config::default().keybinds(),
        },
        false,
        true,
    );
    press(&mut keys, &prefix());
    let routed = keys.route_host(host_key(b"x"), Some(left()), false, Mode::Panes, |key| {
        key.committed_text(true)
    });
    assert!(matches!(
        &routed[..],
        [Routed::Key(pane, key)] if *pane == left() && key.code == KeyCode::Char('x')
    ));
}

#[test]
fn a_leaving_pane_gets_the_release_of_each_key_it_holds_but_typed_text() {
    let mut keys = keys();
    let up = TerminalKey::new(KeyCode::Up, KeyModifiers::NONE);
    let down = TerminalKey::new(KeyCode::Down, KeyModifiers::NONE);
    press_in(&mut keys, &up, left());
    press_in(&mut keys, &host_key(b"a"), left());
    press_in(&mut keys, &down, right());

    let released = keys.release(left());
    assert!(matches!(
        &released[..],
        [key] if key.code == KeyCode::Up && key.kind == KeyEventKind::Release
    ));
    assert!(keys.release(left()).is_empty());
    assert_eq!(
        press_in(&mut keys, &up.with_kind(KeyEventKind::Release), left()),
        []
    );
    assert_eq!(keys.release(right()).len(), 1);
}

#[test]
fn a_key_held_while_its_pane_leaves_stays_quiet_until_pressed_again() {
    let mut keys = keys();
    let up = TerminalKey::new(KeyCode::Up, KeyModifiers::NONE);
    press_in(&mut keys, &up, left());
    assert_eq!(keys.release(left()).len(), 1);

    let repeat = up.clone().with_kind(KeyEventKind::Repeat);
    let release = up.clone().with_kind(KeyEventKind::Release);
    assert_eq!(press_in(&mut keys, &repeat, right()), []);
    assert_eq!(press_in(&mut keys, &release, right()), []);

    assert_eq!(press_in(&mut keys, &up, right()), [Outcome::Pane(right())]);
    assert_eq!(
        press_in(&mut keys, &release, right()),
        [Outcome::Pane(right())]
    );
}

fn copying(keys: &mut Keys, key: &TerminalKey) -> Vec<Outcome> {
    outcomes(keys.route_host(key.clone(), Some(left()), true, Mode::Panes, |_| None))
}

#[test]
fn ctrl_or_super_c_copies_a_selection_and_its_repeats_and_release_stay_away_from_the_pane() {
    for modifiers in [KeyModifiers::CONTROL, KeyModifiers::SUPER] {
        let mut keys = keys();
        let copy = key('c', modifiers);

        assert_eq!(copying(&mut keys, &copy), [Outcome::Copy]);
        assert_eq!(
            press(&mut keys, &copy.clone().with_kind(KeyEventKind::Repeat)),
            []
        );
        assert_eq!(press(&mut keys, &copy.with_kind(KeyEventKind::Release)), []);
    }
}

#[test]
fn ctrl_c_reaches_the_pane_without_a_copyable_selection() {
    let mut keys = keys();
    assert_eq!(
        press(&mut keys, &key('c', KeyModifiers::CONTROL)),
        [Outcome::Pane(left())]
    );
}

#[test]
fn other_keys_reach_the_pane_while_a_selection_is_copyable() {
    let mut keys = keys();
    assert_eq!(
        copying(&mut keys, &key('c', KeyModifiers::NONE)),
        [Outcome::Pane(left())]
    );
}

#[test]
fn a_binding_held_while_a_selection_is_copyable_still_repeats() {
    let mut keys = keys();
    let next = key('n', KeyModifiers::CONTROL);
    assert_eq!(
        copying(&mut keys, &next),
        [Outcome::Action(KeybindAction::NextTab)]
    );
    assert_eq!(
        press(&mut keys, &next.clone().with_kind(KeyEventKind::Repeat)),
        [Outcome::Action(KeybindAction::NextTab)]
    );
    assert_eq!(press(&mut keys, &next.with_kind(KeyEventKind::Release)), []);
}

fn in_copy_mode(keys: &mut Keys, key: &TerminalKey) -> Vec<Outcome> {
    outcomes(keys.route_host(key.clone(), Some(left()), false, Mode::Copy, |_| Some("j")))
}

#[test]
fn in_copy_mode_typed_text_and_direct_bindings_are_copy_commands() {
    let mut keys = keys();
    assert_eq!(
        in_copy_mode(&mut keys, &key('j', KeyModifiers::NONE)),
        [Outcome::CopyMode]
    );
    assert_eq!(
        in_copy_mode(&mut keys, &key('n', KeyModifiers::CONTROL)),
        [Outcome::CopyMode]
    );
}

#[test]
fn in_copy_mode_the_prefix_still_runs_bindings_and_twice_stays_a_copy_command() {
    let mut keys = keys();
    assert_eq!(in_copy_mode(&mut keys, &prefix()), []);
    assert_eq!(
        in_copy_mode(&mut keys, &key('z', KeyModifiers::NONE)),
        [Outcome::Action(KeybindAction::Zoom)]
    );
    assert_eq!(in_copy_mode(&mut keys, &prefix()), []);
    assert_eq!(in_copy_mode(&mut keys, &prefix()), [Outcome::CopyMode]);
}

#[test]
fn a_held_copy_mode_key_repeats_as_a_command_and_never_reaches_the_pane() {
    let mut keys = keys();
    let down = key('j', KeyModifiers::NONE);
    let repeat = down.clone().with_kind(KeyEventKind::Repeat);
    let release = down.clone().with_kind(KeyEventKind::Release);

    assert_eq!(in_copy_mode(&mut keys, &down), [Outcome::CopyMode]);
    assert_eq!(in_copy_mode(&mut keys, &repeat), [Outcome::CopyMode]);
    assert_eq!(in_copy_mode(&mut keys, &release), []);

    assert_eq!(in_copy_mode(&mut keys, &down), [Outcome::CopyMode]);
    assert_eq!(press(&mut keys, &repeat), []);
    assert_eq!(press(&mut keys, &release), []);
}

#[test]
fn a_key_held_before_copy_mode_stops_repeating_and_its_release_reaches_its_pane() {
    for pane in [left(), right()] {
        let mut keys = keys();
        let down = key('x', KeyModifiers::NONE);
        let repeat = down.clone().with_kind(KeyEventKind::Repeat);
        let release = down.clone().with_kind(KeyEventKind::Release);

        assert_eq!(press_in(&mut keys, &down, pane), [Outcome::Pane(pane)]);
        assert_eq!(in_copy_mode(&mut keys, &repeat), [], "{pane:?}");
        assert_eq!(
            in_copy_mode(&mut keys, &release),
            [Outcome::Pane(pane)],
            "{pane:?}"
        );
    }
}

#[test]
fn in_the_search_prompt_the_prefix_typed_text_and_bindings_are_prompt_keys() {
    let mut keys = keys();
    let typed = key('j', KeyModifiers::NONE);
    for key in [
        prefix(),
        key('z', KeyModifiers::NONE),
        typed.clone(),
        typed.clone().with_kind(KeyEventKind::Repeat),
        key('n', KeyModifiers::CONTROL),
    ] {
        assert_eq!(
            outcomes(
                keys.route_host(key.clone(), Some(left()), false, Mode::Prompt, |_| Some(
                    "j"
                ))
            ),
            [Outcome::CopyMode],
            "{key:?}"
        );
    }
}

fn navigating_in(keys: &mut Keys, session: u64, key: &TerminalKey) -> Vec<Outcome> {
    outcomes(keys.route_host(
        key.clone(),
        Some(left()),
        false,
        Mode::Navigate(session),
        |_| Some("j"),
    ))
}

fn navigating(keys: &mut Keys, key: &TerminalKey) -> Vec<Outcome> {
    navigating_in(keys, 1, key)
}

fn plain(code: KeyCode) -> TerminalKey {
    TerminalKey::new(code, KeyModifiers::NONE)
}

#[test]
fn in_navigate_mode_escape_the_prefix_and_ctrl_bracket_leave_it() {
    let mut keys = keys();
    for key in [
        plain(KeyCode::Esc),
        prefix(),
        key('[', KeyModifiers::CONTROL),
    ] {
        assert_eq!(navigating(&mut keys, &key), [Outcome::Dismiss], "{key:?}");
    }
}

#[test]
fn in_navigate_mode_up_and_down_move_the_workspace_selection() {
    let mut keys = keys();
    assert_eq!(
        navigating(&mut keys, &plain(KeyCode::Up)),
        [Outcome::Navigate(Navigation::Move(-1))]
    );
    assert_eq!(
        navigating(&mut keys, &plain(KeyCode::Down)),
        [Outcome::Navigate(Navigation::Move(1))]
    );
}

#[test]
fn in_navigate_mode_enter_accepts_the_selection_and_a_modified_enter_does_not() {
    let mut keys = keys();
    assert_eq!(
        navigating(&mut keys, &plain(KeyCode::Enter)),
        [Outcome::Navigate(Navigation::Accept)]
    );
    assert_eq!(
        navigating(
            &mut keys,
            &TerminalKey::new(KeyCode::Enter, KeyModifiers::ALT)
        ),
        []
    );
}

#[test]
fn in_navigate_mode_a_digit_jumps_to_that_workspace() {
    let mut keys = keys();
    assert_eq!(
        navigating(&mut keys, &key('1', KeyModifiers::NONE)),
        [Outcome::Navigate(Navigation::Jump(
            KeybindAction::SwitchWorkspace(0)
        ))]
    );
    assert_eq!(
        navigating(&mut keys, &key('9', KeyModifiers::NONE)),
        [Outcome::Navigate(Navigation::Jump(
            KeybindAction::SwitchWorkspace(8)
        ))]
    );
}

#[test]
fn in_navigate_mode_tab_and_back_tab_cycle_the_shown_panes() {
    let mut keys = keys();
    assert_eq!(
        navigating(&mut keys, &plain(KeyCode::Tab)),
        [Outcome::Navigate(Navigation::Cycle(1))]
    );
    assert_eq!(
        navigating(&mut keys, &plain(KeyCode::BackTab)),
        [Outcome::Navigate(Navigation::Cycle(-1))]
    );
}

#[test]
fn in_navigate_mode_arrows_and_the_pane_keys_focus_panes_and_stay() {
    let mut keys = keys();
    for (key, action) in [
        (plain(KeyCode::Left), KeybindAction::FocusPaneLeft),
        (plain(KeyCode::Right), KeybindAction::FocusPaneRight),
        (key('h', KeyModifiers::NONE), KeybindAction::FocusPaneLeft),
        (key('j', KeyModifiers::NONE), KeybindAction::FocusPaneDown),
        (key('k', KeyModifiers::NONE), KeybindAction::FocusPaneUp),
        (key('l', KeyModifiers::NONE), KeybindAction::FocusPaneRight),
    ] {
        assert_eq!(
            navigating(&mut keys, &key),
            [Outcome::Action(action)],
            "{key:?}"
        );
    }
}

#[test]
fn in_navigate_mode_a_prefix_binding_runs_without_the_prefix_and_leaves() {
    let mut keys = keys();
    assert_eq!(
        navigating(&mut keys, &key('z', KeyModifiers::NONE)),
        [Outcome::Action(KeybindAction::Zoom), Outcome::Dismiss]
    );
    assert_eq!(
        navigating(&mut keys, &key('?', KeyModifiers::NONE)),
        [Outcome::Action(KeybindAction::Help), Outcome::Dismiss]
    );
}

#[test]
fn in_navigate_mode_a_prefix_custom_command_runs_and_leaves() {
    let config: Config = toml::from_str(
        r#"
[[keys.command]]
key = "prefix+y"
command = "true"
"#,
    )
    .expect("config parses");
    let mut keys = keys_with(config.keybinds(), false);
    assert_eq!(
        navigating(&mut keys, &key('y', KeyModifiers::NONE)),
        [Outcome::Command, Outcome::Dismiss]
    );
}

#[test]
fn in_navigate_mode_the_configured_cycle_keys_cycle_the_shown_panes_unless_the_selection_vanished()
{
    let config: Config = toml::from_str(
        r#"
[keys]
cycle_pane_next = "prefix+u"
cycle_pane_previous = "prefix+i"
"#,
    )
    .expect("config parses");
    let mut keys = keys_with(config.keybinds(), false);
    let next = key('u', KeyModifiers::NONE);
    assert_eq!(
        navigating(&mut keys, &next),
        [Outcome::Navigate(Navigation::Cycle(1))]
    );
    assert_eq!(
        navigating(&mut keys, &next.clone().with_kind(KeyEventKind::Repeat)),
        []
    );
    assert_eq!(
        navigating(&mut keys, &key('i', KeyModifiers::NONE)),
        [Outcome::Navigate(Navigation::Cycle(-1))]
    );
    assert_eq!(
        outcomes(
            keys.route_host(next, Some(left()), false, Mode::NavigateVanished(1), |_| {
                None
            })
        ),
        []
    );
}

#[test]
fn in_navigate_mode_an_indexed_prefix_binding_becomes_a_jump() {
    let config: Config = toml::from_str(
        r#"
[keys]
switch_tab = "prefix+alt+1..9"
"#,
    )
    .expect("config parses");
    let mut keys = keys_with(config.keybinds(), false);
    assert_eq!(
        navigating(&mut keys, &key('2', KeyModifiers::ALT)),
        [Outcome::Navigate(Navigation::Jump(
            KeybindAction::SwitchTab(1)
        ))]
    );
}

#[test]
fn in_navigate_mode_no_key_reaches_a_pane_and_direct_bindings_do_nothing() {
    let mut keys = keys();
    let typed = key('u', KeyModifiers::NONE);
    for key in [
        typed.clone(),
        typed.with_kind(KeyEventKind::Release),
        key('n', KeyModifiers::CONTROL),
    ] {
        assert_eq!(navigating(&mut keys, &key), [], "{key:?}");
    }
}

#[test]
fn a_prefix_pending_when_navigate_mode_opens_is_dropped() {
    let mut keys = keys();
    press(&mut keys, &prefix());
    assert_eq!(navigating(&mut keys, &plain(KeyCode::Up)).len(), 1);
    assert_eq!(
        press(&mut keys, &key('z', KeyModifiers::NONE)),
        [Outcome::Pane(left())]
    );
}

#[test]
fn a_key_held_before_navigate_mode_opened_stops_repeating_and_its_release_reaches_its_pane() {
    let mut keys = keys();
    let down = key('u', KeyModifiers::NONE);
    assert_eq!(press(&mut keys, &down), [Outcome::Pane(left())]);
    assert_eq!(
        navigating(&mut keys, &down.clone().with_kind(KeyEventKind::Repeat)),
        []
    );
    assert_eq!(
        navigating(&mut keys, &down.with_kind(KeyEventKind::Release)),
        [Outcome::Pane(left())]
    );
}

fn in_a_dialog(keys: &mut Keys, key: &TerminalKey) -> Vec<Outcome> {
    outcomes(
        keys.route_host(key.clone(), Some(left()), false, Mode::Dialog, |_| {
            Some("j")
        }),
    )
}

#[test]
fn in_a_dialog_every_key_and_its_repeats_go_to_the_dialog_and_none_to_a_pane_or_a_binding() {
    let mut keys = keys();
    let typed = key('j', KeyModifiers::NONE);
    for key in [
        typed.clone(),
        typed.clone().with_kind(KeyEventKind::Repeat),
        prefix(),
        key('z', KeyModifiers::NONE),
        key('n', KeyModifiers::CONTROL),
        TerminalKey::new(KeyCode::Esc, KeyModifiers::NONE),
    ] {
        assert_eq!(in_a_dialog(&mut keys, &key), [Outcome::Dialog], "{key:?}");
    }
    assert_eq!(
        in_a_dialog(&mut keys, &typed.with_kind(KeyEventKind::Release)),
        []
    );
}

#[test]
fn a_prefix_pending_when_a_dialog_opens_runs_no_binding_there_or_after() {
    let mut keys = keys();
    press(&mut keys, &prefix());
    assert_eq!(
        in_a_dialog(&mut keys, &key('z', KeyModifiers::NONE)),
        [Outcome::Dialog]
    );
    assert_eq!(
        press(&mut keys, &key('z', KeyModifiers::NONE)),
        [Outcome::Pane(left())]
    );
}

#[test]
fn a_held_tab_that_left_navigate_mode_does_not_repeat_in_the_next_navigate_session() {
    let mut keys = keys();
    let tab = plain(KeyCode::Tab);
    assert_eq!(
        navigating(&mut keys, &tab),
        [Outcome::Navigate(Navigation::Cycle(1))]
    );

    assert_eq!(
        navigating(&mut keys, &tab.clone().with_kind(KeyEventKind::Repeat)),
        []
    );
    assert_eq!(
        navigating(&mut keys, &tab.with_repeat_count(3)),
        [Outcome::Navigate(Navigation::Cycle(1))]
    );
}

#[test]
fn with_the_selection_vanished_navigate_mode_only_moves_accepts_or_leaves() {
    let mut keys = keys();
    let mut vanished = |key: &TerminalKey| {
        outcomes(keys.route_host(
            key.clone(),
            Some(left()),
            false,
            Mode::NavigateVanished(1),
            |_| None,
        ))
    };
    for key in [
        key('x', KeyModifiers::NONE),
        key('z', KeyModifiers::NONE),
        key('1', KeyModifiers::NONE),
        key('h', KeyModifiers::NONE),
        plain(KeyCode::Tab),
        plain(KeyCode::Left),
    ] {
        assert_eq!(vanished(&key), [], "{key:?}");
    }
    assert_eq!(
        vanished(&plain(KeyCode::Up)),
        [Outcome::Navigate(Navigation::Move(-1))]
    );
    assert_eq!(
        vanished(&plain(KeyCode::Enter)),
        [Outcome::Navigate(Navigation::Accept)]
    );
    for key in [
        plain(KeyCode::Esc),
        prefix(),
        key('[', KeyModifiers::CONTROL),
    ] {
        assert_eq!(vanished(&key), [Outcome::Dismiss], "{key:?}");
    }
}

#[test]
fn the_prefix_is_held_from_its_press_until_the_next_key() {
    let mut keys = keys();
    assert!(!keys.prefixed());
    press(&mut keys, &prefix());
    assert!(keys.prefixed());
    press(&mut keys, &key('u', KeyModifiers::NONE));
    assert!(!keys.prefixed());
}

#[test]
fn a_movement_key_held_into_a_reopened_navigate_session_moves_only_on_a_fresh_press() {
    let mut keys = keys();
    let down = plain(KeyCode::Down);
    assert_eq!(
        navigating(&mut keys, &down),
        [Outcome::Navigate(Navigation::Move(1))]
    );
    assert_eq!(
        navigating(&mut keys, &plain(KeyCode::Esc)),
        [Outcome::Dismiss]
    );
    press(&mut keys, &prefix());
    assert_eq!(
        press(&mut keys, &key('w', KeyModifiers::NONE)),
        [Outcome::Action(KeybindAction::WorkspacePicker)]
    );

    assert_eq!(
        navigating_in(&mut keys, 2, &down.clone().with_kind(KeyEventKind::Repeat)),
        []
    );
    assert_eq!(
        navigating_in(&mut keys, 2, &down),
        [Outcome::Navigate(Navigation::Move(1))]
    );
}
