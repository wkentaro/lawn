use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex};

struct State {
    terminal: Value,
    value: Value,
    version: u64,
    calls: Vec<(&'static str, String)>,
    conflicts: usize,
    replace_on_conflict: Option<Value>,
    error_after_commit: bool,
    save_error: Option<String>,
}

struct Fake {
    state: Arc<Mutex<State>>,
    barrier: Option<Arc<Barrier>>,
    read_once: AtomicBool,
}

impl Fake {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                terminal: json!({"address": "t7", "created_ms": 7, "pid": 9999}),
                value: Value::Null,
                version: 0,
                calls: Vec::new(),
                conflicts: 0,
                replace_on_conflict: None,
                error_after_commit: false,
                save_error: None,
            })),
            barrier: None,
            read_once: AtomicBool::new(false),
        }
    }

    fn writes(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(method, _)| *method == "save")
            .count()
    }

    fn document(&self) -> Document {
        serde_json::from_value(self.state.lock().unwrap().value.clone()).unwrap()
    }
}

impl Backend for Fake {
    fn terminal(&self, target: &str) -> Result<Value, String> {
        let mut state = self.state.lock().unwrap();
        state.calls.push(("terminal", target.into()));
        Ok(state.terminal.clone())
    }

    fn load(&self, name: &str) -> Result<Doc, String> {
        let result = {
            let mut state = self.state.lock().unwrap();
            state.calls.push(("load", name.into()));
            Doc {
                value: state.value.clone(),
                version: state.version,
            }
        };
        if !self.read_once.swap(true, Ordering::SeqCst) {
            if let Some(barrier) = &self.barrier {
                barrier.wait();
            }
        }
        Ok(result)
    }

    fn save(&self, name: &str, value: &Value, version: u64) -> Result<u64, WriteFailure> {
        let mut state = self.state.lock().unwrap();
        state.calls.push(("save", name.into()));
        if let Some(error) = state.save_error.take() {
            return Err(WriteFailure::Other(error));
        }
        if state.conflicts > 0 {
            state.conflicts -= 1;
            if let Some(value) = state.replace_on_conflict.take() {
                state.value = value;
                state.version += 1;
            }
            return Err(WriteFailure::Conflict);
        }
        if version != state.version {
            return Err(WriteFailure::Conflict);
        }
        state.value = value.clone();
        state.version += 1;
        if state.error_after_commit {
            state.error_after_commit = false;
            return Err(WriteFailure::Other(
                "doc.set: outcome unknown; the request may have taken effect".into(),
            ));
        }
        Ok(state.version)
    }
}

fn report(seq: u64) -> Report {
    Report {
        source: "integration".into(),
        agent: "worker".into(),
        reporter_id: "invocation-1".into(),
        seq,
        cwd: std::env::current_dir()
            .unwrap()
            .join("nonexistent repo's directory")
            .to_str()
            .unwrap()
            .to_owned(),
        argv: vec![
            "/missing/agent's tool".into(),
            "".into(),
            "session with spaces".into(),
            "quote'and\\slash".into(),
        ],
        program: None,
    }
}

fn document(reports: Vec<Report>) -> Value {
    serde_json::to_value(Document {
        schema: SCHEMA,
        record_only: true,
        terminal: TerminalIdentity {
            address: "t7".into(),
            created_ms: 7,
        },
        reports,
    })
    .unwrap()
}

#[test]
fn paths_quotes_and_argument_boundaries_round_trip_without_a_layout() {
    let fake = Fake::new();
    let report = report(1);
    let outcome = record_with(&fake, "named-terminal", &report).unwrap();
    assert_eq!(
        outcome,
        Outcome {
            terminal: "t7".into(),
            status: RecordStatus::Recorded,
            seq: 1
        }
    );
    let stored = fake.document();
    assert!(stored.record_only);
    assert_eq!(stored.reports, [report]);
    assert_eq!(stored.terminal.address, "t7");
    let state = fake.state.lock().unwrap();
    assert_eq!(state.calls[0], ("terminal", "named-terminal".into()));
    for (_, name) in &state.calls[1..] {
        assert_eq!(
            name,
            &format!("{}-resume-t7", crate::config::app_dir_name())
        );
    }
}

