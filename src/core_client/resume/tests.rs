use super::*;

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).into()).collect()
}

#[test]
fn only_resume_recording_is_handled() {
    for words in [
        &[][..],
        &["lawn"],
        &["lawn", "resume"],
        &["lawn", "pane", "close", "t1"],
    ] {
        assert!(parse(&args(words)).is_none());
    }
    assert_eq!(
        parse(&args(&["lawn", "record-resume", "agent-terminal"]))
            .unwrap()
            .unwrap(),
        "agent-terminal"
    );
}

#[test]
fn help_describes_recording_without_execution() {
    let error = parse(&args(&["lawn", "record-resume", "--help"]))
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
    let help = error.to_string();
    assert!(help.contains("reporter_id"));
    assert!(help.contains("never executes the reported command"));
    assert!(help.contains("not connected"));
}

#[test]
fn help_says_reports_are_recorded_only_on_linux_and_macos() {
    let error = parse(&args(&["lawn", "record-resume", "--help"]))
        .unwrap()
        .unwrap_err();
    let help = error.to_string();
    assert!(help.contains("only on Linux and macOS"));
}

#[test]
fn recording_requires_exactly_one_terminal() {
    for words in [
        &["lawn", "record-resume"][..],
        &["lawn", "record-resume", ""][..],
        &["lawn", "record-resume", "t1", "extra"][..],
        &["lawn", "record-resume", "box/t7"][..],
        &["lawn", "record-resume", "--unknown"][..],
    ] {
        assert!(parse(&args(words)).unwrap().is_err());
    }
}
