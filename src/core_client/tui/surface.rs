use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::io;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::session::{Operation, PaneChrome, Session, Tiles};
use crate::core_client::view::{Geometry, View};
use crate::input::KeybindAction;
use crate::layout::PaneId;
use crate::pane::ScrollMetrics;
use crate::protocol::{CellData, CursorState, PaneSurfacePatchRow};
use crate::session_doc::Layout;
use crate::ui::theme::Palette;
use crate::ui::ScrollbarThumb;

struct Shown {
    view: View,
    place: Place,
    geometry: Geometry,
    told_focus: Option<bool>,
}

enum Place {
    Visible {
        inner: Rect,
        alternate: bool,
        gutter: Option<Gutter>,
        retained: Option<Retained>,
    },
    Hidden(Vec<CellData>),
}

struct Gutter {
    track: Rect,
    thumb: Option<ScrollbarThumb>,
}

#[derive(Clone, Copy)]
struct Gesture {
    pane: PaneId,
    button: MouseButton,
    owner: Owner,
}

#[derive(Clone, Copy)]
enum Owner {
    Terminal { last: MouseEvent },
    Thumb { grab: u16 },
    Track,
}

enum Retained {
    At(Rect),
    Image(Vec<CellData>),
}

impl Shown {
    fn tell_focus(&mut self, focus: bool) {
        if self.told_focus != Some(focus) && self.view.focus(focus) {
            self.told_focus = Some(focus);
        }
    }

    fn gutter_is_stale(&self, scrollbars: bool) -> bool {
        scrollbars
            && matches!(self.place, Place::Visible { alternate, .. } if alternate != self.view.alternate_screen())
    }
}

impl Gutter {
    fn thumb_for(&self, metrics: Option<ScrollMetrics>) -> Option<ScrollbarThumb> {
        metrics.and_then(|metrics| crate::ui::scrollbar_thumb(metrics, self.track))
    }

    fn draw(
        &mut self,
        metrics: Option<ScrollMetrics>,
        palette: &Palette,
        focused: bool,
    ) -> Vec<PaneSurfacePatchRow> {
        self.thumb = self.thumb_for(metrics);
        let mut buffer = Buffer::empty(self.track);
        if let Some(metrics) = metrics {
            crate::ui::render_pane_scrollbar_buffer(
                &mut buffer,
                metrics,
                self.track,
                palette,
                focused,
            );
        }
        buffer
            .content
            .iter()
            .zip(self.track.y..)
            .map(|(cell, y)| PaneSurfacePatchRow {
                x: self.track.x,
                y,
                cells: vec![CellData::from_ratatui_cell(cell)],
            })
            .collect()
    }

    fn redraw(
        &mut self,
        view: &View,
        palette: &Palette,
        focused: bool,
    ) -> Vec<PaneSurfacePatchRow> {
        let metrics = view.scroll_metrics();
        if self.thumb_for(metrics) == self.thumb {
            return Vec::new();
        }
        self.draw(metrics, palette, focused)
    }
}

pub(super) struct Surface {
    session: Session,
    views: HashMap<PaneId, Shown>,
    attaching: HashMap<PaneId, bool>,
    host_focused: bool,
    tiles: Tiles,
    chrome: PaneChrome,
    palette: Palette,
    host: Geometry,
    scrollbars: bool,
    gesture: Option<Gesture>,
    relayout: bool,
    canvas: Vec<CellData>,
    canvas_cols: u16,
}

impl Surface {
    pub(super) fn new(
        host: Geometry,
        chrome: PaneChrome,
        palette: Palette,
        scrollbars: bool,
    ) -> Self {
        Self {
            session: Session::default(),
            views: HashMap::new(),
            attaching: HashMap::new(),
            host_focused: true,
            tiles: Tiles::default(),
            chrome,
            palette,
            host,
            scrollbars,
            gesture: None,
            relayout: true,
            canvas: Vec::new(),
            canvas_cols: 0,
        }
    }

    pub(super) fn update(&mut self, layout: Layout) {
        self.session.update(layout);
        self.relayout = true;
        self.sync_focus();
    }

