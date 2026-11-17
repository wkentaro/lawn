use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::io;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::session::{PaneChrome, Session, Tiles};
use crate::core_client::view::{Geometry, View};
use crate::layout::PaneId;
use crate::protocol::{CellData, CursorState, PaneSurfacePatchRow};
use crate::session_doc::Layout;
use crate::ui::theme::Palette;

struct Shown {
    view: View,
    inner: Rect,
    geometry: Geometry,
    retained: Option<Rect>,
    told_focus: Option<bool>,
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
    relayout: bool,
    canvas: Vec<CellData>,
    canvas_cols: u16,
}

impl Surface {
    pub(super) fn new(host: Geometry, chrome: PaneChrome, palette: Palette) -> Self {
        Self {
            session: Session::default(),
            views: HashMap::new(),
            attaching: HashMap::new(),
            host_focused: true,
            tiles: Tiles::default(),
            chrome,
            palette,
            host,
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

    pub(super) fn host_focus(&mut self, focused: bool) {
        self.host_focused = focused;
        self.sync_focus();
    }

    fn sync_focus(&mut self) {
        let focused = self.session.focused_id().filter(|_| self.host_focused);
        for (id, shown) in &mut self.views {
            let focus = focused == Some(*id);
            if shown.told_focus != Some(focus) {
                shown.view.focus(focus);
                shown.told_focus = Some(focus);
            }
        }
    }

    pub(super) fn resize(&mut self, host: Geometry) {
        self.host = host;
        self.relayout = true;
    }

    pub(super) fn forget(&mut self, id: PaneId) {
        if self.views.remove(&id).is_some() {
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
                        inner: Rect::default(),
                        geometry,
                        retained: None,
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
    }

    pub(super) fn focused(&self) -> Option<&View> {
        Some(&self.views.get(&self.session.focused_id()?)?.view)
    }

    pub(super) fn mouse(&mut self, mut event: MouseEvent) {
        let Some(tile) = self.tiles.tiles.iter().find(|tile| {
            tile.info
                .inner_rect
                .contains(ratatui::layout::Position::new(event.column, event.row))
        }) else {
            return;
        };
        let id = tile.info.id;
        if event.kind == MouseEventKind::Down(MouseButton::Left)
            && self.session.focus(&tile.terminal)
        {
            self.relayout = true;
            self.sync_focus();
        }
        if let Some(shown) = self.views.get(&id) {
            event.column -= shown.inner.x;
            event.row -= shown.inner.y;
            shown.view.mouse(event);
        }
    }

    pub(super) fn frame(
        &mut self,
        dirty: &HashSet<PaneId>,
        attach: impl FnMut(&str, PaneId, Geometry),
    ) -> (Vec<PaneSurfacePatchRow>, Option<CursorState>) {
        let rows = if self.relayout {
            self.relayout = false;
            self.lay_out(attach);
            self.compose()
        } else {
            let mut rows = Vec::new();
            for id in dirty {
                if let Some(shown) = self.views.get_mut(id) {
                    rows.extend(placed(shown.view.draw(), shown.inner));
                }
            }
            if !self.canvas.is_empty() {
                crate::protocol::surface_delta::apply_rows(&mut self.canvas, self.host.cols, &rows);
            }
            rows
        };
        (rows, self.cursor())
    }

    fn lay_out(&mut self, mut attach: impl FnMut(&str, PaneId, Geometry)) {
        let area = Rect::new(0, 0, self.host.cols, self.host.rows);
        self.tiles = self.session.tiles(area, self.chrome);
        let mut views = std::mem::take(&mut self.views);
        for tile in &self.tiles.tiles {
            let inner = tile.info.inner_rect;
            if inner.is_empty() {
                continue;
            }
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
            if shown.geometry == geometry {
                shown.view.invalidate();
            } else {
                shown.view.resize(geometry);
            }
            let same_size = shown.inner.width == inner.width && shown.inner.height == inner.height;
            shown.retained = same_size.then_some(shown.inner);
            shown.inner = inner;
            shown.geometry = geometry;
            self.views.insert(tile.info.id, shown);
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
        for shown in self.views.values_mut() {
            let rows = placed(shown.view.draw(), shown.inner);
            if let Some(was) = shown.retained.filter(|_| rows.is_empty()) {
                copy(
                    (&previous, self.canvas_cols),
                    was,
                    (&mut cells, cols),
                    shown.inner,
                );
            }
            crate::protocol::surface_delta::apply_rows(&mut cells, cols, &rows);
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
        let shown = self.views.get_mut(&self.session.focused_id()?)?;
        let mut cursor = shown.view.cursor()?;
        cursor.x += shown.inner.x;
        cursor.y += shown.inner.y;
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
