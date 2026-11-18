use std::time::{Duration, Instant};

use crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::config::ToastClipboardPosition;
use crate::core_client::view::View;
use crate::layout::PaneId;
use crate::pane::ScrollMetrics;
use crate::selection::{absolute_row_for_viewport, Selection};

const DOUBLE_CLICK: Duration = Duration::from_millis(350);
const AUTOSCROLL: Duration = Duration::from_millis(30);
const FLASH: Duration = Duration::from_millis(500);

#[derive(Clone, Copy)]
pub(super) struct Copying {
    pub(super) on_select: bool,
    pub(super) toast: Option<ToastClipboardPosition>,
}

#[derive(Clone, Copy)]
struct Click {
    pane: PaneId,
    row: u16,
    col: u16,
    at: Instant,
}

impl Click {
    fn doubles(&self, next: &Self) -> bool {
        self.pane == next.pane
            && next.at.saturating_duration_since(self.at) <= DOUBLE_CLICK
            && self.row.abs_diff(next.row) <= 1
            && self.col.abs_diff(next.col) <= 1
    }
}

struct Words {
    revision: u64,
    row: u32,
    bounds: (u16, u16),
    cursor: (u32, u16),
    dragged: bool,
}

impl Words {
    fn stale(&self, view: Option<&View>) -> bool {
        view.is_none_or(|view| view.content_revision() != self.revision)
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Screen {
    size: (u16, u16),
    shown: (u16, u16),
    alternate: bool,
}

impl Screen {
    pub(super) fn of(view: &View, inner: Rect) -> Self {
        Self {
            size: view.size(),
            shown: (inner.width, inner.height),
            alternate: view.alternate_screen(),
        }
    }
}

#[derive(Clone, Copy)]
struct Autoscroll {
    up: bool,
    column: u16,
    row: u16,
    inner: Rect,
    due: Instant,
}

pub(super) struct Select {
    on_select: bool,
    selection: Option<Selection>,
    screen: Screen,
    words: Option<Words>,
    click: Option<Click>,
    autoscroll: Option<Autoscroll>,
    flash: Option<Instant>,
}

impl Select {
    pub(super) fn new(on_select: bool) -> Self {
        Self {
            on_select,
            selection: None,
            screen: Screen::default(),
            words: None,
            click: None,
            autoscroll: None,
            flash: None,
        }
    }

    pub(super) fn pane(&self) -> Option<PaneId> {
        self.selection.as_ref().map(|selection| selection.pane_id)
    }

    pub(super) fn shown(&self) -> Option<&Selection> {
        self.selection
            .as_ref()
            .filter(|selection| selection.is_visible())
    }

    pub(super) fn copyable(&self) -> bool {
        !self.on_select && self.shown().is_some()
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.autoscroll
            .map(|autoscroll| autoscroll.due)
            .into_iter()
            .chain(self.flash)
            .min()
    }

    pub(super) fn clear(&mut self) {
        self.selection = None;
        self.words = None;
        self.autoscroll = None;
        self.flash = None;
    }

    pub(super) fn hold(&mut self, selection: Selection, screen: Screen) {
        self.clear();
        self.screen = screen;
        self.selection = Some(selection);
    }

    pub(super) fn follow(&mut self, shown: Option<(&View, Rect)>) {
        let _still = still(shown);
        self.check(shown);
    }

    fn check(&mut self, shown: Option<(&View, Rect)>) {
        let moved = self.selection.is_some()
            && shown.is_none_or(|(view, inner)| Screen::of(view, inner) != self.screen);
        let stale = self
            .words
            .as_ref()
            .is_some_and(|words| words.stale(shown.map(|(view, _)| view)));
        if moved || stale {
            self.clear();
        }
    }

    fn text(&self, shown: Option<(&View, Rect)>) -> Option<String> {
        shown?
            .0
            .selected_text(self.selection.as_ref()?)
            .filter(|text| !text.is_empty())
    }

    pub(super) fn press(
        &mut self,
        target: Option<(PaneId, &View, Rect)>,
        event: MouseEvent,
        now: Instant,
    ) {
        self.clear();
        let previous = self.click.take();
        let Some((pane, view, inner)) = target else {
            return;
        };
        let _still = view.still();
        let Some(frame) = view.frame().filter(|_| !view.holds_output()) else {
            return;
        };
        let click = Click {
            pane,
            row: event.row.saturating_sub(inner.y),
            col: event.column.saturating_sub(inner.x),
            at: now,
        };
        self.screen = Screen::of(view, inner);
        let metrics = frame.scroll;
        let plain = event.modifiers.is_empty();
        let revision = view.content_revision();
        if plain
            && frame.revision == revision
            && previous.is_some_and(|previous| previous.doubles(&click))
        {
            let anchor = (absolute_row_for_viewport(click.row, metrics), click.col);
            let Some(bounds) = word_at(view, pane, self.screen.shown.0, anchor) else {
                return;
            };
            self.words = Some(Words {
                revision,
                row: anchor.0,
                bounds,
                cursor: anchor,
                dragged: false,
            });
            self.selection = Some(Selection::absolute_range(
                pane,
                (anchor.0, bounds.0),
                (anchor.0, bounds.1),
            ));
            return;
        }
        if plain {
            self.click = Some(click);
        }
        self.selection = Some(Selection::anchor(pane, click.row, click.col, metrics));
    }

    pub(super) fn drag(&mut self, view: &View, inner: Rect, column: u16, row: u16, now: Instant) {
        let _still = view.still();
        self.check(Some((view, inner)));
        let Some(selection) = &self.selection else {
            return;
        };
        let metrics = view.frame().and_then(|frame| frame.scroll);
        let moved = selection.anchor_screen_pos(inner, metrics) != (row, column);
        let was_dragging = selection.is_dragging();
        self.extend(view, inner, (column, row), metrics);
        let dragging = match &self.words {
            Some(words) => words.dragged,
            None => was_dragging || moved,
        };
        self.autoscroll = None;
        if !dragging {
            return;
        }
        if let Some(selection) = &mut self.selection {
            selection.force_dragging();
        }
        self.click = None;
        let Some(metrics) = view.scroll_metrics() else {
            return;
        };
        let bottom = inner.bottom().saturating_sub(1);
        let (up, lines) = if row < inner.y {
            (true, edge_lines(inner.y - row))
        } else if row > bottom {
            (false, edge_lines(row - bottom))
        } else if row == inner.y {
            (true, 0)
        } else if row == bottom {
            (false, 0)
        } else {
            return;
        };
        self.scroll(
            view,
            inner,
            (column, row),
            metrics,
            scrolled(metrics, up, lines),
        );
        self.autoscroll = Some(Autoscroll {
            up,
            column,
            row,
            inner,
            due: now + AUTOSCROLL,
        });
    }

    pub(super) fn wheel(&mut self, view: &View, inner: Rect, event: MouseEvent) {
        let up = match event.kind {
            MouseEventKind::ScrollUp => true,
            MouseEventKind::ScrollDown => false,
            _ => return,
        };
        let _still = view.still();
        self.check(Some((view, inner)));
        if self.selection.is_none() {
            return;
        }
        let Some(metrics) = view.scroll_metrics() else {
            return;
        };
        let offset = scrolled(metrics, up, view.wheel_lines());
        self.scroll(view, inner, (event.column, event.row), metrics, offset);
    }

    pub(super) fn release(&mut self, shown: Option<(&View, Rect)>, now: Instant) -> Option<String> {
        let _still = still(shown);
        self.check(shown);
        self.autoscroll = None;
        let selection = self.selection.as_mut()?;
        let finished = selection.finish();
        if self.words.is_none() {
            if !finished {
                if selection.is_just_click() {
                    self.selection = None;
                }
                return None;
            }
            self.click = None;
        }
        if !self.on_select {
            self.words = None;
            return None;
        }
        let text = self.text(shown);
        match self.words.take() {
            Some(Words { dragged: false, .. }) => self.flash = Some(now + FLASH),
            _ => self.selection = None,
        }
        text
    }

    pub(super) fn end(&mut self, shown: Option<(&View, Rect)>) {
        let _still = still(shown);
        self.check(shown);
        self.autoscroll = None;
        self.words = None;
        if let Some(selection) = &mut self.selection {
            selection.finish();
            if selection.is_just_click() {
                self.selection = None;
            }
        }
    }

    pub(super) fn copy(&mut self, shown: Option<(&View, Rect)>) -> Option<String> {
        let _still = still(shown);
        self.check(shown);
        let text = self.shown().and_then(|_| self.text(shown));
        self.clear();
        text
    }

    pub(super) fn tick(&mut self, view: Option<(&View, Rect)>, now: Instant) {
        if self.flash.is_some_and(|flash| now >= flash) {
            self.clear();
        }
        let Some(autoscroll) = self.autoscroll.filter(|autoscroll| now >= autoscroll.due) else {
            return;
        };
        self.autoscroll = None;
        let _still = still(view);
        self.check(view);
        let Some((view, metrics)) = view
            .filter(|(_, inner)| self.selection.is_some() && *inner == autoscroll.inner)
            .and_then(|(view, _)| Some((view, view.scroll_metrics()?)))
        else {
            return;
        };
        let offset = scrolled(metrics, autoscroll.up, 1);
        if offset == metrics.offset_from_bottom {
            return;
        }
        let pointer = (autoscroll.column, autoscroll.row);
        self.scroll(view, autoscroll.inner, pointer, metrics, offset);
        self.autoscroll = Some(Autoscroll {
            due: now + AUTOSCROLL,
            ..autoscroll
        });
    }

    fn scroll(
        &mut self,
        view: &View,
        inner: Rect,
        pointer: (u16, u16),
        metrics: ScrollMetrics,
        offset_from_bottom: usize,
    ) {
        if offset_from_bottom == metrics.offset_from_bottom {
            return;
        }
        let metrics = ScrollMetrics {
            offset_from_bottom,
            ..metrics
        };
        self.extend(view, inner, pointer, Some(metrics));
        view.scroll_to(offset_from_bottom);
    }

    fn extend(
        &mut self,
        view: &View,
        inner: Rect,
        (column, row): (u16, u16),
        metrics: Option<ScrollMetrics>,
    ) {
        let Some(selection) = &mut self.selection else {
            return;
        };
        let Some(words) = &mut self.words else {
            selection.drag(column, row, inner, metrics);
            return;
        };
        let (viewport_row, col) = crate::selection::clamp_to_pane(column, row, inner);
        let cursor = (absolute_row_for_viewport(viewport_row, metrics), col);
        if cursor == words.cursor {
            return;
        }
        words.cursor = cursor;
        words.dragged = true;
        let (start, end) =
            word_at(view, selection.pane_id, inner.width, cursor).unwrap_or((col, col));
        *selection = Selection::absolute_range(
            selection.pane_id,
            (words.row, words.bounds.0).min((cursor.0, start)),
            (words.row, words.bounds.1).max((cursor.0, end)),
        );
    }
}

fn word_at(view: &View, pane: PaneId, width: u16, (row, col): (u32, u16)) -> Option<(u16, u16)> {
    let last = width.saturating_sub(1);
    let text = view.selected_text(&Selection::absolute_range(pane, (row, 0), (row, last)))?;
    crate::app::actions::word_bounds_at_column(&text, col)
}

fn still<'a>(shown: Option<(&'a View, Rect)>) -> Option<impl Drop + 'a> {
    shown.map(|(view, _)| view.still())
}

fn edge_lines(distance: u16) -> usize {
    usize::from(distance).saturating_mul(3).clamp(3, 15)
}

fn scrolled(metrics: ScrollMetrics, up: bool, lines: usize) -> usize {
    if up {
        metrics
            .offset_from_bottom
            .saturating_add(lines)
            .min(metrics.max_offset_from_bottom)
    } else {
        metrics.offset_from_bottom.saturating_sub(lines)
    }
}