    pub(super) fn act(&mut self, action: KeybindAction, confirm_close: bool) -> Option<Operation> {
        let area = Rect::new(0, 0, self.host.cols, self.host.rows);
        if self.session.navigate(action, area, self.chrome) {
            self.relayout = true;
            self.sync_focus();
        }
        self.session.operation(action, confirm_close)
    }

    pub(super) fn created(&mut self, terminal: String) {
        if self.session.created(terminal) {
            self.relayout = true;
            self.sync_focus();
        }
    }

    pub(super) fn host_focus(&mut self, focused: bool) {
        self.host_focused = focused;
        if !focused {
            self.end_gesture(None);
        }
        self.sync_focus();
    }

    fn reporting_focus(&self) -> Option<PaneId> {
        self.session.focused_id().filter(|_| self.host_focused)
    }

    fn sync_focus(&mut self) {
        let focused = self.reporting_focus();
        for (id, shown) in &mut self.views {
            shown.tell_focus(focused == Some(*id));
        }
    }

    pub(super) fn resize(&mut self, host: Geometry) {
        self.host = host;
        self.relayout = true;
    }

    pub(super) fn forget(&mut self, id: PaneId) {
        if let Some(shown) = self.views.remove(&id) {
            self.end_gesture(Some((id, &shown.view)));
            self.relayout = true;
        } else if let Some(detached) = self.attaching.get_mut(&id) {
            *detached = true;
        }
    }

