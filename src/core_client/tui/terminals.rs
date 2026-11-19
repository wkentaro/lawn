use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::arrivals::Arrivals;
use crate::api::schema::AgentStatus;
use crate::detect::{Agent, AgentState};
use crate::hc::{Core, HcResult};
use crate::pane::{core_status, CoreStatus};
use crate::workspace::derive_label_from_cwd;

#[derive(Debug, PartialEq)]
pub(super) enum Report {
    Snapshot {
        folders: HashMap<String, PathBuf>,
        statuses: Vec<(String, CoreStatus)>,
    },
    Folder(String, PathBuf),
    Status(String, CoreStatus),
    Names(HashMap<String, String>),
    Name(String, Option<String>),
    Removed(String),
    Resume(Option<String>),
    Forget(Option<String>),
}

const NAME_LABEL: &str = "lawn-name";

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

pub(super) struct Moved {
    pub(super) terminal: String,
    pub(super) agent: Option<Agent>,
    pub(super) from: AgentState,
    pub(super) to: AgentState,
    pub(super) parked: bool,
}

#[derive(Default)]
pub(super) struct Terminals {
    folders: HashMap<String, (PathBuf, String)>,
    statuses: HashMap<String, Status>,
    names: HashMap<String, String>,
    changes: u64,
    followed: bool,
    moves: Arrivals<Moved>,
}

impl Terminals {
    pub(super) fn folder_name(&self, terminal: &str) -> Option<&str> {
        self.folders.get(terminal).map(|(_, name)| name.as_str())
    }

    pub(super) fn folder(&self, terminal: &str) -> Option<&PathBuf> {
        self.folders.get(terminal).map(|(folder, _)| folder)
    }

    pub(super) fn name(&self, terminal: &str) -> Option<&str> {
        self.names.get(terminal).map(String::as_str)
    }

    pub(super) fn status(&self, terminal: &str) -> Option<&Status> {
        self.statuses.get(terminal)
    }

    pub(super) fn moves(&mut self, holds: impl Fn(&str) -> bool) -> Vec<Moved> {
        self.moves.take(holds)
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
        parks: impl Fn(&str) -> bool,
    ) -> bool {
        match report {
            Report::Snapshot { folders, statuses } => {
                let known = folders
                    .into_iter()
                    .map(|(terminal, folder)| {
                        let name = derive_label_from_cwd(&folder);
                        (terminal, (folder, name))
                    })
                    .collect();
                let old = std::mem::replace(&mut self.folders, known);
                let mut drawn = old.keys().chain(self.folders.keys()).any(|terminal| {
                    names(terminal)
                        && old.get(terminal).map(|(_, name)| name.as_str())
                            != self.folder_name(terminal)
                });
                let listed: HashSet<&str> = statuses
                    .iter()
                    .map(|(terminal, _)| terminal.as_str())
                    .collect();
                self.moves.retain(|terminal| listed.contains(terminal));
                self.statuses.retain(|terminal, _| {
                    let stays = listed.contains(terminal.as_str());
                    drawn |= !stays && shows(terminal);
                    stays
                });
                let drawn = statuses
                    .into_iter()
                    .fold(drawn, |drawn, (terminal, status)| {
                        self.report(terminal, status, &shows, &parks) | drawn
                    });
                if !std::mem::replace(&mut self.followed, true) {
                    self.moves.clear();
                }
                drawn
            }
            Report::Folder(terminal, folder) => {
                let name = derive_label_from_cwd(&folder);
                let renamed = names(&terminal) && self.folder_name(&terminal) != Some(&name);
                self.folders.insert(terminal, (folder, name));
                renamed
            }
            Report::Status(terminal, status) => self.report(terminal, status, &shows, &parks),
            Report::Removed(terminal) => {
                self.moves.remove(&terminal);
                self.statuses.remove(&terminal).is_some() && shows(&terminal)
            }
            Report::Names(names) => {
                let old = std::mem::replace(&mut self.names, names);
                old.keys().chain(self.names.keys()).any(|terminal| {
                    shows(terminal) && old.get(terminal) != self.names.get(terminal)
                })
            }
            Report::Name(terminal, name) => {
                let old = match name {
                    Some(name) => self.names.insert(terminal.clone(), name),
                    None => self.names.remove(&terminal),
                };
                shows(&terminal) && old.as_ref() != self.names.get(&terminal)
            }
            Report::Resume(_) | Report::Forget(_) => false,
        }
    }

