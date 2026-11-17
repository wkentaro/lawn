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
                record_only: true,
                terminal: terminal.clone(),
                reports: Vec::new(),
            });
        }
        if value["schema"].as_u64() != Some(u64::from(SCHEMA)) {
            return Err("unsupported resume-record schema; update Lawn before writing it".into());
        }
        let document: Self = serde_json::from_value(value)
            .map_err(|error| format!("cannot read resume records: {error}"))?;
        if !document.record_only || document.terminal != *terminal {
            return Err("resume records have a different purpose or terminal identity".into());
        }
        let mut reporters = HashSet::new();
        for report in &document.reports {
            report.validate()?;
            if !reporters.insert((&report.source, &report.agent, &report.reporter_id)) {
                return Err("resume records contain a duplicate reporter identity".into());
            }
        }
        Ok(document)
    }

    fn apply(&mut self, report: &Report) -> Result<(RecordStatus, u64), String> {
        if let Some(stored) = self
            .reports
            .iter_mut()
            .find(|stored| stored.same_reporter(report))
        {
            if report.seq < stored.seq {
                return Ok((RecordStatus::Stale, stored.seq));
            }
            if report.seq == stored.seq {
                return if report == stored {
                    Ok((RecordStatus::Unchanged, stored.seq))
                } else {
                    Err("the same reporter sequence contains a different resume report".into())
                };
            }
            if report.program != stored.program {
                return Err("reporter_id was reused with different program context".into());
            }
            *stored = report.clone();
        } else {
            self.reports.push(report.clone());
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
    report.validate()?;
    Ok(report)
}

pub(crate) fn validate_local_target(target: &str) -> Result<(), String> {
    if target.is_empty() || target.contains('/') {
        return Err("resume recording requires a local terminal".into());
    }
    Ok(())
}

pub(crate) fn record(core: &Core, target: &str, report: &Report) -> Result<Outcome, String> {
    record_with(core, target, report)
}

fn record_with(backend: &impl Backend, target: &str, report: &Report) -> Result<Outcome, String> {
    report.validate()?;
    validate_local_target(target)?;
    let record = backend.terminal(target)?;
    let terminal = TerminalIdentity {
        address: record["address"]
            .as_str()
            .ok_or("the core returned no terminal address")?
            .into(),
        created_ms: record["created_ms"]
            .as_u64()
            .ok_or("the core returned no terminal creation time")?,
    };
    if terminal.address.trim().is_empty()
        || terminal.address.contains('/')
        || terminal.address.chars().any(char::is_control)
    {
        return Err("the core returned no canonical local terminal address".into());
    }
    let name = format!(
        "{}-resume-{}",
        crate::config::app_dir_name(),
        terminal.address
    );
    for _ in 0..EDIT_ATTEMPTS {
        let loaded = backend.load(&name)?;
        let mut document = Document::decode(loaded.value, &terminal)?;
        let (status, seq) = document.apply(report)?;
        let outcome = Outcome {
            terminal: terminal.address.clone(),
            status,
            seq,
        };
        if status != RecordStatus::Recorded {
            return Ok(outcome);
        }
        let value = document.encode()?;
        match backend.save(&name, &value, loaded.version) {
            Ok(_) => return Ok(outcome),
            Err(WriteFailure::Conflict) => continue,
            Err(WriteFailure::Other(error)) => return Err(error),
        }
    }
    Err(format!("{name} kept changing; try the same report again"))
}

#[cfg(test)]
mod tests;
