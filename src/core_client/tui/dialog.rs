use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

use super::session::{Closing, Operation, Session};
use crate::config::LiveKeybindConfig;
use crate::input::{KeybindAction, TerminalKey};
use crate::pane::ScrollMetrics;
use crate::protocol::{CellData, CursorState};
use crate::session_doc::Arrangement;
use crate::ui::text_editor::TextEditor;
use crate::ui::theme::Palette;

mod picker;

pub(super) use picker::Move;
use picker::Target;

const CLOSE_TITLE: &str = "Close workspace?";
const WHEEL_ROWS: usize = 3;
const PAGE_ROWS: isize = 8;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Dialog {
    Confirm(Closing),
    Help(Help),
    Rename(Rename),
    Move(Move),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Rename {
    title: &'static str,
    input: TextEditor,
    naming: Naming,
    kept: Option<String>,
    edited: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Naming {
    Workspace(String),
    Tab(String),
    Pane(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Help {
    query: TextEditor,
    searching: bool,
    scroll: usize,
}

#[derive(Debug, PartialEq)]
pub(super) enum Outcome {
    Stay,
    Close,
    Accept,
}

#[derive(Default)]
struct Hits {
    area: Rect,
    primary: Rect,
    clear: Rect,
    cancel: Rect,
    scrollbar: Rect,
    metrics: Option<ScrollMetrics>,
    max_scroll: usize,
    rows: Vec<(Rect, Target)>,
    action: Option<Target>,
    cursor: Option<CursorState>,
}

pub(super) struct Modal {
    dialog: Dialog,
    frame: Option<(Rect, Vec<CellData>)>,
    hits: Hits,
    outdated: bool,
    grab: Option<u16>,
    #[cfg(test)]
    copies: usize,
}

impl Modal {
    pub(super) fn new(dialog: Dialog) -> Self {
        Self {
            dialog,
            frame: None,
            hits: Hits::default(),
            outdated: false,
            grab: None,
            #[cfg(test)]
            copies: 0,
        }
    }

    pub(super) fn accepted(&self, session: &Session) -> Option<Operation> {
        match &self.dialog {
            Dialog::Confirm(closing) => session.confirmed(closing),
            Dialog::Rename(rename) => rename.saved(session),
            Dialog::Move(moving) => moving.accepted(session),
            Dialog::Help(_) => None,
        }
    }

    pub(super) fn forget(&mut self) {
        self.frame = None;
        self.hits = Hits {
            max_scroll: self.hits.max_scroll,
            ..Hits::default()
        };
        self.grab = None;
    }

    pub(super) fn session_changed(&mut self) {
        self.outdated |= matches!(self.dialog, Dialog::Move(_));
    }

    pub(super) fn edits(&self) -> bool {
        match &self.dialog {
            Dialog::Help(help) => help.searching,
            Dialog::Rename(_) | Dialog::Move(_) => true,
            Dialog::Confirm(_) => false,
        }
    }

    pub(super) fn stale(&self, host: Rect) -> bool {
        self.outdated || self.frame.as_ref().is_none_or(|frame| frame.0 != host)
    }

    #[cfg(test)]
    pub(super) fn copies(&self) -> usize {
        self.copies
    }

    pub(super) fn redraw(&mut self) {
        self.outdated = true;
    }

    pub(super) fn image(&self) -> Option<&(Rect, Vec<CellData>)> {
        self.frame.as_ref()
    }

    pub(super) fn cursor(&self) -> Option<CursorState> {
        self.hits.cursor.clone()
    }

    pub(super) fn text(&mut self, text: &str, session: &Session) {
        self.changing(|dialog, _, _| {
            match dialog {
                Dialog::Help(help) => {
                    if help.searching && help.query.insert(text) {
                        help.scroll = 0;
                    }
                }
                Dialog::Rename(rename) => rename.edit(|input| {
                    input.insert(text);
                }),
                Dialog::Move(moving) => moving.text(text, session),
                Dialog::Confirm(_) => {}
            }
            Outcome::Stay
        });
    }

    pub(super) fn key(
        &mut self,
        key: &TerminalKey,
        keybinds: &LiveKeybindConfig,
        session: &Session,
    ) -> Outcome {
        self.changing(|dialog, hits, _| match dialog {
            Dialog::Confirm(_) => match key.code {
                KeyCode::Enter => Outcome::Accept,
                KeyCode::Esc => Outcome::Close,
                _ => Outcome::Stay,
            },
            Dialog::Help(help) => help.key(key, hits.max_scroll),
            Dialog::Rename(rename) => rename.key(key),
            Dialog::Move(moving) => moving.key(key, keybinds, session, hits),
        })
    }

    pub(super) fn mouse(&mut self, event: MouseEvent, session: &Session) -> Outcome {
        if event.kind == MouseEventKind::Moved || self.frame.is_none() {
            return Outcome::Stay;
        }
        let point = Position::new(event.column, event.row);
        self.changing(|dialog, hits, grab| match dialog {
            Dialog::Confirm(_) => match event.kind {
                MouseEventKind::Down(MouseButton::Left) if hits.primary.contains(point) => {
                    Outcome::Accept
                }
                MouseEventKind::Down(MouseButton::Left) => Outcome::Close,
                _ => Outcome::Stay,
            },
            Dialog::Help(help) => help.mouse(event, point, hits, grab),
            Dialog::Rename(rename) => match event.kind {
                MouseEventKind::Down(MouseButton::Left) if hits.primary.contains(point) => {
                    Outcome::Accept
                }
                MouseEventKind::Down(MouseButton::Left) if hits.clear.contains(point) => {
                    rename.edit(TextEditor::clear);
                    Outcome::Stay
                }
                MouseEventKind::Down(MouseButton::Left) => Outcome::Close,
                _ => Outcome::Stay,
            },
            Dialog::Move(moving) => moving.mouse(event, point, hits, grab, session),
        })
    }

    pub(super) fn draw(
        &mut self,
        mut buffer: Buffer,
        keybinds: &LiveKeybindConfig,
        palette: &Palette,
        session: &Session,
    ) {
        crate::ui::dialog::dim_backdrop(&mut buffer);
        let hits = match &mut self.dialog {
            Dialog::Confirm(closing) => crate::ui::dialog::render_confirm(
                &mut buffer,
                CLOSE_TITLE,
                closing.detail(),
                palette,
            )
            .map(|drawn| Hits {
                area: drawn.area,
                primary: drawn.primary,
                cancel: drawn.cancel,
                cursor: drawn.cursor,
                ..Hits::default()
            }),
            Dialog::Help(help) => crate::ui::dialog::render_help(
                &mut buffer,
                &help.query,
                help.searching,
                help.scroll,
                keybinds,
                palette,
            )
            .map(|drawn| {
                help.scroll = help.scroll.min(drawn.max_scroll);
                Hits {
                    area: drawn.area,
                    cancel: drawn.close,
                    scrollbar: drawn.scrollbar,
                    metrics: Some(drawn.scroll_metrics),
                    max_scroll: drawn.max_scroll,
                    cursor: drawn.cursor,
                    ..Hits::default()
                }
            }),
            Dialog::Rename(rename) => {
                crate::ui::dialog::render_rename(&mut buffer, rename.title, &rename.input, palette)
                    .map(|drawn| Hits {
                        area: drawn.area,
                        primary: drawn.primary,
                        clear: drawn.clear,
                        cancel: drawn.cancel,
                        cursor: drawn.cursor,
                        ..Hits::default()
                    })
            }
            Dialog::Move(moving) => moving.draw(&mut buffer, palette, session),
        }
        .unwrap_or_default();
        let cells = buffer
            .content
            .iter()
            .map(CellData::from_ratatui_cell)
            .collect();
        self.frame = Some((buffer.area, cells));
        self.hits = hits;
        self.outdated = false;
    }

    fn changing(
        &mut self,
        change: impl FnOnce(&mut Dialog, &Hits, &mut Option<u16>) -> Outcome,
    ) -> Outcome {
        let before = self.dialog.clone();
        #[cfg(test)]
        {
            self.copies += 1;
        }
        let outcome = change(&mut self.dialog, &self.hits, &mut self.grab);
        self.outdated |= self.dialog != before;
        outcome
    }
}

impl Rename {
    pub(super) fn of(
        action: KeybindAction,
        session: &Session,
        selected: Option<&str>,
    ) -> Option<Self> {
        let workspace = session.focused_workspace()?;
        match action {
            KeybindAction::RenameWorkspace => {
                let workspace = session
                    .workspaces()
                    .iter()
                    .find(|workspace| Some(workspace.id.as_str()) == selected)
                    .unwrap_or(workspace);
                let input = TextEditor::new(session.label(workspace), false);
                Some(Self {
                    title: "rename workspace",
                    kept: workspace.name.is_some().then(|| input.as_str().to_owned()),
                    input,
                    naming: Naming::Workspace(workspace.id.clone()),
                    edited: false,
                })
            }
            KeybindAction::RenameTab => {
                let focused = session.focused_tab()?;
                let (index, tab) = workspace
                    .tabs
                    .iter()
                    .enumerate()
                    .find(|(_, tab)| tab.id == focused)?;
                let input = TextEditor::new(&super::chrome::tab_label(index, tab), false);
                Some(Self {
                    title: "rename tab",
                    kept: Some(input.as_str().to_owned()),
                    input,
                    naming: Naming::Tab(tab.id.clone()),
                    edited: false,
                })
            }
            KeybindAction::RenamePane => {
                let terminal = session.focused()?;
                let name = session.name(terminal);
                let input = TextEditor::new(name.unwrap_or_default(), name.is_none());
                Some(Self {
                    title: "rename pane",
                    kept: (name.is_none() || !input.is_empty()).then(|| input.as_str().to_owned()),
                    input,
                    naming: Naming::Pane(terminal.to_owned()),
                    edited: false,
                })
            }
            _ => None,
        }
    }

    fn key(&mut self, key: &TerminalKey) -> Outcome {
        match key.code {
            KeyCode::Enter => return Outcome::Accept,
            KeyCode::Esc => return Outcome::Close,
            _ => {}
        }
        if key
            .generated_text
            .as_deref()
            .is_some_and(|text| !text.is_empty())
        {
            self.edit(|input| {
                input.handle_key(key);
            });
        } else if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL
            || key.code == KeyCode::Backspace && key.modifiers.contains(KeyModifiers::SUPER)
        {
            self.edit(TextEditor::clear);
        } else {
            self.edit(|input| {
                input.handle_key(key);
            });
        }
        Outcome::Stay
    }

    fn edit(&mut self, change: impl FnOnce(&mut TextEditor)) {
        let before = self.input.as_str().to_owned();
        change(&mut self.input);
        self.edited |= self.input.as_str() != before;
    }

    fn saved(&self, session: &Session) -> Option<Operation> {
        let name = self.input.trim();
        let untouched = |kept: &String| kept == self.input.as_str() || kept == name;
        let unedited_pane = !self.edited && matches!(self.naming, Naming::Pane(_));
        if unedited_pane || self.kept.as_ref().is_some_and(untouched) {
            return None;
        }
        let arrangement = match &self.naming {
            Naming::Pane(terminal) => {
                return session.owns(terminal).then(|| Operation::Name {
                    terminal: terminal.clone(),
                    name: name.to_owned(),
                })
            }
            _ if name.is_empty() => return None,
            Naming::Workspace(workspace) => Arrangement::NameWorkspace {
                workspace: workspace.clone(),
                name: name.to_owned(),
            },
            Naming::Tab(id) => Arrangement::NameTab {
                tab: id.clone(),
                name: name.to_owned(),
            },
        };
        Some(Operation::Arrange(arrangement))
    }
}

impl Help {
    fn key(&mut self, key: &TerminalKey, max_scroll: usize) -> Outcome {
        let typed = crate::input::keybind_help_text_char(key);
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        if self.searching {
            if let Some(changed) = self.query.handle_key(key) {
                if changed {
                    self.scroll = 0;
                }
                return Outcome::Stay;
            }
            let step = match code {
                KeyCode::Esc => {
                    self.leave_search();
                    None
                }
                KeyCode::Enter => return Outcome::Close,
                KeyCode::Char('p') if modifiers == KeyModifiers::CONTROL => Some(-1),
                KeyCode::Char('n') if modifiers == KeyModifiers::CONTROL => Some(1),
                KeyCode::Char(_) => None,
                code => step(code),
            };
            if let Some(step) = step {
                self.scroll_by(step, max_scroll);
            }
            return Outcome::Stay;
        }
        match code {
            KeyCode::Esc | KeyCode::Enter => return Outcome::Close,
            KeyCode::Home => self.scroll = 0,
            KeyCode::End => self.scroll = max_scroll,
            KeyCode::Char('k') => self.scroll_by(-1, max_scroll),
            KeyCode::Char('j') => self.scroll_by(1, max_scroll),
            _ if typed == Some('/') => {
                self.searching = true;
                self.scroll = 0;
            }
            _ if typed == Some('?') => return Outcome::Close,
            code => {
                if let Some(step) = step(code) {
                    self.scroll_by(step, max_scroll);
                }
            }
        }
        Outcome::Stay
    }

    fn mouse(
        &mut self,
        event: MouseEvent,
        point: Position,
        hits: &Hits,
        grab: &mut Option<u16>,
    ) -> Outcome {
        match event.kind {
            MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_sub(WHEEL_ROWS),
            MouseEventKind::ScrollDown => {
                self.scroll = self.scroll.saturating_add(WHEEL_ROWS).min(hits.max_scroll);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                *grab = None;
                if hits.scrollbar.contains(point) {
                    let Some(metrics) = hits.metrics else {
                        return Outcome::Stay;
                    };
                    match crate::ui::scrollbar_thumb_grab_offset(metrics, hits.scrollbar, point.y) {
                        Some(offset) => *grab = Some(offset),
                        None => {
                            let offset = crate::ui::scrollbar_offset_from_row(
                                metrics,
                                hits.scrollbar,
                                point.y,
                            );
                            self.scroll = metrics.max_offset_from_bottom.saturating_sub(offset);
                        }
                    }
                } else if hits.cancel.contains(point) {
                    if !self.searching {
                        return Outcome::Close;
                    }
                    self.leave_search();
                } else if !hits.area.contains(point) {
                    return Outcome::Close;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let (Some(offset), Some(metrics)) = (*grab, hits.metrics) {
                    let from_bottom = crate::ui::scrollbar_offset_from_drag_row(
                        metrics,
                        hits.scrollbar,
                        point.y,
                        offset,
                    );
                    self.scroll = metrics.max_offset_from_bottom.saturating_sub(from_bottom);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => *grab = None,
            _ => {}
        }
        Outcome::Stay
    }

    fn leave_search(&mut self) {
        self.searching = false;
        self.query.clear();
        self.scroll = 0;
    }

    fn scroll_by(&mut self, step: isize, max_scroll: usize) {
        self.scroll = self.scroll.saturating_add_signed(step).min(max_scroll);
    }
}

fn step(code: KeyCode) -> Option<isize> {
    match code {
        KeyCode::Up => Some(-1),
        KeyCode::Down => Some(1),
        KeyCode::PageUp => Some(-PAGE_ROWS),
        KeyCode::PageDown => Some(PAGE_ROWS),
        _ => None,
    }
}
