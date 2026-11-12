use serde::{Deserialize, Serialize};

use super::{bounded_text, BackgroundCurrentState, TerminalState, MAX_ID_BYTES, MAX_TASKS};

const MAX_STATUS_BYTES: usize = 128;
const MAX_COMMAND_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum ShellSnapshotHook {
    Stop,
    SubagentStop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShellTask {
    pub task_id: String,
    pub reported_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum ShellObservation {
    SnapshotObserved {
        hook_event: ShellSnapshotHook,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hook_agent_id: Option<String>,
        tasks: Vec<ShellTask>,
        truncated: bool,
    },
    BashBackgroundObserved {
        task_id: String,
        tool_use_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hook_agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ShellObservationRecord {
    #[serde(flatten)]
    pub observation: ShellObservation,
    pub received_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ShellObservations {
    pub current_state: BackgroundCurrentState,
    pub outcome: BackgroundCurrentState,
    pub last_received_snapshot: Option<ShellObservationRecord>,
    pub bash_reports: Vec<ShellObservationRecord>,
    pub truncated: bool,
}

fn valid_command(command: &Option<String>) -> bool {
    command.as_ref().is_none_or(|value| {
        value.len() <= MAX_COMMAND_BYTES
            && !value
                .chars()
                .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
    })
}

impl ShellObservation {
    fn is_valid(&self) -> bool {
        match self {
            Self::SnapshotObserved {
                hook_event,
                hook_agent_id,
                tasks,
                ..
            } => {
                (*hook_event != ShellSnapshotHook::SubagentStop || hook_agent_id.is_some())
                    && hook_agent_id
                        .as_ref()
                        .is_none_or(|id| bounded_text(id, MAX_ID_BYTES))
                    && tasks.len() <= MAX_TASKS
                    && tasks.iter().all(|task| {
                        bounded_text(&task.task_id, MAX_ID_BYTES)
                            && bounded_text(&task.reported_status, MAX_STATUS_BYTES)
                            && valid_command(&task.command)
                    })
            }
            Self::BashBackgroundObserved {
                task_id,
                tool_use_id,
                hook_agent_id,
                command,
            } => {
                bounded_text(task_id, MAX_ID_BYTES)
                    && bounded_text(tool_use_id, MAX_ID_BYTES)
                    && hook_agent_id
                        .as_ref()
                        .is_none_or(|id| bounded_text(id, MAX_ID_BYTES))
                    && valid_command(command)
            }
        }
    }
}

impl TerminalState {
    pub(crate) fn report_background_shell(
        &mut self,
        source: &str,
        session_id: &str,
        observation: ShellObservation,
        received_at_ms: u64,
    ) -> Result<(), &'static str> {
        if !observation.is_valid() {
            return Err("invalid_background_report");
        }
        self.validate_background_owner(source, session_id)?;
        self.reconcile_background_owner();
        let background = &mut self.background;
        background
            .session_id
            .get_or_insert_with(|| session_id.to_owned());
        let shell = background.shell.get_or_insert_with(|| ShellObservations {
            current_state: BackgroundCurrentState::Unknown,
            outcome: BackgroundCurrentState::Unknown,
            last_received_snapshot: None,
            bash_reports: Vec::new(),
            truncated: false,
        });
        match &observation {
            ShellObservation::SnapshotObserved { .. } => {
                // Receipt order does not establish the order in which hooks produced snapshots.
                shell.last_received_snapshot = Some(ShellObservationRecord {
                    observation,
                    received_at_ms,
                });
            }
            ShellObservation::BashBackgroundObserved {
                task_id,
                tool_use_id,
                hook_agent_id,
                ..
            } => {
                if shell.bash_reports.iter().any(|record| {
                    matches!(&record.observation,
                        ShellObservation::BashBackgroundObserved { task_id: task, tool_use_id: tool, hook_agent_id: reporter, .. }
                        if task == task_id && tool == tool_use_id && reporter == hook_agent_id)
                }) {
                    return Ok(());
                }
                if shell.bash_reports.len() == MAX_TASKS {
                    if !shell.truncated {
                        shell.truncated = true;
                        background.revision += 1;
                    }
                    return Ok(());
                }
                shell.bash_reports.push(ShellObservationRecord {
                    observation,
                    received_at_ms,
                });
            }
        }
        background.revision += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_resume::{AgentSessionRef, PersistedAgentSession};
    use crate::detect::{Agent, AgentState};

    fn terminal() -> TerminalState {
        let mut terminal = TerminalState::new(crate::terminal::TerminalId::alloc(), "/tmp".into());
        terminal.set_detected_state(Some(Agent::Claude), AgentState::Working);
        terminal.set_persisted_agent_session(PersistedAgentSession {
            source: "herdr:claude".into(),
            agent: "claude".into(),
            session_ref: AgentSessionRef::id("root").unwrap(),
        });
        terminal
    }

    fn bash(task_id: &str, tool_use_id: &str, reporter: Option<&str>) -> ShellObservation {
        ShellObservation::BashBackgroundObserved {
            task_id: task_id.into(),
            tool_use_id: tool_use_id.into(),
            hook_agent_id: reporter.map(str::to_owned),
            command: Some("printf '日本語\\n'\nprintf done".into()),
        }
    }

    fn snapshot(tasks: Vec<ShellTask>, truncated: bool) -> ShellObservation {
        ShellObservation::SnapshotObserved {
            hook_event: ShellSnapshotHook::SubagentStop,
            hook_agent_id: Some("reporter".into()),
            tasks,
            truncated,
        }
    }

    fn report(terminal: &mut TerminalState, observation: ShellObservation, at: u64) {
        terminal
            .report_background_shell("herdr:claude", "root", observation, at)
            .unwrap();
    }

    #[test]
    fn shell_records_keep_first_bash_receipt_and_last_received_snapshot() {
        let mut terminal = terminal();
        assert!(terminal.background_snapshot().shell_observations.is_none());
        report(&mut terminal, bash("shell", "tool", None), 10);
        assert!(terminal
            .background_snapshot()
            .shell_observations
            .unwrap()
            .last_received_snapshot
            .is_none());
        let before = terminal.background_snapshot();
        let mut duplicate = bash("shell", "tool", None);
        if let ShellObservation::BashBackgroundObserved { command, .. } = &mut duplicate {
            *command = Some("changed".into());
        }
        report(&mut terminal, duplicate, 20);
        assert_eq!(terminal.background_snapshot(), before);
        report(&mut terminal, bash("shell", "other-tool", None), 21);
        report(&mut terminal, bash("shell", "tool", Some("reporter")), 22);
        let tasks = vec![ShellTask {
            task_id: "unrelated-shell".into(),
            reported_status: "future-native-status".into(),
            command: None,
        }];
        report(&mut terminal, snapshot(tasks, true), 30);
        let shell = terminal.background_snapshot().shell_observations.unwrap();
        let value = serde_json::to_value(&shell).unwrap();
        assert_eq!(
            value["last_received_snapshot"]["event"],
            "snapshot_observed"
        );
        assert_eq!(
            value["last_received_snapshot"]["tasks"][0]["reported_status"],
            "future-native-status"
        );
        assert_eq!(value["last_received_snapshot"]["hook_agent_id"], "reporter");
        assert_eq!(value["last_received_snapshot"]["received_at_ms"], 30);
        assert_eq!(
            value["bash_reports"][0]["event"],
            "bash_background_observed"
        );
        assert_eq!(value["bash_reports"][0]["received_at_ms"], 10);
        assert_eq!(
            serde_json::from_value::<ShellObservations>(value).unwrap(),
            shell
        );
        // A later receipt replaces the snapshot even if its timestamp is earlier.
        report(&mut terminal, snapshot(Vec::new(), false), 5);
        let empty = terminal.background_snapshot().shell_observations.unwrap();
        assert_eq!(empty.bash_reports, shell.bash_reports);
        assert_eq!(
            empty.last_received_snapshot.unwrap().observation,
            snapshot(Vec::new(), false)
        );
        assert_eq!(empty.current_state, BackgroundCurrentState::Unknown);
        assert_eq!(empty.outcome, BackgroundCurrentState::Unknown);
        assert!(!empty.truncated);
        assert!(terminal.background_snapshot().records.is_empty());
    }

    #[test]
    fn shell_bounds_keep_records_and_disclose_dropped_bash_reports() {
        let mut terminal = terminal();
        for index in 0..=MAX_TASKS {
            report(
                &mut terminal,
                bash(&index.to_string(), "tool", None),
                index as u64,
            );
        }
        let before = terminal.background_snapshot();
        report(&mut terminal, bash("0", "tool", None), 999);
        report(&mut terminal, bash("dropped", "tool", None), 1000);
        assert_eq!(terminal.background_snapshot(), before);
        let shell = before.shell_observations.unwrap();
        assert_eq!(shell.bash_reports.len(), MAX_TASKS);
        assert_eq!(shell.bash_reports[0].received_at_ms, 0);
        assert!(shell.truncated);
        let task = ShellTask {
            task_id: "task".into(),
            reported_status: "pending".into(),
            command: None,
        };
        report(
            &mut terminal,
            snapshot(vec![task.clone(); MAX_TASKS], true),
            1001,
        );
        let before = terminal.background_snapshot();
        assert!(terminal
            .report_background_shell(
                "herdr:claude",
                "root",
                snapshot(vec![task; MAX_TASKS + 1], true),
                1002
            )
            .is_err());
        assert_eq!(terminal.background_snapshot(), before);
    }

    #[test]
    fn shell_validation_is_bounded_and_atomic_for_all_nested_fields() {
        let mut terminal = terminal();
        let valid = serde_json::json!({
            "event":"snapshot_observed", "hook_event":"SubagentStop", "hook_agent_id":"r".repeat(MAX_ID_BYTES),
            "tasks":[{"task_id":"t".repeat(MAX_ID_BYTES), "reported_status":"s".repeat(MAX_STATUS_BYTES), "command":"é".repeat(MAX_COMMAND_BYTES / 2)}],
            "truncated":false
        });
        report(
            &mut terminal,
            serde_json::from_value(valid.clone()).unwrap(),
            1,
        );
        let before = terminal.background_snapshot();
        for (path, invalid) in [
            ("/hook_agent_id", serde_json::Value::Null),
            ("/hook_agent_id", "".into()),
            ("/hook_agent_id", "r".repeat(MAX_ID_BYTES + 1).into()),
            ("/hook_agent_id", "bad\u{1b}".into()),
            ("/tasks/0/task_id", "".into()),
            ("/tasks/0/task_id", "t".repeat(MAX_ID_BYTES + 1).into()),
            ("/tasks/0/reported_status", "".into()),
            (
                "/tasks/0/reported_status",
                "s".repeat(MAX_STATUS_BYTES + 1).into(),
            ),
            ("/tasks/0/reported_status", "bad\n".into()),
            (
                "/tasks/0/command",
                "é".repeat(MAX_COMMAND_BYTES / 2 + 1).into(),
            ),
            ("/tasks/0/command", "bad\u{1b}[0m".into()),
        ] {
            let mut value = valid.clone();
            *value.pointer_mut(path).unwrap() = invalid;
            let observation = serde_json::from_value(value).unwrap();
            assert_eq!(
                terminal.report_background_shell("herdr:claude", "root", observation, 2),
                Err("invalid_background_report"),
                "{path}"
            );
            assert_eq!(terminal.background_snapshot(), before);
        }
        let valid = serde_json::to_value(bash("task", "tool", None)).unwrap();
        for (field, invalid) in [
            ("task_id", "".into()),
            ("task_id", "x".repeat(MAX_ID_BYTES + 1)),
            ("tool_use_id", "bad\n".into()),
            ("tool_use_id", "x".repeat(MAX_ID_BYTES + 1)),
            ("hook_agent_id", "".into()),
            ("command", "x".repeat(MAX_COMMAND_BYTES + 1)),
        ] {
            let mut value = valid.clone();
            value[field] = invalid.into();
            assert!(
                terminal
                    .report_background_shell(
                        "herdr:claude",
                        "root",
                        serde_json::from_value(value).unwrap(),
                        3
                    )
                    .is_err(),
                "{field}"
            );
            assert_eq!(terminal.background_snapshot(), before);
        }
        report(
            &mut terminal,
            ShellObservation::SnapshotObserved {
                hook_event: ShellSnapshotHook::Stop,
                hook_agent_id: None,
                tasks: Vec::new(),
                truncated: false,
            },
            4,
        );
    }

    #[test]
    fn shell_owner_validation_resume_replacement_and_respawn_share_pr1_identity() {
        let mut terminal = terminal();
        report(&mut terminal, bash("task", "tool", None), 1);
        report(&mut terminal, snapshot(Vec::new(), false), 2);
        let before = terminal.background_snapshot();
        for (source, session) in [
            ("custom:claude", "root"),
            ("herdr:claude", "foreign"),
            ("herdr:claude", ""),
        ] {
            assert!(terminal
                .report_background_shell(source, session, bash("other", "tool", None), 3)
                .is_err());
            assert_eq!(terminal.background_snapshot(), before);
        }
        terminal.set_agent_session_ref_for_session_start(
            "herdr:claude".into(),
            "claude".into(),
            Some(AgentSessionRef::id("root").unwrap()),
            Some(1),
            Some("resume".into()),
        );
        assert_eq!(terminal.background_snapshot(), before);
        for state in [AgentState::Idle, AgentState::Blocked, AgentState::Working] {
            terminal.set_detected_state(Some(Agent::Claude), state);
            report(&mut terminal, bash("task", "tool", None), 4);
            assert_eq!(terminal.state, state);
            assert_eq!(terminal.last_agent_completion_seq, None);
            assert!(terminal.hook_report_is_newer("herdr:claude", Some(2)));
        }
        terminal.set_detected_state(Some(Agent::Codex), AgentState::Working);
        assert_eq!(
            terminal.report_background_shell(
                "herdr:claude",
                "root",
                bash("other", "tool", None),
                5
            ),
            Err("background_owner_mismatch")
        );
        terminal.set_detected_state(Some(Agent::Claude), AgentState::Idle);
        terminal.set_agent_session_ref_for_session_start(
            "herdr:claude".into(),
            "claude".into(),
            Some(AgentSessionRef::id("replacement").unwrap()),
            Some(2),
            Some("clear".into()),
        );
        assert!(terminal.background_snapshot().shell_observations.is_none());
        assert!(terminal.background.shell.is_none());
        assert!(terminal
            .report_background_shell("herdr:claude", "root", snapshot(Vec::new(), false), 6)
            .is_err());
        terminal
            .report_background_shell(
                "herdr:claude",
                "replacement",
                bash("fresh", "tool", None),
                7,
            )
            .unwrap();
        terminal.clear_agent_runtime_identity_after_respawn();
        assert!(terminal.background_snapshot().shell_observations.is_none());
        assert!(terminal.background.shell.is_none());
    }
}
