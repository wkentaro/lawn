use super::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex};

struct State {
    terminal: Value,
    value: Value,
    version: u64,
    calls: Vec<(&'static str, String)>,
    conflicts: usize,
    replace_on_conflict: Option<Value>,
    terminal_on_conflict: Option<Value>,
    error_after_commit: bool,
    save_error: Option<String>,
    programs: HashMap<u32, u64>,
    inside: bool,
    kernel_foreground: Option<u32>,
    reporter_in_job: bool,
    boot: Option<String>,
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
                terminal: json!({"address": "t7", "created_ms": 7, "pid": 9999, "state": "live"}),
                value: Value::Null,
                version: 0,
                calls: Vec::new(),
                conflicts: 0,
                replace_on_conflict: None,
                terminal_on_conflict: None,
                error_after_commit: false,
                save_error: None,
                programs: HashMap::from([(9999, 1)]),
                inside: true,
                kernel_foreground: None,
                reporter_in_job: true,
                boot: None,
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

    fn payloads(&self) -> Vec<Report> {
        self.document()
            .reports
            .iter()
            .map(Report::payload)
            .collect()
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
            if let Some(terminal) = state.terminal_on_conflict.take() {
                state.terminal = terminal;
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

    fn started(&self, pid: u32) -> Option<u64> {
        self.state.lock().unwrap().programs.get(&pid).copied()
    }

    fn runs_inside(&self, _root: u32) -> bool {
        self.state.lock().unwrap().inside
    }

    fn foreground(&self, _root: u32) -> Option<u32> {
        self.state.lock().unwrap().kernel_foreground
    }

    fn reporter_in_job(&self, _root: u32, _job: u32) -> bool {
        self.state.lock().unwrap().reporter_in_job
    }

    fn boot(&self) -> Option<String> {
        self.state.lock().unwrap().boot.clone()
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
            "agent-tool".into(),
            "".into(),
            "session with spaces".into(),
            "quote\"and\\slash".into(),
        ],
        program: None,
        recorded: None,
        job: None,
    }
}

fn document(reports: Vec<Report>) -> Value {
    serde_json::to_value(Document {
        schema: SCHEMA,
        record_only: false,
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
    assert!(!stored.record_only);
    assert_eq!(fake.payloads(), [report]);
    assert_eq!(stored.terminal.address, "t7");
    let state = fake.state.lock().unwrap();
    assert_eq!(state.calls[0], ("terminal", "named-terminal".into()));
    let document = format!("{}-resume-t7", crate::config::app_dir_name());
    for (method, name) in &state.calls[1..] {
        let expected = if *method == "terminal" {
            "t7"
        } else {
            &document
        };
        assert_eq!(name, expected);
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
    assert_eq!(fake.payloads(), [report(3)]);
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
    let stored = fake.payloads();
    assert_eq!(stored.len(), 4);
    assert_eq!(stored[0], newer);
    assert!(stored[1..].iter().all(|report| report.seq == 1));
}

fn program(pid: u32, started_ms: u64) -> Option<ReportedProgram> {
    Some(ReportedProgram {
        pid,
        started_ms: Some(started_ms),
        boot: None,
    })
}

#[test]
fn a_report_from_inside_the_terminal_belongs_to_its_root_program() {
    let fake = Fake::new();
    record_with(&fake, "t7", &report(1)).unwrap();
    assert_eq!(fake.document().reports[0].program, program(9999, 1));
}

#[test]
fn a_report_records_the_boot_its_program_started_in() {
    let fake = Fake::new();
    fake.state.lock().unwrap().boot = Some("b1".into());
    record_with(&fake, "t7", &report(1)).unwrap();
    let stamped = fake.document().reports[0].program.clone().unwrap();
    assert_eq!(stamped.boot.as_deref(), Some("b1"));
}

#[test]
fn a_report_from_outside_the_terminal_is_ignored() {
    let fake = Fake::new();
    fake.state.lock().unwrap().inside = false;
    let outcome = record_with(&fake, "t7", &report(1)).unwrap();
    assert_eq!(outcome.status, RecordStatus::Ignored);
    assert_eq!(fake.writes(), 0);
}

#[test]
fn a_program_the_caller_sends_is_ignored() {
    let fake = Fake::new();
    let mut claimed = report(1);
    claimed.program = program(42, 0);
    record_with(&fake, "t7", &claimed).unwrap();
    assert_eq!(fake.document().reports[0].program, program(9999, 1));
    let mut value = json!(report(2));
    value["program"] = json!({"pid": 0});
    value["recorded"] = json!(99);
    value["job"] = json!(42);
    let read = read_report(serde_json::to_vec(&value).unwrap().as_slice()).unwrap();
    assert_eq!(read, report(2));
}

#[test]
fn the_same_reporter_reports_again_from_a_new_program() {
    let fake = Fake::new();
    record_with(&fake, "t7", &report(1)).unwrap();
    {
        let mut state = fake.state.lock().unwrap();
        state.terminal["pid"] = json!(4242);
        state.programs.insert(4242, 5);
    }
    assert_eq!(
        record_with(&fake, "t7", &report(2)).unwrap().status,
        RecordStatus::Recorded
    );
    assert_eq!(fake.document().reports[0].program, program(4242, 5));
}

#[test]
fn programs_in_a_record_only_document_are_dropped() {
    let fake = Fake::new();
    let mut old = report(1);
    old.reporter_id = "old".into();
    old.program = program(9999, 1);
    let mut value = document(vec![old]);
    value["record_only"] = json!(true);
    fake.state.lock().unwrap().value = value;
    record_with(&fake, "t7", &report(1)).unwrap();
    let stored = fake.document();
    assert!(!stored.record_only);
    assert_eq!(stored.reports.len(), 2);
    assert_eq!(stored.reports[0].program, None);
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
    assert_eq!(fake.payloads(), [report(3)]);
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
fn unknown_schemas_and_identities_are_never_replaced() {
    for change in ["schema", "terminal", "unknown_field", "duplicate"] {
        let fake = Fake::new();
        let mut value = document(vec![report(1)]);
        match change {
            "schema" => value["schema"] = json!(2),
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_detached_reporter_still_runs_inside_the_program_that_started_it() {
    use std::os::unix::process::CommandExt;

    const ROOT: &str = "LAWN_TEST_DETACHED_REPORTER_ROOT";
    if let Some(root) = std::env::var_os(ROOT) {
        assert!(runs_inside(root.to_str().unwrap().parse().unwrap()));
        println!("the detached reporter runs inside");
        return;
    }
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "core_resume::tests::a_detached_reporter_still_runs_inside_the_program_that_started_it",
            "--nocapture",
        ])
        .env(ROOT, std::process::id().to_string());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("the detached reporter runs inside"));
}

#[cfg(not(unix))]
#[test]
fn a_reporter_off_unix_never_runs_inside_the_program_that_started_it() {
    const ROOT: &str = "LAWN_TEST_OFF_UNIX_REPORTER_ROOT";
    if let Some(root) = std::env::var_os(ROOT) {
        assert!(!runs_inside(root.to_str().unwrap().parse().unwrap()));
        println!("the reporter does not run inside");
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "core_resume::tests::a_reporter_off_unix_never_runs_inside_the_program_that_started_it",
            "--nocapture",
        ])
        .env(ROOT, std::process::id().to_string())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("the reporter does not run inside"));
}

fn status(outcome: &Outcome) -> Value {
    serde_json::to_value(outcome).unwrap()["status"].clone()
}

fn methods(fake: &Fake) -> Vec<&'static str> {
    fake.state
        .lock()
        .unwrap()
        .calls
        .iter()
        .map(|(method, _)| *method)
        .collect()
}

#[test]
fn every_attempt_reads_the_terminal_after_the_document() {
    let fake = Fake::new();
    {
        let mut state = fake.state.lock().unwrap();
        state.conflicts = 1;
        let mut restarted = state.terminal.clone();
        restarted["pid"] = json!(5000);
        state.terminal_on_conflict = Some(restarted);
        state.programs.insert(5000, 2);
    }
    record_with(&fake, "t7", &report(1)).unwrap();
    assert_eq!(fake.document().reports[0].program, program(5000, 2));
    assert_eq!(
        methods(&fake)[1..],
        ["load", "terminal", "save", "load", "terminal", "save"]
    );
}

#[test]
fn only_a_live_root_with_a_start_token_is_stamped() {
    for case in ["lost", "no token", "no pid"] {
        let fake = Fake::new();
        {
            let mut state = fake.state.lock().unwrap();
            match case {
                "lost" => state.terminal["state"] = json!("lost"),
                "no token" => state.programs.clear(),
                _ => state.terminal["pid"] = Value::Null,
            }
        }
        let outcome = record_with(&fake, "t7", &report(1)).unwrap();
        assert_eq!(status(&outcome), json!("ignored"), "{case}");
        assert_eq!(fake.writes(), 0, "{case}");
    }
}

#[test]
fn an_unbound_report_neither_replaces_nor_outranks_a_bound_one() {
    let fake = Fake::new();
    fake.state.lock().unwrap().inside = false;
    let mut bound = report(1);
    bound.program = program(9999, 1);
    let value = document(vec![bound]);
    fake.state.lock().unwrap().value = value.clone();
    let outcome = record_with(&fake, "t7", &report(2)).unwrap();
    assert_eq!(status(&outcome), json!("ignored"));
    assert_eq!(fake.state.lock().unwrap().value, value);
}

#[test]
fn a_retry_without_a_stamp_keeps_the_stored_stamp() {
    let fake = Fake::new();
    record_with(&fake, "t7", &report(1)).unwrap();
    fake.state.lock().unwrap().terminal["pid"] = Value::Null;
    let outcome = record_with(&fake, "t7", &report(1)).unwrap();
    assert_eq!(status(&outcome), json!("ignored"));
    assert_eq!(fake.document().reports[0].program, program(9999, 1));
}

#[test]
fn a_report_takes_its_job_from_the_kernel_before_the_core_notices_it() {
    let fake = Fake::new();
    {
        let mut state = fake.state.lock().unwrap();
        state.terminal["foreground"] = json!({"pid": 9999});
        state.kernel_foreground = Some(12345);
    }
    record_with(&fake, "t7", &report(1)).unwrap();
    assert_eq!(fake.document().reports[0].job, Some(12345));
}

#[test]
fn a_report_records_its_order_and_the_foreground_job() {
    let fake = Fake::new();
    {
        let mut state = fake.state.lock().unwrap();
        state.version = 4;
        state.kernel_foreground = Some(12345);
    }
    record_with(&fake, "t7", &report(1)).unwrap();
    let mut second = report(1);
    second.reporter_id = "invocation-2".into();
    fake.state.lock().unwrap().kernel_foreground = Some(9999);
    record_with(&fake, "t7", &second).unwrap();
    let mut third = report(1);
    third.reporter_id = "invocation-3".into();
    fake.state.lock().unwrap().kernel_foreground = None;
    record_with(&fake, "t7", &third).unwrap();
    let reports = fake.state.lock().unwrap().value["reports"].clone();
    assert_eq!(reports[0]["recorded"], json!(5));
    assert_eq!(reports[0]["job"], json!(12345));
    assert_eq!(reports[1]["recorded"], json!(6));
    assert_eq!(reports[1].get("job"), None);
    assert_eq!(reports[2].get("job"), None);
}

#[test]
fn a_reporter_outside_the_foreground_job_is_stored_without_that_job() {
    let fake = Fake::new();
    {
        let mut state = fake.state.lock().unwrap();
        state.kernel_foreground = Some(5000);
        state.reporter_in_job = false;
    }
    let outcome = record_with(&fake, "t7", &report(1)).unwrap();
    assert_eq!(outcome.status, RecordStatus::Recorded);
    let stored = &fake.document().reports[0];
    assert_eq!(stored.program, program(9999, 1));
    assert_eq!(stored.job, None);
}

#[test]
fn a_job_only_the_core_samples_is_not_recorded() {
    let fake = Fake::new();
    fake.state.lock().unwrap().terminal["foreground"] = json!({"pid": 12345});
    record_with(&fake, "t7", &report(1)).unwrap();
    assert_eq!(fake.document().reports[0].job, None);
}

#[test]
fn a_report_that_cannot_be_stamped_is_ignored_before_its_order_is_checked() {
    let fake = Fake::new();
    record_with(&fake, "t7", &report(3)).unwrap();
    fake.state.lock().unwrap().inside = false;
    let mut changed = report(3);
    changed.argv.push("different".into());
    for sent in [changed, report(2)] {
        let outcome = record_with(&fake, "t7", &sent).unwrap();
        assert_eq!(outcome.status, RecordStatus::Ignored, "{}", sent.seq);
    }
    assert_eq!(fake.writes(), 1);
}

#[test]
fn a_command_resume_cannot_type_is_refused_without_a_write() {
    let fake = Fake::new();
    let mut working = report(1);
    working.argv = vec!["worker".into(), "--resume".into(), "s".into()];
    record_with(&fake, "t7", &working).unwrap();
    let mut unusable = report(2);
    unusable.argv = vec!["/usr/bin/worker".into(), "--resume".into(), "s".into()];
    assert!(record_with(&fake, "t7", &unusable).is_err());
    assert_eq!(fake.writes(), 1);
    assert_eq!(fake.payloads(), [working]);
}
