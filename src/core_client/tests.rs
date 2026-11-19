use std::path::PathBuf;

use super::cli::parse;
use super::commands::{Command, Program};
use crate::session_doc::{Direction, Placement};

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

fn parsed(words: &[&str]) -> Command {
    parse(&args(words))
        .expect("handled")
        .expect("valid arguments")
}

fn here() -> PathBuf {
    std::env::current_dir().expect("current dir")
}

#[test]
fn unbuilt_commands_fall_through_to_the_old_cli() {
    for words in [
        &[][..],
        &["--help"],
        &["--version"],
        &["workspace", "list"],
        &["pane", "list"],
        &["tab"],
        &["server"],
    ] {
        assert!(parse(&args(words)).is_none(), "{words:?}");
    }
}

#[test]
fn the_skill_needs_no_core_or_terminal() {
    std::env::set_var(crate::hc::HC_BIN_ENV, "/nonexistent/lawn-skill-test-hc");
    assert!(matches!(
        super::maybe_run(&args(&["lawn", "--skill"])),
        Some(Ok(()))
    ));
}

#[test]
fn the_skill_does_not_override_help() {
    for words in [
        &["lawn", "--help", "--skill"][..],
        &["lawn", "--skill", "--help"],
    ] {
        assert!(super::maybe_run(&args(words)).is_none());
    }
}

#[test]
fn ls_takes_json() {
    assert_eq!(parsed(&["ls"]), Command::Ls { json: false });
    assert_eq!(parsed(&["ls", "--json"]), Command::Ls { json: true });
}

#[test]
fn create_commands_take_a_program_after_the_separator() {
    assert_eq!(
        parsed(&["workspace", "create", "--name", "main", "--", "sleep", "-x"]),
        Command::WorkspaceCreate {
            name: Some("main".to_owned()),
            program: Program {
                cwd: here(),
                argv: args(&["sleep", "-x"]),
            },
        }
    );
    let elsewhere = here().join("elsewhere");
    let elsewhere_arg = elsewhere.to_str().expect("UTF-8 path");
    assert_eq!(
        parsed(&["tab", "create", "--workspace", "w0", "--cwd", elsewhere_arg]),
        Command::TabCreate {
            workspace: Some("w0".to_owned()),
            name: None,
            program: Program {
                cwd: elsewhere.clone(),
                argv: Vec::new(),
            },
        }
    );
}

#[test]
fn a_relative_cwd_is_made_absolute() {
    let Command::WorkspaceCreate { program, .. } = parsed(&["workspace", "create", "--cwd", "sub"])
    else {
        panic!("not workspace create");
    };
    assert_eq!(program.cwd, here().join("sub"));
}

#[test]
fn pane_split_defaults_to_an_even_right_split() {
    assert_eq!(
        parsed(&["pane", "split", "t3"]),
        Command::PaneSplit {
            beside: "t3".to_owned(),
            direction: Direction::Right,
            ratio: 0.5,
            program: Program {
                cwd: here(),
                argv: Vec::new(),
            },
        }
    );
    let Command::PaneSplit {
        direction, ratio, ..
    } = parsed(&["pane", "split", "t3", "--down", "--ratio", "0.25"])
    else {
        panic!("not pane split");
    };
    assert_eq!((direction, ratio), (Direction::Down, 0.25));
}

#[test]
fn terminal_commands_need_a_terminal() {
    assert_eq!(
        parsed(&["pane", "close", "t3"]),
        Command::PaneClose {
            terminal: "t3".to_owned()
        }
    );
    assert_eq!(
        parsed(&["park", "t3"]),
        Command::Park {
            terminal: "t3".to_owned()
        }
    );
    for words in [&["pane", "close"][..], &["park"], &["unpark"]] {
        assert!(matches!(parse(&args(words)), Some(Err(_))), "{words:?}");
    }
}

#[test]
fn unpark_takes_an_optional_placement() {
    let unpark = |to| Command::Unpark {
        terminal: "t3".to_owned(),
        to,
    };
    assert_eq!(parsed(&["unpark", "t3"]), unpark(None));
    assert_eq!(
        parsed(&["unpark", "t3", "--beside", "t1", "--down"]),
        unpark(Some(Placement::Beside {
            terminal: "t1".to_owned(),
            direction: Direction::Down,
        }))
    );
    assert_eq!(
        parsed(&["unpark", "t3", "--tab", "--workspace", "w0"]),
        unpark(Some(Placement::NewTab {
            workspace: Some("w0".to_owned()),
        }))
    );
    for words in [
        &["unpark", "t3", "--beside", "t1", "--tab"][..],
        &["unpark", "t3", "--down"],
        &["unpark", "t3", "--workspace", "w0"],
    ] {
        assert!(matches!(parse(&args(words)), Some(Err(_))), "{words:?}");
    }
}

#[test]
fn ls_text_lists_a_parked_terminal_only_under_parked() {
    let mut layout = crate::session_doc::Layout::default();
    layout.add_workspace(Some("main".to_owned()), "t1".to_owned());
    layout.split("t1", Direction::Right, 0.5, "t2".to_owned());
    layout.park("t2");
    assert_eq!(
        super::commands::ls_text(&layout),
        "ws0 main\n  tab1\n    t1\nparked:\n  t2\n"
    );
}

#[cfg(unix)]
mod live;

#[cfg(unix)]
fn run_inside_a_pane(words: &str) -> std::process::Output {
    const WORDS: &str = "LAWN_TEST_NESTED_WORDS";
    let config = std::env::temp_dir().join(format!("lawn-nested-test-{}", std::process::id()));
    std::fs::create_dir_all(&config).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "core_client::tests::a_child_runs_the_words_it_was_given",
            "--nocapture",
        ])
        .env(WORDS, words)
        .env(crate::HERDR_ENV_VAR, crate::HERDR_ENV_VALUE)
        .env("XDG_CONFIG_HOME", &config)
        .env_remove(crate::config::CONFIG_PATH_ENV_VAR)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    std::fs::remove_dir_all(&config).unwrap();
    output
}

#[cfg(unix)]
#[test]
fn a_child_runs_the_words_it_was_given() {
    let Some(words) = std::env::var_os("LAWN_TEST_NESTED_WORDS") else {
        return;
    };
    let words: Vec<String> = words
        .to_str()
        .unwrap()
        .split(' ')
        .map(str::to_owned)
        .collect();
    let _ = super::maybe_run(&words);
    println!("the command ran");
}

#[cfg(unix)]
#[test]
fn the_tui_refuses_to_open_inside_a_herdr_pane() {
    let output = run_inside_a_pane("lawn");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("nested lawn is disabled"));
}

#[cfg(unix)]
#[test]
fn commands_still_run_inside_a_herdr_pane() {
    let output = run_inside_a_pane("lawn record-resume");

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("nested lawn is disabled"));
}