    pub(super) fn attached(&mut self, id: PaneId, geometry: Geometry, view: io::Result<View>) {
        let detached = self.attaching.remove(&id).unwrap_or(true);
        self.relayout = true;
        match view {
            Ok(view) if !detached && self.terminal(id).is_some() => {
                self.views.insert(
                    id,
                    Shown {
                        view,
                        place: Place::Visible {
                            inner: Rect::default(),
                            alternate: false,
                            gutter: None,
                            retained: None,
                        },
                        geometry,
                        told_focus: None,
                    },
                );
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%error, terminal = self.terminal(id), "the pane did not attach");
            }
        }
        self.sync_focus();
    }

    pub(super) fn needs_layout(&self) -> bool {
        self.relayout
    }

    pub(super) fn terminal(&self, id: PaneId) -> Option<&str> {
        self.tiles
            .tiles
            .iter()
            .find(|tile| tile.info.id == id)
            .map(|tile| tile.terminal.as_str())
            .or_else(|| {
                self.tiles
                    .hidden
                    .iter()
                    .find(|(hidden, _)| *hidden == id)
                    .map(|(_, terminal)| terminal.as_str())
            })
    }

    pub(super) fn focused_id(&self) -> Option<PaneId> {
        self.session.focused_id()
    }

    pub(super) fn focused(&self) -> Option<&View> {
        self.view(self.session.focused_id()?)
    }

    pub(super) fn view(&self, id: PaneId) -> Option<&View> {
        Some(&self.views.get(&id)?.view)
    }

    pub(super) fn mouse(&mut self, event: MouseEvent) {
        if let Some(mut gesture) = self.gesture {
            match event.kind {
                MouseEventKind::Drag(button) | MouseEventKind::Up(button)
                    if button == gesture.button =>
                {
                    let up = matches!(event.kind, MouseEventKind::Up(_));
                    match &mut gesture.owner {
                        Owner::Terminal { last } => {
                            if let Some((delivered, _)) = self.to_terminal(gesture.pane, event) {
                                *last = delivered;
                            }
                        }
                        Owner::Thumb { grab } => self.drag_bar(gesture.pane, *grab, event.row),
                        Owner::Track => {}
                    }
                    self.gesture = (!up).then_some(gesture);
                    return;
                }
                MouseEventKind::Down(_) | MouseEventKind::Drag(_) | MouseEventKind::Up(_) => return,
                _ => {}
            }
        }
        let point = ratatui::layout::Position::new(event.column, event.row);
        let Some(tile) = self
            .tiles
            .tiles
            .iter()
            .find(|tile| tile.info.inner_rect.contains(point))
        else {
            return;
        };
        let id = tile.info.id;
        let on_bar = self.track(id).is_some_and(|track| track.contains(point));
        let press = event.kind == MouseEventKind::Down(MouseButton::Left);
        let focuses = press
            || !on_bar
                && matches!(
                    event.kind,
                    MouseEventKind::ScrollUp
                        | MouseEventKind::ScrollDown
                        | MouseEventKind::ScrollLeft
                        | MouseEventKind::ScrollRight
                );
        if focuses && self.session.focus(&tile.terminal) {
            self.relayout = true;
            self.sync_focus();
        }
        let button = match event.kind {
            MouseEventKind::Down(button) => button,
            MouseEventKind::Drag(_) | MouseEventKind::Up(_) => return,
            _ => {
                if !on_bar {
                    self.to_terminal(id, event);
                }
                return;
            }
        };
        let owner = if on_bar {
            self.press_bar(id, event)
        } else {
            match self.to_terminal(id, event) {
                Some((last, true)) => Owner::Terminal { last },
                _ => Owner::Track,
            }
        };
        self.gesture = Some(Gesture {
            pane: id,
            button,
            owner,
        });
    }

    fn to_terminal(&self, id: PaneId, mut event: MouseEvent) -> Option<(MouseEvent, bool)> {
        let Some(Shown {
            view,
            place: Place::Visible { inner, .. },
            ..
        }) = self.views.get(&id)
        else {
            return None;
        };
        event.column = event
            .column
            .saturating_sub(inner.x)
            .min(inner.width.saturating_sub(1));
        event.row = event
            .row
            .saturating_sub(inner.y)
            .min(inner.height.saturating_sub(1));
        let sent = view.mouse(event);
        Some((event, sent))
    }

    fn end_gesture(&mut self, leaving: Option<(PaneId, &View)>) {
        let Some(gesture) = self.gesture else {
            return;
        };
        if leaving.is_some_and(|(id, _)| id != gesture.pane) {
            return;
        }
        self.gesture = None;
        let Owner::Terminal { last } = gesture.owner else {
            return;
        };
        let view = match leaving {
            Some((_, view)) => Some(view),
            None => self.view(gesture.pane),
        };
        if let Some(view) = view {
            let (cols, rows) = view.size();
            view.mouse(MouseEvent {
                kind: MouseEventKind::Up(gesture.button),
                column: last.column.min(cols.saturating_sub(1)),
                row: last.row.min(rows.saturating_sub(1)),
                ..last
            });
        }
    }

    fn track(&self, id: PaneId) -> Option<Rect> {
        match &self.views.get(&id)?.place {
            Place::Visible {
                gutter: Some(gutter),
                ..
            } => Some(gutter.track),
            _ => None,
        }
    }

    fn bar(&self, id: PaneId) -> Option<(&View, Rect, ScrollMetrics)> {
        let shown = self.views.get(&id)?;
        let Place::Visible {
            gutter: Some(Gutter { track, .. }),
            ..
        } = shown.place
        else {
            return None;
        };
        let metrics = shown
            .view
            .scroll_metrics()
            .filter(|metrics| crate::ui::should_show_scrollbar(*metrics))?;
        Some((&shown.view, track, metrics))
    }

    fn press_bar(&self, id: PaneId, event: MouseEvent) -> Owner {
        let Some((view, track, metrics)) = self.bar(id) else {
            return Owner::Track;
        };
        if event.kind != MouseEventKind::Down(MouseButton::Left) {
            return Owner::Track;
        }
        match crate::ui::scrollbar_thumb_grab_offset(metrics, track, event.row) {
            Some(grab) => Owner::Thumb { grab },
            None => {
                view.scroll_to(crate::ui::scrollbar_offset_from_row(
                    metrics, track, event.row,
                ));
                Owner::Track
            }
        }
    }

    fn drag_bar(&self, id: PaneId, grab: u16, row: u16) {
        let Some((view, track, metrics)) = self.bar(id) else {
            return;
        };
        let offset = crate::ui::scrollbar_offset_from_drag_row(metrics, track, row, grab);
        if offset != metrics.offset_from_bottom {
            view.scroll_to(offset);
        }
    }

    pub(super) fn frame(
        &mut self,
        dirty: &HashSet<PaneId>,
        attach: impl FnMut(&str, PaneId, Geometry),
        leave: impl FnMut(PaneId, &View),
    ) -> (Vec<PaneSurfacePatchRow>, Option<CursorState>) {
        let focused = self.reporting_focus();
        for id in dirty {
            if let Some(shown) = self.views.get_mut(id) {
                shown.tell_focus(focused == Some(*id));
                self.relayout = self.relayout || shown.gutter_is_stale(self.scrollbars);
            }
        }
        let rows = if self.relayout {
            self.relayout = false;
            self.lay_out(attach, leave);
            self.compose()
        } else {
            let mut rows = Vec::new();
            let focused = self.session.focused_id();
            for id in dirty {
                if let Some(Shown {
                    view,
                    place: Place::Visible { inner, gutter, .. },
                    ..
                }) = self.views.get_mut(id)
                {
                    rows.extend(placed(view.draw(), *inner));
                    if let Some(gutter) = gutter {
                        rows.extend(gutter.redraw(view, &self.palette, focused == Some(*id)));
                    }
                }
            }
            if !self.canvas.is_empty() {
                crate::protocol::surface_delta::apply_rows(&mut self.canvas, self.host.cols, &rows);
            }
            rows
        };
        (rows, self.cursor())
    }

    fn lay_out(
        &mut self,
        mut attach: impl FnMut(&str, PaneId, Geometry),
        mut leave: impl FnMut(PaneId, &View),
    ) {
        let area = Rect::new(0, 0, self.host.cols, self.host.rows);
        self.tiles = self.session.tiles(area, self.chrome);
        let mut views = std::mem::take(&mut self.views);
        for tile in &self.tiles.tiles {
            if tile.info.inner_rect.is_empty() {
                continue;
            }
            let alternate = views
                .get(&tile.info.id)
                .is_some_and(|shown| shown.view.alternate_screen());
            let (inner, track) = crate::ui::scrollbar_gutter_for(
                tile.info.inner_rect,
                self.scrollbars && !alternate,
            );
            let geometry = Geometry {
                cols: inner.width,
                rows: inner.height,
                ..self.host
            };
            let Some(mut shown) = views.remove(&tile.info.id) else {
                if let Entry::Vacant(attempt) = self.attaching.entry(tile.info.id) {
                    attempt.insert(false);
                    attach(&tile.terminal, tile.info.id, geometry);
                }
                continue;
            };
            let retained = match std::mem::replace(&mut shown.place, Place::Hidden(Vec::new())) {
                Place::Visible { inner: was, .. } => (was.width == inner.width
                    && was.height == inner.height)
                    .then_some(Retained::At(was)),
                Place::Hidden(image) => (shown.geometry.cols == inner.width
                    && shown.geometry.rows == inner.height)
                    .then_some(Retained::Image(image)),
            };
            if shown.geometry == geometry {
                shown.view.invalidate();
            } else {
                shown.view.resize(geometry);
            }
            shown.place = Place::Visible {
                inner,
                alternate,
                gutter: track.map(|track| Gutter { track, thumb: None }),
                retained,
            };
            shown.geometry = geometry;
            self.views.insert(tile.info.id, shown);
        }
        for (id, mut shown) in views {
            self.end_gesture(Some((id, &shown.view)));
            if !self.tiles.hidden.iter().any(|(hidden, _)| *hidden == id) {
                leave(id, &shown.view);
                continue;
            }
            if let Place::Visible { inner, .. } = shown.place {
                shown.place = Place::Hidden(cut(&self.canvas, self.canvas_cols, inner));
            }
            self.views.insert(id, shown);
        }
    }

    fn compose(&mut self) -> Vec<PaneSurfacePatchRow> {
        let (cols, rows) = (self.host.cols, self.host.rows);
        let previous = std::mem::take(&mut self.canvas);
        if cols == 0 || rows == 0 {
            return Vec::new();
        }
        let mut buffer = Buffer::empty(Rect::new(0, 0, cols, rows));
        let infos: Vec<_> = self
            .tiles
            .tiles
            .iter()
            .map(|tile| tile.info.clone())
            .collect();
        crate::ui::paint_pane_borders(
            &mut buffer,
            &infos,
            &self.tiles.splits,
            self.chrome.gaps,
            &self.palette,
            |_| None,
        );
        let mut cells: Vec<CellData> = buffer
            .content
            .iter()
            .map(CellData::from_ratatui_cell)
            .collect();
        let focused = self.session.focused_id();
        for (id, shown) in &mut self.views {
            let Place::Visible {
                inner,
                gutter,
                retained,
                ..
            } = &mut shown.place
            else {
                continue;
            };
            let rows = placed(shown.view.draw(), *inner);
            match retained.take().filter(|_| rows.is_empty()) {
                Some(Retained::At(was)) => {
                    copy(
                        (&previous, self.canvas_cols),
                        was,
                        (&mut cells, cols),
                        *inner,
                    );
                }
                Some(Retained::Image(image)) => {
                    let was = Rect::new(0, 0, inner.width, inner.height);
                    copy((&image, inner.width), was, (&mut cells, cols), *inner);
                }
                None => {}
            }
            crate::protocol::surface_delta::apply_rows(&mut cells, cols, &rows);
            if let Some(gutter) = gutter {
                let rows = gutter.draw(
                    shown.view.scroll_metrics(),
                    &self.palette,
                    focused == Some(*id),
                );
                crate::protocol::surface_delta::apply_rows(&mut cells, cols, &rows);
            }
        }
        self.canvas = cells;
        self.canvas_cols = cols;
        self.canvas
            .chunks(usize::from(cols))
            .zip(0..)
            .map(|(cells, y)| PaneSurfacePatchRow {
                x: 0,
                y,
                cells: cells.to_vec(),
            })
            .collect()
    }

    fn cursor(&mut self) -> Option<CursorState> {
        let Shown {
            view,
            place: Place::Visible { inner, .. },
            ..
        } = self.views.get_mut(&self.session.focused_id()?)?
        else {
            return None;
        };
        let mut cursor = view.cursor()?;
        cursor.x += inner.x;
        cursor.y += inner.y;
        Some(cursor)
    }
}

