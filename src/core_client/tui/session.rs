use std::collections::HashMap;
use std::path::PathBuf;

use ratatui::layout::Rect;

use super::terminals::{Moved, Report, Status, Terminals};
use crate::api::schema::AgentStatus;
use crate::config::PaneBordersConfig;
use crate::input::KeybindAction;
use crate::layout::{NavDirection, PaneId, PaneInfo, SplitBorder, TileLayout};
use crate::session_doc::{
    Arrangement, Destination, Direction, Layout, Node, Place, Tab, Workspace,
};

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
    Name { terminal: String, name: String },
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
    places: HashMap<String, (usize, usize)>,
    focus: Option<Focus>,
    ids: HashMap<String, PaneId>,
    next_id: u32,
    tab_of_workspace: HashMap<String, String>,
    pane_of_tab: HashMap<String, String>,
    previous_of_tab: HashMap<String, String>,
    zoomed: bool,
    created: Option<String>,
    terminals: Terminals,
    #[cfg(test)]
    pub(super) placements: usize,
    #[cfg(test)]
    pub(super) offstage_walks: usize,
}

impl Session {
    pub(super) fn holds(&self, layout: &Layout) -> bool {
        self.layout == *layout
    }

    pub(super) fn update(&mut self, layout: Layout) {
        self.layout = layout;
        self.places.clear();
        for (index, workspace) in self.layout.workspaces.iter().enumerate() {
            for (tab_index, tab) in workspace.tabs.iter().enumerate() {
                for terminal in tab.root.terminals() {
                    self.places
                        .entry(terminal.to_owned())
                        .or_insert((index, tab_index));
                }
            }
        }
        let places = &self.places;
        self.ids.retain(|terminal, _| places.contains_key(terminal));
        let focus = self.reconciled_focus();
        self.set_focus(focus);
        self.reveal_created();
    }

    pub(super) fn heard(&mut self, report: Report, tiled: impl Fn(&str) -> bool) -> bool {
        let workspaces = &self.layout.workspaces;
        let parked = &self.layout.parked;
        let places = &self.places;
        let naming_panes = matches!(report, Report::Names(_) | Report::Name(..));
        self.terminals.hear(
            report,
            |terminal| {
                workspaces.iter().any(|workspace| {
                    workspace.name.is_none() && naming(workspace) == Some(terminal)
                })
            },
            |terminal| match naming_panes {
                true => tiled(terminal),
                false => !parked.contains(terminal) && places.contains_key(terminal),
            },
            |terminal| parked.contains(terminal),
        )
    }

    pub(super) fn owns(&self, terminal: &str) -> bool {
        self.layout.owns(terminal)
    }

    pub(super) fn moves(&mut self) -> Vec<Moved> {
        let places = &self.places;
        self.terminals
            .moves(|terminal| places.contains_key(terminal))
    }

    pub(super) fn shows(&self, terminal: &str) -> bool {
        self.place(terminal).is_some()
    }

    fn place(&self, terminal: &str) -> Option<(usize, &Workspace, usize, &Tab)> {
        if self.layout.parked.contains(terminal) {
            return None;
        }
        let (index, tab_index) = *self.places.get(terminal)?;
        let workspace = self.layout.workspaces.get(index)?;
        Some((index, workspace, tab_index, workspace.tabs.get(tab_index)?))
    }

    pub(super) fn status(&self, terminal: &str) -> Option<&Status> {
        self.terminals
            .status(terminal)
            .filter(|_| self.shows(terminal))
    }

    pub(super) fn in_focused_tab(&self, terminal: &str) -> bool {
        self.focus.as_ref().is_some_and(|focus| {
            self.place(terminal).is_some_and(|(_, workspace, _, tab)| {
                workspace.id == focus.workspace && tab.id == focus.tab
            })
        })
    }

    pub(super) fn context(&self, terminal: &str) -> Option<String> {
        let (index, workspace, tab_index, tab) = self.place(terminal)?;
        let place = format!("{} · {}", self.label(workspace), index + 1);
        Some(match workspace.tabs.len() > 1 {
            true => format!("{place} · {}", super::chrome::tab_label(tab_index, tab)),
            false => place,
        })
    }

