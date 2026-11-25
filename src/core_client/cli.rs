use std::io;
use std::path::PathBuf;

use clap::{value_parser, Arg, ArgAction, ArgMatches};

use super::commands::{self, Command, Program};
use crate::hc::Core;
use crate::session_doc::{Direction, Placement};

const DEFAULT_RATIO: f32 = 0.5;

/// Runs a rewritten command, or returns `None` so the old CLI handles it.
pub(crate) fn maybe_run(args: &[String]) -> Option<io::Result<()>> {
    let args = args.get(1..)?;
    let parsed = parse(args)?;
    let command = match parsed {
        Ok(command) => command,
        Err(error) => error.exit(),
    };
    let session = crate::session::active_name()
        .unwrap_or_else(|| crate::session::DEFAULT_SESSION_NAME.to_owned());
    Some(execute(&session, command).map_err(io::Error::other))
}

fn execute(session: &str, command: Command) -> Result<(), String> {
    let core = Core::connect().map_err(|err| err.to_string())?;
    if command == (Command::Ls { json: false }) {
        print!("{}", commands::ls_text(&commands::ls(&core, session)?));
        return Ok(());
    }
    println!("{}", commands::run(&core, session, command)?);
    Ok(())
}

pub(super) fn handles(args: &[String]) -> bool {
    let words: Vec<&str> = args.iter().take(2).map(String::as_str).collect();
    matches!(
        words.as_slice(),
        ["ls", ..]
            | ["park", ..]
            | ["unpark", ..]
            | ["workspace", "create"]
            | ["tab", "create"]
            | ["pane", "split" | "close"]
            | ["start", "drop"]
    )
}

/// Parses `args` (without the program name) when they name a rewritten
/// command.
pub(super) fn parse(args: &[String]) -> Option<Result<Command, clap::Error>> {
    if !handles(args) {
        return None;
    }
    Some(
        command()
            .try_get_matches_from(args)
            .and_then(|matches| from_matches(&matches)),
    )
}

fn command() -> clap::Command {
    let terminal = || Arg::new("terminal").value_name("TERMINAL").required(true);
    let down = || Arg::new("down").long("down").action(ArgAction::SetTrue);
    clap::Command::new("lawn")
        .bin_name("lawn")
        .no_binary_name(true)
        .subcommand_required(true)
        .subcommand(
            clap::Command::new("ls").arg(Arg::new("json").long("json").action(ArgAction::SetTrue)),
        )
        .subcommand(
            clap::Command::new("workspace")
                .subcommand_required(true)
                .subcommand(program(
                    clap::Command::new("create")
                        .arg(Arg::new("on").long("on").value_name("MACHINE"))
                        .arg(name()),
                )),
        )
        .subcommand(
            clap::Command::new("tab")
                .subcommand_required(true)
                .subcommand(program(
                    clap::Command::new("create")
                        .arg(Arg::new("workspace").long("workspace").value_name("W"))
                        .arg(name()),
                )),
        )
        .subcommand(
            clap::Command::new("pane")
                .subcommand_required(true)
                .subcommand(program(
                    clap::Command::new("split").arg(terminal()).arg(down()).arg(
                        Arg::new("ratio")
                            .long("ratio")
                            .value_name("R")
                            .value_parser(value_parser!(f32)),
                    ),
                ))
                .subcommand(clap::Command::new("close").arg(terminal())),
        )
        .subcommand(clap::Command::new("park").arg(terminal()))
        .subcommand(
            clap::Command::new("start")
                .subcommand_required(true)
                .subcommand(
                    clap::Command::new("drop")
                        .arg(Arg::new("reference").value_name("START").required(true)),
                ),
        )
        .subcommand(
            clap::Command::new("unpark")
                .arg(terminal())
                .arg(
                    Arg::new("beside")
                        .long("beside")
                        .value_name("TERMINAL")
                        .conflicts_with("tab"),
                )
                .arg(down().requires("beside"))
                .arg(Arg::new("tab").long("tab").action(ArgAction::SetTrue))
                .arg(
                    Arg::new("workspace")
                        .long("workspace")
                        .value_name("W")
                        .requires("tab"),
                ),
        )
}

