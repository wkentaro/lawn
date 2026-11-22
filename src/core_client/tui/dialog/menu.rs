use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

use super::{Hits, Outcome};
use crate::core_client::tui::session::Session;
use crate::input::{KeybindAction, TerminalKey};
use crate::ui::theme::Palette;

#[derive(Clone, Debug, PartialEq)]
pub(in crate::core_client::tui) enum Aim {
    Workspace(String),
    Tab(String),
    Pane(String),
    Global,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::core_client::tui) enum Item {
    Rename,
    Close,
    NewTab,
    Park,
    RenamePane,
    ClearPaneName,
    SwapWithFocused(String),
    Move,
    SplitRight,
    SplitDown,
    Zoom,
    ClosePane,
    Keybinds,
    ReloadConfig,
    Detach,
}

impl Item {
    pub(in crate::core_client::tui) fn action(&self) -> Option<KeybindAction> {
        match self {
            Item::Keybinds => Some(KeybindAction::Help),
            Item::ReloadConfig => Some(KeybindAction::ReloadConfig),
            Item::Detach => Some(KeybindAction::Detach),
            _ => None,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Item::Rename => "Rename",
            Item::Close => "Close",
            Item::NewTab => "New tab",
            Item::Park => "Park",
            Item::RenamePane => "Rename pane",
            Item::ClearPaneName => "Clear pane name",
            Item::SwapWithFocused(_) => "Swap with focused pane",
            Item::Move => "Move to...",
            Item::SplitRight => "Split right",
            Item::SplitDown => "Split down",
            Item::Zoom => "Zoom",
            Item::ClosePane => "Close pane",
            Item::Keybinds => "keybinds",
            Item::ReloadConfig => "reload config",
            Item::Detach => "detach",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::core_client::tui) struct Menu {
    aim: Aim,
    at: Position,
    highlighted: Option<Item>,
}

impl Menu {
    pub(in crate::core_client::tui) fn new(aim: Aim, at: Position) -> Self {
        Self {
            aim,
            at,
            highlighted: None,
        }
    }

    pub(in crate::core_client::tui) fn launcher() -> Self {
        Self::new(Aim::Global, Position::default())
    }

    pub(super) fn launched(&self) -> bool {
        self.aim == Aim::Global
    }

    pub(super) fn chosen(&self, session: &Session, hits: &Hits) -> Option<(Aim, Item)> {
        let drawn = self.drawn(hits)?;
        (self.on_screen(session) && self.items(session).contains(drawn))
            .then(|| (self.aim.clone(), drawn.clone()))
    }

    pub(in crate::core_client::tui) fn on_screen(&self, session: &Session) -> bool {
        match &self.aim {
            Aim::Workspace(id) => session.workspaces().iter().any(|w| &w.id == id),
            Aim::Tab(id) => session
                .focused_workspace()
                .is_some_and(|workspace| workspace.tabs.iter().any(|tab| &tab.id == id)),
            Aim::Pane(terminal) => session.on_screen(terminal),
            Aim::Global => true,
        }
    }

    pub(super) fn key(&mut self, key: &TerminalKey, hits: &Hits) -> Outcome {
        let step = match key.code {
            KeyCode::Enter => return Outcome::Accept,
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Up => -1,
            KeyCode::Down => 1,
            KeyCode::Char('k') if self.launched() => -1,
            KeyCode::Char('j') if self.launched() => 1,
            _ => return Outcome::Stay,
        };
        let next = self
            .drawn(hits)
            .and_then(|current| hits.items.iter().position(|(_, item)| item == current))
            .map_or(0, |index| index.saturating_add_signed(step));
        if let Some((_, item)) = hits.items.get(next.min(hits.items.len().saturating_sub(1))) {
            self.highlighted = Some(item.clone());
        }
        Outcome::Stay
    }

    pub(super) fn mouse(&mut self, event: MouseEvent, point: Position, hits: &Hits) -> Outcome {
        let row = hits
            .items
            .iter()
            .find(|(rect, _)| rect.contains(point))
            .map(|(_, item)| item);
        match event.kind {
            MouseEventKind::Moved => {
                if let Some(item) = row.filter(|item| self.highlighted.as_ref() != Some(*item)) {
                    self.highlighted = Some(item.clone());
                }
                Outcome::Stay
            }
            MouseEventKind::Down(MouseButton::Left) => match row {
                Some(item) => {
                    self.highlighted = Some(item.clone());
                    Outcome::Accept
                }
                None => Outcome::Close,
            },
            _ => Outcome::Stay,
        }
    }

    pub(super) fn draw(
        &self,
        buffer: &mut Buffer,
        launcher: Option<Rect>,
        palette: &Palette,
        session: &Session,
    ) -> Option<Hits> {
        let items = self.items(session);
        let labels: Vec<&str> = items.iter().map(Item::label).collect();
        let highlighted = match &self.highlighted {
            Some(highlighted) => items.iter().position(|item| item == highlighted),
            None => Some(0),
        };
        let highlighted = highlighted.unwrap_or(usize::MAX);
        let drawn = match self.aim {
            Aim::Global => {
                let items: Vec<(&str, bool)> = labels.iter().map(|label| (*label, false)).collect();
                crate::ui::dialog::render_launcher_menu(
                    buffer,
                    launcher?,
                    &items,
                    highlighted,
                    palette,
                )
            }
            _ => crate::ui::dialog::render_context_menu(
                buffer,
                self.at.x,
                self.at.y,
                &labels,
                highlighted,
                palette,
            ),
        }?;
        Some(Hits {
            area: drawn.area,
            items: drawn
                .rows
                .into_iter()
                .filter(|(rect, _)| rect.width > u16::from(self.launched()))
                .filter_map(|(rect, index)| Some((rect, items.get(index)?.clone())))
                .collect(),
            ..Hits::default()
        })
    }

    fn drawn<'a>(&'a self, hits: &'a Hits) -> Option<&'a Item> {
        let mut drawn = hits.items.iter().map(|(_, item)| item);
        match &self.highlighted {
            Some(highlighted) => drawn.find(|item| *item == highlighted),
            None => drawn.next(),
        }
    }

    fn items(&self, session: &Session) -> Vec<Item> {
        match &self.aim {
            Aim::Workspace(_) => vec![Item::Rename, Item::Close],
            Aim::Tab(_) => vec![Item::NewTab, Item::Rename, Item::Close],
            Aim::Global => vec![Item::Keybinds, Item::ReloadConfig, Item::Detach],
            Aim::Pane(terminal) => {
                let mut items = vec![Item::Park, Item::RenamePane];
                if session.name(terminal).is_some() {
                    items.push(Item::ClearPaneName);
                }
                if let Some(focused) = session.focused().filter(|focused| focused != terminal) {
                    items.push(Item::SwapWithFocused(focused.to_owned()));
                }
                if !session.zoomed() {
                    items.push(Item::Move);
                }
                items.extend([
                    Item::SplitRight,
                    Item::SplitDown,
                    Item::Zoom,
                    Item::ClosePane,
                ]);
                items
            }
        }
    }
}
