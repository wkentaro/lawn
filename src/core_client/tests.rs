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

#[cfg(unix)]
mod live;
