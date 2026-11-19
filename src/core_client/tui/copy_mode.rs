use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::select::{Screen, Select};
use crate::copy_mode::{
    copy_mode_command_char, copy_mode_page_lines, first_non_blank_col, last_character_col,
};
use crate::core_client::view::{Frame, View};
use crate::input::TerminalKey;
use crate::layout::PaneId;
use crate::pane::{
    ScrollMetrics, TerminalSearchDirection, TerminalSearchWindow, TerminalTextPoint,
    TerminalWordMotion,
};
use crate::protocol::CellData;
use crate::selection::Selection;
use crate::ui::text_editor::{self, TextEditor};
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
    Search(TerminalSearchDirection),
    Repeat { reverse: bool },
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
                '/' => Self::Search(TerminalSearchDirection::Forward),
                '?' => Self::Search(TerminalSearchDirection::Backward),
                'n' => Self::Repeat { reverse: false },
                'N' => Self::Repeat { reverse: true },
                _ => return None,
            },
        })
    }
}

pub(super) enum Step {
    Stay,
    Leave(Option<String>),
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct Marks {
    pub(super) cursor: Rect,
    pub(super) bar: Rect,
    pub(super) selecting: bool,
    pub(super) matches: Vec<Rect>,
    pub(super) current: Vec<Rect>,
    query: bool,
    count: Option<(usize, usize)>,
    prompt: Option<(TerminalSearchDirection, TextEditor)>,
}

struct Found {
    revision: u64,
    window: TerminalSearchWindow,
}

pub(super) struct CopyMode {
    pane: PaneId,
    screen: Screen,
    cursor: (u32, u16),
    anchor: Option<Anchor>,
    entry: usize,
    history: usize,
    prompt: Option<(TerminalSearchDirection, TextEditor)>,
    query: String,
    direction: Option<TerminalSearchDirection>,
    found: Option<Found>,
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
            prompt: None,
            query: String::new(),
            direction: None,
            found: None,
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

    pub(super) fn prompting(&self) -> bool {
        self.prompt.is_some()
    }

    pub(super) fn type_text(&mut self, text: &str) {
        if let Some((_, editor)) = self.prompt.as_mut() {
            editor.insert(text);
        }
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
        if let Some((_, editor)) = self.prompt.as_mut() {
            match key.code {
                KeyCode::Enter => {}
                KeyCode::Esc => {
                    self.prompt = None;
                    return Step::Stay;
                }
                _ => {
                    editor.handle_key(key);
                    return Step::Stay;
                }
            }
        }
        let Some((view, inner)) = shown else {
            return Step::Stay;
        };
        let _still = view.still();
        let Some(metrics) = self.settle(view, inner, select, true) else {
            return Step::Leave(None);
        };
        if let Some((direction, editor)) = self.prompt.take() {
            self.search(
                view,
                inner,
                select,
                editor.as_str().to_owned(),
                direction,
                false,
            );
            return Step::Stay;
        }
        match Command::of(key) {
            Some(command) => self.run(command, view, inner, metrics, select),
            None => Step::Stay,
        }
    }

