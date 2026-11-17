use std::collections::HashSet;
use std::io;
use std::sync::Arc;

use bytes::Bytes;
use crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use tokio::sync::{mpsc, Notify};

use crate::events::AppEvent;
use crate::hc::Core;
use crate::input::TerminalKey;
use crate::layout::PaneId;
use crate::pane::TerminalDirtyPatchOutcome;
use crate::protocol::{CellData, CursorState, PaneSurfacePatchRow};
use crate::render_signal::RenderSignal;
use crate::terminal::TerminalRuntime;

const EVENT_QUEUE: usize = 64;

/// The host's grid, and its cell size in pixels, zero when unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Geometry {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) cell_width_px: u32,
    pub(crate) cell_height_px: u32,
}

#[derive(Clone, Default)]
pub(crate) struct Wake {
    notify: Arc<Notify>,
    dirty: Arc<RenderSignal>,
}

impl Wake {
    pub(crate) async fn wait(&self) {
        self.notify.notified().await;
    }

    pub(crate) fn take(&self) -> HashSet<PaneId> {
        let request = self.dirty.take();
        let mut sources = request.pty_sources;
        sources.extend(request.terminal_title_sources);
        sources
    }
}

/// What the terminal asks of the host beyond its screen.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// The program copied these bytes with OSC 52.
    Copy(Vec<u8>),
    /// The program rang the bell this many times.
    Bell(u16),
    /// The program has ended.
    Ended(PaneId),
    Detached(PaneId),
}

#[derive(Clone)]
pub(crate) struct EventSender(mpsc::Sender<AppEvent>);

pub(crate) struct Events {
    sender: mpsc::Sender<AppEvent>,
    receiver: mpsc::Receiver<AppEvent>,
}

impl Default for Events {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel(EVENT_QUEUE);
        Self { sender, receiver }
    }
}

impl Events {
    pub(crate) fn sender(&self) -> EventSender {
        EventSender(self.sender.clone())
    }

    pub(crate) async fn recv(&mut self) -> Event {
        while let Some(event) = self.receiver.recv().await {
            match event {
                AppEvent::ClipboardWrite { content } => return Event::Copy(content),
                AppEvent::TerminalBell { count, .. } => return Event::Bell(count),
                AppEvent::PaneDied {
                    pane_id,
                    exit_reason: crate::platform::ChildExitReason::Exited,
                } => return Event::Ended(pane_id),
                AppEvent::PaneDied { pane_id, .. } => return Event::Detached(pane_id),
                _ => {}
            }
        }
        std::future::pending().await
    }
}

/// One herdr-core terminal, parsed in this process and drawn row by row as
/// its screen changes.
pub(crate) struct View {
    runtime: TerminalRuntime,
    repaint: bool,
    cursor: Option<CursorState>,
}

impl View {
    /// Attaches to `address` at `geometry`, steering the terminal's size.
    /// Needs a tokio runtime.
    pub(crate) fn attach(
        core: Arc<Core>,
        address: &str,
        geometry: Geometry,
        id: PaneId,
        wake: &Wake,
        events: &EventSender,
    ) -> io::Result<Self> {
        let runtime = TerminalRuntime::attach_core_terminal(
            id,
            core,
            address,
            geometry.rows,
            geometry.cols,
            events.0.clone(),
            wake.notify.clone(),
            wake.dirty.clone(),
        )?;
        let mut view = Self::new(runtime);
        view.resize(geometry);
        Ok(view)
    }

    fn new(runtime: TerminalRuntime) -> Self {
        Self {
            runtime,
            repaint: true,
            cursor: None,
        }
    }

    /// `(cols, rows)`.
    pub(crate) fn size(&self) -> (u16, u16) {
        let (rows, cols) = self.runtime.current_size();
        (cols, rows)
    }

    pub(crate) fn resize(&mut self, geometry: Geometry) {
        self.runtime.resize(
            geometry.rows,
            geometry.cols,
            geometry.cell_width_px,
            geometry.cell_height_px,
        );
        self.repaint = true;
    }

    pub(crate) fn invalidate(&mut self) {
        self.repaint = true;
    }

