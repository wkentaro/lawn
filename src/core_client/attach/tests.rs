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
