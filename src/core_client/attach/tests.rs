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
