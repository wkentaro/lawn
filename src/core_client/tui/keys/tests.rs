use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers, ModifierKeyCode};

use super::{Keys, Routed};
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
        })
        .collect()
}

fn press_in(keys: &mut Keys, key: &TerminalKey, pane: PaneId) -> Vec<Outcome> {
    outcomes(keys.route_host(key.clone(), Some(pane), false, false, |_| None))
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
    let routed = keys.route_host(erase.clone(), Some(left()), false, false, |_| None);
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
        outcomes(keys.route_host(committed.clone(), Some(left()), false, false, |_| Some("n"))),
        [Outcome::Text(left(), "n".to_owned())]
    );

    press(&mut keys, &prefix());
    assert_eq!(
        outcomes(keys.route_host(
            key('z', KeyModifiers::NONE),
            Some(left()),
            false,
            false,
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
    let routed = keys.route_host(host_key(b"x"), Some(left()), false, false, |key| {
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
    outcomes(keys.route_host(key.clone(), Some(left()), true, false, |_| None))
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
    outcomes(keys.route_host(key.clone(), Some(left()), false, true, |_| Some("j")))
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