    pub(super) fn parked(&self, terminal: &str) -> bool {
        self.layout.parked.contains(terminal)
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

    pub(super) fn name(&self, terminal: &str) -> Option<&str> {
        self.terminals.name(terminal)
    }

    pub(super) fn folder(&self, terminal: &str) -> Option<&PathBuf> {
        self.terminals.folder(terminal)
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

    pub(super) fn arrangement(&self, action: KeybindAction, area: Rect) -> Option<Arrangement> {
        let focus = self.focus.as_ref()?;
        match action {
            KeybindAction::MoveTabPrevious | KeybindAction::MoveTabNext => {
                Some(Arrangement::MoveTab {
                    tab: focus.tab.clone(),
                    to: match action {
                        KeybindAction::MoveTabNext => Place::Next,
                        _ => Place::Previous,
                    },
                })
            }
            KeybindAction::SwapPaneLeft
            | KeybindAction::SwapPaneDown
            | KeybindAction::SwapPaneUp
            | KeybindAction::SwapPaneRight => {
                let (terminal, toward) = (focus.terminal.clone()?, nav_direction(action)?);
                Some(Arrangement::Swap {
                    terminal,
                    toward,
                    area,
                })
            }
            KeybindAction::ResizePaneLeft
            | KeybindAction::ResizePaneDown
            | KeybindAction::ResizePaneUp
            | KeybindAction::ResizePaneRight => {
                let (terminal, toward) = (focus.terminal.clone()?, nav_direction(action)?);
                Some(Arrangement::Resize {
                    terminal,
                    toward,
                    area,
                })
            }
            _ => None,
        }
    }

    pub(super) fn operation(
        &self,
        action: KeybindAction,
        confirm_close: bool,
        selected: Option<&str>,
    ) -> Option<Request> {
        if action == KeybindAction::NewWorkspace {
            return Some(Request::Run(Operation::Start(Destination::Workspace {
                name: None,
            })));
        }
        let focus = match selected.filter(|_| action == KeybindAction::CloseWorkspace) {
            Some(id) => self.entered(
                self.layout
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == id)?,
            )?,
            None => self.focus.clone()?,
        };
        self.operation_at(&focus, action, confirm_close)
    }

    pub(super) fn operation_on(
        &self,
        terminal: &str,
        action: KeybindAction,
        confirm_close: bool,
    ) -> Option<Request> {
        if !self.on_screen(terminal) {
            return None;
        }
        let focus = Focus {
            terminal: Some(terminal.to_owned()),
            ..self.focus.clone()?
        };
        self.operation_at(&focus, action, confirm_close)
    }

    pub(super) fn on_screen(&self, terminal: &str) -> bool {
        self.focus
            .as_ref()
            .is_some_and(|focus| self.tiled_in(focus, self.zoomed, terminal))
    }

    fn tiled_in(&self, focus: &Focus, zoomed: bool, terminal: &str) -> bool {
        tab(&self.layout, focus).is_some_and(|tab| tab.root.contains(terminal))
            && self.tiling(focus, zoomed)(terminal)
    }

    fn tiling<'a>(&'a self, focus: &'a Focus, zoomed: bool) -> impl Fn(&str) -> bool + 'a {
        move |terminal| {
            !self.layout.parked.contains(terminal)
                && (!zoomed || focus.terminal.as_deref() == Some(terminal))
        }
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

    pub(super) fn between(&self, path: &[bool]) -> Option<(String, String)> {
        let (first, second) = self.layout.split_sides(self.focused()?, path)?;
        Some((first.to_owned(), second.to_owned()))
    }

    pub(super) fn split(
        &self,
        area: Rect,
        direction: ratatui::layout::Direction,
        sides: (&str, &str),
    ) -> Option<SplitBorder> {
        if self.zoomed {
            return None;
        }
        self.layout
            .split_between(self.focused()?, area, direction, sides)
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
        Focus {
            workspace: workspace.id.clone(),
            tab: tab.id.clone(),
            terminal: self.tab_pane(tab).map(str::to_owned),
        }
    }

    pub(super) fn tab_pane<'a>(&'a self, tab: &'a Tab) -> Option<&'a str> {
        let parked = &self.layout.parked;
        self.pane_of_tab
            .get(&tab.id)
            .map(String::as_str)
            .filter(|terminal| tab.root.contains(terminal) && !parked.contains(*terminal))
            .or_else(|| {
                tab.root
                    .find_terminal(&|terminal| !parked.contains(terminal))
            })
    }

    fn shown_in(&self, focus: &Focus, terminal: &str) -> bool {
        Some(terminal) != focus.terminal.as_deref() && self.tiled_in(focus, false, terminal)
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
        self.id_of(self.focused()?)
    }

    pub(super) fn id_of(&self, terminal: &str) -> Option<PaneId> {
        self.ids.get(terminal).copied()
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
        assigned(&mut self.ids, &mut self.next_id, terminal)
    }

    pub(super) fn tiles(&mut self, area: Rect, chrome: PaneChrome) -> Tiles {
        self.arranged(area, chrome, self.zoomed)
    }

    pub(super) fn unzoomed(&mut self, area: Rect, chrome: PaneChrome) -> Tiles {
        self.arranged(area, chrome, false)
    }

    pub(super) fn owned(&mut self) -> HashMap<PaneId, String> {
        let terminals: Vec<String> = self
            .layout
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.tabs)
            .flat_map(|tab| tab.root.terminals())
            .map(str::to_owned)
            .collect();
        terminals
            .into_iter()
            .map(|terminal| (self.id(&terminal), terminal))
            .collect()
    }

    pub(super) fn reachable(&mut self) -> Vec<PaneId> {
        let Some(focus) = self.focus.as_ref() else {
            return Vec::new();
        };
        let terminals: Vec<String> = match (self.zoomed, &focus.terminal) {
            (true, Some(terminal)) => vec![terminal.clone()],
            _ => tab(&self.layout, focus).map_or_else(Vec::new, |tab| {
                tab.root
                    .terminals()
                    .into_iter()
                    .filter(|terminal| !self.layout.parked.contains(*terminal))
                    .map(str::to_owned)
                    .collect()
            }),
        };
        terminals.iter().map(|terminal| self.id(terminal)).collect()
    }

    pub(super) fn offstage(
        &mut self,
        area: impl Fn(&Workspace) -> Rect,
        chrome: PaneChrome,
        needs: impl Fn(PaneId) -> bool,
    ) -> Vec<Tile> {
        #[cfg(test)]
        {
            self.offstage_walks += 1;
        }
        let shown = self
            .focus
            .as_ref()
            .map(|focus| (focus.workspace.as_str(), focus.tab.as_str()));
        let (ids, parked) = (&self.ids, &self.layout.parked);
        let needed = |terminal: &str| ids.get(terminal).is_some_and(|id| needs(*id));
        let mut placing = Vec::new();
        for workspace in &self.layout.workspaces {
            let area = area(workspace);
            for tab in &workspace.tabs {
                let terminals = tab.root.terminals();
                if shown != Some((workspace.id.as_str(), tab.id.as_str()))
                    && terminals
                        .iter()
                        .any(|terminal| !parked.contains(*terminal) && needed(terminal))
                {
                    placing.push((tab.root.clone(), None, area));
                }
                placing.extend(
                    terminals
                        .into_iter()
                        .filter(|terminal| parked.contains(*terminal) && needed(terminal))
                        .map(|terminal| {
                            let alone = Node::Pane {
                                terminal: terminal.to_owned(),
                            };
                            (alone, Some(terminal.to_owned()), area)
                        }),
                );
            }
        }
        placing
            .into_iter()
            .filter_map(|(root, alone, area)| {
                let zoomed = alone.is_some();
                self.placed(&root, alone.as_deref(), zoomed, area, chrome)
            })
            .flat_map(|(tiles, _)| tiles)
            .collect()
    }

    fn arranged(&mut self, area: Rect, chrome: PaneChrome, zoomed: bool) -> Tiles {
        let Some(focus) = self.focus.clone() else {
            return Tiles::default();
        };
        let Some(tab) = tab(&self.layout, &focus).cloned() else {
            return Tiles::default();
        };
        let Some((tiles, layout)) =
            self.placed(&tab.root, focus.terminal.as_deref(), zoomed, area, chrome)
        else {
            return Tiles::default();
        };
        let splits = layout.splits(area);
        let hidden: Vec<&str> = {
            let tiling = self.tiling(&focus, zoomed);
            tab.root
                .terminals()
                .into_iter()
                .filter(|terminal| !self.layout.parked.contains(*terminal) && !tiling(terminal))
                .collect()
        };
        let hidden = hidden
            .into_iter()
            .map(|terminal| (self.id(terminal), terminal.to_owned()))
            .collect();
        Tiles {
            tiles,
            splits,
            hidden,
        }
    }

    fn placed(
        &mut self,
        root: &Node,
        focused: Option<&str>,
        zoomed: bool,
        area: Rect,
        chrome: PaneChrome,
    ) -> Option<(Vec<Tile>, TileLayout)> {
        #[cfg(test)]
        {
            self.placements += 1;
        }
        let focused = focused.map_or(PaneId::from_raw(0), |terminal| self.id(terminal));
        let projected = match zoomed {
            true => Some(crate::layout::Node::Pane(focused)),
            false => self.project(root),
        }?;
        let layout = TileLayout::from_saved(projected, focused);
        let terminals: HashMap<PaneId, &str> = root
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
        let tiles = infos
            .into_iter()
            .filter_map(|mut info| {
                info.inner_rect = crate::ui::pane_inner_rect(info.rect, info.borders);
                Some(Tile {
                    terminal: (*terminals.get(&info.id)?).to_owned(),
                    info,
                })
            })
            .collect();
        Some((tiles, layout))
    }

    fn project(&mut self, node: &Node) -> Option<crate::layout::Node> {
        let (ids, next_id) = (&mut self.ids, &mut self.next_id);
        node.shown(&self.layout.parked, &mut |terminal| {
            assigned(ids, next_id, terminal)
        })
    }

    fn locate(&self, terminal: &str) -> Option<Focus> {
        if !self.shows(terminal) {
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

fn nav_direction(action: KeybindAction) -> Option<NavDirection> {
    match action {
        KeybindAction::SwapPaneLeft | KeybindAction::ResizePaneLeft => Some(NavDirection::Left),
        KeybindAction::SwapPaneDown | KeybindAction::ResizePaneDown => Some(NavDirection::Down),
        KeybindAction::SwapPaneUp | KeybindAction::ResizePaneUp => Some(NavDirection::Up),
        KeybindAction::SwapPaneRight | KeybindAction::ResizePaneRight => Some(NavDirection::Right),
        _ => None,
    }
}

fn assigned(ids: &mut HashMap<String, PaneId>, next_id: &mut u32, terminal: &str) -> PaneId {
    *ids.entry(terminal.to_owned()).or_insert_with(|| {
        *next_id += 1;
        PaneId::from_raw(*next_id)
    })
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
