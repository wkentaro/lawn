use std::collections::HashMap;

use ratatui::layout::Rect;

use super::terminals::{Report, Status, Terminals};
use crate::api::schema::AgentStatus;
use crate::config::PaneBordersConfig;
use crate::input::KeybindAction;
use crate::layout::{NavDirection, PaneId, PaneInfo, SplitBorder, TileLayout};
use crate::session_doc::{Arrangement, Destination, Direction, Layout, Node, Tab, Workspace};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PaneChrome {
    pub(super) borders: PaneBordersConfig,
    pub(super) gaps: bool,
    pub(super) outer_borders: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Focus {
    workspace: String,
    tab: String,
    terminal: Option<String>,
}

#[derive(Clone)]
pub(super) struct Tile {
    pub(super) terminal: String,
    pub(super) info: PaneInfo,
}

#[derive(Default)]
pub(super) struct Tiles {
    pub(super) tiles: Vec<Tile>,
    pub(super) splits: Vec<SplitBorder>,
    pub(super) hidden: Vec<(PaneId, String)>,
}

#[derive(Debug, PartialEq)]
pub(super) enum Operation {
    Start(Destination),
    Close(Vec<String>),
    Park(String),
    Arrange(Arrangement),
}

#[derive(Debug, PartialEq)]
pub(super) enum Request {
    Run(Operation),
    Confirm(Closing),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Closing {
    action: KeybindAction,
    focus: Focus,
    detail: String,
}

impl Closing {
    pub(super) fn detail(&self) -> &str {
        &self.detail
    }
}

#[derive(Default)]
pub(super) struct Session {
    layout: Layout,
    focus: Option<Focus>,
    ids: HashMap<String, PaneId>,
    next_id: u32,
    tab_of_workspace: HashMap<String, String>,
    pane_of_tab: HashMap<String, String>,
    previous_of_tab: HashMap<String, String>,
    zoomed: bool,
    created: Option<String>,
    terminals: Terminals,
}

impl Session {
    pub(super) fn update(&mut self, layout: Layout) {
        self.layout = layout;
        self.ids.retain(|terminal, _| self.layout.owns(terminal));
        let focus = self.reconciled_focus();
        self.set_focus(focus);
        self.reveal_created();
    }

    pub(super) fn heard(&mut self, report: Report) -> bool {
        let layout = &self.layout;
        let workspaces = &layout.workspaces;
        self.terminals.hear(
            report,
            |terminal| {
                workspaces.iter().any(|workspace| {
                    workspace.name.is_none() && naming(workspace) == Some(terminal)
                })
            },
            |terminal| layout.shows(terminal),
        )
    }

    pub(super) fn look(&mut self, presented: impl Fn(PaneId) -> bool) -> bool {
        self.terminals.see(
            self.ids
                .iter()
                .filter(|(_, id)| presented(**id))
                .map(|(terminal, _)| terminal.as_str()),
        )
    }

    pub(super) fn workspace_status(&self, workspace: &Workspace) -> AgentStatus {
        workspace
            .tabs
            .iter()
            .flat_map(|tab| self.statuses(tab))
            .max_by_key(|(_, status)| crate::app::tab_attention_priority(status.state, status.seen))
            .map_or(AgentStatus::Unknown, |(_, status)| status.shown())
    }

    pub(super) fn tab_status(&self, tab: &Tab) -> Option<(AgentStatus, usize)> {
        let rank = |status| match status {
            AgentStatus::Blocked => 4,
            AgentStatus::Working => 3,
            AgentStatus::Idle => 2,
            AgentStatus::Done => 1,
            AgentStatus::Unknown => 0,
        };
        self.statuses(tab)
            .map(|(_, status)| status.shown())
            .fold(None, |rolled, status| {
                let (top, blocked) = rolled.unwrap_or((status, 0));
                Some((
                    if rank(status) > rank(top) {
                        status
                    } else {
                        top
                    },
                    blocked + usize::from(status == AgentStatus::Blocked),
                ))
            })
    }

    pub(super) fn agents(&self) -> impl Iterator<Item = (usize, usize, &str, &Status)> {
        self.layout
            .workspaces
            .iter()
            .enumerate()
            .flat_map(move |(workspace, entry)| {
                entry.tabs.iter().enumerate().flat_map(move |(tab, entry)| {
                    self.statuses(entry)
                        .map(move |(terminal, status)| (workspace, tab, terminal, status))
                })
            })
    }

    fn statuses<'a>(&'a self, tab: &'a Tab) -> impl Iterator<Item = (&'a str, &'a Status)> {
        tab.root.terminals().into_iter().filter_map(|terminal| {
            if self.layout.parked.contains(terminal) {
                return None;
            }
            Some((terminal, self.terminals.status(terminal)?))
        })
    }

    pub(super) fn reveal(&mut self, terminal: &str) -> bool {
        self.locate(terminal)
            .is_some_and(|focus| self.move_focus(focus))
    }

    pub(super) fn label<'a>(&'a self, workspace: &'a Workspace) -> &'a str {
        workspace
            .name
            .as_deref()
            .or_else(|| self.terminals.folder_name(naming(workspace)?))
            .unwrap_or("workspace")
    }

    pub(super) fn created(&mut self, terminal: String) -> bool {
        self.created = Some(terminal);
        self.reveal_created()
    }

    fn reveal_created(&mut self) -> bool {
        let Some(focus) = self
            .created
            .as_deref()
            .and_then(|terminal| self.locate(terminal))
        else {
            return false;
        };
        self.created = None;
        self.move_focus(focus)
    }

    pub(super) fn navigate(
        &mut self,
        action: KeybindAction,
        area: Rect,
        chrome: PaneChrome,
    ) -> bool {
        let Some(focus) = self.focus.clone() else {
            return false;
        };
        let terminal = match action {
            KeybindAction::FocusPaneLeft => self.toward(NavDirection::Left, area, chrome),
            KeybindAction::FocusPaneDown => self.toward(NavDirection::Down, area, chrome),
            KeybindAction::FocusPaneUp => self.toward(NavDirection::Up, area, chrome),
            KeybindAction::FocusPaneRight => self.toward(NavDirection::Right, area, chrome),
            KeybindAction::CyclePaneNext => self.cycled(&focus, 1),
            KeybindAction::CyclePanePrevious => self.cycled(&focus, -1),
            KeybindAction::LastPane => self
                .previous_of_tab
                .get(&focus.tab)
                .filter(|terminal| self.shown_in(&focus, terminal))
                .cloned(),
            KeybindAction::Zoom => {
                self.zoomed = !self.zoomed && focus.terminal.is_some();
                return focus.terminal.is_some();
            }
            _ => None,
        };
        let next = match terminal {
            Some(terminal) => Some(Focus {
                terminal: Some(terminal),
                ..focus.clone()
            }),
            None => self.switched(&focus, action),
        };
        next.is_some_and(|next| self.move_focus(next))
    }

    pub(super) fn operation(&self, action: KeybindAction, confirm_close: bool) -> Option<Request> {
        if action == KeybindAction::NewWorkspace {
            return Some(Request::Run(Operation::Start(Destination::Workspace {
                name: None,
            })));
        }
        self.operation_at(self.focus.as_ref()?, action, confirm_close)
    }

    pub(super) fn confirmed(&self, closing: &Closing) -> Option<Operation> {
        match self.operation_at(&closing.focus, closing.action, false)? {
            Request::Run(operation) => Some(operation),
            Request::Confirm(_) => None,
        }
    }

    fn operation_at(
        &self,
        focus: &Focus,
        action: KeybindAction,
        confirm_close: bool,
    ) -> Option<Request> {
        let workspace = self
            .layout
            .workspaces
            .iter()
            .find(|workspace| workspace.id == focus.workspace)?;
        let tab = workspace.tabs.iter().find(|tab| tab.id == focus.tab);
        let shown = |tab: &Tab| -> Vec<String> {
            tab.root
                .terminals()
                .into_iter()
                .filter(|terminal| !self.layout.parked.contains(*terminal))
                .map(str::to_owned)
                .collect()
        };
        let alone = || {
            workspace
                .tabs
                .iter()
                .filter(|tab| !shown(tab).is_empty())
                .count()
                <= 1
        };
        let confirm = || {
            Some(Request::Confirm(Closing {
                action,
                focus: focus.clone(),
                detail: self.closing_detail(workspace),
            }))
        };
        let closing = match action {
            KeybindAction::SplitVertical | KeybindAction::SplitHorizontal => {
                return Some(Request::Run(Operation::Start(Destination::Split {
                    beside: focus.terminal.clone()?,
                    direction: if action == KeybindAction::SplitVertical {
                        Direction::Right
                    } else {
                        Direction::Down
                    },
                    ratio: 0.5,
                })));
            }
            KeybindAction::NewTab => {
                return Some(Request::Run(Operation::Start(Destination::Tab {
                    workspace: Some(workspace.id.clone()),
                    name: None,
                })));
            }
            KeybindAction::ParkPane => {
                return Some(Request::Run(Operation::Park(focus.terminal.clone()?)))
            }
            KeybindAction::ClosePane if confirm_close && shown(tab?).len() == 1 && alone() => {
                return confirm()
            }
            KeybindAction::ClosePane => {
                let shown = shown(tab?);
                vec![focus
                    .terminal
                    .clone()
                    .filter(|terminal| shown.contains(terminal))?]
            }
            KeybindAction::CloseTab if confirm_close && alone() => return confirm(),
            KeybindAction::CloseTab => shown(tab?),
            KeybindAction::CloseWorkspace if confirm_close => return confirm(),
            KeybindAction::CloseWorkspace => workspace
                .tabs
                .iter()
                .flat_map(|tab| tab.root.terminals())
                .map(str::to_owned)
                .collect(),
            _ => return None,
        };
        (!closing.is_empty()).then_some(Request::Run(Operation::Close(closing)))
    }

    fn closing_detail(&self, workspace: &Workspace) -> String {
        let terminals: Vec<&str> = workspace
            .tabs
            .iter()
            .flat_map(|tab| tab.root.terminals())
            .collect();
        let panes = terminals.len();
        let parked = terminals
            .iter()
            .filter(|terminal| self.layout.parked.contains(**terminal))
            .count();
        let counted = match (panes, parked) {
            (1, 0) => "1 pane".to_owned(),
            (_, 0) => format!("{panes} panes"),
            (1, _) => "1 parked pane".to_owned(),
            _ => format!("{panes} panes, {parked} parked"),
        };
        format!("{} — {counted}", self.label(workspace))
    }

    fn toward(
        &mut self,
        direction: NavDirection,
        area: Rect,
        chrome: PaneChrome,
    ) -> Option<String> {
        let tiles = self.arranged(area, chrome, false).tiles;
        let focused = self.focused_id()?;
        let infos: Vec<PaneInfo> = tiles.iter().map(|tile| tile.info.clone()).collect();
        let from = infos.iter().find(|info| info.id == focused)?;
        let to = crate::layout::find_in_direction(from, direction, &infos)?;
        tiles
            .into_iter()
            .find(|tile| tile.info.id == to)
            .map(|tile| tile.terminal)
    }

    fn cycled(&self, focus: &Focus, step: isize) -> Option<String> {
        let terminals: Vec<&str> = tab(&self.layout, focus)?
            .root
            .terminals()
            .into_iter()
            .filter(|terminal| !self.layout.parked.contains(*terminal))
            .collect();
        let current = terminals
            .iter()
            .position(|terminal| Some(*terminal) == focus.terminal.as_deref())?;
        terminals
            .get(wrapped(current, step, terminals.len()))
            .map(|terminal| (*terminal).to_owned())
    }

    fn switched(&self, focus: &Focus, action: KeybindAction) -> Option<Focus> {
        let workspaces = &self.layout.workspaces;
        let current = workspaces
            .iter()
            .position(|workspace| workspace.id == focus.workspace)?;
        let workspace = &workspaces[current];
        let tab_index = workspace.tabs.iter().position(|tab| tab.id == focus.tab);
        let (workspace, tab) = match action {
            KeybindAction::PreviousTab => {
                (workspace, wrapped(tab_index?, -1, workspace.tabs.len()))
            }
            KeybindAction::NextTab => (workspace, wrapped(tab_index?, 1, workspace.tabs.len())),
            KeybindAction::SwitchTab(index) => (workspace, index),
            KeybindAction::PreviousWorkspace => {
                return self.entered(&workspaces[wrapped(current, -1, workspaces.len())])
            }
            KeybindAction::NextWorkspace => {
                return self.entered(&workspaces[wrapped(current, 1, workspaces.len())])
            }
            KeybindAction::SwitchWorkspace(index) => return self.entered(workspaces.get(index)?),
            _ => return None,
        };
        Some(self.landing(workspace, workspace.tabs.get(tab)?))
    }

    fn entered(&self, workspace: &Workspace) -> Option<Focus> {
        let tab = workspace.tabs.get(self.active_tab(workspace)?)?;
        Some(self.landing(workspace, tab))
    }

    pub(super) fn active_tab(&self, workspace: &Workspace) -> Option<usize> {
        self.tab_of_workspace
            .get(&workspace.id)
            .and_then(|remembered| workspace.tabs.iter().position(|tab| &tab.id == remembered))
            .or_else(|| (!workspace.tabs.is_empty()).then_some(0))
    }

    fn landing(&self, workspace: &Workspace, tab: &Tab) -> Focus {
        let parked = &self.layout.parked;
        let terminal = self
            .pane_of_tab
            .get(&tab.id)
            .map(String::as_str)
            .filter(|terminal| tab.root.contains(terminal) && !parked.contains(*terminal))
            .or_else(|| {
                tab.root
                    .terminals()
                    .into_iter()
                    .find(|terminal| !parked.contains(*terminal))
            });
        Focus {
            workspace: workspace.id.clone(),
            tab: tab.id.clone(),
            terminal: terminal.map(str::to_owned),
        }
    }

    fn shown_in(&self, focus: &Focus, terminal: &str) -> bool {
        Some(terminal) != focus.terminal.as_deref()
            && self.layout.shows(terminal)
            && tab(&self.layout, focus).is_some_and(|tab| tab.root.contains(terminal))
    }

    fn move_focus(&mut self, focus: Focus) -> bool {
        if self.focus.as_ref() == Some(&focus) {
            return false;
        }
        self.set_focus(Some(focus));
        true
    }

    fn set_focus(&mut self, focus: Option<Focus>) {
        if let (Some(old), Some(new)) = (&self.focus, &focus) {
            if old.tab == new.tab && old.terminal != new.terminal {
                if let Some(terminal) = &old.terminal {
                    self.previous_of_tab
                        .insert(old.tab.clone(), terminal.clone());
                }
            }
        }
        if self.focus.as_ref().map(|old| (&old.tab, &old.terminal))
            != focus.as_ref().map(|new| (&new.tab, &new.terminal))
        {
            self.zoomed = false;
        }
        if let Some(new) = &focus {
            self.tab_of_workspace
                .insert(new.workspace.clone(), new.tab.clone());
            if let Some(terminal) = &new.terminal {
                self.pane_of_tab.insert(new.tab.clone(), terminal.clone());
            }
        }
        self.focus = focus;
    }

    pub(super) fn focused_id(&self) -> Option<PaneId> {
        self.ids.get(self.focused()?).copied()
    }

    pub(super) fn focused(&self) -> Option<&str> {
        self.focus.as_ref()?.terminal.as_deref()
    }

    pub(super) fn workspaces(&self) -> &[Workspace] {
        &self.layout.workspaces
    }

    pub(super) fn focused_workspace(&self) -> Option<&Workspace> {
        let focus = self.focus.as_ref()?;
        self.layout
            .workspaces
            .iter()
            .find(|workspace| workspace.id == focus.workspace)
    }

    pub(super) fn focused_tab(&self) -> Option<&str> {
        Some(self.focus.as_ref()?.tab.as_str())
    }

    pub(super) fn zoomed(&self) -> bool {
        self.zoomed
    }

    pub(super) fn focus(&mut self, terminal: &str) -> bool {
        let Some(focus) = self.focus.clone() else {
            return false;
        };
        if !self.shown_in(&focus, terminal) {
            return false;
        }
        self.move_focus(Focus {
            terminal: Some(terminal.to_owned()),
            ..focus
        })
    }

    pub(super) fn id(&mut self, terminal: &str) -> PaneId {
        let next_id = &mut self.next_id;
        *self.ids.entry(terminal.to_owned()).or_insert_with(|| {
            *next_id += 1;
            PaneId::from_raw(*next_id)
        })
    }

    pub(super) fn tiles(&mut self, area: Rect, chrome: PaneChrome) -> Tiles {
        self.arranged(area, chrome, self.zoomed)
    }

    fn arranged(&mut self, area: Rect, chrome: PaneChrome, zoomed: bool) -> Tiles {
        let Some(focus) = self.focus.clone() else {
            return Tiles::default();
        };
        let Some(tab) = tab(&self.layout, &focus).cloned() else {
            return Tiles::default();
        };
        let focused = focus
            .terminal
            .as_deref()
            .map_or(PaneId::from_raw(0), |terminal| self.id(terminal));
        let root = match zoomed {
            true => Some(crate::layout::Node::Pane(focused)),
            false => self.project(&tab.root),
        };
        let Some(root) = root else {
            return Tiles::default();
        };
        let hidden: Vec<&str> = match zoomed {
            true => tab
                .root
                .terminals()
                .into_iter()
                .filter(|terminal| {
                    Some(*terminal) != focus.terminal.as_deref()
                        && !self.layout.parked.contains(*terminal)
                })
                .collect(),
            false => Vec::new(),
        };
        let hidden = hidden
            .into_iter()
            .map(|terminal| (self.id(terminal), terminal.to_owned()))
            .collect();
        let layout = TileLayout::from_saved(root, focused);
        let terminals: HashMap<PaneId, &str> = tab
            .root
            .terminals()
            .into_iter()
            .filter_map(|terminal| Some((*self.ids.get(terminal)?, terminal)))
            .collect();
        let infos = crate::ui::apply_pane_chrome(
            layout.panes(area),
            chrome.borders,
            chrome.gaps,
            chrome.outer_borders,
        );
        Tiles {
            tiles: infos
                .into_iter()
                .filter_map(|mut info| {
                    info.inner_rect = crate::ui::pane_inner_rect(info.rect, info.borders);
                    Some(Tile {
                        terminal: (*terminals.get(&info.id)?).to_owned(),
                        info,
                    })
                })
                .collect(),
            splits: layout.splits(area),
            hidden,
        }
    }

    fn project(&mut self, node: &Node) -> Option<crate::layout::Node> {
        match node {
            Node::Pane { terminal } if self.layout.parked.contains(terminal) => None,
            Node::Pane { terminal } => Some(crate::layout::Node::Pane(self.id(terminal))),
            Node::Split {
                direction,
                ratio,
                first,
                second,
            } => match (self.project(first), self.project(second)) {
                (Some(first), Some(second)) => Some(crate::layout::Node::Split {
                    direction: match direction {
                        Direction::Right => ratatui::layout::Direction::Horizontal,
                        Direction::Down => ratatui::layout::Direction::Vertical,
                    },
                    ratio: *ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (first, second) => first.or(second),
            },
        }
    }

    fn locate(&self, terminal: &str) -> Option<Focus> {
        if !self.layout.shows(terminal) {
            return None;
        }
        self.layout.workspaces.iter().find_map(|workspace| {
            let tab = workspace
                .tabs
                .iter()
                .find(|tab| tab.root.contains(terminal))?;
            Some(Focus {
                workspace: workspace.id.clone(),
                tab: tab.id.clone(),
                terminal: Some(terminal.to_owned()),
            })
        })
    }

    fn reconciled_focus(&self) -> Option<Focus> {
        let previous = self.focus.as_ref();
        let following = previous
            .and_then(|focus| focus.terminal.as_deref())
            .and_then(|terminal| self.locate(terminal));
        if following.is_some() {
            return following;
        }
        let workspace = previous
            .and_then(|focus| {
                self.layout
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == focus.workspace)
            })
            .or_else(|| self.layout.workspaces.first())?;
        let tab = previous
            .and_then(|focus| workspace.tabs.iter().find(|tab| tab.id == focus.tab))
            .or_else(|| workspace.tabs.first())?;
        Some(self.landing(workspace, tab))
    }
}

fn naming(workspace: &Workspace) -> Option<&str> {
    Some(workspace.tabs.first()?.root.first_terminal())
}

fn wrapped(index: usize, step: isize, len: usize) -> usize {
    (index as isize + step).rem_euclid(len.max(1) as isize) as usize
}

fn tab<'a>(layout: &'a Layout, focus: &Focus) -> Option<&'a Tab> {
    layout
        .workspaces
        .iter()
        .find(|workspace| workspace.id == focus.workspace)?
        .tabs
        .iter()
        .find(|tab| tab.id == focus.tab)
}

#[cfg(test)]
mod tests;
