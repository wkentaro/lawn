use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::select::{Screen, Select};
use crate::copy_mode::{
    copy_mode_command_char, copy_mode_page_lines, first_non_blank_col, last_character_col,
};
use crate::core_client::view::View;
use crate::input::TerminalKey;
use crate::layout::PaneId;
use crate::pane::{ScrollMetrics, TerminalWordMotion};
use crate::protocol::CellData;
use crate::selection::Selection;
use crate::ui::theme::Palette;

#[derive(Clone, Copy)]
enum Anchor {
    Cell((u32, u16)),
    Line(u32),
}

enum Command {
    Escape,
    Quit,
    Yank,
    Select { linewise: bool },
    Move { rows: i8, cols: i8 },
    Page { up: bool, half: bool },
    Edge { top: bool },
    LineStart,
    LineEnd,
    FirstNonBlank,
    Word(TerminalWordMotion),
    Paragraph(i8),
}

impl Command {
    fn of(key: &TerminalKey) -> Option<Self> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        Some(match key.code {
            KeyCode::Esc => Self::Escape,
            KeyCode::Enter => Self::Yank,
            KeyCode::Left => Self::Move { rows: 0, cols: -1 },
            KeyCode::Down => Self::Move { rows: 1, cols: 0 },
            KeyCode::Up => Self::Move { rows: -1, cols: 0 },
            KeyCode::Right => Self::Move { rows: 0, cols: 1 },
            KeyCode::PageUp => Self::Page {
                up: true,
                half: false,
            },
            KeyCode::PageDown => Self::Page {
                up: false,
                half: false,
            },
            KeyCode::Home => Self::LineStart,
            KeyCode::End => Self::LineEnd,
            KeyCode::Char(page @ ('b' | 'f' | 'u' | 'd')) if control => Self::Page {
                up: matches!(page, 'b' | 'u'),
                half: matches!(page, 'u' | 'd'),
            },
            _ => match copy_mode_command_char(key.clone())? {
                'q' => Self::Quit,
                'y' => Self::Yank,
                'v' | ' ' => Self::Select { linewise: false },
                'V' => Self::Select { linewise: true },
                'h' => Self::Move { rows: 0, cols: -1 },
                'j' => Self::Move { rows: 1, cols: 0 },
                'k' => Self::Move { rows: -1, cols: 0 },
                'l' => Self::Move { rows: 0, cols: 1 },
                'g' => Self::Edge { top: true },
                'G' => Self::Edge { top: false },
                '0' => Self::LineStart,
                '$' => Self::LineEnd,
                '^' => Self::FirstNonBlank,
                'w' => Self::Word(TerminalWordMotion::NextStart),
                'b' => Self::Word(TerminalWordMotion::PreviousStart),
                'e' => Self::Word(TerminalWordMotion::NextEnd),
                'W' => Self::Word(TerminalWordMotion::NextBigStart),
                'B' => Self::Word(TerminalWordMotion::PreviousBigStart),
                'E' => Self::Word(TerminalWordMotion::NextBigEnd),
                '{' => Self::Paragraph(-1),
                '}' => Self::Paragraph(1),
                _ => return None,
            },
        })
    }
}

pub(super) enum Step {
    Stay,
    Leave(Option<String>),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Marks {
    pub(super) cursor: Rect,
    pub(super) bar: Rect,
    pub(super) selecting: bool,
}

pub(super) struct CopyMode {
    pane: PaneId,
    screen: Screen,
    cursor: (u32, u16),
    anchor: Option<Anchor>,
    entry: usize,
    history: usize,
}

impl CopyMode {
    pub(super) fn enter(pane: PaneId, view: &View, inner: Rect) -> Option<Self> {
        let _still = view.still();
        if view.frame().is_none() || view.holds_output() {
            return None;
        }
        let metrics = view.scroll_metrics()?;
        let top = top(metrics);
        let cursor = view
            .cursor_cell()
            .filter(|(x, y)| *x < inner.width && *y < inner.height)
            .map_or(
                (
                    top.saturating_add(u32::from(inner.height.saturating_sub(1))),
                    0,
                ),
                |(x, y)| (top.saturating_add(u32::from(y)), x),
            );
        Some(Self {
            pane,
            screen: Screen::of(view, inner),
            cursor,
            anchor: None,
            entry: metrics.offset_from_bottom,
            history: metrics.max_offset_from_bottom,
        })
    }

    pub(super) fn pane(&self) -> PaneId {
        self.pane
    }

    pub(super) fn follow(&mut self, view: &View, inner: Rect, select: &mut Select, active: bool) {
        let _still = view.still();
        self.settle(view, inner, select, active);
    }