#[test]
fn nonnumeric_terminal_addresses_are_preserved() {
    let fake = Fake::new();
    fake.state.lock().unwrap().terminal["address"] = json!("t1b");
    let outcome = record_with(&fake, "alias", &report(1)).unwrap();
    assert_eq!(outcome.terminal, "t1b");
    assert_eq!(fake.document().terminal.address, "t1b");
    assert!(fake.state.lock().unwrap().calls[1]
        .1
        .ends_with("-resume-t1b"));
}

#[test]
fn older_and_identical_reports_do_not_write() {
    let fake = Fake::new();
    record_with(&fake, "t7", &report(3)).unwrap();
    assert_eq!(
        record_with(&fake, "t7", &report(2)).unwrap().status,
        RecordStatus::Stale
    );
    assert_eq!(
        record_with(&fake, "t7", &report(3)).unwrap().status,
        RecordStatus::Unchanged
    );
    assert_eq!(fake.writes(), 1);
    assert_eq!(fake.document().reports[0].seq, 3);
}

#[test]
fn equal_sequences_with_different_payloads_conflict() {
    let fake = Fake::new();
    record_with(&fake, "t7", &report(3)).unwrap();
    let mut changed = report(3);
    changed.argv.push("different".into());
    assert!(record_with(&fake, "t7", &changed)
        .unwrap_err()
        .contains("different resume report"));
    assert_eq!(fake.writes(), 1);
    assert_eq!(fake.document().reports, [report(3)]);
}

#[test]
fn equal_sequences_require_the_exact_cwd_text() {
    let fake = Fake::new();
    let original = report(1);
    record_with(&fake, "t7", &original).unwrap();
    let mut value = json!(original);
    value["cwd"] = json!(format!(
        "{}{}",
        value["cwd"].as_str().unwrap(),
        std::path::MAIN_SEPARATOR
    ));
    let changed = serde_json::from_value(value).unwrap();
    assert!(record_with(&fake, "t7", &changed).is_err());
    assert_eq!(fake.writes(), 1);
}

#[test]
fn newer_sequences_update_only_their_reporter() {
    let fake = Fake::new();
    let original = report(1);
    record_with(&fake, "t7", &original).unwrap();
    for (field, value) in [
        ("source", "other-source"),
        ("agent", "other-agent"),
        ("reporter_id", "other-invocation"),
    ] {
        let mut separate = original.clone();
        match field {
            "source" => separate.source = value.into(),
            "agent" => separate.agent = value.into(),
            _ => separate.reporter_id = value.into(),
        }
        record_with(&fake, "t7", &separate).unwrap();
    }
    let mut newer = report(2);
    newer.argv.push("new".into());
    record_with(&fake, "t7", &newer).unwrap();
    let stored = fake.document();
    assert_eq!(stored.reports.len(), 4);
    assert_eq!(stored.reports[0], newer);
    assert!(stored.reports[1..].iter().all(|report| report.seq == 1));
}

#[test]
fn program_context_is_a_caller_claim_not_the_current_terminal_pid() {
    let fake = Fake::new();
    let mut claimed = report(1);
    claimed.program = Some(ReportedProgram {
        pid: 42,
        started_ms: None,
    });
    record_with(&fake, "t7", &claimed).unwrap();
    assert_eq!(fake.document().reports, [claimed.clone()]);
    claimed.seq += 1;
    claimed.program.as_mut().unwrap().pid = 43;
    assert!(record_with(&fake, "t7", &claimed)
        .unwrap_err()
        .contains("different program context"));
    assert_eq!(fake.writes(), 1);
}

#[test]
fn conflicts_reload_before_applying_sequence_order() {
    let fake = Fake::new();
    {
        let mut state = fake.state.lock().unwrap();
        state.conflicts = 1;
        state.replace_on_conflict = Some(document(vec![report(3)]));
    }
    let result = record_with(&fake, "t7", &report(2)).unwrap();
    assert_eq!(result.status, RecordStatus::Stale);
    assert_eq!(result.seq, 3);
    assert_eq!(fake.writes(), 1);
    assert_eq!(fake.document().reports, [report(3)]);
}

