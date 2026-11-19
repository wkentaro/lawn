use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::endings::{Ending, Endings};
use crate::api::schema::AgentStatus;
use crate::detect::{Agent, AgentState};
use crate::pane::{core_status, CoreStatus};
use crate::workspace::derive_label_from_cwd;

#[derive(Debug, PartialEq)]
pub(super) enum Report {
    Snapshot(HashMap<String, PathBuf>),
    Folder(String, PathBuf),
    Statuses(HashMap<String, CoreStatus>),
    Status(String, CoreStatus),
    Ending(Ending),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Status {
    pub(super) agent: Option<Agent>,
    pub(super) state: AgentState,
    pub(super) seen: bool,
    pub(super) changed: u64,
}

impl Status {
    pub(super) fn shown(&self) -> AgentStatus {
        crate::app::pane_agent_status(self.state, self.seen)
    }
}

#[derive(Default)]
pub(super) struct Terminals {
    folder_names: HashMap<String, String>,
    statuses: HashMap<String, Status>,
    changes: u64,
    endings: Endings,
}

impl Terminals {
    pub(super) fn folder_name(&self, terminal: &str) -> Option<&str> {
        self.folder_names.get(terminal).map(String::as_str)
    }

    pub(super) fn status(&self, terminal: &str) -> Option<&Status> {
        self.statuses.get(terminal)
    }

    pub(super) fn see<'a>(&mut self, terminals: impl IntoIterator<Item = &'a str>) -> bool {
        terminals.into_iter().fold(false, |seen, terminal| {
            match self.statuses.get_mut(terminal) {
                Some(status) if !status.seen => {
                    status.seen = true;
                    true
                }
                _ => seen,
            }
        })
    }

    pub(super) fn hear(
        &mut self,
        report: Report,
        names: impl Fn(&str) -> bool,
        shows: impl Fn(&str) -> bool,
    ) -> bool {
        match report {
            Report::Snapshot(folders) => {
                let known = folders
                    .into_iter()
                    .map(|(terminal, folder)| (terminal, derive_label_from_cwd(&folder)))
                    .collect();
                let old = std::mem::replace(&mut self.folder_names, known);
                old.keys().chain(self.folder_names.keys()).any(|terminal| {
                    names(terminal) && old.get(terminal) != self.folder_names.get(terminal)
                })
            }
            Report::Folder(terminal, folder) => {
                let name = derive_label_from_cwd(&folder);
                let renamed = names(&terminal) && self.folder_names.get(&terminal) != Some(&name);
                self.folder_names.insert(terminal, name);
                renamed
            }
            Report::Statuses(statuses) => {
                let mut drawn = false;
                self.statuses.retain(|terminal, _| {
                    let stays = statuses.contains_key(terminal);
                    drawn |= !stays && shows(terminal);
                    stays
                });
                statuses
                    .into_iter()
                    .fold(drawn, |drawn, (terminal, status)| {
                        self.report(terminal, status, &shows) | drawn
                    })
            }
            Report::Status(terminal, status) => self.report(terminal, status, &shows),
            Report::Ending(ending) => {
                self.endings.hear(ending);
                false
            }
        }
    }

    pub(super) fn endings(&mut self) -> &mut Endings {
        &mut self.endings
    }

    fn report(
        &mut self,
        terminal: String,
        reported: CoreStatus,
        shows: impl Fn(&str) -> bool,
    ) -> bool {
        let before = self.statuses.get(&terminal).copied();
        let previous = before.map_or(AgentState::Unknown, |status| status.state);
        if reported.state == AgentState::Unknown {
            self.statuses.remove(&terminal);
        } else {
            let seen = match reported.state {
                AgentState::Idle
                    if reported.finished
                        && matches!(previous, AgentState::Working | AgentState::Blocked) =>
                {
                    false
                }
                AgentState::Idle => before.is_none_or(|status| status.seen),
                _ => true,
            };
            let changed = match before {
                Some(status) if status.state == reported.state => status.changed,
                _ => {
                    self.changes += 1;
                    self.changes
                }
            };
            self.statuses.insert(
                terminal.clone(),
                Status {
                    agent: reported.agent,
                    state: reported.state,
                    seen,
                    changed,
                },
            );
        }
        let drawn = |status: Option<&Status>| status.map(|status| (status.agent, status.shown()));
        shows(&terminal) && drawn(before.as_ref()) != drawn(self.statuses.get(&terminal))
    }
}

