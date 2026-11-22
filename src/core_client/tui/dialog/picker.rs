use crossterm::event::{
    KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Position;

use super::{Hits, Outcome, PAGE_ROWS};
use crate::config::LiveKeybindConfig;
use crate::core_client::tui::session::{Operation, Session};
use crate::input::{KeybindAction, KeybindMatch, TerminalKey};
use crate::session_doc::{Arrangement, Destination, Direction, Tab, Workspace};
use crate::ui::dialog::{
    Navigator, NavigatorListing, NavigatorMove, NavigatorRow, NavigatorRowKind,
};
use crate::ui::text_editor::TextEditor;
use crate::ui::theme::Palette;

const WHEEL_STEP: isize = 3;

#[derive(Clone, Debug, PartialEq)]
pub(in crate::core_client::tui) struct Move {
    terminal: String,
    query: TextEditor,
    selected: Option<Target>,
    scroll: Option<usize>,
    prefixed: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Choice {
    target: Target,
    label: String,
    workspace: String,
    detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Workspace(String),
    Tab(String),
    NewWorkspace,
}

impl Move {
    pub(in crate::core_client::tui) fn of(session: &Session) -> Option<Self> {
        let terminal = session.focused()?.to_owned();
        title(session, &terminal)?;
        let workspace = session.focused_workspace()?.id.clone();
        Some(Self {
            terminal,
            query: TextEditor::default(),
            selected: Some(Target::Workspace(workspace)),
            scroll: None,
            prefixed: false,
        })
    }

    pub(super) fn text(&mut self, text: &str, session: &Session) {
        if self.query.insert(text) {
            self.searched(session);
        }
    }

    pub(super) fn key(
        &mut self,
        key: &TerminalKey,
        keybinds: &LiveKeybindConfig,
        session: &Session,
        hits: &Hits,
    ) -> Outcome {
        let moves = |binding: Option<KeybindMatch>| {
            matches!(binding, Some(KeybindMatch::Action(KeybindAction::MovePane)))
        };
        if key.kind == KeyEventKind::Repeat && (self.prefixed || keybinds.matches_prefix(key)) {
            return Outcome::Stay;
        }
        if std::mem::take(&mut self.prefixed) {
            let closes = key.code == KeyCode::Esc
                || moves(crate::input::resolve_prefix_binding(
                    &keybinds.keybinds,
                    key,
                ));
            return if closes {
                Outcome::Close
            } else {
                Outcome::Stay
            };
        }
        if moves(crate::input::resolve_direct_binding(
            &keybinds.keybinds,
            key,
        )) {
            return Outcome::Close;
        }
        if keybinds.matches_prefix(key) {
            self.prefixed = true;
            return Outcome::Stay;
        }
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        match code {
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Enter => {
                let drawn = self
                    .selected
                    .clone()
                    .filter(|selected| hits.rows.iter().any(|(_, target)| target == selected));
                return drawn.map_or(Outcome::Stay, |target| self.accept(target, session));
            }
            _ => {}
        }
        if let Some(changed) = self.query.handle_key(key) {
            if changed {
                self.searched(session);
            }
            return Outcome::Stay;
        }
        let step = match code {
            KeyCode::Up => -1,
            KeyCode::Down => 1,
            KeyCode::PageUp => -PAGE_ROWS,
            KeyCode::PageDown => PAGE_ROWS,
            KeyCode::Char('p') if modifiers == KeyModifiers::CONTROL => -1,
            KeyCode::Char('n') if modifiers == KeyModifiers::CONTROL => 1,
            _ => return Outcome::Stay,
        };
        self.step(step, session);
        Outcome::Stay
    }

    pub(super) fn mouse(
        &mut self,
        event: MouseEvent,
        point: Position,
        hits: &Hits,
        grab: &mut Option<u16>,
        session: &Session,
    ) -> Outcome {
        match event.kind {
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
                            self.scroll_to(
                                metrics.max_offset_from_bottom.saturating_sub(offset),
                                metrics.viewport_rows,
                                session,
                            );
                        }
                    }
                } else if hits.cancel.contains(point) {
                    return Outcome::Close;
                } else if let Some(target) = hits
                    .action
                    .as_ref()
                    .filter(|_| hits.primary.contains(point))
                {
                    return self.accept(target.clone(), session);
                } else if let Some((_, target)) =
                    hits.rows.iter().find(|(rect, _)| rect.contains(point))
                {
                    self.selected = Some(target.clone());
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
                    self.scroll_to(
                        metrics.max_offset_from_bottom.saturating_sub(from_bottom),
                        metrics.viewport_rows,
                        session,
                    );
                }
            }
            MouseEventKind::Up(MouseButton::Left) => *grab = None,
            MouseEventKind::ScrollUp => self.step(-WHEEL_STEP, session),
            MouseEventKind::ScrollDown => self.step(WHEEL_STEP, session),
            _ => {}
        }
        Outcome::Stay
    }

    pub(super) fn draw(
        &self,
        buffer: &mut Buffer,
        palette: &Palette,
        session: &Session,
    ) -> Option<Hits> {
        let shown = self.shown(session);
        let label = title(session, &self.terminal).unwrap_or_default();
        let rows: Vec<NavigatorRow> = shown
            .iter()
            .map(|choice| NavigatorRow {
                kind: match choice.target {
                    Target::Workspace(_) => NavigatorRowKind::Workspace,
                    Target::Tab(_) => NavigatorRowKind::Tab,
                    Target::NewWorkspace => NavigatorRowKind::NewWorkspace,
                },
                depth: u8::from(matches!(choice.target, Target::Tab(_))),
                label: &choice.label,
                meta: "",
                detail: &choice.detail,
                agent: None,
                status: None,
                stale: false,
                current: false,
                parked: false,
                moving: false,
            })
            .collect();
        let drawn = crate::ui::dialog::render_navigator(
            buffer,
            &Navigator {
                listing: NavigatorListing::Tabs,
                query: &self.query,
                parked: false,
                moving: Some(NavigatorMove {
                    label: &label,
                    tab: false,
                }),
                scroll: self.scroll,
                selected: self.selected_index(&shown),
                rows: &rows,
                background: &|_| 0,
            },
            palette,
        )?;
        let target = |index: usize| shown.get(index).map(|choice| choice.target.clone());
        Some(Hits {
            area: drawn.area,
            cancel: drawn.close,
            scrollbar: drawn.scrollbar,
            metrics: drawn.scroll_metrics,
            cursor: drawn.cursor,
            primary: drawn.preview,
            rows: drawn
                .rows
                .iter()
                .filter_map(|(rect, index)| Some((*rect, target(*index)?)))
                .collect(),
            action: self.selected_index(&shown).and_then(target),
            ..Hits::default()
        })
    }

    pub(super) fn accepted(&self, session: &Session) -> Option<Operation> {
        if session.zoomed() || !session.shows(&self.terminal) {
            return None;
        }
        let shown = self.shown(session);
        let target = &shown.get(self.selected_index(&shown)?)?.target;
        let (to, into) = match target {
            Target::Workspace(id) => (
                Destination::Tab {
                    workspace: Some(id.clone()),
                    name: None,
                },
                None,
            ),
            Target::Tab(id) => {
                let tab = session
                    .workspaces()
                    .iter()
                    .flat_map(|workspace| workspace.tabs.iter())
                    .find(|tab| tab.id == *id && !tab.root.contains(&self.terminal))?;
                let beside = session.tab_pane(tab)?.to_owned();
                (
                    Destination::Split {
                        beside,
                        direction: Direction::Right,
                        ratio: 0.5,
                    },
                    Some(id.clone()),
                )
            }
            Target::NewWorkspace => (
                Destination::Workspace {
                    name: session.name(&self.terminal).map(str::to_owned),
                },
                None,
            ),
        };
        Some(Operation::Arrange(Arrangement::MovePane {
            terminal: self.terminal.clone(),
            to,
            tab: into,
        }))
    }

    fn shown(&self, session: &Session) -> Vec<Choice> {
        let query = self.query.trim().to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        let matches = |names: &[&str]| {
            if words.is_empty() {
                return true;
            }
            let names: Vec<String> = names.iter().map(|name| name.to_lowercase()).collect();
            words
                .iter()
                .all(|word| names.iter().any(|name| name.contains(word)))
        };
        let mut shown = Vec::new();
        let mut header = None;
        for choice in choices(session, &self.terminal) {
            match choice.target {
                Target::Workspace(_) if matches(&[&choice.label]) => {
                    header = None;
                    shown.push(choice);
                }
                Target::Workspace(_) => header = Some(choice),
                Target::Tab(_) if matches(&[&choice.workspace, &choice.label]) => {
                    shown.extend(header.take());
                    shown.push(choice);
                }
                Target::Tab(_) => {}
                Target::NewWorkspace => shown.push(choice),
            }
        }
        shown
    }

    fn accept(&mut self, target: Target, session: &Session) -> Outcome {
        if !self
            .shown(session)
            .iter()
            .any(|choice| choice.target == target)
        {
            return Outcome::Stay;
        }
        self.selected = Some(target);
        Outcome::Accept
    }

    fn selected_index(&self, shown: &[Choice]) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        shown.iter().position(|choice| choice.target == *selected)
    }

    fn searched(&mut self, session: &Session) {
        let shown = self.shown(session);
        self.selected = shown
            .iter()
            .find(|choice| matches!(choice.target, Target::Tab(_)))
            .or(shown.first())
            .map(|choice| choice.target.clone());
        self.scroll = None;
    }

    fn step(&mut self, by: isize, session: &Session) {
        let shown = self.shown(session);
        let last = shown.len().saturating_sub(1);
        let next = self
            .selected_index(&shown)
            .map_or(0, |index| index.saturating_add_signed(by).min(last));
        self.selected = shown.get(next).map(|choice| choice.target.clone());
    }

    fn scroll_to(&mut self, scroll: usize, viewport: usize, session: &Session) {
        let shown = self.shown(session);
        let viewport = viewport.max(1);
        let scroll = scroll.min(shown.len().saturating_sub(viewport));
        let selected = self
            .selected_index(&shown)
            .unwrap_or_default()
            .clamp(scroll, scroll + viewport - 1)
            .min(shown.len().saturating_sub(1));
        self.selected = shown.get(selected).map(|choice| choice.target.clone());
        self.scroll = Some(scroll);
    }
}

