use serde::{Deserialize, Serialize};

use super::{Direction, Layout, Sessions, Store};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Destination {
    Workspace {
        #[serde(default)]
        name: Option<String>,
    },
    Tab {
        #[serde(default)]
        workspace: Option<String>,
        #[serde(default)]
        name: Option<String>,
    },
    Split {
        beside: String,
        direction: Direction,
        ratio: f32,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct PendingStart {
    pub(crate) destination: Destination,
    #[serde(default)]
    pub(crate) reply_received: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Created {
    pub(crate) terminal: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) workspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tab: Option<String>,
}

impl Destination {
    pub(crate) fn normalized(mut self) -> Self {
        if let Self::Split { ratio, .. } = &mut self {
            *ratio = if ratio.is_nan() {
                0.5
            } else {
                ratio.clamp(0.1, 0.9)
            };
        }
        self
    }

    pub(crate) fn validate(&self, layout: &Layout) -> Result<(), String> {
        match self {
            Self::Workspace { .. } => Ok(()),
            Self::Tab { workspace, .. } => {
                let workspace = workspace
                    .as_deref()
                    .or_else(|| layout.workspaces.first().map(|w| w.id.as_str()))
                    .ok_or_else(|| "the session has no workspace".to_owned())?;
                if layout.workspaces.iter().any(|w| w.id == workspace) {
                    Ok(())
                } else {
                    Err(format!("no workspace {workspace}"))
                }
            }
            Self::Split { beside, .. } if layout.shows(beside) => Ok(()),
            Self::Split { beside, .. } => Err(format!("{beside} is not shown")),
        }
    }

    pub(crate) fn place(&self, layout: &mut Layout, terminal: &str) -> Result<Created, String> {
        let mut created = Created {
            terminal: terminal.to_owned(),
            workspace: None,
            tab: None,
        };
        match self {
            Self::Workspace { name } => {
                let (workspace, tab) = layout.add_workspace(name.clone(), terminal.to_owned());
                created.workspace = Some(workspace);
                created.tab = Some(tab);
            }
            Self::Tab { workspace, name } => {
                let workspace = workspace
                    .as_deref()
                    .or_else(|| layout.workspaces.first().map(|w| w.id.as_str()))
                    .ok_or_else(|| "the session has no workspace".to_owned())?
                    .to_owned();
                created.tab = Some(
                    layout
                        .add_tab(&workspace, name.clone(), terminal.to_owned())
                        .ok_or_else(|| format!("no workspace {workspace}"))?,
                );
            }
            Self::Split {
                beside,
                direction,
                ratio,
            } => {
                if !layout.split(beside, *direction, *ratio, terminal.to_owned()) {
                    return Err(format!("{beside} is not shown"));
                }
            }
        }
        Ok(created)
    }
}

impl Store {
    /// Saves the start before the caller submits terminal.run. An uncertain
    /// document write must not be followed by a start. References use the
    /// document version, so concurrent sessions cannot reserve the same one.
    pub(crate) fn reserve_start(&self, destination: &Destination) -> Result<String, String> {
        let (reserved, _, _) = self.write(|sessions, version| {
            let empty = Layout::default();
            let layout = sessions.get(&self.session).unwrap_or(&empty);
            if let Some(reference) = layout.pending_starts.keys().next() {
                return Err(format!("start {reference} is still unconfirmed in session {}; no new program was started", self.session));
            }
            self.put_pending(sessions, version, destination.clone())
        })?;
        reserved
    }

    pub(crate) fn reserve_first_start(&self) -> Result<Option<String>, String> {
        let (reserved, _, _) = self.write(|sessions, version| {
            let occupied = sessions.get(&self.session).is_some_and(|layout| {
                !layout.workspaces.is_empty() || !layout.pending_starts.is_empty()
            });
            if occupied {
                return Ok(None);
            }
            self.put_pending(sessions, version, Destination::Workspace { name: None })
                .map(Some)
        })?;
        reserved
    }

    fn put_pending(
        &self,
        sessions: &mut Sessions,
        version: u64,
        destination: Destination,
    ) -> Result<String, String> {
        let empty = Layout::default();
        let layout = sessions.get(&self.session).unwrap_or(&empty);
        let mut destination = destination.normalized();
        if let Destination::Tab {
            workspace: target, ..
        } = &mut destination
        {
            if target.is_none() {
                *target = layout
                    .workspaces
                    .first()
                    .map(|workspace| workspace.id.clone());
            }
        }
        destination.validate(layout)?;
        let reference = format!("{}:{version}", self.name);
        sessions
            .entry(self.session.clone())
            .or_default()
            .pending_starts
            .insert(
                reference.clone(),
                PendingStart {
                    destination,
                    reply_received: false,
                },
            );
        Ok(reference)
    }
}
