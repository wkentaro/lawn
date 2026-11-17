use std::collections::HashMap;

use ratatui::layout::Rect;

use crate::config::PaneBordersConfig;
use crate::layout::{PaneId, PaneInfo, SplitBorder, TileLayout};
use crate::session_doc::{Direction, Layout, Node, Tab};

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
}

#[derive(Default)]
pub(super) struct Session {
    layout: Layout,
    focus: Option<Focus>,
    ids: HashMap<String, PaneId>,
    next_id: u32,
}

impl Session {
    pub(super) fn update(&mut self, layout: Layout) {
        self.layout = layout;
        self.ids.retain(|terminal, _| self.layout.owns(terminal));
        self.focus = self.reconciled_focus();
    }

    pub(super) fn focused_id(&self) -> Option<PaneId> {
        self.ids.get(self.focused()?).copied()
    }

    pub(super) fn focused(&self) -> Option<&str> {
        self.focus.as_ref()?.terminal.as_deref()
    }

    pub(super) fn focus(&mut self, terminal: &str) -> bool {
        let Some(focus) = self.focus.as_mut() else {
            return false;
        };
        let shown_here = self.layout.shows(terminal)
            && tab(&self.layout, focus).is_some_and(|tab| tab.root.contains(terminal));
        if !shown_here || focus.terminal.as_deref() == Some(terminal) {
            return false;
        }
        focus.terminal = Some(terminal.to_owned());
        true
    }

    pub(super) fn id(&mut self, terminal: &str) -> PaneId {
        let next_id = &mut self.next_id;
        *self.ids.entry(terminal.to_owned()).or_insert_with(|| {
            *next_id += 1;
            PaneId::from_raw(*next_id)
        })
    }

    pub(super) fn tiles(&mut self, area: Rect, chrome: PaneChrome) -> Tiles {
        let Some(focus) = self.focus.clone() else {
            return Tiles::default();
        };
        let Some(tab) = tab(&self.layout, &focus).cloned() else {
            return Tiles::default();
        };
        let Some(root) = self.project(&tab.root) else {
            return Tiles::default();
        };
        let focused = focus
            .terminal
            .as_deref()
            .map_or(PaneId::from_raw(0), |terminal| self.id(terminal));
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

    fn reconciled_focus(&self) -> Option<Focus> {
        let parked = &self.layout.parked;
        let previous = self.focus.as_ref();
        let following = previous
            .and_then(|focus| focus.terminal.as_deref())
            .filter(|terminal| self.layout.shows(terminal))
            .and_then(|terminal| {
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
            });
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
        Some(Focus {
            workspace: workspace.id.clone(),
            tab: tab.id.clone(),
            terminal: tab
                .root
                .terminals()
                .into_iter()
                .find(|terminal| !parked.contains(*terminal))
                .map(str::to_owned),
        })
    }
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
