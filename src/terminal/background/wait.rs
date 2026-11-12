use serde::{Deserialize, Serialize};

use super::{bounded_text, TerminalState, MAX_ID_BYTES, MAX_TASKS};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskKind {
    Subagent,
    Shell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BackgroundTaskRef {
    pub kind: BackgroundTaskKind,
    pub task_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BackgroundWaitDeclaration {
    pub kind: BackgroundTaskKind,
    pub task_id: String,
    pub declared_at_ms: u64,
}

impl BackgroundWaitDeclaration {
    fn matches(&self, task: &BackgroundTaskRef) -> bool {
        self.kind == task.kind && self.task_id == task.task_id
    }
}

impl TerminalState {
    pub(crate) fn set_background_wait(
        &mut self,
        source: &str,
        session_id: &str,
        tasks: Vec<BackgroundTaskRef>,
        waiting: bool,
        received_at_ms: u64,
    ) -> Result<(Vec<BackgroundTaskRef>, Vec<BackgroundTaskRef>), &'static str> {
        if tasks
            .iter()
            .any(|task| !bounded_text(&task.task_id, MAX_ID_BYTES))
        {
            return Err("invalid_background_wait");
        }
        let mut unique = Vec::new();
        for task in tasks {
            if !unique.contains(&task) {
                if unique.len() == MAX_TASKS {
                    return Err("invalid_background_wait");
                }
                unique.push(task);
            }
        }
        self.validate_background_owner(source, session_id)?;
        self.reconcile_background_owner();
        let background = &mut self.background;
        let mut matched = Vec::new();
        let mut unmatched = Vec::new();
        let mut additions = Vec::new();
        for task in unique {
            let declared = background
                .wait_declarations
                .iter()
                .any(|entry| entry.matches(&task));
            let known = declared
                || match task.kind {
                    BackgroundTaskKind::Subagent => background
                        .records
                        .iter()
                        .any(|record| record.task_id == task.task_id),
                    BackgroundTaskKind::Shell => background
                        .shell
                        .as_ref()
                        .is_some_and(|shell| shell.contains_task(&task.task_id)),
                };
            if known {
                if waiting && !declared {
                    additions.push(BackgroundWaitDeclaration {
                        kind: task.kind,
                        task_id: task.task_id.clone(),
                        declared_at_ms: received_at_ms,
                    });
                }
                matched.push(task);
            } else {
                unmatched.push(task);
            }
        }
        let before = background.wait_declarations.len();
        if waiting {
            if before + additions.len() > MAX_TASKS {
                return Err("background_wait_capacity_exceeded");
            }
            background.wait_declarations.append(&mut additions);
        } else {
            background
                .wait_declarations
                .retain(|entry| !matched.iter().any(|task| entry.matches(task)));
        }
        if background.wait_declarations.len() != before {
            background.revision += 1;
        }
        Ok((matched, unmatched))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_resume::{AgentSessionRef, PersistedAgentSession};
    use crate::detect::{Agent, AgentState};
    use crate::terminal::background::shell::{ShellSnapshotHook, ShellTask};
    use crate::terminal::background::{BackgroundCurrentState, BackgroundEvent, ShellObservation};

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

    fn task(kind: BackgroundTaskKind, id: &str) -> BackgroundTaskRef {
        BackgroundTaskRef {
            kind,
            task_id: id.into(),
        }
    }

    fn subagent(terminal: &mut TerminalState, id: &str) {
        terminal
            .report_background(
                "herdr:claude",
                "root",
                BackgroundEvent::SubagentStarted,
                id,
                None,
                1,
            )
            .unwrap();
    }

    fn snapshot(terminal: &mut TerminalState, ids: &[&str]) {
        terminal
            .report_background_shell(
                "herdr:claude",
                "root",
                ShellObservation::SnapshotObserved {
                    hook_event: ShellSnapshotHook::Stop,
                    hook_agent_id: None,
                    tasks: ids
                        .iter()
                        .map(|id| ShellTask {
                            task_id: (*id).into(),
                            reported_status: "unknown-native-status".into(),
                            command: None,
                        })
                        .collect(),
                    truncated: false,
                },
                2,
            )
            .unwrap();
    }

    fn set(
        terminal: &mut TerminalState,
        tasks: Vec<BackgroundTaskRef>,
        waiting: bool,
        at: u64,
    ) -> (Vec<BackgroundTaskRef>, Vec<BackgroundTaskRef>) {
        terminal
            .set_background_wait("herdr:claude", "root", tasks, waiting, at)
            .unwrap()
    }

    #[test]
    fn wait_dedupes_in_order_and_keeps_kind_namespaces_and_first_receipt() {
        let mut terminal = terminal();
        subagent(&mut terminal, "same");
        snapshot(&mut terminal, &["same"]);
        let child = task(BackgroundTaskKind::Subagent, "same");
        let shell = task(BackgroundTaskKind::Shell, "same");
        let missing = task(BackgroundTaskKind::Shell, "missing");
        let mut refs = vec![
            shell.clone(),
            missing.clone(),
            child.clone(),
            shell.clone(),
            missing.clone(),
        ];
        refs.extend(vec![child.clone(); MAX_TASKS]);
        let revision = terminal.background_snapshot().revision;
        assert_eq!(
            set(&mut terminal, refs.clone(), true, 10),
            (vec![shell.clone(), child.clone()], vec![missing.clone()])
        );
        let declared = terminal.background_snapshot();
        assert_eq!(declared.revision, revision + 1);
        assert_eq!(declared.wait_declarations.len(), 2);
        assert!(declared
            .wait_declarations
            .iter()
            .all(|entry| entry.declared_at_ms == 10));
        set(&mut terminal, refs, true, 20);
        assert_eq!(terminal.background_snapshot(), declared);
        set(&mut terminal, vec![child.clone()], false, 30);
        assert_eq!(terminal.background_snapshot().wait_declarations.len(), 1);
        let cleared = terminal.background_snapshot();
        assert_eq!(set(&mut terminal, vec![child], false, 40).0.len(), 1);
        assert_eq!(terminal.background_snapshot(), cleared);
        assert_eq!(
            terminal.background_snapshot().wait_declarations[0].kind,
            BackgroundTaskKind::Shell
        );
    }

    #[test]
    fn wait_survives_stop_empty_snapshot_and_resume_and_can_clear_disappeared_shell() {
        let mut terminal = terminal();
        snapshot(&mut terminal, &["snapshot-only"]);
        let shell = task(BackgroundTaskKind::Shell, "snapshot-only");
        set(&mut terminal, vec![shell.clone()], true, 10);
        subagent(&mut terminal, "child");
        let child = task(BackgroundTaskKind::Subagent, "child");
        set(&mut terminal, vec![child.clone()], true, 11);
        terminal
            .report_background(
                "herdr:claude",
                "root",
                BackgroundEvent::SubagentStopObserved,
                "child",
                None,
                12,
            )
            .unwrap();
        snapshot(&mut terminal, &[]);
        let before = terminal.background_snapshot();
        assert_eq!(before.wait_declarations.len(), 2);
        terminal.set_agent_session_ref_for_session_start(
            "herdr:claude".into(),
            "claude".into(),
            Some(AgentSessionRef::id("root").unwrap()),
            Some(1),
            Some("resume".into()),
        );
        assert_eq!(terminal.background_snapshot(), before);
        assert_eq!(
            set(&mut terminal, vec![shell.clone()], true, 15),
            (vec![shell.clone()], vec![])
        );
        assert_eq!(terminal.background_snapshot(), before);
        assert_eq!(set(&mut terminal, vec![shell], false, 20).0.len(), 1);
        assert_eq!(terminal.background_snapshot().wait_declarations.len(), 1);
        terminal.set_agent_session_ref_for_session_start(
            "herdr:claude".into(),
            "claude".into(),
            Some(AgentSessionRef::id("replacement").unwrap()),
            Some(2),
            Some("clear".into()),
        );
        assert!(terminal.background_snapshot().wait_declarations.is_empty());
        assert_eq!(
            terminal.set_background_wait("herdr:claude", "root", vec![child], true, 30),
            Err("background_owner_mismatch")
        );
        terminal
            .report_background(
                "herdr:claude",
                "replacement",
                BackgroundEvent::SubagentStarted,
                "new",
                None,
                31,
            )
            .unwrap();
        terminal
            .set_background_wait(
                "herdr:claude",
                "replacement",
                vec![task(BackgroundTaskKind::Subagent, "new")],
                true,
                32,
            )
            .unwrap();
        terminal.clear_agent_runtime_identity_after_respawn();
        assert!(terminal.background_snapshot().wait_declarations.is_empty());
        assert!(terminal.background.wait_declarations.is_empty());
    }

    #[test]
    fn wait_capacity_is_transactional_and_inputs_are_bounded() {
        let mut terminal = terminal();
        let refs: Vec<_> = (0..MAX_TASKS)
            .map(|index| {
                let id = index.to_string();
                subagent(&mut terminal, &id);
                task(BackgroundTaskKind::Subagent, &id)
            })
            .collect();
        set(&mut terminal, refs[..127].to_vec(), true, 10);
        snapshot(&mut terminal, &["extra"]);
        let before = terminal.background_snapshot();
        assert_eq!(
            terminal.set_background_wait(
                "herdr:claude",
                "root",
                vec![
                    refs[0].clone(),
                    refs[127].clone(),
                    task(BackgroundTaskKind::Shell, "extra")
                ],
                true,
                20
            ),
            Err("background_wait_capacity_exceeded")
        );
        assert_eq!(terminal.background_snapshot(), before);
        let mut with_duplicates = refs.clone();
        with_duplicates.push(refs[0].clone());
        set(&mut terminal, with_duplicates, true, 30);
        assert_eq!(
            terminal.background_snapshot().wait_declarations.len(),
            MAX_TASKS
        );
        let full = terminal.background_snapshot();
        set(&mut terminal, vec![refs[0].clone()], true, 40);
        assert_eq!(terminal.background_snapshot(), full);
        let mut too_many_unique = refs.clone();
        too_many_unique.push(task(BackgroundTaskKind::Shell, "extra"));
        for waiting in [true, false] {
            assert_eq!(
                terminal.set_background_wait(
                    "herdr:claude",
                    "root",
                    too_many_unique.clone(),
                    waiting,
                    50
                ),
                Err("invalid_background_wait")
            );
            assert_eq!(terminal.background_snapshot(), full);
        }
        for tasks in [
            vec![task(BackgroundTaskKind::Shell, "")],
            vec![task(BackgroundTaskKind::Shell, "bad\n")],
            vec![task(
                BackgroundTaskKind::Shell,
                &"x".repeat(MAX_ID_BYTES + 1),
            )],
        ] {
            assert_eq!(
                terminal.set_background_wait("herdr:claude", "root", tasks, true, 50),
                Err("invalid_background_wait")
            );
            assert_eq!(terminal.background_snapshot(), full);
        }
        for (source, owner) in [("custom:claude", "root"), ("herdr:claude", "foreign")] {
            assert!(terminal
                .set_background_wait(source, owner, refs[..1].to_vec(), false, 50)
                .is_err());
            assert_eq!(terminal.background_snapshot(), full);
        }
    }

    #[test]
    fn wait_matches_retained_bash_and_never_changes_detection_or_completion() {
        let mut terminal = terminal();
        terminal
            .report_background_shell(
                "herdr:claude",
                "root",
                ShellObservation::BashBackgroundObserved {
                    task_id: "bash".into(),
                    tool_use_id: "tool".into(),
                    hook_agent_id: None,
                    command: None,
                },
                1,
            )
            .unwrap();
        snapshot(&mut terminal, &[]);
        for state in [AgentState::Idle, AgentState::Working, AgentState::Blocked] {
            terminal.set_detected_state(Some(Agent::Claude), state);
            assert_eq!(
                set(
                    &mut terminal,
                    vec![task(BackgroundTaskKind::Shell, "bash")],
                    true,
                    10
                )
                .0
                .len(),
                1
            );
            assert_eq!(terminal.state, state);
            assert_eq!(terminal.last_agent_completion_seq, None);
            assert!(terminal.hook_authority.is_none());
            assert!(terminal.hook_report_is_newer("herdr:claude", Some(1)));
            assert_eq!(
                terminal.background_snapshot().current_state,
                BackgroundCurrentState::Unknown
            );
        }
        assert_eq!(
            set(
                &mut terminal,
                vec![task(BackgroundTaskKind::Subagent, "bash")],
                true,
                10
            )
            .1
            .len(),
            1
        );
    }
}
