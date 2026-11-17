use super::{Direction, Layout, Node, ReturnPosition, Tab, Workspace};

const MIN_RATIO: f32 = 0.1;
const MAX_RATIO: f32 = 0.9;
const EVEN_RATIO: f32 = 0.5;

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

impl Layout {
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
        let Some(node) = self
            .workspaces
            .iter_mut()
            .flat_map(|workspace| workspace.tabs.iter_mut())
            .find_map(|tab| tab.root.pane_mut(beside))
        else {
            return false;
        };
        let shown = std::mem::replace(node, empty_pane());
        *node = Node::Split {
            direction,
            ratio,
            first: Box::new(shown),
            second: Box::new(Node::Pane { terminal }),
        };
        true
    }

    /// Removes the pane showing `terminal`, dropping a tab or workspace it
    /// leaves empty, and returns where it was.
    pub(crate) fn remove(&mut self, terminal: &str) -> Option<ReturnPosition> {
        let (w, t) = self
            .workspaces
            .iter()
            .enumerate()
            .find_map(|(w, workspace)| {
                workspace
                    .tabs
                    .iter()
                    .position(|tab| tab.root.shows(terminal))
                    .map(|t| (w, t))
            })?;
        let workspace = &mut self.workspaces[w];
        let tab = &mut workspace.tabs[t];
        let position = |beside, direction| ReturnPosition {
            workspace: workspace.id.clone(),
            tab: tab.id.clone(),
            beside,
            direction,
        };
        if let Some((beside, direction)) = tab.root.take(terminal) {
            return Some(position(Some(beside), direction));
        }
        let position = position(None, Direction::Right);
        workspace.tabs.remove(t);
        if workspace.tabs.is_empty() {
            self.workspaces.remove(w);
        }
        Some(position)
    }

    /// Takes `terminal` out of view, remembering where it returns.
    pub(crate) fn park(&mut self, terminal: &str) -> bool {
        let Some(position) = self.remove(terminal) else {
            return false;
        };
        self.parked.insert(terminal.to_owned(), position);
        true
    }

    /// Shows `terminal` again, at `to` or where it was parked from.
    pub(crate) fn unpark(&mut self, terminal: &str, to: Option<Placement>) -> Result<(), String> {
        if self.shows(terminal) {
            return Err(format!("{terminal} is already shown"));
        }
        let placement = match to {
            Some(placement) => self.checked(placement)?,
            None => self.return_placement(terminal),
        };
        self.parked.remove(terminal);
        let terminal = terminal.to_owned();
        match placement {
            Resolved::Beside { beside, direction } => {
                self.split(&beside, direction, EVEN_RATIO, terminal);
            }
            Resolved::Tab(workspace) => {
                self.add_tab(&workspace, None, terminal);
            }
            Resolved::Workspace => {
                self.add_workspace(None, terminal);
            }
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

    fn return_placement(&self, terminal: &str) -> Resolved {
        let Some(position) = self.parked.get(terminal) else {
            return self.first_tab();
        };
        let Some(workspace) = self.workspaces.iter().find(|w| w.id == position.workspace) else {
            return self.first_tab();
        };
        let Some(tab) = workspace.tabs.iter().find(|t| t.id == position.tab) else {
            return Resolved::Tab(workspace.id.clone());
        };
        let beside = position
            .beside
            .as_deref()
            .filter(|beside| tab.root.shows(beside))
            .unwrap_or_else(|| tab.root.last_terminal());
        Resolved::Beside {
            beside: beside.to_owned(),
            direction: position.direction,
        }
    }

    fn first_tab(&self) -> Resolved {
        match self.workspaces.first() {
            Some(workspace) => Resolved::Tab(workspace.id.clone()),
            None => Resolved::Workspace,
        }
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

    fn take(&mut self, terminal: &str) -> Option<(String, Direction)> {
        let Node::Split {
            direction,
            first,
            second,
            ..
        } = self
        else {
            return None;
        };
        let direction = *direction;
        let is_removed =
            |node: &Node| matches!(node, Node::Pane { terminal: shown } if shown == terminal);
        let sibling = if is_removed(first) {
            second
        } else if is_removed(second) {
            first
        } else {
            return first.take(terminal).or_else(|| second.take(terminal));
        };
        let beside = sibling.first_terminal().to_owned();
        let sibling = std::mem::replace(&mut **sibling, empty_pane());
        *self = sibling;
        Some((beside, direction))
    }
}

fn empty_pane() -> Node {
    Node::Pane {
        terminal: String::new(),
    }
}

#[cfg(test)]
mod tests;
