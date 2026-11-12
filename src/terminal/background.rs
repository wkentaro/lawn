//! Hook observations are historical evidence, never execution or completion authority.

use serde::{Deserialize, Serialize};

use super::TerminalState;

mod shell;
pub use shell::{ShellObservation, ShellObservations};
mod wait;
pub use wait::{BackgroundTaskKind, BackgroundTaskRef, BackgroundWaitDeclaration};

const MAX_TASKS: usize = 128;
const MAX_ID_BYTES: usize = 256;
const MAX_TYPE_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundEvent {
    SubagentStarted,
    SubagentStopObserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundObservation {
    StartObserved,
    StopObserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundKind {
    Subagent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundCoverage {
    ObservationsOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundCurrentState {
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BackgroundRecord {
    pub task_id: String,
    pub kind: BackgroundKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    pub observation: BackgroundObservation,
    pub start_observed_at_ms: Option<u64>,
    pub stop_observed_at_ms: Option<u64>,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BackgroundSnapshot {
    pub session_id: Option<String>,
    pub coverage: BackgroundCoverage,
    pub current_state: BackgroundCurrentState,
    pub records: Vec<BackgroundRecord>,
    pub revision: u64,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_observations: Option<ShellObservations>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wait_declarations: Vec<BackgroundWaitDeclaration>,
}

#[derive(Default)]
pub(super) struct BackgroundObservations {
    session_id: Option<String>,
    records: Vec<BackgroundRecord>,
    revision: u64,
    truncated: bool,
    shell: Option<ShellObservations>,
    wait_declarations: Vec<BackgroundWaitDeclaration>,
}

fn bounded_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

impl TerminalState {
    fn background_session_id(&self) -> Option<&str> {
        let (source, agent, session_ref) = match self.hook_authority.as_ref() {
            Some(authority) if authority.session_ref.is_some() => (
                authority.source.as_str(),
                authority.agent_label.as_str(),
                authority.session_ref.as_ref()?,
            ),
            _ => {
                let session = self.persisted_agent_session.as_ref()?;
                (
                    session.source.as_str(),
                    session.agent.as_str(),
                    &session.session_ref,
                )
            }
        };
        (source == "herdr:claude"
            && agent == "claude"
            && session_ref.kind == crate::agent_resume::AgentSessionRefKind::Id)
            .then_some(session_ref.value.as_str())
    }

    pub(super) fn reconcile_background_owner(&mut self) {
        if self
            .background
            .session_id
            .as_deref()
            .is_some_and(|owner| self.background_session_id() != Some(owner))
        {
            self.background = BackgroundObservations::default();
        }
    }

    pub(crate) fn report_background(
        &mut self,
        source: &str,
        session_id: &str,
        event: BackgroundEvent,
        task_id: &str,
        task_type: Option<&str>,
        received_at_ms: u64,
    ) -> Result<(), &'static str> {
        if !bounded_text(task_id, MAX_ID_BYTES)
            || task_type.is_some_and(|value| !bounded_text(value, MAX_TYPE_BYTES))
        {
            return Err("invalid_background_report");
        }
        self.validate_background_owner(source, session_id)?;
        self.reconcile_background_owner();
        let observations = &mut self.background;
        observations
            .session_id
            .get_or_insert_with(|| session_id.to_owned());
        let index = match observations
            .records
            .iter()
            .position(|record| record.task_id == task_id)
        {
            Some(index) => index,
            None => {
                if observations.records.len() == MAX_TASKS {
                    if !observations.truncated {
                        observations.revision += 1;
                        observations.truncated = true;
                    }
                    // Keep stop evidence instead of evicting it and reopening a reordered start.
                    return Ok(());
                }
                observations.records.push(BackgroundRecord {
                    task_id: task_id.to_owned(),
                    kind: BackgroundKind::Subagent,
                    task_type: None,
                    observation: BackgroundObservation::StartObserved,
                    start_observed_at_ms: None,
                    stop_observed_at_ms: None,
                    revision: 0,
                });
                observations.records.len() - 1
            }
        };
        let record = &mut observations.records[index];
        let timestamp = match event {
            BackgroundEvent::SubagentStarted => &mut record.start_observed_at_ms,
            BackgroundEvent::SubagentStopObserved => &mut record.stop_observed_at_ms,
        };
        let new_observation = timestamp.is_none();
        if new_observation {
            *timestamp = Some(received_at_ms);
        }
        let new_type = record.task_type.is_none() && task_type.is_some();
        if new_type {
            record.task_type = task_type.map(str::to_owned);
        }
        if new_observation || new_type {
            observations.revision += 1;
            record.revision = observations.revision;
        }
        if record.stop_observed_at_ms.is_some() {
            record.observation = BackgroundObservation::StopObserved;
        }
        Ok(())
    }

    fn validate_background_owner(
        &self,
        source: &str,
        session_id: &str,
    ) -> Result<(), &'static str> {
        if source != "herdr:claude" || !bounded_text(session_id, MAX_ID_BYTES) {
            return Err("invalid_background_report");
        }
        if self.background_session_id() != Some(session_id)
            || self
                .detected_agent
                .is_some_and(|agent| agent != crate::detect::Agent::Claude)
            || self.hook_authority.as_ref().is_some_and(|authority| {
                authority.source != source || authority.agent_label != "claude"
            })
        {
            return Err("background_owner_mismatch");
        }
        Ok(())
    }

    pub(crate) fn background_record_count(&self) -> usize {
        if self.background_session_id() != self.background.session_id.as_deref() {
            return 0;
        }
        let mut shell_ids = std::collections::BTreeSet::new();
        if let Some(shell) = &self.background.shell {
            if let Some(record) = &shell.last_received_snapshot {
                if let ShellObservation::SnapshotObserved { tasks, .. } = &record.observation {
                    shell_ids.extend(tasks.iter().map(|task| task.task_id.as_str()));
                }
            }
            for record in &shell.bash_reports {
                if let ShellObservation::BashBackgroundObserved { task_id, .. } =
                    &record.observation
                {
                    shell_ids.insert(task_id.as_str());
                }
            }
        }
        self.background.records.len() + shell_ids.len()
    }

    pub(crate) fn background_snapshot(&self) -> BackgroundSnapshot {
        let current_session = self.background_session_id();
        let current_owner = current_session == self.background.session_id.as_deref();
        BackgroundSnapshot {
            session_id: current_session.map(str::to_owned),
            coverage: BackgroundCoverage::ObservationsOnly,
            current_state: BackgroundCurrentState::Unknown,
            records: if current_owner {
                self.background.records.clone()
            } else {
                Vec::new()
            },
            revision: if current_owner {
                self.background.revision
            } else {
                0
            },
            truncated: current_owner && self.background.truncated,
            shell_observations: current_owner
                .then(|| self.background.shell.clone())
                .flatten(),
            wait_declarations: if current_owner {
                self.background.wait_declarations.clone()
            } else {
                Vec::new()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_resume::{AgentSessionRef, PersistedAgentSession};
    use crate::detect::{Agent, AgentState};

    fn terminal() -> TerminalState {
        let mut terminal = TerminalState::new(crate::terminal::TerminalId::alloc(), "/tmp".into());
        terminal.set_detected_state(Some(Agent::Claude), AgentState::Idle);
        terminal.set_persisted_agent_session(PersistedAgentSession {
            source: "herdr:claude".into(),
            agent: "claude".into(),
            session_ref: AgentSessionRef::id("root").unwrap(),
        });
        terminal
    }

    fn report(terminal: &mut TerminalState, event: BackgroundEvent, task: &str, at: u64) {
        terminal
            .report_background("herdr:claude", "root", event, task, None, at)
            .unwrap();
    }

    #[test]
    fn background_duplicates_reordering_and_reuse_keep_historical_evidence() {
        let mut terminal = terminal();
        report(
            &mut terminal,
            BackgroundEvent::SubagentStopObserved,
            "child",
            10,
        );
        report(&mut terminal, BackgroundEvent::SubagentStarted, "child", 20);
        let snapshot = terminal.background_snapshot();
        report(&mut terminal, BackgroundEvent::SubagentStarted, "child", 30);
        report(
            &mut terminal,
            BackgroundEvent::SubagentStopObserved,
            "child",
            40,
        );
        assert_eq!(terminal.background_snapshot(), snapshot);
        assert_eq!(
            snapshot.records[0].observation,
            BackgroundObservation::StopObserved
        );
        assert_eq!(snapshot.records[0].start_observed_at_ms, Some(20));
        assert_eq!(snapshot.records[0].stop_observed_at_ms, Some(10));
        assert_eq!(snapshot.current_state, BackgroundCurrentState::Unknown);
        assert_eq!(terminal.state, AgentState::Idle);
        assert!(terminal.hook_authority.is_none());
        assert_eq!(terminal.last_agent_completion_seq, None);
        assert!(terminal.hook_report_is_newer("herdr:claude", Some(1)));
    }

    #[test]
    fn background_rejects_foreign_owners_and_unbounded_text() {
        let mut terminal = terminal();
        for (source, session, task, task_type) in [
            ("custom:claude", "root", "child", None),
            ("herdr:claude", "other", "child", None),
            ("herdr:claude", "root", "", None),
            ("herdr:claude", "root", "bad\n", None),
            ("herdr:claude", "root", "child", Some("")),
        ] {
            assert!(terminal
                .report_background(
                    source,
                    session,
                    BackgroundEvent::SubagentStarted,
                    task,
                    task_type,
                    1
                )
                .is_err());
        }
        for (session, task, task_type) in [
            ("x".repeat(MAX_ID_BYTES + 1), "child".into(), None),
            ("root".into(), "x".repeat(MAX_ID_BYTES + 1), None),
            (
                "root".into(),
                "child".into(),
                Some("x".repeat(MAX_TYPE_BYTES + 1)),
            ),
        ] {
            assert!(terminal
                .report_background(
                    "herdr:claude",
                    &session,
                    BackgroundEvent::SubagentStarted,
                    &task,
                    task_type.as_deref(),
                    1
                )
                .is_err());
        }
        assert!(terminal.background_snapshot().records.is_empty());
    }

    #[test]
    fn background_bounds_preserve_existing_stops_and_disclose_truncation() {
        let mut terminal = terminal();
        for index in 0..=MAX_TASKS {
            report(
                &mut terminal,
                BackgroundEvent::SubagentStarted,
                &index.to_string(),
                1,
            );
        }
        report(&mut terminal, BackgroundEvent::SubagentStopObserved, "0", 2);
        let snapshot = terminal.background_snapshot();
        assert_eq!(snapshot.records.len(), MAX_TASKS);
        assert!(snapshot.truncated);
        assert_eq!(
            snapshot.records[0].observation,
            BackgroundObservation::StopObserved
        );
    }

    #[test]
    fn background_duplicate_metadata_fills_missing_type_without_overwriting_it() {
        let mut terminal = terminal();
        report(&mut terminal, BackgroundEvent::SubagentStarted, "child", 1);
        for task_type in ["Explore", "different"] {
            terminal
                .report_background(
                    "herdr:claude",
                    "root",
                    BackgroundEvent::SubagentStarted,
                    "child",
                    Some(task_type),
                    2,
                )
                .unwrap();
        }
        let snapshot = terminal.background_snapshot();
        assert_eq!(snapshot.records[0].task_type.as_deref(), Some("Explore"));
        assert_eq!(snapshot.records[0].start_observed_at_ms, Some(1));
        assert_eq!(snapshot.revision, 2);
    }

    #[test]
    fn background_session_replacement_clears_but_same_id_resume_keeps_history() {
        let mut terminal = terminal();
        report(&mut terminal, BackgroundEvent::SubagentStarted, "child", 1);
        let snapshot = terminal.background_snapshot();
        terminal.set_agent_session_ref_for_session_start(
            "herdr:claude".into(),
            "claude".into(),
            Some(AgentSessionRef::id("root").unwrap()),
            Some(1),
            Some("resume".into()),
        );
        assert_eq!(terminal.background_snapshot(), snapshot);
        terminal.set_agent_session_ref_for_session_start(
            "herdr:claude".into(),
            "claude".into(),
            Some(AgentSessionRef::id("replacement").unwrap()),
            Some(2),
            Some("clear".into()),
        );
        assert!(terminal.background_snapshot().records.is_empty());
        assert!(terminal.background.records.is_empty());
        assert!(terminal
            .report_background(
                "herdr:claude",
                "root",
                BackgroundEvent::SubagentStopObserved,
                "child",
                None,
                2
            )
            .is_err());
        terminal.clear_agent_runtime_identity_after_respawn();
        assert!(terminal.background_snapshot().session_id.is_none());
    }

    #[test]
    fn background_identity_claims_leave_status_and_completion_to_detection() {
        let mut terminal = terminal();
        report(&mut terminal, BackgroundEvent::SubagentStarted, "child", 1);
        for state in [AgentState::Working, AgentState::Blocked, AgentState::Idle] {
            terminal.set_detected_state(Some(Agent::Claude), state);
            report(
                &mut terminal,
                BackgroundEvent::SubagentStopObserved,
                "child",
                2,
            );
            assert_eq!(terminal.state, state);
            assert_eq!(terminal.last_agent_completion_seq, None);
            assert!(terminal.hook_authority.is_none());
        }
        terminal.set_detected_state(Some(Agent::Codex), AgentState::Working);
        assert!(terminal
            .report_background(
                "herdr:claude",
                "root",
                BackgroundEvent::SubagentStarted,
                "other",
                None,
                3
            )
            .is_err());
        terminal.set_persisted_agent_session(PersistedAgentSession {
            source: "custom:claude".into(),
            agent: "claude".into(),
            session_ref: AgentSessionRef::id("root").unwrap(),
        });
        assert!(terminal.background_snapshot().records.is_empty());
        assert!(terminal
            .report_background(
                "herdr:claude",
                "root",
                BackgroundEvent::SubagentStarted,
                "child",
                None,
                3
            )
            .is_err());
    }
}