    pub(super) fn drop_anchor(&mut self) {
        self.anchor = None;
    }

    pub(super) fn quit(&self, view: &View) {
        let _still = view.still();
        view.scroll_to(self.entry);
    }

    pub(super) fn key(
        &mut self,
        shown: Option<(&View, Rect)>,
        key: &TerminalKey,
        select: &mut Select,
    ) -> Step {
        let Some((view, inner)) = shown else {
            return Step::Stay;
        };
        let _still = view.still();
        let Some(metrics) = self.settle(view, inner, select, true) else {
            return Step::Leave(None);
        };
        match Command::of(key) {
            Some(command) => self.run(command, view, inner, metrics, select),
            None => Step::Stay,
        }
    }

    pub(super) fn marks(
        &self,
        inner: Rect,
        area: Rect,
        drawn: Option<ScrollMetrics>,
        select: &Select,
    ) -> Marks {
        if inner.is_empty() || area.is_empty() {
            return Marks::default();
        }
        let (row, col) = self.cursor;
        let cursor = drawn
            .and_then(|metrics| u16::try_from(row.checked_sub(top(metrics))?).ok())
            .filter(|y| *y < inner.height && col < inner.width)
            .map_or_else(Rect::default, |y| {
                Rect::new(inner.x + col, inner.y + y, 1, 1)
            });
        Marks {
            cursor,
            bar: Rect::new(area.x, area.bottom() - 1, area.width, 1),
            selecting: self.selecting(select),
        }
    }

    fn selecting(&self, select: &Select) -> bool {
        self.anchor.is_some()
            || select
                .shown()
                .is_some_and(|selection| selection.pane_id == self.pane)
    }

    fn settle(
        &mut self,
        view: &View,
        inner: Rect,
        select: &mut Select,
        active: bool,
    ) -> Option<ScrollMetrics> {
        let metrics = view.scroll_metrics()?;
        let screen = Screen::of(view, inner);
        let erased = metrics.max_offset_from_bottom < self.history;
        self.history = metrics.max_offset_from_bottom;
        if screen != self.screen || erased {
            self.screen = screen;
            self.anchor = None;
            if select.pane() == Some(self.pane) {
                select.clear();
            }
            self.cursor = (
                self.cursor.0.min(last_row(metrics, inner.height)),
                self.cursor.1.min(inner.width.saturating_sub(1)),
            );
        }
        if active && select.pane().is_none() {
            self.sync(select, inner.width.saturating_sub(1));
        }
        Some(metrics)
    }

    fn run(
        &mut self,
        command: Command,
        view: &View,
        inner: Rect,
        metrics: ScrollMetrics,
        select: &mut Select,
    ) -> Step {
        let last_col = inner.width.saturating_sub(1);
        let last_row = last_row(metrics, inner.height);
        let (row, col) = self.cursor;
        match command {
            Command::Escape if self.selecting(select) => {
                self.anchor = None;
                select.clear();
                return Step::Stay;
            }
            Command::Escape | Command::Quit => return self.leave(view, select, false),
            Command::Yank => return self.leave(view, select, true),
            Command::Select { linewise } => {
                let (anchor, selection) = if linewise {
                    (
                        Anchor::Line(row),
                        Selection::line_range(self.pane, row, row, last_col),
                    )
                } else {
                    (
                        Anchor::Cell(self.cursor),
                        Selection::absolute_anchor(self.pane, self.cursor),
                    )
                };
                self.anchor = Some(anchor);
                select.hold(selection, self.screen);
                return Step::Stay;
            }
            Command::Move { rows, cols } => {
                self.cursor = (
                    row.saturating_add_signed(i32::from(rows)).min(last_row),
                    col.saturating_add_signed(i16::from(cols)).min(last_col),
                );
                self.reveal(view, inner.height, metrics);
            }
            Command::Page { up, half } => {
                let lines = copy_mode_page_lines(inner.height, half);
                let offset = if up {
                    self.cursor.0 = row.saturating_sub(index(lines));
                    metrics
                        .offset_from_bottom
                        .saturating_add(lines)
                        .min(metrics.max_offset_from_bottom)
                } else {
                    self.cursor.0 = row.saturating_add(index(lines)).min(last_row);
                    metrics.offset_from_bottom.saturating_sub(lines)
                };
                scroll(view, metrics, offset);
            }
            Command::Edge { top } => {
                let offset = if top {
                    self.cursor.0 = 0;
                    metrics.max_offset_from_bottom
                } else {
                    self.cursor.0 = last_row;
                    0
                };
                scroll(view, metrics, offset);
            }
            Command::LineStart => self.cursor.1 = 0,
            Command::LineEnd | Command::FirstNonBlank => {
                let line = Selection::absolute_range(self.pane, (row, 0), (row, last_col));
                if let Some(text) = view.selected_text(&line) {
                    let found = match command {
                        Command::LineEnd => last_character_col(&text),
                        _ => first_non_blank_col(&text),
                    };
                    self.cursor.1 = found.unwrap_or(0).min(last_col);
                }
                self.reveal(view, inner.height, metrics);
            }
            Command::Word(motion) => {
                if let Some((row, col)) = view.word_motion_target(self.cursor, motion) {
                    self.cursor = (row, col.min(last_col));
                }
                self.reveal(view, inner.height, metrics);
            }
            Command::Paragraph(direction) => {
                if let Some(target) = view.paragraph_motion_target(row, direction) {
                    self.cursor.0 = target;
                }
                self.reveal(view, inner.height, metrics);
            }
        }
        self.sync(select, last_col);
        Step::Stay
    }