pub(super) fn follow() -> io::Result<mpsc::UnboundedReceiver<Report>> {
    let (reports, received) = mpsc::unbounded_channel();
    crate::thread_spawn::spawn_named("lawn-terminals", move || {
        while listen(&reports) {
            std::thread::sleep(super::follow::RECONNECT);
        }
    })?;
    Ok(received)
}

fn listen(reports: &mpsc::UnboundedSender<Report>) -> bool {
    let core = match crate::core_client::core() {
        Ok(core) => core,
        Err(error) => {
            tracing::debug!(%error, "herdr core is unreachable");
            return !reports.is_closed();
        }
    };
    let subscription = match core.subscribe(json!({})) {
        Ok(subscription) => subscription,
        Err(error) => {
            tracing::debug!(%error, "the core terminals cannot be followed");
            return !reports.is_closed();
        }
    };
    if reports.send(snapshot(&subscription.snapshot)).is_err()
        || reports.send(statuses(&subscription.snapshot)).is_err()
        || reports.send(finished(&subscription.snapshot)).is_err()
    {
        return false;
    }
    crate::pane::read_events(
        &core,
        subscription.channel,
        subscription.stream,
        |message| {
            if let Some(report) = reported(message) {
                reports.send(report).ok();
            }
            if let Some(report) = status_reported(message) {
                reports.send(report).ok();
            }
            if let Some(report) = ended_reported(message) {
                reports.send(report).ok();
            }
        },
    );
    !reports.is_closed()
}

fn reported(message: &Value) -> Option<Report> {
    let params = &message["params"];
    match (message["method"].as_str()?, params["type"].as_str()) {
        ("event", Some("created")) => {
            let (terminal, folder) = record(&params["data"])?;
            Some(Report::Folder(terminal, folder))
        }
        ("event", Some("foreground")) => Some(Report::Folder(
            params["terminal"].as_str()?.to_owned(),
            params["data"]["cwd"].as_str()?.into(),
        )),
        ("missed", _) => Some(snapshot(params)),
        _ => None,
    }
}

fn snapshot(snapshot: &Value) -> Report {
    Report::Snapshot(
        snapshot["terminals"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(record)
            .collect(),
    )
}

fn status_reported(message: &Value) -> Option<Report> {
    let params = &message["params"];
    match (message["method"].as_str()?, params["type"].as_str()) {
        ("event", Some("status")) => Some(Report::Status(
            params["terminal"].as_str()?.to_owned(),
            core_status(&params["data"]["status"]),
        )),
        ("missed", _) => Some(statuses(params)),
        _ => None,
    }
}

fn statuses(snapshot: &Value) -> Report {
    Report::Statuses(
        snapshot["terminals"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|record| {
                Some((
                    record["address"].as_str()?.to_owned(),
                    core_status(&record["status"]),
                ))
            })
            .collect(),
    )
}

fn ended_reported(message: &Value) -> Option<Report> {
    let params = &message["params"];
    let ending = match (message["method"].as_str()?, params["type"].as_str()) {
        ("missed", _) => return Some(finished(params)),
        ("event", Some("finished")) => Ending::Finished,
        ("event", Some("created" | "restarted")) => Ending::Live,
        ("event", Some("removed")) => Ending::Removed,
        _ => return None,
    };
    Some(Report::Ending(ending(
        params["terminal"].as_str()?.to_owned(),
    )))
}

fn finished(snapshot: &Value) -> Report {
    let addresses = |state: &str| {
        snapshot["terminals"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|record| record["state"] == state)
            .filter_map(|record| Some(record["address"].as_str()?.to_owned()))
            .collect()
    };
    Report::Ending(Ending::Snapshot {
        finished: addresses("finished"),
        live: addresses("live"),
    })
}

fn record(record: &Value) -> Option<(String, PathBuf)> {
    let folder = record
        .pointer("/foreground/cwd")
        .and_then(Value::as_str)
        .or_else(|| record["cwd"].as_str())?;
    Some((record["address"].as_str()?.to_owned(), folder.into()))
}

#[cfg(all(test, unix))]
mod tests;
