use std::io;

use clap::error::ErrorKind;
use clap::{Arg, Command};

use crate::api::schema::{PaneReportAgentSessionParams, PaneReportSessionProjection};
use crate::cli::projection::ParseError;
use crate::core_resume::{read_report, record, record_session, validate_local_target, Report};
use crate::hc::Core;

const HOOK_REPORTER: &str = "hook";

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
When that program is gone and the core has put a new shell in the terminal,
the Lawn TUI types the last stored such report into the shell once, unless
[session] resume_agents_on_restore is off. A report sent by a job of the root
shell is forgotten when the root shell is back in the foreground, because
that agent has quit. Remote terminals are not supported.
Reports are recorded only on Linux and macOS; elsewhere this command prints
"ignored".

The Claude and Codex hooks report through `lawn pane report-agent-session`
instead, for the terminal in HC_TERMINAL_ID."#;

#[derive(Debug, PartialEq)]
enum Request {
    Record(String),
    Session(String, PaneReportAgentSessionParams),
}

fn parse(args: &[String], own_terminal: Option<String>) -> Option<Result<Request, clap::Error>> {
    let words: Vec<&str> = args.iter().skip(1).take(2).map(String::as_str).collect();
    match words.as_slice() {
        ["record-resume", ..] => Some(record_resume(&args[2..])),
        ["pane", "report-agent-session"] => {
            let terminal = own_terminal?;
            Some(
                hook(
                    "lawn pane report-agent-session",
                    PaneReportSessionProjection::arguments(),
                    &args[3..],
                    PaneReportSessionProjection::from_matches,
                )
                .map(|params| Request::Session(terminal, params)),
            )
        }
        _ => None,
    }
}

fn record_resume(args: &[String]) -> Result<Request, clap::Error> {
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
        .try_get_matches_from(args)
        .map(|matches| {
            Request::Record(
                matches
                    .get_one::<String>("terminal")
                    .cloned()
                    .unwrap_or_default(),
            )
        })
}

fn hook<T>(
    name: &'static str,
    arguments: Vec<Arg>,
    args: &[String],
    read: fn(&clap::ArgMatches) -> Result<T, ParseError>,
) -> Result<T, clap::Error> {
    let matches = crate::cli::projection::command(name, arguments)
        .bin_name(name)
        .no_binary_name(true)
        .about("Report an agent session from a hook; Lawn records it for the terminal it runs in")
        .try_get_matches_from(args)?;
    read(&matches).map_err(|error| match error {
        ParseError::Clap(error) => error,
        ParseError::Usage(message) => clap::Error::raw(ErrorKind::InvalidValue, message),
        ParseError::Io(error) => clap::Error::raw(ErrorKind::Io, error),
    })
}

pub(super) fn maybe_run(args: &[String]) -> Option<io::Result<()>> {
    let parsed = parse(args, own_terminal())?;
    crate::platform::begin_cli_output();
    let request = match parsed {
        Ok(request) => request,
        Err(error) => error.exit(),
    };
    Some(execute(request).map_err(io::Error::other))
}

fn execute(request: Request) -> Result<(), String> {
    match request {
        Request::Record(terminal) => {
            let report = read_report(io::stdin().lock())?;
            let core = Core::connect().map_err(|error| error.to_string())?;
            let outcome = record(&core, &terminal, &report)?;
            println!(
                "{}",
                serde_json::to_string(&outcome).map_err(|error| error.to_string())?
            );
        }
        Request::Session(terminal, params) => {
            let replaces = replaces(&params);
            let Some(report) = session_report(params, &current_dir()?)? else {
                return Ok(());
            };
            let core =
                match std::env::var_os("HC_TERMINAL_SOCKET").filter(|socket| !socket.is_empty()) {
                    Some(socket) => Core::connect_at(&socket),
                    None => Core::connect(),
                }
                .map_err(|error| error.to_string())?;
            record_session(&core, &terminal, &report, replaces)?;
        }
    }
    Ok(())
}

fn replaces(params: &PaneReportAgentSessionParams) -> bool {
    crate::terminal::state::TerminalState::session_report_allows_session_replacement(
        params.source.trim(),
        &params.agent,
        crate::agent_resume::normalize_session_start_source(params.session_start_source.clone())
            .as_deref(),
    )
}

fn session_report(
    params: PaneReportAgentSessionParams,
    cwd: &str,
) -> Result<Option<Report>, String> {
    let source = params.source.trim();
    let Some(plan) = crate::agent_resume::session_ref_from_report(
        source,
        &params.agent,
        params.agent_session_id,
        params.agent_session_path,
    )
    .and_then(|session| crate::agent_resume::plan(source, &params.agent, &session)) else {
        return Ok(None);
    };
    Ok(Some(Report {
        source: source.to_owned(),
        agent: plan.agent,
        reporter_id: HOOK_REPORTER.to_owned(),
        seq: params.seq.ok_or("a session report needs --seq")?,
        cwd: cwd.to_owned(),
        argv: plan.argv,
        program: None,
        recorded: None,
        job: None,
    }))
}

fn own_terminal() -> Option<String> {
    std::env::var("HC_TERMINAL_ID")
        .ok()
        .filter(|terminal| !terminal.is_empty())
}

fn current_dir() -> Result<String, String> {
    std::env::current_dir()
        .map_err(|error| error.to_string())?
        .into_os_string()
        .into_string()
        .map_err(|_| "the current directory is not valid UTF-8".to_owned())
}

#[cfg(test)]
mod tests;