#[test]
fn independent_concurrent_writers_keep_every_report() {
    let fake = Fake::new();
    let barrier = Arc::new(Barrier::new(4));
    let writers = (0..4)
        .map(|index| {
            let writer = Fake {
                state: fake.state.clone(),
                barrier: Some(barrier.clone()),
                read_once: AtomicBool::new(false),
            };
            std::thread::spawn(move || {
                let mut report = report(1);
                report.reporter_id = format!("invocation-{index}");
                record_with(&writer, "t7", &report).unwrap()
            })
        })
        .collect::<Vec<_>>();
    for writer in writers {
        assert_eq!(writer.join().unwrap().status, RecordStatus::Recorded);
    }
    let stored = fake.document();
    assert_eq!(stored.reports.len(), 4);
    assert_eq!(
        stored
            .reports
            .iter()
            .map(|report| &report.reporter_id)
            .collect::<HashSet<_>>()
            .len(),
        4
    );
}

#[test]
fn repeated_cas_conflicts_are_bounded() {
    let fake = Fake::new();
    fake.state.lock().unwrap().conflicts = EDIT_ATTEMPTS + 1;
    assert!(record_with(&fake, "t7", &report(1))
        .unwrap_err()
        .contains("kept changing"));
    assert_eq!(fake.writes(), EDIT_ATTEMPTS);
    assert!(fake.state.lock().unwrap().value.is_null());
}

#[test]
fn an_unknown_write_is_not_retried_but_an_exact_later_report_is_safe() {
    let fake = Fake::new();
    fake.state.lock().unwrap().error_after_commit = true;
    assert!(record_with(&fake, "t7", &report(1))
        .unwrap_err()
        .contains("outcome unknown"));
    assert_eq!(fake.writes(), 1);
    assert_eq!(
        record_with(&fake, "t7", &report(1)).unwrap().status,
        RecordStatus::Unchanged
    );
    assert_eq!(fake.writes(), 1);
}

#[test]
fn other_write_errors_do_not_authorize_retries() {
    let fake = Fake::new();
    fake.state.lock().unwrap().save_error = Some("conflict in unrelated error text".into());
    assert_eq!(
        record_with(&fake, "t7", &report(1)).unwrap_err(),
        "conflict in unrelated error text"
    );
    assert_eq!(fake.writes(), 1);
    assert!(fake.state.lock().unwrap().value.is_null());
}

#[test]
fn unknown_schemas_purposes_and_identities_are_never_replaced() {
    for change in [
        "schema",
        "record_only",
        "terminal",
        "unknown_field",
        "duplicate",
    ] {
        let fake = Fake::new();
        let mut value = document(vec![report(1)]);
        match change {
            "schema" => value["schema"] = json!(2),
            "record_only" => value["record_only"] = json!(false),
            "terminal" => value["terminal"]["created_ms"] = json!(8),
            "unknown_field" => value["future"] = json!(true),
            _ => value["reports"]
                .as_array_mut()
                .unwrap()
                .push(json!(report(1))),
        }
        fake.state.lock().unwrap().value = value.clone();
        assert!(record_with(&fake, "t7", &report(2)).is_err(), "{change}");
        assert_eq!(fake.writes(), 0);
        assert_eq!(fake.state.lock().unwrap().value, value);
    }
}

#[test]
fn remote_and_missing_terminal_identities_are_refused() {
    let fake = Fake::new();
    assert!(record_with(&fake, "box/t7", &report(1)).is_err());
    assert!(fake.state.lock().unwrap().calls.is_empty());
    for address in ["box/t7", "", " ", "t7\n"] {
        let fake = Fake::new();
        fake.state.lock().unwrap().terminal["address"] = json!(address);
        assert!(record_with(&fake, "alias", &report(1)).is_err());
        assert_eq!(fake.state.lock().unwrap().calls.len(), 1);
    }
    let fake = Fake::new();
    fake.state
        .lock()
        .unwrap()
        .terminal
        .as_object_mut()
        .unwrap()
        .remove("created_ms");
    assert!(record_with(&fake, "t7", &report(1)).is_err());
    assert_eq!(fake.writes(), 0);
}

