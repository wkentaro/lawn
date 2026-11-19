use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

use super::session::Closing;
use crate::config::LiveKeybindConfig;
use crate::input::TerminalKey;
use crate::pane::ScrollMetrics;
use crate::protocol::{CellData, CursorState};
use crate::ui::text_editor::TextEditor;
use crate::ui::theme::Palette;

const CLOSE_TITLE: &str = "Close workspace?";
const WHEEL_ROWS: usize = 3;
const PAGE_ROWS: isize = 8;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Dialog {
    Confirm(Closing),
    Help(Help),
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
    cancel: Rect,
    scrollbar: Rect,
    metrics: Option<ScrollMetrics>,
    max_scroll: usize,
    cursor: Option<CursorState>,
}

pub(super) struct Modal {
    dialog: Dialog,
    image: Option<(Rect, Vec<CellData>)>,
    hits: Hits,
    grab: Option<u16>,
}

impl Modal {
    pub(super) fn new(dialog: Dialog) -> Self {
        Self {
            dialog,
            image: None,
            hits: Hits::default(),
            grab: None,
        }
    }

    pub(super) fn closing(&self) -> Option<&Closing> {
        match &self.dialog {
            Dialog::Confirm(closing) => Some(closing),
            Dialog::Help(_) => None,
        }
    }

    pub(super) fn edits(&self) -> bool {
        matches!(&self.dialog, Dialog::Help(help) if help.searching)
    }

    pub(super) fn stale(&self, host: Rect) -> bool {
        self.image.as_ref().is_none_or(|(area, _)| *area != host)
    }

    pub(super) fn redraw(&mut self) {
        self.image = None;
    }

    pub(super) fn image(&self) -> Option<&(Rect, Vec<CellData>)> {
        self.image.as_ref()
    }

    pub(super) fn cursor(&self) -> Option<CursorState> {
        self.hits.cursor.clone()
    }

    pub(super) fn text(&mut self, text: &str) {
        self.changing(|dialog, _, _| {
            if let Dialog::Help(help) = dialog {
                if help.searching && help.query.insert(text) {
                    help.scroll = 0;
                }
            }
            Outcome::Stay
        });
    }

    pub(super) fn key(&mut self, key: &TerminalKey) -> Outcome {
        self.changing(|dialog, hits, _| match dialog {
            Dialog::Confirm(_) => match key.code {
                KeyCode::Enter => Outcome::Accept,
                KeyCode::Esc => Outcome::Close,
                _ => Outcome::Stay,
            },
            Dialog::Help(help) => help.key(key, hits.max_scroll),
        })
    }

    pub(super) fn mouse(&mut self, event: MouseEvent) -> Outcome {
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
        })
    }

    pub(super) fn draw(
        &mut self,
        mut buffer: Buffer,
        keybinds: &LiveKeybindConfig,
        palette: &Palette,
    ) {
        crate::ui::dialog::dim_backdrop(&mut buffer);
        self.hits = match &mut self.dialog {
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
        }
        .unwrap_or_default();
        let cells = buffer
            .content
            .iter()
            .map(CellData::from_ratatui_cell)
            .collect();
        self.image = Some((buffer.area, cells));
    }

    fn changing(
        &mut self,
        change: impl FnOnce(&mut Dialog, &Hits, &mut Option<u16>) -> Outcome,
    ) -> Outcome {
        let before = self.dialog.clone();
        let outcome = change(&mut self.dialog, &self.hits, &mut self.grab);
        if self.dialog != before {
            self.image = None;
        }
        outcome
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
