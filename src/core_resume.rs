use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::hc::{Core, Doc};

const SCHEMA: u32 = 1;
const EDIT_ATTEMPTS: usize = 8;
const MAX_REPORT_BYTES: usize = 64 * 1024;
const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 512;
const MAX_CWD_BYTES: usize = 4096;
const MAX_ARGS: usize = 64;
const MAX_ARGV_BYTES: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReportedProgram {
    pub(crate) pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) started_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) boot: Option<String>,
}

impl ReportedProgram {
    #[cfg(any(unix, test))]
    fn ended(&self, current: &Self) -> bool {
        self.pid != current.pid
            || matches!((self.started_ms, current.started_ms), (Some(was), Some(now)) if was != now)
            || matches!((&self.boot, &current.boot), (Some(was), Some(now)) if was != now)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Report {
    pub(crate) source: String,
    pub(crate) agent: String,
    pub(crate) reporter_id: String,
    pub(crate) seq: u64,
    pub(crate) cwd: String,
    pub(crate) argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) program: Option<ReportedProgram>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) recorded: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) job: Option<u32>,
}

impl Report {
    fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("source", &self.source),
            ("agent", &self.agent),
            ("reporter_id", &self.reporter_id),
        ] {
            if value.trim().is_empty()
                || value.len() > MAX_IDENTIFIER_BYTES
                || value.chars().any(char::is_control)
            {
                return Err(format!("{name} must be nonblank, at most {MAX_IDENTIFIER_BYTES} bytes, and contain no control characters"));
            }
        }
        if self.argv.is_empty() || self.argv[0].is_empty() {
            return Err("argv must contain a nonempty executable".into());
        }
        if self.argv.len() > MAX_ARGS
            || self.argv.iter().map(String::len).sum::<usize>() > MAX_ARGV_BYTES
            || self
                .argv
                .iter()
                .any(|arg| arg.chars().any(char::is_control))
        {
            return Err(format!("argv allows at most {MAX_ARGS} arguments and {MAX_ARGV_BYTES} bytes, with no control characters"));
        }
        if !Path::new(&self.cwd).is_absolute()
            || self.cwd.len() > MAX_CWD_BYTES
            || self.cwd.chars().any(char::is_control)
        {
            return Err(format!("cwd must be absolute, at most {MAX_CWD_BYTES} bytes, and contain no control characters"));
        }
        if self
            .program
            .as_ref()
            .is_some_and(|program| program.pid == 0)
        {
            return Err("a reported program pid must be nonzero".into());
        }
        Ok(())
    }

    fn payload(&self) -> Self {
        Self {
            program: None,
            recorded: None,
            job: None,
            ..self.clone()
        }
    }

    fn same_reporter(&self, other: &Self) -> bool {
        self.source == other.source
            && self.agent == other.agent
            && self.reporter_id == other.reporter_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalIdentity {
    address: String,
    created_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema: u32,
    record_only: bool,
    terminal: TerminalIdentity,
    reports: Vec<Report>,
}

impl Document {
    fn decode(value: Value, terminal: &TerminalIdentity) -> Result<Self, String> {
        if value.is_null() {
            return Ok(Self {
                schema: SCHEMA,
                record_only: false,
                terminal: terminal.clone(),
                reports: Vec::new(),
            });
        }
        if value["schema"].as_u64() != Some(u64::from(SCHEMA)) {
            return Err("unsupported resume-record schema; update Lawn before writing it".into());
        }
        let mut document: Self = serde_json::from_value(value)
            .map_err(|error| format!("cannot read resume records: {error}"))?;
        if document.terminal != *terminal {
            return Err("resume records have a different terminal identity".into());
        }
        let mut reporters = HashSet::new();
        for report in &mut document.reports {
            report.validate()?;
            if !reporters.insert((
                report.source.clone(),
                report.agent.clone(),
                report.reporter_id.clone(),
            )) {
                return Err("resume records contain a duplicate reporter identity".into());
            }
            if document.record_only {
                report.program = None;
                report.job = None;
            }
        }
        document.record_only = false;
        Ok(document)
    }

    fn apply(&mut self, report: &Report) -> Result<(RecordStatus, u64), String> {
        if report.program.is_none() {
            return Ok((RecordStatus::Ignored, report.seq));
        }
        let stored = self
            .reports
            .iter_mut()
            .find(|stored| stored.same_reporter(report));
        if let Some(stored) = &stored {
            if report.seq < stored.seq {
                return Ok((RecordStatus::Stale, stored.seq));
            }
            if report.seq == stored.seq {
                return if report.payload() == stored.payload() {
                    Ok((RecordStatus::Unchanged, stored.seq))
                } else {
                    Err("the same reporter sequence contains a different resume report".into())
                };
            }
        }
        match stored {
            Some(stored) => *stored = report.clone(),
            None => self.reports.push(report.clone()),
        }
        Ok((RecordStatus::Recorded, report.seq))
    }

    fn encode(&self) -> Result<Value, String> {
        let value = serde_json::to_value(self).map_err(|error| error.to_string())?;
        if serde_json::to_vec(&value)
            .map_err(|error| error.to_string())?
            .len()
            > MAX_DOCUMENT_BYTES
        {
            return Err("resume records exceed the core's 1 MiB document limit".into());
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecordStatus {
    Recorded,
    Unchanged,
    Stale,
    Ignored,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Outcome {
    terminal: String,
    status: RecordStatus,
    seq: u64,
}

enum WriteFailure {
    Conflict,
    Other(String),
}

trait Backend {
    fn terminal(&self, target: &str) -> Result<Value, String>;
    fn load(&self, name: &str) -> Result<Doc, String>;
    fn save(&self, name: &str, value: &Value, version: u64) -> Result<u64, WriteFailure>;
    fn started(&self, pid: u32) -> Option<u64>;
    fn runs_inside(&self, root: u32) -> bool;
    fn foreground(&self, root: u32) -> Option<u32>;
    fn reporter_in_job(&self, root: u32, job: u32) -> bool;
    fn boot(&self) -> Option<String>;
    #[cfg(any(unix, test))]
    fn can_send(&self) -> bool;
    #[cfg(any(unix, test))]
    fn cwd(&self, pid: u32) -> Option<std::path::PathBuf>;
    #[cfg(any(unix, test))]
    fn send(&self, target: &str, text: &str) -> Result<(), String>;
}

impl Backend for Core {
    fn terminal(&self, target: &str) -> Result<Value, String> {
        self.call("terminal.get", json!({"terminal": target}))
            .map_err(|error| error.to_string())
    }

    fn load(&self, name: &str) -> Result<Doc, String> {
        self.doc_get(name, None).map_err(|error| error.to_string())
    }

    fn save(&self, name: &str, value: &Value, version: u64) -> Result<u64, WriteFailure> {
        self.doc_set(name, None, value, version).map_err(|error| {
            if error.kind() == Some("conflict") {
                WriteFailure::Conflict
            } else {
                WriteFailure::Other(error.to_string())
            }
        })
    }

    fn started(&self, pid: u32) -> Option<u64> {
        crate::platform::process_start_token(pid)
    }

    fn runs_inside(&self, root: u32) -> bool {
        runs_inside(root)
    }

    fn foreground(&self, root: u32) -> Option<u32> {
        crate::platform::foreground_process_group_id(root)
    }

    fn reporter_in_job(&self, root: u32, job: u32) -> bool {
        reporter_in_job(root, job)
    }

    fn boot(&self) -> Option<String> {
        crate::platform::boot_id()
    }

    #[cfg(any(unix, test))]
    fn can_send(&self) -> bool {
        self.has_method("terminal.send")
    }

    #[cfg(any(unix, test))]
    fn cwd(&self, pid: u32) -> Option<std::path::PathBuf> {
        crate::platform::process_cwd(pid)
    }

    #[cfg(any(unix, test))]
    fn send(&self, target: &str, text: &str) -> Result<(), String> {
        self.call("terminal.send", json!({"terminal": target, "text": text}))
            .map(drop)
            .map_err(|error| error.to_string())
    }
}

#[cfg(unix)]
fn runs_inside(root: u32) -> bool {
    let reporter = std::process::id();
    crate::platform::process_start_token(reporter)
        .and_then(|token| crate::platform::live_pane_process_group(root, reporter, token))
        .is_some()
        || crate::platform::process_has_ancestor(reporter, root)
}

#[cfg(not(unix))]
fn runs_inside(_root: u32) -> bool {
    false
}

#[cfg(unix)]
fn reporter_in_job(root: u32, job: u32) -> bool {
    crate::platform::process_in_job(std::process::id(), root, job)
}

#[cfg(not(unix))]
fn reporter_in_job(_root: u32, _job: u32) -> bool {
    false
}

pub(crate) fn read_report(input: impl Read) -> Result<Report, String> {
    let mut bytes = Vec::new();
    input
        .take((MAX_REPORT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read resume report: {error}"))?;
    if bytes.len() > MAX_REPORT_BYTES {
        return Err("resume report exceeds 64 KiB".into());
    }
    let report: Report = serde_json::from_slice(&bytes)
        .map_err(|error| format!("cannot parse resume report: {error}"))?;
    let report = report.payload();
    report.validate()?;
    Ok(report)
}

pub(crate) fn validate_local_target(target: &str) -> Result<(), String> {
    if target.is_empty() || target.contains('/') {
        return Err("resume recording requires a local terminal".into());
    }
    Ok(())
}

fn document_name(address: &str) -> String {
    format!("{}-resume-{address}", crate::config::app_dir_name())
}

#[derive(Clone)]
struct Terminal {
    record: Value,
    identity: TerminalIdentity,
    document: String,
}

impl Terminal {
    fn get(backend: &impl Backend, target: &str) -> Result<Self, String> {
        validate_local_target(target)?;
        let record = backend.terminal(target)?;
        let identity = TerminalIdentity {
            address: record["address"]
                .as_str()
                .ok_or("the core returned no terminal address")?
                .into(),
            created_ms: record["created_ms"]
                .as_u64()
                .ok_or("the core returned no terminal creation time")?,
        };
        if identity.address.trim().is_empty()
            || identity.address.contains('/')
            || identity.address.chars().any(char::is_control)
        {
            return Err("the core returned no canonical local terminal address".into());
        }
        let document = document_name(&identity.address);
        Ok(Self {
            record,
            identity,
            document,
        })
    }

    fn pid(&self) -> Option<u32> {
        self.record["pid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|pid| *pid != 0)
    }

    fn live_root(&self) -> Option<u32> {
        self.pid().filter(|_| self.record["state"] == "live")
    }
}

fn edit<T>(
    backend: &impl Backend,
    target: &str,
    mut change: impl FnMut(&Terminal, &mut Document, u64) -> Result<(T, bool), String>,
) -> Result<T, String> {
    let resolved = Terminal::get(backend, target)?;
    for _ in 0..EDIT_ATTEMPTS {
        let loaded = backend.load(&resolved.document)?;
        let terminal = Terminal::get(backend, &resolved.identity.address)?;
        if terminal.document != resolved.document {
            return Err(format!("{target} changed its address; try again"));
        }
        let mut document = Document::decode(loaded.value, &terminal.identity)?;
        let (outcome, changed) = change(&terminal, &mut document, loaded.version)?;
        if !changed {
            return Ok(outcome);
        }
        match backend.save(&resolved.document, &document.encode()?, loaded.version) {
            Ok(_) => return Ok(outcome),
            Err(WriteFailure::Conflict) => continue,
            Err(WriteFailure::Other(error)) => return Err(error),
        }
    }
    Err(format!("{} kept changing; try again", resolved.document))
}

pub(crate) fn record(core: &Core, target: &str, report: &Report) -> Result<Outcome, String> {
    record_with(core, target, report)
}

fn record_with(backend: &impl Backend, target: &str, report: &Report) -> Result<Outcome, String> {
    let report = report.payload();
    report.validate()?;
    crate::agent_resume::validate_resume_argv(&report.argv)?;
    edit(backend, target, |terminal, document, version| {
        let root = terminal
            .live_root()
            .filter(|root| backend.runs_inside(*root));
        let program = root.and_then(|pid| {
            backend.started(pid).map(|started| ReportedProgram {
                pid,
                started_ms: Some(started),
                boot: backend.boot(),
            })
        });
        let job = program.as_ref().and_then(|program| {
            backend
                .foreground(program.pid)
                .filter(|job| *job != program.pid && backend.reporter_in_job(program.pid, *job))
        });
        let report = Report {
            job,
            recorded: program.as_ref().map(|_| version + 1),
            program,
            ..report.clone()
        };
        let (status, seq) = document.apply(&report)?;
        let outcome = Outcome {
            terminal: terminal.identity.address.clone(),
            status,
            seq,
        };
        Ok((outcome, status == RecordStatus::Recorded))
    })
}

#[cfg(unix)]
pub(crate) fn resume(core: &Core, target: &str) -> Result<bool, String> {
    resume_with(core, target)
}

#[cfg(any(unix, test))]
fn resume_with(backend: &impl Backend, target: &str) -> Result<bool, String> {
    if !backend.can_send() {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            tracing::warn!("the core cannot type into terminals, so agents are not resumed");
        });
        return Ok(false);
    }
    let claimed = edit(backend, target, |terminal, document, _| {
        let Some(root) = terminal
            .live_root()
            .filter(|root| backend.foreground(*root).is_some())
        else {
            return Ok((None, false));
        };
        let current = ReportedProgram {
            pid: root,
            started_ms: backend.started(root),
            boot: backend.boot(),
        };
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut document.reports)
            .into_iter()
            .partition(|report| {
                report
                    .program
                    .as_ref()
                    .is_some_and(|program| program.ended(&current))
            });
        document.reports = kept;
        let newest = gone
            .into_iter()
            .max_by_key(|report| (report.recorded, report.seq));
        let claimed = newest.is_some();
        Ok((
            newest.map(|report| (report, terminal.clone(), root)),
            claimed,
        ))
    })?;
    let Some((report, terminal, root)) = claimed else {
        return Ok(false);
    };
    let address = &terminal.identity.address;
    if let Err(error) = crate::agent_resume::validate_resume_argv(&report.argv) {
        tracing::warn!(%error, terminal = address, agent = report.agent, "dropped a resume command");
        return Ok(false);
    }
    if !Path::new(&report.cwd).is_dir() {
        tracing::warn!(
            terminal = address,
            cwd = report.cwd,
            agent = report.agent,
            "the resume directory is gone"
        );
        return Ok(false);
    }
    if backend.foreground(root) != Some(root) {
        tracing::warn!(
            terminal = address,
            agent = report.agent,
            "the recovered shell is not at its prompt"
        );
        return Ok(false);
    }
    let Some(command) = crate::app::agent_resume::shell_command_from_argv(&report.argv) else {
        return Ok(false);
    };
    let text = if backend
        .cwd(root)
        .is_some_and(|cwd| cwd == Path::new(&report.cwd))
    {
        command
    } else {
        format!(
            "cd {} && {command}",
            crate::app::agent_resume::shell_quote(&report.cwd)
        )
    };
    backend.send(address, &text)?;
    Ok(true)
}

#[cfg(unix)]
pub(crate) fn forget(core: &Core, target: &str) -> Result<bool, String> {
    forget_with(core, target)
}

#[cfg(any(unix, test))]
fn forget_with(backend: &impl Backend, target: &str) -> Result<bool, String> {
    edit(backend, target, |terminal, document, _| {
        let Some(root) = terminal
            .live_root()
            .filter(|root| backend.foreground(*root) == Some(*root))
        else {
            return Ok((false, false));
        };
        let current = ReportedProgram {
            pid: root,
            started_ms: backend.started(root),
            boot: backend.boot(),
        };
        let before = document.reports.len();
        document.reports.retain(|report| {
            report.job.is_none()
                || report
                    .program
                    .as_ref()
                    .is_none_or(|program| program.ended(&current))
        });
        let forgot = document.reports.len() != before;
        Ok((forgot, forgot))
    })
}

#[cfg(test)]
mod tests;