    pub(super) fn marks(
        &self,
        inner: Rect,
        area: Rect,
        drawn: Option<Frame>,
        select: &Select,
        active: bool,
    ) -> Marks {
        if inner.is_empty() || area.is_empty() {
            return Marks::default();
        }
        let scroll = drawn.and_then(|frame| frame.scroll);
        let found = self
            .found
            .as_ref()
            .filter(|found| drawn.is_some_and(|frame| frame.revision == found.revision));
        let (matches, current) = found
            .zip(scroll)
            .map(|(found, metrics)| highlights(&found.window, metrics, inner))
            .unwrap_or_default();
        if !active {
            return Marks {
                matches,
                current,
                ..Marks::default()
            };
        }
        let (row, col) = self.cursor;
        let cursor = scroll
            .and_then(|metrics| u16::try_from(row.checked_sub(top(metrics))?).ok())
            .filter(|y| *y < inner.height && col < inner.width)
            .map_or_else(Rect::default, |y| {
                Rect::new(inner.x + col, inner.y + y, 1, 1)
            });
        Marks {
            cursor,
            bar: Rect::new(area.x, area.bottom() - 1, area.width, 1),
            selecting: self.selecting(select),
            matches,
            current,
            query: !self.query.is_empty(),
            count: found
                .and_then(|found| Some((found.window.current_global? + 1, found.window.total))),
            prompt: self.prompt.clone(),
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
        let drawn = view.frame().map(|frame| frame.revision);
        if self
            .found
            .as_ref()
            .is_some_and(|found| Some(found.revision) != drawn)
        {
            self.found = None;
        }
        if screen != self.screen || erased {
            self.screen = screen;
            self.anchor = None;
            self.found = None;
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
            Command::Escape if self.selecting(select) || !self.query.is_empty() => {
                self.anchor = None;
                select.clear();
                self.query.clear();
                self.direction = None;
                self.found = None;
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
            Command::Search(direction) => {
                self.prompt = Some((direction, TextEditor::default()));
                return Step::Stay;
            }
            Command::Repeat { reverse } => {
                if let Some(direction) = self.direction {
                    let direction = match (direction, reverse) {
                        (TerminalSearchDirection::Forward, true) => {
                            TerminalSearchDirection::Backward
                        }
                        (TerminalSearchDirection::Backward, true) => {
                            TerminalSearchDirection::Forward
                        }
                        (direction, false) => direction,
                    };
                    self.search(view, inner, select, self.query.clone(), direction, true);
                }
                return Step::Stay;
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

    fn search(
        &mut self,
        view: &View,
        inner: Rect,
        select: &mut Select,
        query: String,
        direction: TerminalSearchDirection,
        repeat: bool,
    ) {
        let revision = view.content_revision();
        if query.is_empty() || view.frame().map(|frame| frame.revision) != Some(revision) {
            return;
        }
        let Some(metrics) = view.scroll_metrics() else {
            return;
        };
        let last_col = inner.width.saturating_sub(1);
        let previous = self
            .current()
            .filter(|(_, start, _)| repeat && (start.row, start.col.min(last_col)) == self.cursor)
            .map(|(_, start, end)| (start, end));
        let Some(window) = view.search(&query, direction, point(self.cursor), previous) else {
            return;
        };
        let target = window
            .current
            .and_then(|index| window.matches.get(index))
            .map(|found| found.start);
        self.query = query;
        if !repeat {
            self.direction = Some(direction);
        }
        self.found = Some(Found { revision, window });
        if let Some(target) = target {
            self.cursor = (target.row, target.col.min(last_col));
            self.reveal(view, inner.height.saturating_sub(1), metrics);
            self.sync(select, last_col);
        }
    }

    fn current(&self) -> Option<(u64, TerminalTextPoint, TerminalTextPoint)> {
        let found = self.found.as_ref()?;
        let current = found.window.matches.get(found.window.current?)?;
        Some((found.revision, current.start, current.end))
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
        let found = self
            .current()
            .filter(|(revision, _, _)| *revision == view.content_revision())
            .map(|(_, start, end)| {
                Selection::absolute_range(self.pane, (start.row, start.col), (end.row, end.col))
            });
        let text = select
            .shown()
            .or(found.as_ref())
            .filter(|_| copy)
            .and_then(|selection| view.selected_text(selection))
            .filter(|text| !text.is_empty());
        select.clear();
        view.scroll_to(self.entry);
        Step::Leave(text)
    }
}

pub(super) fn bar(marks: &Marks, palette: &Palette) -> Vec<CellData> {
    let area = Rect::new(0, 0, marks.bar.width, 1);
    let mut buffer = Buffer::empty(area);
    let base = Style::default().fg(palette.overlay0).bg(palette.panel_bg);
    buffer.set_style(area, base);
    let key = Style::default()
        .fg(palette.accent)
        .bg(palette.panel_bg)
        .add_modifier(Modifier::BOLD);
    if let Some((direction, editor)) = &marks.prompt {
        prompt(&mut buffer, *direction, editor, (base, key), palette);
    } else {
        let count = marks
            .count
            .map(|(current, total)| format!(" {current}/{total}"))
            .or_else(|| marks.query.then(|| " 0/0".to_owned()))
            .unwrap_or_default();
        let repeat = format!(" repeat{count}  ");
        let select = if marks.selecting {
            " selecting  "
        } else {
            " select  "
        };
        let (exit_keys, exit) = if marks.query || marks.selecting {
            ("esc", " clear  q exit")
        } else {
            ("q/esc", " exit")
        };
        let mut x = 0;
        for (text, style) in [
            (" COPY ", accent(palette)),
            (" ", base),
            ("h/j/k/l w/b/e { }", key),
            (" move  ", base),
            ("/ ?", key),
            (" search  ", base),
            ("n/N", key),
            (repeat.as_str(), base),
            ("v/space", key),
            (select, base),
            ("y/enter", key),
            (" copy  ", base),
            (exit_keys, key),
            (exit, base),
        ] {
            if x >= area.width {
                break;
            }
            x = buffer
                .set_stringn(x, 0, text, usize::from(area.width - x), style)
                .0;
        }
    }
    buffer
        .content
        .iter()
        .map(CellData::from_ratatui_cell)
        .collect()
}

fn prompt(
    buffer: &mut Buffer,
    direction: TerminalSearchDirection,
    editor: &TextEditor,
    (base, key): (Style, Style),
    palette: &Palette,
) {
    let width = buffer.area.width;
    buffer.set_stringn(0, 0, " COPY ", usize::from(width), accent(palette));
    let marker = match direction {
        TerminalSearchDirection::Forward => "/",
        TerminalSearchDirection::Backward => "?",
    };
    if width >= 8 {
        buffer.set_string(7, 0, marker, key);
    }
    let footer = "  enter search  esc cancel";
    let footer_width = if width >= 50 { footer.len() as u16 } else { 0 };
    let prefix = width.min(8);
    let field = Rect::new(prefix, 0, width.saturating_sub(prefix + footer_width), 1);
    let text = Style::default().fg(palette.text).bg(palette.panel_bg);
    if let Some(cell) = text_editor::render(buffer, field, editor, text)
        .and_then(|cursor| buffer.cell_mut((cursor.x, cursor.y)))
    {
        cell.set_style(Style::default().fg(palette.panel_bg).bg(palette.text));
    }
    if footer_width > 0 {
        buffer.set_string(width - footer_width, 0, footer, base);
    }
}

pub(super) fn match_cell(palette: &Palette) -> CellData {
    let mut cell = Cell::default();
    cell.set_style(Style::default().fg(palette.text).bg(palette.surface1));
    CellData::from_ratatui_cell(&cell)
}

pub(super) fn cursor_cell(palette: &Palette) -> CellData {
    let mut cell = Cell::default();
    cell.set_style(accent(palette));
    CellData::from_ratatui_cell(&cell)
}

pub(super) fn accent(palette: &Palette) -> Style {
    Style::default()
        .fg(crate::ui::chrome::panel_contrast_fg(palette))
        .bg(palette.accent)
        .add_modifier(Modifier::BOLD)
}

fn highlights(
    window: &TerminalSearchWindow,
    metrics: ScrollMetrics,
    inner: Rect,
) -> (Vec<Rect>, Vec<Rect>) {
    let top = top(metrics);
    let bottom = top.saturating_add(u32::from(inner.height.saturating_sub(1)));
    let last_col = inner.width.saturating_sub(1);
    let (mut matches, mut current) = (Vec::new(), Vec::new());
    let first = window.matches.partition_point(|found| found.end.row < top);
    for (index, found) in window
        .matches
        .iter()
        .enumerate()
        .skip(first)
        .take_while(|(_, found)| found.start.row <= bottom)
    {
        let rects = if window.current == Some(index) {
            &mut current
        } else {
            &mut matches
        };
        for row in found.start.row.max(top)..=found.end.row.min(bottom) {
            let start = if row == found.start.row {
                found.start.col
            } else {
                0
            };
            let end = if row == found.end.row {
                found.end.col
            } else {
                last_col
            }
            .min(last_col);
            let Ok(y) = u16::try_from(row - top) else {
                continue;
            };
            if start <= end {
                rects.push(Rect::new(inner.x + start, inner.y + y, end - start + 1, 1));
            }
        }
    }
    (matches, current)
}

fn point((row, col): (u32, u16)) -> TerminalTextPoint {
    TerminalTextPoint { row, col }
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
