use std::collections::HashSet;
use std::io;
use std::sync::Arc;

use bytes::Bytes;
use crossterm::event::{KeyCode, KeyEventKind, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use tokio::sync::{mpsc, Notify};

use crate::events::AppEvent;
use crate::hc::Core;
use crate::input::TerminalKey;
use crate::layout::PaneId;
use crate::pane::{
    ScrollMetrics, TerminalDirtyPatchOutcome, TerminalSearchDirection, TerminalSearchWindow,
    TerminalTextPoint, WheelRouting,
};
use crate::protocol::{CellData, CursorState, PaneSurfacePatchRow};
use crate::render_signal::RenderSignal;
use crate::terminal::TerminalRuntime;

const EVENT_QUEUE: usize = 64;
const SEARCH_QUERY_BYTES: usize = 4096;
const SEARCH_WINDOW: usize = 1024;

/// The host's grid, and its cell size in pixels, zero when unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Geometry {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) cell_width_px: u32,
    pub(crate) cell_height_px: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Placing {
    pub(crate) geometry: Geometry,
    pub(crate) shown: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Scrollback {
    pub(crate) limit_bytes: usize,
    pub(crate) wheel_lines: usize,
}

impl From<&crate::config::Config> for Scrollback {
    fn from(config: &crate::config::Config) -> Self {
        Self {
            limit_bytes: config.advanced.scrollback_limit_bytes,
            wheel_lines: config.ui.mouse_scroll_lines(),
        }
    }
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

    pub(crate) fn request(&self, pane: PaneId) {
        if self.dirty.request_pty(pane) {
            self.notify.notify_one();
        }
    }

    pub(crate) fn present(&self, panes: HashSet<PaneId>) {
        self.dirty.set_immediate_pty_sources(panes);
    }

    pub(crate) fn presents_output(&self) -> bool {
        self.dirty.has_pending_visible_pty_source(|_| true)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(crate) revision: u64,
    pub(crate) scroll: Option<ScrollMetrics>,
}

/// One herdr-core terminal, parsed in this process and drawn row by row as
/// its screen changes.
pub(crate) struct View {
    runtime: TerminalRuntime,
    id: PaneId,
    wake: Wake,
    wheel_lines: usize,
    repaint: bool,
    cursor: Option<CursorState>,
    frame: Option<Frame>,
}

impl View {
    /// Attaches to `address` at `geometry`, steering the terminal's size.
    /// Needs a tokio runtime.
    pub(crate) fn attach(
        core: Arc<Core>,
        address: &str,
        placing: Placing,
        id: PaneId,
        wake: &Wake,
        events: &EventSender,
        scrollback: Scrollback,
    ) -> io::Result<Self> {
        let Placing { geometry, shown } = placing;
        let runtime = TerminalRuntime::attach_core_terminal(
            id,
            core,
            address,
            geometry.rows,
            geometry.cols,
            !shown,
            scrollback.limit_bytes,
            events.0.clone(),
            wake.notify.clone(),
            wake.dirty.clone(),
        )?;
        Ok(Self::new(runtime, id, wake.clone(), scrollback.wheel_lines))
    }

    fn new(runtime: TerminalRuntime, id: PaneId, wake: Wake, wheel_lines: usize) -> Self {
        Self {
            runtime,
            id,
            wake,
            wheel_lines: wheel_lines.max(1),
            repaint: true,
            cursor: None,
            frame: None,
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

    pub(crate) fn show(&mut self, geometry: Geometry) {
        self.runtime.claim_size(
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
                self.frame = Some(Frame {
                    revision: snapshot.content_revision,
                    scroll: snapshot.scroll_metrics,
                });
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
        let Some((buffer, frame)) = self.render_all(area) else {
            self.repaint = true;
            return Vec::new();
        };
        self.frame = Some(frame);
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

    fn render_all(&self, area: Rect) -> Option<(ratatui::buffer::Buffer, Frame)> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))
                .ok()?;
        let mut drawn = None;
        terminal
            .draw(|target| {
                let _writers = self.runtime.exclude_writers();
                if self.runtime.try_render(target, area, true) {
                    drawn = Some(Frame {
                        revision: self.runtime.content_seq(),
                        scroll: self.runtime.scroll_metrics(),
                    });
                }
            })
            .ok()?;
        drawn.map(|frame| (terminal.backend().buffer().clone(), frame))
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
                visible: cursor.visible && !crate::ui::pane_is_scrolled_back(&self.runtime),
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

    pub(crate) fn cwd(&self) -> Option<std::path::PathBuf> {
        self.runtime.cwd()
    }

    pub(crate) fn alternate_screen(&self) -> bool {
        self.runtime.alternate_screen_active()
    }

    pub(crate) fn frame(&self) -> Option<Frame> {
        self.frame
    }

    pub(crate) fn holds_output(&self) -> bool {
        self.runtime.synchronized_output_active()
    }

    pub(crate) fn scroll_metrics(&self) -> Option<ScrollMetrics> {
        self.runtime.scroll_metrics()
    }

    pub(crate) fn still(&self) -> impl Drop + '_ {
        self.runtime.exclude_writers()
    }

    pub(crate) fn content_revision(&self) -> u64 {
        self.runtime.content_seq()
    }

    pub(crate) fn scroll_by(&mut self, wheel_lines: usize) {
        self.wheel_lines = wheel_lines.max(1);
    }

    pub(crate) fn wheel_lines(&self) -> usize {
        self.wheel_lines
    }

    pub(crate) fn scrollback_text(&self) -> String {
        self.runtime.recent_unwrapped_text_snapshot(usize::MAX).text
    }

    pub(crate) fn selected_text(&self, selection: &crate::selection::Selection) -> Option<String> {
        self.runtime.extract_selection(selection)
    }

    pub(crate) fn cursor_cell(&self) -> Option<(u16, u16)> {
        let (_, cursor) = self.runtime.cursor_snapshot();
        cursor
            .filter(|cursor| cursor.visible && !crate::ui::pane_is_scrolled_back(&self.runtime))
            .map(|cursor| (cursor.x, cursor.y))
    }

    pub(crate) fn word_motion_target(
        &self,
        (row, col): (u32, u16),
        motion: crate::pane::TerminalWordMotion,
    ) -> Option<(u32, u16)> {
        let target = self.runtime.word_motion_target(row, col, motion)?;
        Some((target.row, target.col))
    }

    pub(crate) fn paragraph_motion_target(&self, row: u32, direction: i8) -> Option<u32> {
        Some(self.runtime.paragraph_motion_target(row, direction)?.row)
    }

    pub(crate) fn search(
        &self,
        query: &str,
        direction: TerminalSearchDirection,
        cursor: TerminalTextPoint,
        previous: Option<(TerminalTextPoint, TerminalTextPoint)>,
    ) -> Option<TerminalSearchWindow> {
        (query.len() <= SEARCH_QUERY_BYTES).then(|| {
            self.runtime.search_text_window(
                query,
                query.chars().any(char::is_uppercase),
                direction,
                cursor,
                previous,
                SEARCH_WINDOW,
            )
        })
    }

    pub(crate) fn scroll_to(&self, offset_from_bottom: usize) {
        self.runtime
            .set_scroll_offset_from_bottom(offset_from_bottom);
        self.redraw();
    }

    pub(crate) fn key(&self, key: TerminalKey) {
        let event = key.as_key_event();
        if matches!(event.code, KeyCode::PageUp | KeyCode::PageDown)
            && event.modifiers.is_empty()
            && self.runtime.plain_page_keys_use_host_scrollback() == Some(true)
        {
            if event.kind != KeyEventKind::Release {
                let page = usize::from(self.runtime.current_size().0);
                self.scroll(event.code == KeyCode::PageUp, page);
            }
            return;
        }
        if event.kind != KeyEventKind::Release {
            self.to_bottom();
        }
        self.send(self.runtime.encode_terminal_key(key));
    }

    pub(crate) fn text(&self, text: &str) {
        self.to_bottom();
        self.send(text.as_bytes().to_vec());
    }

    pub(crate) fn paste(&self, text: String) {
        self.to_bottom();
        if let Err(error) = self.runtime.try_send_paste(text) {
            tracing::debug!(%error, "paste did not reach the core terminal");
        }
    }

    pub(crate) fn mouse(&self, event: MouseEvent) -> bool {
        let report = match event.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                match self.runtime.wheel_routing() {
                    Some(WheelRouting::MouseReport) => self.mouse_report(event),
                    Some(WheelRouting::AlternateScroll) => {
                        self.runtime.encode_alternate_scroll(event.kind)
                    }
                    Some(WheelRouting::HostScroll) | None => {
                        let up = event.kind == MouseEventKind::ScrollUp;
                        self.scroll(up, self.wheel_lines);
                        return false;
                    }
                }
            }
            _ => self.mouse_report(event),
        }
        .unwrap_or_default();
        if !report.is_empty() && event.kind != MouseEventKind::Moved {
            self.to_bottom();
        }
        self.send(report)
    }

    fn scroll(&self, up: bool, lines: usize) {
        let lines = lines.max(1);
        if up {
            self.runtime.scroll_up(lines);
        } else {
            self.runtime.scroll_down(lines);
        }
        self.redraw();
    }

    fn to_bottom(&self) {
        if crate::ui::pane_is_scrolled_back(&self.runtime) {
            self.runtime.scroll_reset();
            self.redraw();
        }
    }

    fn redraw(&self) {
        self.wake.request(self.id);
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
            | MouseEventKind::ScrollRight => {
                self.runtime
                    .encode_mouse_wheel(event.kind, position, event.modifiers)
            }
        }
    }

    pub(crate) fn focus(&self, focused: bool) -> bool {
        let event = if focused {
            crate::ghostty::FocusEvent::Gained
        } else {
            crate::ghostty::FocusEvent::Lost
        };
        self.runtime.try_send_focus_event(event)
    }

    fn send(&self, bytes: Vec<u8>) -> bool {
        if bytes.is_empty() {
            return false;
        }
        match self.runtime.try_send_bytes(Bytes::from(bytes)) {
            Ok(()) => true,
            Err(error) => {
                tracing::debug!(%error, "input did not reach the core terminal");
                false
            }
        }
    }
}

#[cfg(test)]
impl View {
    fn test_new(runtime: TerminalRuntime) -> Self {
        Self::new(
            runtime,
            PaneId::from_raw(0),
            Wake::default(),
            crate::config::DEFAULT_MOUSE_SCROLL_LINES,
        )
    }

    pub(in crate::core_client) fn test(cols: u16, rows: u16) -> Self {
        Self::test_new(TerminalRuntime::test_with_screen_bytes(cols, rows, b""))
    }

    pub(in crate::core_client) fn test_with_input(
        cols: u16,
        rows: u16,
    ) -> (Self, tokio::sync::mpsc::Receiver<Bytes>) {
        let (runtime, input) =
            TerminalRuntime::test_with_channel_and_scrollback_bytes(cols, rows, 100_000, b"", 16);
        (Self::test_new(runtime), input)
    }

    pub(in crate::core_client) fn test_feed(&self, bytes: &[u8]) {
        self.runtime.test_process_pty_bytes(bytes);
    }
}

#[cfg(test)]
mod tests;
