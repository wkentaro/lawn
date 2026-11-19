use super::{Direction, Layout, Node, Tab, Workspace};

const MIN_RATIO: f32 = 0.1;
const MAX_RATIO: f32 = 0.9;
pub(super) const EVEN_RATIO: f32 = 0.5;

/// Where an unparked terminal goes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Placement {
    Beside {
        terminal: String,
        direction: Direction,
    },
    NewTab {
        workspace: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Arrangement {
    NameWorkspace { workspace: String, name: String },
    NameTab { tab: String, name: String },
}

impl Layout {
    #[cfg(any(unix, test))]
    pub(crate) fn arrange(&mut self, arrangement: &Arrangement) -> bool {
        match arrangement {
            Arrangement::NameWorkspace { workspace, name } => self
                .workspaces
                .iter_mut()
                .find(|w| w.id == *workspace)
                .map(|w| w.name = Some(name.clone()))
                .is_some(),
            Arrangement::NameTab { tab, name } => self
                .workspaces
                .iter_mut()
                .flat_map(|workspace| workspace.tabs.iter_mut())
                .find(|t| t.id == *tab)
                .map(|t| t.name = Some(name.clone()))
                .is_some(),
        }
    }

    /// Adds a workspace whose one tab shows `terminal`; returns the workspace
    /// and tab ids.
    pub(crate) fn add_workspace(
        &mut self,
        name: Option<String>,
        terminal: String,
    ) -> (String, String) {
        let workspace = self.new_id("ws");
        let tab = self.new_id("tab");
        self.workspaces.push(Workspace {
            id: workspace.clone(),
            name,
            tabs: vec![Tab {
                id: tab.clone(),
                name: None,
                root: Node::Pane { terminal },
            }],
        });
        (workspace, tab)
    }

    /// Adds a tab showing `terminal` to `workspace`, or `None` when it
    /// doesn't exist.
    pub(crate) fn add_tab(
        &mut self,
        workspace: &str,
        name: Option<String>,
        terminal: String,
    ) -> Option<String> {
        let index = self.workspaces.iter().position(|w| w.id == workspace)?;
        let tab = self.new_id("tab");
        self.workspaces[index].tabs.push(Tab {
            id: tab.clone(),
            name,
            root: Node::Pane { terminal },
        });
        Some(tab)
    }

    /// Splits the pane showing `beside`, with `terminal` second.
    pub(crate) fn split(
        &mut self,
        beside: &str,
        direction: Direction,
        ratio: f32,
        terminal: String,
    ) -> bool {
        let ratio = if ratio.is_nan() {
            EVEN_RATIO
        } else {
            ratio.clamp(MIN_RATIO, MAX_RATIO)
        };
        !self.parked.contains(beside) && self.insert_beside(beside, direction, ratio, terminal)
    }

    pub(super) fn insert_beside(
        &mut self,
        beside: &str,
        direction: Direction,
        ratio: f32,
        terminal: String,
    ) -> bool {
        let Some(node) = self
            .workspaces
            .iter_mut()
            .flat_map(|workspace| workspace.tabs.iter_mut())
            .find_map(|tab| tab.root.pane_mut(beside))
        else {
            return false;
        };
        let owned = std::mem::replace(node, empty_pane());
        *node = Node::Split {
            direction,
            ratio,
            first: Box::new(owned),
            second: Box::new(Node::Pane { terminal }),
        };
        true
    }

    pub(crate) fn remove(&mut self, terminal: &str) -> bool {
        self.parked.remove(terminal);
        let Some(source) = self.detach(terminal) else {
            return false;
        };
        self.drop_if_empty(&source);
        true
    }

    pub(crate) fn park(&mut self, terminal: &str) -> bool {
        self.shows(terminal) && self.parked.insert(terminal.to_owned())
    }

    pub(crate) fn unpark(&mut self, terminal: &str, to: Option<Placement>) -> Result<(), String> {
        if self.shows(terminal) {
            return Err(format!("{terminal} is already shown"));
        }
        let placement = match to {
            Some(placement) => self.checked(placement)?,
            None if self.owns(terminal) => {
                self.parked.remove(terminal);
                return Ok(());
            }
            None => self.first_tab(),
        };
        self.parked.remove(terminal);
        let source = self.detach(terminal);
        let terminal = terminal.to_owned();
        match placement {
            Resolved::Beside { beside, direction } => {
                self.insert_beside(&beside, direction, EVEN_RATIO, terminal);
            }
            Resolved::Tab(workspace) => {
                self.add_tab(&workspace, None, terminal);
            }
            Resolved::Workspace => {
                self.add_workspace(None, terminal);
            }
        }
        if let Some(source) = source {
            self.drop_if_empty(&source);
        }
        Ok(())
    }

    fn checked(&self, placement: Placement) -> Result<Resolved, String> {
        match placement {
            Placement::Beside {
                terminal,
                direction,
            } if self.shows(&terminal) => Ok(Resolved::Beside {
                beside: terminal,
                direction,
            }),
            Placement::Beside { terminal, .. } => Err(format!("{terminal} is not shown")),
            Placement::NewTab {
                workspace: Some(workspace),
            } if self.workspaces.iter().any(|w| w.id == workspace) => Ok(Resolved::Tab(workspace)),
            Placement::NewTab {
                workspace: Some(workspace),
            } => Err(format!("no workspace {workspace}")),
            Placement::NewTab { workspace: None } => Ok(self.first_tab()),
        }
    }

    fn first_tab(&self) -> Resolved {
        match self.workspaces.first() {
            Some(workspace) => Resolved::Tab(workspace.id.clone()),
            None => Resolved::Workspace,
        }
    }

    fn detach(&mut self, terminal: &str) -> Option<String> {
        for workspace in &mut self.workspaces {
            let Some(index) = workspace
                .tabs
                .iter()
                .position(|tab| tab.root.contains(terminal))
            else {
                continue;
            };
            if !workspace.tabs[index].root.take(terminal) {
                workspace.tabs.remove(index);
            }
            return Some(workspace.id.clone());
        }
        None
    }

    fn drop_if_empty(&mut self, workspace: &str) {
        self.workspaces
            .retain(|w| w.id != workspace || !w.tabs.is_empty());
    }
}

enum Resolved {
    Beside {
        beside: String,
        direction: Direction,
    },
    Tab(String),
    Workspace,
}

impl Node {
    fn pane_mut(&mut self, terminal: &str) -> Option<&mut Node> {
        match self {
            Node::Pane { terminal: shown } if shown == terminal => Some(self),
            Node::Pane { .. } => None,
            Node::Split { first, second, .. } => first
                .pane_mut(terminal)
                .or_else(|| second.pane_mut(terminal)),
        }
    }

    fn take(&mut self, terminal: &str) -> bool {
        let Node::Split { first, second, .. } = self else {
            return false;
        };
        let is_removed =
            |node: &Node| matches!(node, Node::Pane { terminal: shown } if shown == terminal);
        let sibling = if is_removed(first) {
            second
        } else if is_removed(second) {
            first
        } else {
            return first.take(terminal) || second.take(terminal);
        };
        let sibling = std::mem::replace(&mut **sibling, empty_pane());
        *self = sibling;
        true
    }
}

fn empty_pane() -> Node {
    Node::Pane {
        terminal: String::new(),
    }
}

#[cfg(test)]
mod tests;
