use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::workspace::derive_label_from_cwd;

#[derive(Debug, PartialEq)]
pub(super) enum Report {
    Snapshot(HashMap<String, PathBuf>),
    Folder(String, PathBuf),
}

#[derive(Default)]
pub(super) struct Terminals {
    folder_names: HashMap<String, String>,
}

impl Terminals {
    pub(super) fn folder_name(&self, terminal: &str) -> Option<&str> {
        self.folder_names.get(terminal).map(String::as_str)
    }

    pub(super) fn hear(&mut self, report: Report, names: impl Fn(&str) -> bool) -> bool {
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
        }
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
    if reports.send(snapshot(&subscription.snapshot)).is_err() {
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

fn record(record: &Value) -> Option<(String, PathBuf)> {
    let folder = record
        .pointer("/foreground/cwd")
        .and_then(Value::as_str)
        .or_else(|| record["cwd"].as_str())?;
    Some((record["address"].as_str()?.to_owned(), folder.into()))
}

#[cfg(all(test, unix))]
mod tests;