fn choices(session: &Session, terminal: &str) -> Vec<Choice> {
    let mut choices = Vec::new();
    for workspace in session.workspaces() {
        let label = session.label(workspace);
        choices.push(Choice {
            target: Target::Workspace(workspace.id.clone()),
            label: label.to_owned(),
            workspace: String::new(),
            detail: String::new(),
        });
        for (index, tab) in workspace.tabs.iter().enumerate() {
            if tab.root.contains(terminal) || session.tab_pane(tab).is_none() {
                continue;
            }
            let tab_label = crate::core_client::tui::chrome::tab_label(index, tab);
            choices.push(Choice {
                target: Target::Tab(tab.id.clone()),
                detail: format!("{label} / {tab_label}"),
                workspace: label.to_owned(),
                label: tab_label,
            });
        }
    }
    choices.push(Choice {
        target: Target::NewWorkspace,
        label: "+ New workspace".to_owned(),
        workspace: String::new(),
        detail: String::new(),
    });
    choices
}

fn title(session: &Session, terminal: &str) -> Option<String> {
    let (workspace, index, tab) = located(session.workspaces(), terminal)?;
    let shown: Vec<&str> = tab
        .root
        .terminals()
        .into_iter()
        .filter(|shown| !session.parked(shown))
        .collect();
    let name = session.name(terminal);
    let tab_name = tab.name.as_deref().filter(|name| !name.is_empty());
    let position = shown.iter().position(|shown| *shown == terminal)?;
    if shown.len() == 1 {
        return Some(match name.or(tab_name) {
            Some(label) => label.to_owned(),
            None if workspace.tabs.len() > 1 => format!(
                "terminal · {}",
                crate::core_client::tui::chrome::tab_label(index, tab)
            ),
            None => session.label(workspace).to_owned(),
        });
    }
    let pane_name = name.unwrap_or("terminal");
    Some(match tab_name {
        Some(tab_name) if tab_name != pane_name => {
            format!("{tab_name} · {pane_name} · {}", position + 1)
        }
        _ => format!("{pane_name} · {}", position + 1),
    })
}

fn located<'a>(
    workspaces: &'a [Workspace],
    terminal: &str,
) -> Option<(&'a Workspace, usize, &'a Tab)> {
    workspaces.iter().find_map(|workspace| {
        let (index, tab) = workspace
            .tabs
            .iter()
            .enumerate()
            .find(|(_, tab)| tab.root.contains(terminal))?;
        Some((workspace, index, tab))
    })
}