    fn report(
        &mut self,
        terminal: String,
        reported: CoreStatus,
        shows: impl Fn(&str) -> bool,
        parks: impl Fn(&str) -> bool,
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
            let moved = previous != reported.state;
            let changed = match before {
                Some(status) if !moved => status.changed,
                _ => {
                    self.changes += 1;
                    self.changes
                }
            };
            let other_agent = before.is_some_and(|status| status.agent != reported.agent);
            if other_agent || (moved && (reported.state != AgentState::Idle || !seen)) {
                self.moves.insert(
                    terminal.clone(),
                    Moved {
                        terminal: terminal.clone(),
                        agent: reported.agent.or(before.and_then(|status| status.agent)),
                        from: previous,
                        to: reported.state,
                        parked: parks(&terminal),
                    },
                );
            }
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

pub(super) fn follow<T: From<Report> + Send + 'static>(
    reports: mpsc::UnboundedSender<T>,
) -> io::Result<()> {
    crate::thread_spawn::spawn_named("lawn-terminals", move || {
        while listen(&reports) {
            std::thread::sleep(super::follow::RECONNECT);
        }
    })?;
    Ok(())
}

fn listen<T: From<Report>>(reports: &mpsc::UnboundedSender<T>) -> bool {
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
    if reports
        .send(snapshot(&subscription.snapshot).into())
        .is_err()
        || reports.send(names(&subscription.snapshot).into()).is_err()
        || reports.send(Report::Resume(None).into()).is_err()
        || reports.send(Report::Forget(None).into()).is_err()
    {
        return false;
    }
    let mut roots = roots(&subscription.snapshot);
    crate::pane::read_events(
        &core,
        subscription.channel,
        subscription.stream,
        |message| {
            if let Some(report) = reported(message) {
                reports.send(report.into()).ok();
            }
            if let Some(report) = name_reported(message) {
                reports.send(report.into()).ok();
            }
            if let Some(report) = resume_reported(message) {
                reports.send(report.into()).ok();
            }
            if let Some(report) = forget_reported(message, &mut roots) {
                reports.send(report.into()).ok();
            }
        },
    );
    !reports.is_closed()
}

pub(super) fn reported(message: &Value) -> Option<Report> {
    let params = &message["params"];
    match (message["method"].as_str()?, params["type"].as_str()) {
        ("event", Some("created")) => {
            let (terminal, folder) = record(&params["data"])?;
            Some(Report::Folder(terminal, folder))
        }
        ("event", Some("removed")) => {
            Some(Report::Removed(params["terminal"].as_str()?.to_owned()))
        }
        ("event", Some("foreground")) => Some(Report::Folder(
            params["terminal"].as_str()?.to_owned(),
            params["data"]["cwd"].as_str()?.into(),
        )),
        ("event", Some("status")) => Some(Report::Status(
            params["terminal"].as_str()?.to_owned(),
            core_status(&params["data"]["status"]),
        )),
        ("missed", _) => Some(snapshot(params)),
        _ => None,
    }
}

fn resume_reported(message: &Value) -> Option<Report> {
    match (
        message["method"].as_str()?,
        message["params"]["type"].as_str(),
    ) {
        ("event", Some("restarted")) => Some(Report::Resume(Some(
            message["params"]["terminal"].as_str()?.to_owned(),
        ))),
        ("missed", _) => Some(Report::Resume(None)),
        _ => None,
    }
}

fn roots(snapshot: &Value) -> HashMap<String, u64> {
    snapshot["terminals"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(root)
        .collect()
}

fn root(record: &Value) -> Option<(String, u64)> {
    Some((
        record["address"].as_str()?.to_owned(),
        record["pid"].as_u64()?,
    ))
}

fn forget_reported(message: &Value, roots: &mut HashMap<String, u64>) -> Option<Report> {
    let params = &message["params"];
    match (message["method"].as_str()?, params["type"].as_str()) {
        ("event", Some("created" | "restarted")) => {
            let (terminal, pid) = root(&params["data"])?;
            roots.insert(terminal, pid);
            None
        }
        ("event", Some("foreground")) => {
            let terminal = params["terminal"].as_str()?;
            let pid = params["data"]["pid"].as_u64()?;
            (roots.get(terminal) == Some(&pid)).then(|| Report::Forget(Some(terminal.to_owned())))
        }
        ("event", Some("removed")) => {
            roots.remove(params["terminal"].as_str()?);
            None
        }
        ("missed", _) => {
            *roots = self::roots(params);
            Some(Report::Forget(None))
        }
        _ => None,
    }
}

fn snapshot(snapshot: &Value) -> Report {
    let records = || snapshot["terminals"].as_array().into_iter().flatten();
    Report::Snapshot {
        folders: records().filter_map(record).collect(),
        statuses: records()
            .filter_map(|record| {
                Some((
                    record["address"].as_str()?.to_owned(),
                    core_status(&record["status"]),
                ))
            })
            .collect(),
    }
}

fn name_reported(message: &Value) -> Option<Report> {
    let params = &message["params"];
    match (message["method"].as_str()?, params["type"].as_str()) {
        ("event", Some("labels")) => Some(Report::Name(
            params["terminal"].as_str()?.to_owned(),
            name(&params["data"]),
        )),
        ("event", Some("created")) => Some(Report::Name(
            params["data"]["address"].as_str()?.to_owned(),
            name(&params["data"]),
        )),
        ("event", Some("removed")) => {
            Some(Report::Name(params["terminal"].as_str()?.to_owned(), None))
        }
        ("missed", _) => Some(names(params)),
        _ => None,
    }
}

fn names(snapshot: &Value) -> Report {
    Report::Names(
        snapshot["terminals"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|record| Some((record["address"].as_str()?.to_owned(), name(record)?)))
            .collect(),
    )
}

fn name(record: &Value) -> Option<String> {
    record["labels"][NAME_LABEL].as_str().map(str::to_owned)
}

pub(super) fn rename(core: &Core, terminal: &str, name: &str) -> HcResult<Value> {
    core.call(
        "terminal.label",
        json!({"terminal": terminal, "labels": {NAME_LABEL: name}}),
    )
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