fn copy(from: (&[CellData], u16), was: Rect, to: (&mut [CellData], u16), inner: Rect) {
    let start = |rect: Rect, row: u16, cols: u16| {
        usize::from(rect.y + row) * usize::from(cols) + usize::from(rect.x)
    };
    let width = usize::from(inner.width);
    if was.right() > from.1 || usize::from(was.bottom()) * usize::from(from.1) > from.0.len() {
        return;
    }
    for row in 0..inner.height {
        let (from_start, to_start) = (start(was, row, from.1), start(inner, row, to.1));
        to.0[to_start..to_start + width].clone_from_slice(&from.0[from_start..from_start + width]);
    }
}

fn cut(canvas: &[CellData], cols: u16, rect: Rect) -> Vec<CellData> {
    if rect.right() > cols {
        return Vec::new();
    }
    (rect.y..rect.bottom())
        .flat_map(|y| {
            let start = usize::from(y) * usize::from(cols) + usize::from(rect.x);
            canvas
                .get(start..start + usize::from(rect.width))
                .into_iter()
                .flatten()
                .cloned()
        })
        .collect()
}

fn placed(rows: Vec<PaneSurfacePatchRow>, inner: Rect) -> Vec<PaneSurfacePatchRow> {
    crate::core_client::screen::clip(rows, (inner.width, inner.height))
        .into_iter()
        .map(|mut row| {
            row.x += inner.x;
            row.y += inner.y;
            row
        })
        .collect()
}

#[cfg(test)]
mod tests;
