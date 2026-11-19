use std::collections::BTreeSet;

#[cfg(any(unix, test))]
use ratatui::layout::Rect;

use super::{Direction, Layout, Node};
#[cfg(any(unix, test))]
use crate::layout::{NavDirection, PaneInfo, SplitBorder};
use crate::layout::{PaneId, TileLayout};

struct Tiled<'a> {
    layout: TileLayout,
    shown: Vec<&'a str>,
}

impl Node {
    pub(crate) fn shown<'a>(
        &'a self,
        parked: &BTreeSet<String>,
        id: &mut impl FnMut(&'a str) -> PaneId,
    ) -> Option<crate::layout::Node> {
        match self {
            Node::Pane { terminal } if parked.contains(terminal) => None,
            Node::Pane { terminal } => Some(crate::layout::Node::Pane(id(terminal))),
            Node::Split {
                direction,
                ratio,
                first,
                second,
            } => match (first.shown(parked, id), second.shown(parked, id)) {
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
}

impl Layout {
    #[cfg(any(unix, test))]
    pub(crate) fn neighbour(
        &self,
        terminal: &str,
        toward: NavDirection,
        area: Rect,
    ) -> Option<&str> {
        let tiled = self.tiled(terminal)?;
        let panes = tiled.layout.panes(area);
        let to = crate::layout::find_in_direction(tiled.focused(&panes)?, toward, &panes)?;
        tiled.terminal(to)
    }

    #[cfg(any(unix, test))]
    pub(crate) fn resized_split(
        &self,
        terminal: &str,
        toward: NavDirection,
        area: Rect,
    ) -> Option<(&str, &str)> {
        let tiled = self.tiled(terminal)?;
        let rect = tiled.focused(&tiled.layout.panes(area))?.rect;
        let axis = match toward {
            NavDirection::Left | NavDirection::Right => ratatui::layout::Direction::Horizontal,
            NavDirection::Up | NavDirection::Down => ratatui::layout::Direction::Vertical,
        };
        let splits = tiled.layout.splits(area);
        let split =
            crate::layout::nearest_resize_split(&splits, axis, rect, toward).or_else(|| {
                crate::layout::nearest_resize_split(
                    &splits,
                    axis,
                    rect,
                    crate::layout::opposite_direction(toward),
                )
            })?;
        tiled.sides(tiled.layout.root(), &split.path)
    }

    pub(crate) fn split_sides(&self, terminal: &str, path: &[bool]) -> Option<(&str, &str)> {
        let tiled = self.tiled(terminal)?;
        tiled.sides(tiled.layout.root(), path)
    }

    #[cfg(any(unix, test))]
    pub(crate) fn split_between(
        &self,
        terminal: &str,
        area: Rect,
        direction: ratatui::layout::Direction,
        sides: (&str, &str),
    ) -> Option<SplitBorder> {
        let tiled = self.tiled(terminal)?;
        tiled.layout.splits(area).into_iter().find(|split| {
            split.direction == direction
                && tiled.sides(tiled.layout.root(), &split.path) == Some(sides)
        })
    }

    fn tiled<'a>(&'a self, terminal: &str) -> Option<Tiled<'a>> {
        if self.parked.contains(terminal) {
            return None;
        }
        let tab = self
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.tabs)
            .find(|tab| tab.root.contains(terminal))?;
        let mut shown = Vec::new();
        let root = tab.root.shown(&self.parked, &mut |terminal| {
            shown.push(terminal);
            PaneId::from_raw(shown.len() as u32 - 1)
        })?;
        let focused = shown.iter().position(|shown| *shown == terminal)?;
        Some(Tiled {
            layout: TileLayout::from_saved(root, PaneId::from_raw(focused as u32)),
            shown,
        })
    }
}

impl<'a> Tiled<'a> {
    #[cfg(any(unix, test))]
    fn focused<'p>(&self, panes: &'p [PaneInfo]) -> Option<&'p PaneInfo> {
        let focused = self.layout.focused();
        panes.iter().find(|pane| pane.id == focused)
    }

    fn terminal(&self, id: PaneId) -> Option<&'a str> {
        self.shown.get(id.raw() as usize).copied()
    }

    fn sides(&self, node: &crate::layout::Node, path: &[bool]) -> Option<(&'a str, &'a str)> {
        let crate::layout::Node::Split { first, second, .. } = node else {
            return None;
        };
        match path.split_first() {
            None => Some((
                self.terminal(edge(first, true))?,
                self.terminal(edge(second, false))?,
            )),
            Some((false, rest)) => self.sides(first, rest),
            Some((true, rest)) => self.sides(second, rest),
        }
    }
}

fn edge(node: &crate::layout::Node, last: bool) -> PaneId {
    match node {
        crate::layout::Node::Pane(id) => *id,
        crate::layout::Node::Split { second, .. } if last => edge(second, last),
        crate::layout::Node::Split { first, .. } => edge(first, last),
    }
}