fn name() -> Arg {
    Arg::new("name").long("name").value_name("N")
}

fn program(command: clap::Command) -> clap::Command {
    command
        .arg(
            Arg::new("cwd")
                .long("cwd")
                .value_name("DIR")
                .value_parser(value_parser!(PathBuf)),
        )
        .arg(
            Arg::new("argv")
                .value_name("ARGV")
                .num_args(1..)
                .last(true)
                .allow_hyphen_values(true),
        )
}

fn from_matches(matches: &ArgMatches) -> Result<Command, clap::Error> {
    let string = |matches: &ArgMatches, id: &str| matches.get_one::<String>(id).cloned();
    let terminal = |matches: &ArgMatches| string(matches, "terminal").unwrap_or_default();
    let direction = |matches: &ArgMatches| {
        if matches.get_flag("down") {
            Direction::Down
        } else {
            Direction::Right
        }
    };
    let command = match matches.subcommand() {
        Some(("ls", matches)) => Command::Ls {
            json: matches.get_flag("json"),
        },
        Some(("workspace", matches)) => {
            let Some(("create", matches)) = matches.subcommand() else {
                return Err(missing_subcommand());
            };
            let on = string(matches, "on");
            Command::WorkspaceCreate {
                name: string(matches, "name"),
                program: program_of(matches, on.is_some())?,
                on,
            }
        }
        Some(("tab", matches)) => {
            let Some(("create", matches)) = matches.subcommand() else {
                return Err(missing_subcommand());
            };
            let workspace = string(matches, "workspace");
            Command::TabCreate {
                name: string(matches, "name"),
                program: program_of(matches, workspace.as_deref().is_some_and(far))?,
                workspace,
            }
        }
        Some(("pane", matches)) => match matches.subcommand() {
            Some(("split", matches)) => Command::PaneSplit {
                beside: terminal(matches),
                direction: direction(matches),
                ratio: matches
                    .get_one::<f32>("ratio")
                    .copied()
                    .unwrap_or(DEFAULT_RATIO),
                program: program_of(matches, far(&terminal(matches)))?,
            },
            Some(("close", matches)) => Command::PaneClose {
                terminal: terminal(matches),
            },
            _ => return Err(missing_subcommand()),
        },
        Some(("park", matches)) => Command::Park {
            terminal: terminal(matches),
        },
        Some(("start", matches)) => {
            let Some(("drop", matches)) = matches.subcommand() else {
                return Err(missing_subcommand());
            };
            Command::StartDrop {
                reference: string(matches, "reference").unwrap_or_default(),
            }
        }
        Some(("unpark", matches)) => Command::Unpark {
            terminal: terminal(matches),
            to: match string(matches, "beside") {
                Some(beside) => Some(Placement::Beside {
                    terminal: beside,
                    direction: direction(matches),
                }),
                None if matches.get_flag("tab") => Some(Placement::NewTab {
                    workspace: string(matches, "workspace"),
                }),
                None => None,
            },
        },
        _ => return Err(missing_subcommand()),
    };
    Ok(command)
}

fn far(address: &str) -> bool {
    address.contains('/')
}

fn program_of(matches: &ArgMatches, far: bool) -> Result<Program, clap::Error> {
    let cwd = match (matches.get_one::<PathBuf>("cwd"), far) {
        (Some(cwd), false) => Some(std::path::absolute(cwd)?),
        (Some(cwd), true) => Some(cwd.clone()),
        (None, false) => Some(std::env::current_dir()?),
        (None, true) => None,
    };
    let argv = matches
        .get_many::<String>("argv")
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    Ok(Program { cwd, argv })
}

fn missing_subcommand() -> clap::Error {
    command().error(
        clap::error::ErrorKind::MissingSubcommand,
        "missing subcommand",
    )
}
