use super::*;

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).into()).collect()
}

fn inside(words: &[&str]) -> Option<Result<Request, clap::Error>> {
    parse(&args(words), Some("t4".into()))
}

#[test]
fn only_resume_recording_is_handled() {
    for words in [
        &[][..],
        &["lawn"],
        &["lawn", "resume"],
        &["lawn", "pane", "close", "t1"],
    ] {
        assert!(inside(words).is_none());
    }
    assert_eq!(
        inside(&["lawn", "record-resume", "agent-terminal"])
            .unwrap()
            .unwrap(),
        Request::Record("agent-terminal".into())
    );
}

#[test]
fn help_describes_recording_without_execution() {
    let error = inside(&["lawn", "record-resume", "--help"])
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
    let help = error.to_string();
    assert!(help.contains("reporter_id"));
    assert!(help.contains("never executes the reported command"));
    assert!(help.contains("report-agent-session"));
}

#[test]
fn help_says_reports_are_recorded_only_on_linux_and_macos() {
    let error = inside(&["lawn", "record-resume", "--help"])
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
        assert!(inside(words).unwrap().is_err());
    }
}

fn session(words: &[&str]) -> PaneReportAgentSessionParams {
    let mut all = vec!["lawn", "pane", "report-agent-session"];
    all.extend_from_slice(words);
    match inside(&all).unwrap().unwrap() {
        Request::Session(terminal, params) => {
            assert_eq!(terminal, "t4");
            params
        }
        other => panic!("{other:?}"),
    }
}

const CLAUDE: &[&str] = &[
    "w1:p2",
    "--source",
    "herdr:claude",
    "--agent",
    "claude",
    "--seq",
    "42",
    "--agent-session-id",
    "abc-123",
    "--session-start-source",
    "startup",
];

#[test]
fn a_hook_session_report_records_the_agents_resume_command() {
    assert_eq!(
        session_report(session(CLAUDE), "/srv/app").unwrap(),
        Some(Report {
            source: "herdr:claude".into(),
            agent: "claude".into(),
            reporter_id: HOOK_REPORTER.into(),
            seq: 42,
            cwd: "/srv/app".into(),
            argv: vec!["claude".into(), "--resume".into(), "abc-123".into()],
            program: None,
            recorded: None,
            job: None,
        })
    );
}

#[test]
fn a_session_the_planner_cannot_resume_records_nothing() {
    let mut words = CLAUDE.to_vec();
    words[2] = "custom:claude";
    assert_eq!(session_report(session(&words), "/srv/app").unwrap(), None);
}

#[test]
fn a_session_report_without_a_sequence_is_refused() {
    let unsequenced: Vec<&str> = CLAUDE
        .iter()
        .copied()
        .filter(|word| !["--seq", "42"].contains(word))
        .collect();
    assert!(session_report(session(&unsequenced), "/").is_err());
}

#[test]
fn a_session_report_outside_a_core_terminal_is_left_to_the_old_cli() {
    let mut words = vec!["lawn", "pane", "report-agent-session"];
    words.extend_from_slice(CLAUDE);
    words[7] = "no-such-agent";
    let previous = std::env::var_os("HC_TERMINAL_ID");
    std::env::set_var("HC_TERMINAL_ID", "");
    let empty = maybe_run(&args(&words)).is_none();
    std::env::remove_var("HC_TERMINAL_ID");
    let absent = maybe_run(&args(&words)).is_none();
    if let Some(previous) = previous {
        std::env::set_var("HC_TERMINAL_ID", previous);
    }
    assert!(empty);
    assert!(absent);
    assert!(inside(&words).is_some());
}

#[test]
fn other_pane_commands_are_not_hook_reports() {
    assert!(inside(&["lawn", "pane", "report-agent", "p1"]).is_none());
    assert!(inside(&["lawn", "pane", "report-agent-session"])
        .unwrap()
        .is_err());
}

#[test]
fn hook_reports_replace_a_running_session_as_the_old_server_allows() {
    for (source, agent, start, allowed) in [
        ("herdr:claude", "claude", Some("startup"), false),
        ("herdr:claude", "claude", None, false),
        ("herdr:claude", "claude", Some("resume"), true),
        ("herdr:claude", "claude", Some("clear"), true),
        ("herdr:claude", "claude", Some("compact"), true),
        ("herdr:codex", "codex", Some("startup"), true),
        ("herdr:codex", "codex", None, false),
    ] {
        let mut words = vec![
            "t1",
            "--source",
            source,
            "--agent",
            agent,
            "--seq",
            "1",
            "--agent-session-id",
            "s",
        ];
        if let Some(start) = start {
            words.extend(["--session-start-source", start]);
        }
        assert_eq!(replaces(&session(&words)), allowed, "{agent} {start:?}");
    }
}

#[test]
fn a_padded_start_source_replaces_as_the_old_server_reads_it() {
    let words = [
        "t1",
        "--source",
        "herdr:claude",
        "--agent",
        "claude",
        "--seq",
        "1",
        "--agent-session-id",
        "s",
        "--session-start-source",
        " resume ",
    ];
    assert!(replaces(&session(&words)));
}

#[test]
fn a_hook_refuses_a_relative_socket_for_the_core_that_issued_its_terminal() {
    for relative in ["core.sock", "./core.sock", "../run/core.sock"] {
        let error = issuing_socket(Some(relative.into())).unwrap_err();
        assert!(error.contains(relative), "{error}");
    }
}

#[cfg(unix)]
#[test]
fn a_hook_uses_an_absolute_socket_for_the_core_that_issued_its_terminal() {
    assert_eq!(
        issuing_socket(Some("/run/core.sock".into())),
        Ok(Some("/run/core.sock".into()))
    );
    assert_eq!(issuing_socket(Some("".into())), Ok(None));
    assert_eq!(issuing_socket(None), Ok(None));
}
