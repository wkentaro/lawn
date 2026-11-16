use super::*;

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

#[test]
fn only_attach_with_one_terminal_is_taken_from_the_old_cli() {
    assert!(maybe_run(&args(&["lawn", "ls"])).is_none());
    assert!(maybe_run(&args(&["lawn", "attach"])).is_none());
    assert!(maybe_run(&args(&["lawn", "attach", "--help"])).is_none());
    assert!(maybe_run(&args(&["lawn", "attach", "t1", "t2"])).is_none());
}

#[cfg(unix)]
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

#[cfg(unix)]
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
    };
    let legacy = HostKeys {
        kitty: false,
        all: true,
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
