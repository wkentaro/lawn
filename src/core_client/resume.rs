use std::io;

use clap::{Arg, Command};

use crate::core_resume::{read_report, record, validate_local_target};
use crate::hc::Core;

const REPORT_HELP: &str = r#"Reads one JSON report from standard input, at most 64 KiB:

  {
    "source": "integration",
    "agent": "worker",
    "reporter_id": "unique-agent-invocation",
    "seq": 1,
    "cwd": "/absolute/project/directory",
    "argv": ["worker", "--resume", "session-id"]
  }

Use an absolute cwd for this machine. Give each agent invocation a new
reporter_id, with increasing seq values inside that invocation. Repeating an
identical sequence is safe; a different payload at the same sequence conflicts.
Sources, agents, and invocations keep separate records. The argv must start
with a plain command name and contain no apostrophes; other reports are
refused.

Lawn records which program the report belongs to: the terminal's root
program, when this command runs inside it. A report from outside a running
root program is not stored and prints the status "ignored". A program sent
in the report is ignored. Recording never executes the reported command.
Existing agent hooks are not connected to this recorder yet. Remote terminals
are not supported. Reports are recorded only on Linux and macOS; elsewhere
this command prints "ignored"."#;

fn parse(args: &[String]) -> Option<Result<String, clap::Error>> {
    if args.get(1).map(String::as_str) != Some("record-resume") {
        return None;
    }
    Some(
        Command::new("record-resume")
            .bin_name("lawn record-resume")
            .no_binary_name(true)
            .about("Record an agent resume command without running it")
            .after_long_help(REPORT_HELP)
            .arg(
                Arg::new("terminal")
                    .value_name("TERMINAL")
                    .required(true)
                    .value_parser(|target: &str| {
                        validate_local_target(target)?;
                        Ok::<_, String>(target.to_owned())
                    }),
            )
            .try_get_matches_from(&args[2..])
            .map(|matches| {
                matches
                    .get_one::<String>("terminal")
                    .cloned()
                    .unwrap_or_default()
            }),
    )
}

pub(super) fn maybe_run(args: &[String]) -> Option<io::Result<()>> {
    let parsed = parse(args)?;
    crate::platform::begin_cli_output();
    let terminal = match parsed {
        Ok(terminal) => terminal,
        Err(error) => error.exit(),
    };
    Some(execute(&terminal).map_err(io::Error::other))
}

fn execute(terminal: &str) -> Result<(), String> {
    let report = read_report(io::stdin().lock())?;
    let core = Core::connect().map_err(|error| error.to_string())?;
    let outcome = record(&core, terminal, &report)?;
    println!(
        "{}",
        serde_json::to_string(&outcome).map_err(|error| error.to_string())?
    );
    Ok(())
}

#[cfg(test)]
mod tests;