    /// The rows that changed since the last draw, full width. Empty while
    /// the program holds synchronized output, which keeps the damage for
    /// the draw after it.
    pub(crate) fn draw(&mut self) -> Vec<PaneSurfacePatchRow> {
        let (cols, rows) = self.size();
        if cols == 0 || rows == 0 || self.runtime.synchronized_output_active() {
            return Vec::new();
        }
        if !self.repaint {
            if let Some(snapshot) = self.runtime.collect_dirty_patch_snapshot(cols, rows) {
                match snapshot.patch {
                    TerminalDirtyPatchOutcome::Clean => return Vec::new(),
                    TerminalDirtyPatchOutcome::Patch(patch) => {
                        return patch
                            .rows
                            .into_iter()
                            .map(|(y, cells)| PaneSurfacePatchRow { x: 0, y, cells })
                            .collect();
                    }
                    TerminalDirtyPatchOutcome::Fallback => {}
                }
            }
        }
        self.repaint = false;
        let area = Rect::new(0, 0, cols, rows);
        let Some(buffer) = self.render_all(area) else {
            self.repaint = true;
            return Vec::new();
        };
        buffer
            .content
            .chunks(usize::from(buffer.area.width).max(1))
            .zip(0..)
            .map(|(cells, y)| PaneSurfacePatchRow {
                x: 0,
                y,
                cells: cells.iter().map(CellData::from_ratatui_cell).collect(),
            })
            .collect()
    }

    fn render_all(&self, area: Rect) -> Option<ratatui::buffer::Buffer> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))
                .ok()?;
        let mut rendered = false;
        terminal
            .draw(|frame| rendered = self.runtime.try_render(frame, area, true))
            .ok()?;
        rendered.then(|| terminal.backend().buffer().clone())
    }

    /// The program's cursor. Held where it was while the program holds
    /// synchronized output, like the rows.
    pub(crate) fn cursor(&mut self) -> Option<CursorState> {
        let (cols, rows) = self.size();
        let (held, cursor) = self.runtime.cursor_snapshot();
        if held {
            return self.cursor.clone();
        }
        self.cursor = cursor
            .filter(|cursor| cursor.x < cols && cursor.y < rows)
            .map(|cursor| CursorState {
                x: cursor.x,
                y: cursor.y,
                visible: cursor.visible,
                shape: cursor.shape,
            });
        self.cursor.clone()
    }

    /// Whether the program wants every key reported as an escape code, so
    /// the host must report them that way too.
    pub(crate) fn reports_all_keys(&self) -> bool {
        self.runtime.requests_all_key_reports()
    }

    /// Whether the program asked for mouse reports.
    pub(crate) fn mouse_reporting(&self) -> bool {
        self.runtime.mouse_reporting_enabled()
    }

    pub(crate) fn key(&self, key: TerminalKey) {
        self.send(self.runtime.encode_terminal_key(key));
    }

    pub(crate) fn text(&self, text: &str) {
        self.send(text.as_bytes().to_vec());
    }

    pub(crate) fn paste(&self, text: String) {
        if let Err(error) = self.runtime.try_send_paste(text) {
            tracing::debug!(%error, "paste did not reach the core terminal");
        }
    }

    pub(crate) fn mouse(&self, event: MouseEvent) {
        self.send(self.mouse_report(event).unwrap_or_default());
    }

    fn mouse_report(&self, event: MouseEvent) -> Option<Vec<u8>> {
        let position = crate::input::mouse::Position::Cell {
            column: event.column,
            row: event.row,
        };
        match event.kind {
            MouseEventKind::Down(_) | MouseEventKind::Up(_) | MouseEventKind::Drag(_) => self
                .runtime
                .encode_mouse_button(event.kind, position, event.modifiers),
            MouseEventKind::Moved => {
                self.runtime
                    .encode_mouse_motion(event.kind, position, event.modifiers)
            }
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => self
                .runtime
                .encode_mouse_wheel(event.kind, position, event.modifiers)
                .or_else(|| self.runtime.encode_alternate_scroll(event.kind)),
        }
    }

    pub(crate) fn focus(&self, focused: bool) {
        let event = if focused {
            crate::ghostty::FocusEvent::Gained
        } else {
            crate::ghostty::FocusEvent::Lost
        };
        self.runtime.try_send_focus_event(event);
    }

    fn send(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        if let Err(error) = self.runtime.try_send_bytes(Bytes::from(bytes)) {
            tracing::debug!(%error, "input did not reach the core terminal");
        }
    }
}

#[cfg(test)]
impl View {
    pub(in crate::core_client) fn test(cols: u16, rows: u16) -> Self {
        Self::new(TerminalRuntime::test_with_screen_bytes(cols, rows, b""))
    }

    pub(in crate::core_client) fn test_with_input(
        cols: u16,
        rows: u16,
    ) -> (Self, tokio::sync::mpsc::Receiver<Bytes>) {
        let (runtime, input) = TerminalRuntime::test_with_channel_capacity(cols, rows, 16);
        (Self::new(runtime), input)
    }

    pub(in crate::core_client) fn test_feed(&self, bytes: &[u8]) {
        self.runtime.test_process_pty_bytes(bytes);
    }
}

#[cfg(test)]
mod tests;