    fn reveal(&self, view: &View, rows: u16, metrics: ScrollMetrics) {
        let top = top(metrics);
        let reach = u32::from(rows.saturating_sub(1));
        let row = self.cursor.0;
        let wanted = if row < top {
            row
        } else if row > top.saturating_add(reach) {
            row - reach
        } else {
            return;
        };
        let wanted = usize::try_from(wanted).unwrap_or(usize::MAX);
        scroll(
            view,
            metrics,
            metrics.max_offset_from_bottom.saturating_sub(wanted),
        );
    }

    fn sync(&self, select: &mut Select, last_col: u16) {
        let Some(anchor) = self.anchor else {
            return;
        };
        let selection = match anchor {
            Anchor::Cell(anchor) => Selection::absolute_range(self.pane, anchor, self.cursor),
            Anchor::Line(row) => Selection::line_range(self.pane, row, self.cursor.0, last_col),
        };
        select.hold(selection, self.screen);
    }

    fn leave(&self, view: &View, select: &mut Select, copy: bool) -> Step {
        let text = select
            .shown()
            .filter(|_| copy)
            .and_then(|selection| view.selected_text(selection))
            .filter(|text| !text.is_empty());
        select.clear();
        view.scroll_to(self.entry);
        Step::Leave(text)
    }
}

pub(super) fn bar(width: u16, selecting: bool, palette: &Palette) -> Vec<CellData> {
    let area = Rect::new(0, 0, width, 1);
    let mut buffer = Buffer::empty(area);
    let base = Style::default().fg(palette.overlay0).bg(palette.panel_bg);
    buffer.set_style(area, base);
    let key = Style::default()
        .fg(palette.accent)
        .bg(palette.panel_bg)
        .add_modifier(Modifier::BOLD);
    let (select, exit_keys, exit) = if selecting {
        (" selecting  ", "esc", " clear  q exit")
    } else {
        (" select  ", "q/esc", " exit")
    };
    let mut x = 0;
    for (text, style) in [
        (" COPY ", accent(palette)),
        (" ", base),
        ("h/j/k/l w/b/e { }", key),
        (" move  ", base),
        ("v/space", key),
        (select, base),
        ("y/enter", key),
        (" copy  ", base),
        (exit_keys, key),
        (exit, base),
    ] {
        if x >= width {
            break;
        }
        x = buffer
            .set_stringn(x, 0, text, usize::from(width - x), style)
            .0;
    }
    buffer
        .content
        .iter()
        .map(CellData::from_ratatui_cell)
        .collect()
}

pub(super) fn cursor_cell(palette: &Palette) -> CellData {
    let mut cell = Cell::default();
    cell.set_style(accent(palette));
    CellData::from_ratatui_cell(&cell)
}

fn accent(palette: &Palette) -> Style {
    Style::default()
        .fg(crate::ui::chrome::panel_contrast_fg(palette))
        .bg(palette.accent)
        .add_modifier(Modifier::BOLD)
}

fn scroll(view: &View, metrics: ScrollMetrics, offset_from_bottom: usize) {
    if offset_from_bottom != metrics.offset_from_bottom {
        view.scroll_to(offset_from_bottom);
    }
}

fn top(metrics: ScrollMetrics) -> u32 {
    index(
        metrics
            .max_offset_from_bottom
            .saturating_sub(metrics.offset_from_bottom),
    )
}

fn last_row(metrics: ScrollMetrics, rows: u16) -> u32 {
    index(
        metrics
            .max_offset_from_bottom
            .saturating_add(usize::from(rows))
            .saturating_sub(1),
    )
}

fn index(lines: usize) -> u32 {
    u32::try_from(lines).unwrap_or(u32::MAX)
}
