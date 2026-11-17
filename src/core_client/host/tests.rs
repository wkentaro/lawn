use super::*;

#[test]
fn ctrl_backslash_detaches_with_or_without_the_kitty_keyboard() {
    let key = |bytes: &[u8]| match crate::raw_input::parse_framed_input(bytes).pop() {
        Some(crate::raw_input::RawInputEvent::Key(key)) => key,
        other => panic!("{bytes:?} is not a key: {other:?}"),
    };

    assert!(detaches(&key(b"\x1c")));
    assert!(detaches(&key(b"\x1b[92;5u")));
    assert!(!detaches(&key(b"\x1b[92;5:3u")));
    assert!(!detaches(&key(b"\x1b[52;5u")));
    assert!(!detaches(&key(b"\\")));
}

#[test]
fn committed_text_reaches_the_program_as_text_while_typed_keys_stay_reports() {
    let keys = |bytes: &[u8]| -> Vec<crate::input::TerminalKey> {
        crate::raw_input::parse_framed_input(bytes)
            .into_iter()
            .map(|event| match event {
                crate::raw_input::RawInputEvent::Key(key) => key,
                other => panic!("{bytes:?} is not a key: {other:?}"),
            })
            .collect()
    };
    let committed = |host: HostKeys, bytes: &[u8]| -> Vec<Option<String>> {
        keys(bytes)
            .iter()
            .map(|key| host.committed_text(key).map(str::to_owned))
            .collect()
    };
    let kitty = HostKeys {
        kitty: true,
        all: true,
        erase_is_ctrl_h: false,
    };
    let legacy = HostKeys {
        kitty: false,
        all: true,
        erase_is_ctrl_h: false,
    };

    assert_eq!(committed(kitty, b"\x1b[97;;97u"), [None]);
    assert_eq!(committed(kitty, b"a"), [Some("a".to_owned())]);
    assert_eq!(committed(legacy, b"a"), [None]);
    assert_eq!(committed(legacy, "é".as_bytes()), [Some("é".to_owned())]);
    assert_eq!(
        committed(
            HostKeys {
                all: false,
                ..kitty
            },
            b"a"
        ),
        [None]
    );
}

#[test]
fn a_zero_sized_sample_keeps_the_last_size_until_a_valid_one_follows() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::core_client::view::Geometry;

    let size = |cols, rows| Geometry {
        cols,
        rows,
        cell_width_px: 8,
        cell_height_px: 16,
    };
    let (tx, mut sizes) = tokio::sync::mpsc::unbounded_channel();
    let quit = AtomicBool::new(false);
    let mut samples = vec![
        Err(crate::platform::zero_sized_terminal_grid_error()),
        Ok(size(100, 30)),
    ]
    .into_iter();
    let mut signals = vec![true].into_iter();

    follow_host_size(
        size(80, 24),
        || {
            samples.next().unwrap_or_else(|| {
                quit.store(true, Ordering::Release);
                Ok(size(100, 30))
            })
        },
        move || signals.next().unwrap_or(false),
        &quit,
        &tx,
    );

    let mut seen = Vec::new();
    while let Ok(sample) = sizes.try_recv() {
        seen.push(sample.expect("a zero-sized sample is not a lost host"));
    }
    assert_eq!(seen, [size(100, 30)]);
}

#[test]
fn a_ctrl_h_erase_host_sends_backspace_and_alt_backspace() {
    use crossterm::event::{KeyCode, KeyModifiers};
    let key = |bytes: &[u8]| match crate::raw_input::parse_framed_input(bytes).pop() {
        Some(crate::raw_input::RawInputEvent::Key(key)) => key,
        other => panic!("{bytes:?} is not a key: {other:?}"),
    };

    let plain = key(b"\x08").with_host_erase(true);
    assert_eq!(
        (plain.code, plain.modifiers),
        (KeyCode::Backspace, KeyModifiers::empty())
    );
    let alt = key(b"\x1b\x08").with_host_erase(true);
    assert_eq!(
        (alt.code, alt.modifiers),
        (KeyCode::Backspace, KeyModifiers::ALT)
    );
    let untouched = key(b"\x08").with_host_erase(false);
    assert_ne!(untouched.code, KeyCode::Backspace);
}