#[test]
fn null_documents_keep_their_cas_version() {
    let fake = Fake::new();
    fake.state.lock().unwrap().version = 7;
    record_with(&fake, "t7", &report(1)).unwrap();
    assert_eq!(fake.state.lock().unwrap().version, 8);
}

#[test]
fn invalid_reports_do_not_contact_the_core() {
    let invalid = [
        {
            let mut r = report(1);
            r.source = " ".into();
            r
        },
        {
            let mut r = report(1);
            r.agent = "x".repeat(MAX_IDENTIFIER_BYTES + 1);
            r
        },
        {
            let mut r = report(1);
            r.reporter_id.push('\n');
            r
        },
        {
            let mut r = report(1);
            r.argv.clear();
            r
        },
        {
            let mut r = report(1);
            r.argv[0].clear();
            r
        },
        {
            let mut r = report(1);
            r.argv = vec!["x".into(); MAX_ARGS + 1];
            r
        },
        {
            let mut r = report(1);
            r.argv = vec!["x".repeat(MAX_ARGV_BYTES + 1)];
            r
        },
        {
            let mut r = report(1);
            r.argv.push("line\nbreak".into());
            r
        },
        {
            let mut r = report(1);
            r.cwd = "relative".into();
            r
        },
        {
            let mut r = report(1);
            r.cwd.push_str(&"x".repeat(MAX_CWD_BYTES));
            r
        },
        {
            let mut r = report(1);
            r.program = Some(ReportedProgram {
                pid: 0,
                started_ms: None,
            });
            r
        },
    ];
    for report in invalid {
        let fake = Fake::new();
        assert!(record_with(&fake, "t7", &report).is_err());
        assert!(fake.state.lock().unwrap().calls.is_empty());
    }
}

#[test]
fn validation_accepts_the_recording_boundaries() {
    let mut r = report(0);
    r.source = "s".repeat(MAX_IDENTIFIER_BYTES);
    r.argv = vec!["x".repeat(MAX_ARGV_BYTES)];
    r.validate().unwrap();
    r.argv = vec!["x".into(); MAX_ARGS];
    r.validate().unwrap();
}

#[test]
fn stdin_is_bounded_and_unknown_fields_are_refused() {
    let expected = report(1);
    let bytes = serde_json::to_vec(&expected).unwrap();
    assert_eq!(read_report(bytes.as_slice()).unwrap(), expected);
    let mut input = std::io::Cursor::new(vec![b' '; MAX_REPORT_BYTES + 10]);
    assert!(read_report(&mut input).unwrap_err().contains("64 KiB"));
    assert_eq!(input.position(), (MAX_REPORT_BYTES + 1) as u64);
    let mut unknown = json!(expected);
    unknown["unexpected"] = json!(true);
    assert!(read_report(serde_json::to_vec(&unknown).unwrap().as_slice()).is_err());
}

#[test]
fn oversized_documents_are_refused_without_a_write() {
    let fake = Fake::new();
    let mut large = report(1);
    large.reporter_id = "reporter-0000".into();
    large.argv = vec!["x".repeat(MAX_ARGV_BYTES)];
    let row_bytes = serde_json::to_vec(&large).unwrap().len();
    let empty_bytes = serde_json::to_vec(&document(Vec::new())).unwrap().len();
    let count = (MAX_DOCUMENT_BYTES - empty_bytes + 1) / (row_bytes + 1);
    let mut reports = Vec::new();
    for index in 0..count {
        let mut row = large.clone();
        row.reporter_id = format!("reporter-{index:04}");
        reports.push(row);
    }
    let value = document(reports);
    assert!(serde_json::to_vec(&value).unwrap().len() <= MAX_DOCUMENT_BYTES);
    fake.state.lock().unwrap().value = value.clone();
    large.reporter_id = format!("reporter-{count:04}");
    assert!(record_with(&fake, "t7", &large)
        .unwrap_err()
        .contains("1 MiB"));
    assert_eq!(fake.writes(), 0);
    assert_eq!(fake.state.lock().unwrap().value, value);
}
