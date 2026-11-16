use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::hc::Core;

const SCHEMA: u64 = 1;
const EDIT_ATTEMPTS: usize = 8;

/// The herdr-core document holding every Lawn session's layout.
pub(crate) fn name() -> String {
    format!("{}-sessions", crate::config::app_dir_name())
}

/// Every session's layout, keyed by session name.
pub(crate) type Sessions = BTreeMap<String, Layout>;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Layout {
    #[serde(default)]
    pub(crate) workspaces: Vec<Workspace>,
    /// Terminals out of view, keyed by hc address, with where they return.
    #[serde(default)]
    pub(crate) parked: BTreeMap<String, ReturnPosition>,
    #[serde(default)]
    pub(crate) next_id: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Workspace {
    pub(crate) id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) tabs: Vec<Tab>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Tab {
    pub(crate) id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    pub(crate) root: Node,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Node {
    Pane {
        terminal: String,
    },
    Split {
        direction: Direction,
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Direction {
    Right,
    Down,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ReturnPosition {
    pub(crate) workspace: String,
    pub(crate) tab: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) beside: Option<String>,
    pub(crate) direction: Direction,
}

impl Layout {
    pub(crate) fn new_id(&mut self, prefix: &str) -> String {
        let id = format!("{prefix}{}", self.next_id);
        self.next_id += 1;
        id
    }

    /// Every terminal shown in any tab, depth-first.
    pub(crate) fn terminals(&self) -> impl Iterator<Item = &str> {
        self.workspaces
            .iter()
            .flat_map(|workspace| workspace.tabs.iter())
            .flat_map(|tab| tab.root.terminals())
    }

    pub(crate) fn shows(&self, terminal: &str) -> bool {
        self.workspaces
            .iter()
            .flat_map(|workspace| workspace.tabs.iter())
            .any(|tab| tab.root.shows(terminal))
    }
}

impl Node {
    pub(crate) fn terminals(&self) -> Vec<&str> {
        let mut terminals = Vec::new();
        self.collect_terminals(&mut terminals);
        terminals
    }

    pub(crate) fn shows(&self, terminal: &str) -> bool {
        match self {
            Node::Pane { terminal: shown } => shown == terminal,
            Node::Split { first, second, .. } => first.shows(terminal) || second.shows(terminal),
        }
    }

    fn collect_terminals<'a>(&'a self, terminals: &mut Vec<&'a str>) {
        match self {
            Node::Pane { terminal } => terminals.push(terminal),
            Node::Split { first, second, .. } => {
                first.collect_terminals(terminals);
                second.collect_terminals(terminals);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Loaded {
    pub(crate) layout: Layout,
    pub(crate) version: u64,
}

/// One session's view of the sessions document on one core.
pub(crate) struct Store {
    core: Arc<Core>,
    name: String,
    session: String,
    on: Option<String>,
}

impl Store {
    pub(crate) fn new(core: Arc<Core>, session: &str, on: Option<String>) -> Self {
        Store {
            core,
            name: name(),
            session: session.to_owned(),
            on,
        }
    }

    pub(crate) fn load(&self) -> Result<Loaded, String> {
        let (mut sessions, version) = self.load_all()?;
        Ok(Loaded {
            layout: sessions.remove(&self.session).unwrap_or_default(),
            version,
        })
    }

    pub(crate) fn load_all(&self) -> Result<(Sessions, u64), String> {
        let doc = self
            .core
            .doc_get(&self.name, self.on.as_deref())
            .map_err(|err| err.to_string())?;
        Ok((decode(&self.name, doc.value, doc.version)?, doc.version))
    }

    /// Applies `change` to this session's layout and writes it, rereading and
    /// reapplying when another writer got there first.
    pub(crate) fn edit<T>(
        &self,
        mut change: impl FnMut(&mut Layout) -> T,
    ) -> Result<(T, Loaded), String> {
        let (output, mut sessions, version) = self.write(|sessions| {
            let mut layout = sessions.get(&self.session).cloned().unwrap_or_default();
            let before = layout.clone();
            let output = change(&mut layout);
            if layout != before {
                sessions.insert(self.session.clone(), layout);
            }
            output
        })?;
        let layout = sessions.remove(&self.session).unwrap_or_default();
        Ok((output, Loaded { layout, version }))
    }

    /// Applies `change` to every session's layout in one write, rereading and
    /// reapplying when another writer got there first.
    pub(crate) fn edit_all<T>(
        &self,
        change: impl FnMut(&mut Sessions) -> T,
    ) -> Result<(T, u64), String> {
        let (output, _, version) = self.write(change)?;
        Ok((output, version))
    }

    fn write<T>(
        &self,
        mut change: impl FnMut(&mut Sessions) -> T,
    ) -> Result<(T, Sessions, u64), String> {
        for _ in 0..EDIT_ATTEMPTS {
            let (loaded, version) = self.load_all()?;
            let mut sessions = loaded.clone();
            let output = change(&mut sessions);
            if sessions == loaded {
                return Ok((output, sessions, version));
            }
            let value = encode(&sessions)?;
            match self
                .core
                .doc_set(&self.name, self.on.as_deref(), &value, version)
            {
                Ok(version) => return Ok((output, sessions, version)),
                Err(err) if err.kind() == Some("conflict") => continue,
                Err(err) => return Err(err.to_string()),
            }
        }
        Err(format!("{} kept changing; try again", self.name))
    }
}

fn encode(sessions: &Sessions) -> Result<Value, String> {
    let sessions = serde_json::to_value(sessions)
        .map_err(|err| format!("cannot encode the session layouts: {err}"))?;
    Ok(json!({"schema": SCHEMA, "sessions": sessions}))
}

fn decode(name: &str, mut value: Value, version: u64) -> Result<Sessions, String> {
    if version == 0 || value.is_null() {
        return Ok(Sessions::new());
    }
    let schema = value
        .get("schema")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{name} has no schema version"))?;
    if schema > SCHEMA {
        return Err(format!(
            "{name} was written by a newer Lawn (schema {schema}); update Lawn to open it"
        ));
    }
    let sessions = value
        .get_mut("sessions")
        .map(Value::take)
        .unwrap_or_default();
    serde_json::from_value(sessions).map_err(|err| format!("cannot read {name}: {err}"))
}

#[cfg(test)]
mod tests;
