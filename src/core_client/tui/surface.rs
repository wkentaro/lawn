use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Direction, Position, Rect};
use ratatui::style::{Modifier, Style};

use super::chrome::Chrome;
use super::copy_mode::{CopyMode, Marks, Step};
use super::dialog::{Dialog, Modal, Outcome, Rename};
use super::select::{Copying, Select};
use super::session::{Operation, PaneChrome, Request, Session, Tiles};
use super::terminals::Report;
use crate::config::{LiveKeybindConfig, ToastClipboardPosition};
use crate::core_client::view::{Geometry, View};
use crate::input::{KeybindAction, TerminalKey};
use crate::layout::{PaneId, SplitBorder};
use crate::pane::ScrollMetrics;
use crate::protocol::{CellData, CursorState, PaneSurfacePatchRow};
use crate::raw_input::RawInputEvent;
use crate::session_doc::{Arrangement, Layout};
use crate::ui::theme::Palette;
use crate::ui::ScrollbarThumb;

const TOAST: Duration = Duration::from_secs(2);
const SPLIT_DRAG: Duration = Duration::from_millis(33);

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
    Select,
}

struct Splitting {
    direction: Direction,
    grab: i32,
    first: String,
    second: String,
    sent: Option<(Instant, f32)>,
    pending: Option<u16>,
}

impl Splitting {
    fn due(&self) -> Option<Instant> {
        self.pending?;
        Some(self.sent?.0 + SPLIT_DRAG)
    }

    fn pointer(&self, event: MouseEvent) -> u16 {
        match self.direction {
            Direction::Horizontal => event.column,
            Direction::Vertical => event.row,
        }
    }

    fn change(&self, split: &SplitBorder, pointer: u16) -> Option<f32> {
        let (origin, length) = match self.direction {
            Direction::Horizontal => (split.area.x, split.area.width),
            Direction::Vertical => (split.area.y, split.area.height),
        };
        if split.area.is_empty() {
            return None;
        }
        let ratio = ((i32::from(pointer) + self.grab - i32::from(origin)) as f32
            / f32::from(length))
        .clamp(crate::session_doc::MIN_RATIO, crate::session_doc::MAX_RATIO);
        (ratio != split.ratio || self.sent.is_some_and(|(_, last)| last != ratio)).then_some(ratio)
    }

    fn set(&self, ratio: f32) -> Operation {
        Operation::Arrange(Arrangement::SetRatio {
            first: self.first.clone(),
            second: self.second.clone(),
            direction: match self.direction {
                Direction::Horizontal => crate::session_doc::Direction::Right,
                Direction::Vertical => crate::session_doc::Direction::Down,
            },
            ratio,
        })
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
struct Overlays {
    selection: [Rect; 3],
    copy: Marks,
    toast: Rect,
    cover: Rect,
    dialog: Rect,
    resize: Rect,
}

impl Overlays {
    fn rects(&self) -> impl Iterator<Item = &Rect> {
        self.selection
            .iter()
            .chain(&self.copy.matches)
            .chain(&self.copy.current)
            .chain([
                &self.copy.cursor,
                &self.copy.bar,
                &self.toast,
                &self.cover,
                &self.dialog,
                &self.resize,
            ])
    }

    fn covers(&self, y: u16) -> bool {
        self.rects().any(|rect| covers(rect, y))
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum Pointed {
    Copied(String),
    Act(KeybindAction),
    Run(Operation),
}

struct Toast {
    until: Instant,
    image: (Rect, Vec<CellData>),
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
        let metrics = drawn_scroll(view);
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
    outer: Chrome,
    areas: crate::ui::chrome::Layout,
    palette: Palette,
    host: Geometry,
    scrollbars: bool,
    gesture: Option<Gesture>,
    splitting: Option<Splitting>,
    select: Select,
    copy: Option<CopyMode>,
    dialog: Option<Modal>,
    resizing: bool,
    keybinds: LiveKeybindConfig,
    toast_at: Option<ToastClipboardPosition>,
    toast: Option<Toast>,
    highlight: CellData,
    painted: Overlays,
    relayout: bool,
    canvas: Vec<CellData>,
    canvas_cols: u16,
}

impl Surface {
    pub(super) fn new(
        host: Geometry,
        chrome: PaneChrome,
        outer: Chrome,
        palette: Palette,
        scrollbars: bool,
        copying: Copying,
        keybinds: LiveKeybindConfig,
    ) -> Self {
        let mut highlight = ratatui::buffer::Cell::default();
        highlight.set_style(crate::ui::automatic_selection_style(
            &palette,
            crate::terminal_theme::TerminalTheme::default(),
        ));
        Self {
            session: Session::default(),
            views: HashMap::new(),
            attaching: HashMap::new(),
            host_focused: true,
            tiles: Tiles::default(),
            chrome,
            outer,
            areas: crate::ui::chrome::Layout::default(),
            palette,
            host,
            scrollbars,
            gesture: None,
            splitting: None,
            select: Select::new(copying.on_select),
            copy: None,
            dialog: None,
            resizing: false,
            keybinds,
            toast_at: copying.toast,
            toast: None,
            highlight: CellData::from_ratatui_cell(&highlight),
            painted: Overlays::default(),
            relayout: true,
            canvas: Vec::new(),
            canvas_cols: 0,
        }
    }

    pub(super) fn holds(&self, layout: &Layout) -> bool {
        self.session.holds(layout)
    }

    pub(super) fn update(&mut self, layout: Layout) {
        self.session.update(layout);
        self.relayout = true;
        self.sync_focus();
    }

    pub(super) fn act(&mut self, action: KeybindAction, confirm_close: bool) -> Option<Operation> {
        if action == KeybindAction::ToggleSidebar {
            self.outer.toggle();
            self.relayout = true;
            return None;
        }
        if action == KeybindAction::Help {
            self.open(Dialog::Help(Default::default()));
            return None;
        }
        if action == KeybindAction::EnterResizeMode {
            self.resizing = true;
            return None;
        }
        if matches!(
            action,
            KeybindAction::RenameWorkspace | KeybindAction::RenameTab
        ) {
            if let Some(rename) = Rename::of(action, &self.session) {
                self.open(Dialog::Rename(rename));
            }
            return None;
        }
        let area = self.lay_out_chrome().pane_surface;
        if self.session.navigate(action, area, self.chrome) {
            self.relayout = true;
            self.sync_focus();
        }
        if let Some(arrangement) = self.session.arrangement(action, area) {
            return Some(Operation::Arrange(arrangement));
        }
        match self.session.operation(action, confirm_close)? {
            Request::Run(operation) => Some(operation),
            Request::Confirm(closing) => {
                self.open(Dialog::Confirm(closing));
                None
            }
        }
    }

    fn open(&mut self, dialog: Dialog) {
        self.end_gesture(None);
        self.end_drags();
        self.dialog = Some(Modal::new(dialog));
    }

    pub(super) fn dialog_open(&self) -> bool {
        self.dialog.is_some()
    }

    pub(super) fn dialog_input(&mut self, event: &RawInputEvent) -> bool {
        let Some(dialog) = self.dialog.as_mut() else {
            return false;
        };
        match event {
            RawInputEvent::Text(text) => dialog.text(text.as_str()),
            RawInputEvent::Paste(text) => dialog.text(text),
            _ => {}
        }
        true
    }

    pub(super) fn dialog_key(&mut self, key: &TerminalKey) -> Option<Operation> {
        let outcome = self.dialog.as_mut()?.key(key);
        self.settle(outcome)
    }

    pub(super) fn resizing(&self) -> bool {
        self.resizing
    }

    pub(super) fn resize_key(&mut self, key: &TerminalKey) -> Option<Operation> {
        let done = &self.keybinds.keybinds.resize_mode;
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter)
            || done.matches_prefix_key(key)
            || done.matches_direct_key(key)
        {
            self.resizing = false;
            return None;
        }
        let action = match key.code {
            KeyCode::Char('h') | KeyCode::Left => KeybindAction::ResizePaneLeft,
            KeyCode::Char('j') | KeyCode::Down => KeybindAction::ResizePaneDown,
            KeyCode::Char('k') | KeyCode::Up => KeybindAction::ResizePaneUp,
            KeyCode::Char('l') | KeyCode::Right => KeybindAction::ResizePaneRight,
            _ => return None,
        };
        self.act(action, false)
    }

    fn settle(&mut self, outcome: Outcome) -> Option<Operation> {
        if outcome == Outcome::Stay {
            return None;
        }
        let dialog = self.dialog.take()?;
        match outcome {
            Outcome::Accept => dialog.accepted(&self.session),
            Outcome::Stay | Outcome::Close => None,
        }
    }

    pub(super) fn heard(&mut self, report: Report) {
        if self.session.heard(report) {
            self.outer.recheck();
        }
    }

    fn look(&mut self) -> bool {
        let views = &self.views;
        self.host_focused
            && self.session.look(|id| {
                views
                    .get(&id)
                    .is_some_and(|shown| matches!(shown.place, Place::Visible { .. }))
            })
    }

    pub(super) fn created(&mut self, terminal: String) {
        if self.session.created(terminal) {
            self.relayout = true;
            self.sync_focus();
        }
    }

    pub(super) fn host_focus(&mut self, focused: bool) {
        self.host_focused = focused;
        if focused {
            self.outer.recheck();
        } else {
            self.end_gesture(None);
            self.end_drags();
        }
        self.sync_focus();
    }

    fn end_drags(&mut self) {
        self.splitting = None;
        self.outer.cancel_drag();
    }

    fn reporting_focus(&self) -> Option<PaneId> {
        self.session.focused_id().filter(|_| self.host_focused)
    }

    fn sync_focus(&mut self) {
        if self.select.pane() != self.session.focused_id() {
            self.select.clear();
        }
        let focused = self.reporting_focus();
        for (id, shown) in &mut self.views {
            shown.tell_focus(focused == Some(*id));
        }
    }

    pub(super) fn resize(&mut self, host: Geometry) {
        self.host = host;
        self.relayout = true;
    }

    fn lay_out_chrome(&self) -> crate::ui::chrome::Layout {
        self.outer
            .layout(self.host.cols, self.host.rows, &self.session)
    }

    pub(super) fn copyable(&self) -> bool {
        self.dialog.is_none() && !self.copy_mode() && self.select.copyable()
    }

    pub(super) fn switching(&self) -> bool {
        self.outer.switching()
    }

    pub(super) fn close_switcher(&mut self) {
        self.outer.close();
    }

    pub(super) fn copy_mode(&self) -> bool {
        self.active_copy().is_some()
    }

    pub(super) fn prompting(&self) -> bool {
        self.active_copy().is_some_and(CopyMode::prompting)
    }

    fn active_copy(&self) -> Option<&CopyMode> {
        let focused = self.session.focused_id()?;
        self.copy.as_ref().filter(|copy| copy.pane() == focused)
    }

    pub(super) fn copy_mode_input(&mut self, event: &RawInputEvent) -> bool {
        let focused = self.session.focused_id();
        let Some(copy) = self
            .copy
            .as_mut()
            .filter(|copy| Some(copy.pane()) == focused)
        else {
            return false;
        };
        match event {
            RawInputEvent::Text(text) => copy.type_text(text.as_str()),
            RawInputEvent::Paste(text) => copy.type_text(text),
            _ => {}
        }
        true
    }

    pub(super) fn paste_shortcut(
        &mut self,
        key: &TerminalKey,
        clipboard: impl FnOnce() -> Option<String>,
    ) -> bool {
        let edits = self
            .dialog
            .as_ref()
            .map_or_else(|| self.prompting(), Modal::edits);
        if !edits || !crate::ui::text_editor::is_modal_paste_shortcut(key) {
            return false;
        }
        if let Some(text) = clipboard() {
            let paste = RawInputEvent::Paste(text);
            if !self.dialog_input(&paste) {
                self.copy_mode_input(&paste);
            }
        }
        true
    }

    pub(super) fn enter_copy_mode(&mut self) {
        let Some(pane) = self.session.focused_id() else {
            return;
        };
        if self.copy.as_ref().is_some_and(|copy| copy.pane() == pane) {
            return;
        }
        if let Some(parked) = self.copy.take() {
            if let Some(shown) = self.views.get(&parked.pane()) {
                parked.quit(&shown.view);
            }
        }
        let Some((view, inner)) = visible(&self.views, pane) else {
            return;
        };
        self.copy = CopyMode::enter(pane, view, inner);
        if self.copy.is_some() {
            self.select.clear();
        }
    }

    pub(super) fn copy_mode_key(&mut self, key: &TerminalKey) -> Option<String> {
        let focused = self.session.focused_id();
        let copy = self
            .copy
            .as_mut()
            .filter(|copy| Some(copy.pane()) == focused)?;
        let shown = visible(&self.views, copy.pane());
        match copy.key(shown, key, &mut self.select) {
            Step::Stay => None,
            Step::Leave(text) => {
                self.copy = None;
                text
            }
        }
    }

    pub(super) fn copy(&mut self) -> Option<String> {
        let shown = self
            .select
            .pane()
            .and_then(|pane| visible(&self.views, pane));
        self.select.copy(shown)
    }

    pub(super) fn deselect(&mut self) {
        if !self.copy_mode() {
            self.select.clear();
        }
    }

    pub(super) fn copied(&mut self, now: Instant) {
        if let Some(position) = self.toast_at {
            self.toast = Some(Toast {
                until: now + TOAST,
                image: toast_image(self.areas.pane_surface, position, &self.palette),
            });
        }
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.select
            .deadline()
            .into_iter()
            .chain(self.toast.as_ref().map(|toast| toast.until))
            .chain(self.outer.deadline())
            .chain(self.splitting.as_ref().and_then(Splitting::due))
            .min()
    }

    pub(super) fn tick(&mut self, now: Instant, holding: bool) -> Option<Operation> {
        if self.toast.as_ref().is_some_and(|toast| now >= toast.until) {
            self.toast = None;
        }
        self.outer.tick(now);
        let view = self
            .select
            .pane()
            .and_then(|pane| visible(&self.views, pane));
        self.select.tick(view, now);
        let mut splitting = self
            .splitting
            .take_if(|splitting| splitting.due().is_some_and(|due| now >= due))?;
        if holding {
            splitting.pending = None;
            self.splitting = Some(splitting);
            return None;
        }
        let split = self.split(&splitting)?;
        self.flushed(splitting, &split, now)
    }

    pub(super) fn needs_frame(&self) -> bool {
        self.relayout
            || self.outer.stale()
            || self.overlays() != self.painted
            || self
                .dialog
                .as_ref()
                .is_some_and(|dialog| dialog.stale(self.host_area()))
    }

    fn host_area(&self) -> Rect {
        Rect::new(0, 0, self.host.cols, self.host.rows)
    }

    pub(super) fn forget(&mut self, id: PaneId) {
        if self.copy.as_ref().is_some_and(|copy| copy.pane() == id) {
            self.copy = None;
        }
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

    pub(super) fn mouse(&mut self, event: MouseEvent, now: Instant) -> Option<Pointed> {
        if let Some(dialog) = self.dialog.as_mut() {
            let outcome = dialog.mouse(event);
            return self.settle(outcome).map(Pointed::Run);
        }
        let press = event.kind == MouseEventKind::Down(MouseButton::Left);
        if press {
            self.splitting = None;
        }
        if let Some(mut splitting) = self.splitting.take() {
            let split = self.split(&splitting)?;
            let pointer = splitting.pointer(event);
            if event.kind == MouseEventKind::Up(MouseButton::Left) {
                return splitting
                    .change(&split, pointer)
                    .map(|ratio| Pointed::Run(splitting.set(ratio)));
            }
            if event.kind == MouseEventKind::Drag(MouseButton::Left) {
                splitting.pending = Some(pointer);
                if splitting
                    .sent
                    .is_none_or(|(at, _)| now.duration_since(at) >= SPLIT_DRAG)
                {
                    return self.flushed(splitting, &split, now).map(Pointed::Run);
                }
            }
            self.splitting = Some(splitting);
            return None;
        }
        if let Some(mut gesture) = self.gesture {
            match event.kind {
                MouseEventKind::Drag(button) | MouseEventKind::Up(button)
                    if button == gesture.button =>
                {
                    let up = matches!(event.kind, MouseEventKind::Up(_));
                    let mut copied = None;
                    match &mut gesture.owner {
                        Owner::Terminal { last } => {
                            if let Some((delivered, _)) = self.to_terminal(gesture.pane, event) {
                                *last = delivered;
                            }
                        }
                        Owner::Thumb { grab } => self.drag_bar(gesture.pane, *grab, event.row),
                        Owner::Track => {}
                        Owner::Select if up => {
                            let shown = visible(&self.views, gesture.pane);
                            copied = self.select.release(shown, now);
                        }
                        Owner::Select => {
                            if let Some((view, inner)) = visible(&self.views, gesture.pane) {
                                self.select.drag(view, inner, event.column, event.row, now);
                            }
                        }
                    }
                    self.gesture = (!up).then_some(gesture);
                    return copied.map(Pointed::Copied);
                }
                MouseEventKind::Down(_) | MouseEventKind::Drag(_) | MouseEventKind::Up(_) => {
                    return None
                }
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                    if matches!(gesture.owner, Owner::Select) =>
                {
                    if let Some((view, inner)) = visible(&self.views, gesture.pane) {
                        self.select.wheel(view, inner, event);
                    }
                    return None;
                }
                _ => {}
            }
        }
        let hidden = match self.painted.resize.is_empty() {
            true => self.painted.copy.bar,
            false => self.painted.resize,
        };
        if self.outer.claim(event, hidden) {
            if press {
                self.select.press(None, event, now);
            }
            let pointed = self.outer.mouse(
                event,
                &self.session,
                now,
                self.copy_mode() && !self.resizing,
            );
            if let Some(terminal) = self.outer.revealed() {
                if self.session.reveal(&terminal) {
                    self.relayout = true;
                    self.sync_focus();
                }
            }
            self.resizing &= !self.outer.switching();
            self.relayout |= self.lay_out_chrome() != self.areas;
            return pointed;
        }
        let point = Position::new(event.column, event.row);
        if press && !hidden.contains(point) {
            self.splitting = self.split_at(point);
            if self.splitting.is_some() {
                self.select.press(None, event, now);
                return None;
            }
        }
        let Some(tile) = self
            .tiles
            .tiles
            .iter()
            .find(|tile| tile.info.inner_rect.contains(point))
        else {
            if press {
                self.select.press(None, event, now);
            }
            return None;
        };
        let id = tile.info.id;
        let on_bar = self.track(id).is_some_and(|track| track.contains(point));
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
            MouseEventKind::Drag(_) | MouseEventKind::Up(_) => return None,
            _ => {
                if !on_bar {
                    self.to_terminal(id, event);
                }
                return None;
            }
        };
        let owner = if on_bar {
            self.press_bar(id, event)
        } else {
            match self.to_terminal(id, event) {
                Some((last, true)) => Owner::Terminal { last },
                Some(_) if press => Owner::Select,
                _ => Owner::Track,
            }
        };
        if press {
            if let Some(copy) = self.copy.as_mut().filter(|copy| {
                matches!(owner, Owner::Select)
                    && copy.pane() == id
                    && Some(id) == self.session.focused_id()
            }) {
                copy.drop_anchor();
            }
            let target = matches!(owner, Owner::Select)
                .then(|| visible(&self.views, id))
                .flatten()
                .map(|(view, inner)| (id, view, inner));
            self.select.press(target, event, now);
        }
        self.gesture = Some(Gesture {
            pane: id,
            button,
            owner,
        });
        None
    }

    fn split_at(&mut self, point: Position) -> Option<Splitting> {
        if self.relayout {
            return None;
        }
        let frames: Vec<Rect> = self.tiles.tiles.iter().map(|tile| tile.info.rect).collect();
        let split = self.tiles.splits.iter().find(|split| {
            crate::server::client_shell::split_hit_rect(
                split,
                self.chrome.borders.draws_borders(),
                self.chrome.gaps,
                &frames,
            )
            .is_some_and(|hit| hit.contains(point))
        })?;
        let (first, second) = self.session.between(&split.path)?;
        let pointer = match split.direction {
            Direction::Horizontal => point.x,
            Direction::Vertical => point.y,
        };
        Some(Splitting {
            direction: split.direction,
            grab: i32::from(split.pos) - i32::from(pointer),
            first,
            second,
            sent: None,
            pending: None,
        })
    }

    fn split(&self, splitting: &Splitting) -> Option<SplitBorder> {
        self.session.split(
            self.lay_out_chrome().pane_surface,
            splitting.direction,
            (&splitting.first, &splitting.second),
        )
    }

    fn flushed(
        &mut self,
        mut splitting: Splitting,
        split: &SplitBorder,
        now: Instant,
    ) -> Option<Operation> {
        let set = splitting
            .pending
            .take()
            .and_then(|pointer| splitting.change(split, pointer))
            .map(|ratio| {
                splitting.sent = Some((now, ratio));
                splitting.set(ratio)
            });
        self.splitting = Some(splitting);
        set
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
        if matches!(gesture.owner, Owner::Select) {
            self.select.end(visible(&self.views, gesture.pane));
        }
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
        if shown.view.holds_output() && drawn_scroll(&shown.view) != Some(metrics) {
            return None;
        }
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
        let relayout = std::mem::take(&mut self.relayout);
        if relayout {
            self.lay_out(attach, leave);
        }
        if (relayout || self.outer.stale()) && self.look() {
            self.outer.recheck();
        }
        let host = self.host_area();
        let chrome = (relayout || self.outer.stale())
            && self
                .outer
                .refresh(host, self.areas, &self.session, &self.palette);
        let focused = self.session.focused_id();
        if let Some(copy) = self.copy.as_mut() {
            if let Some((view, inner)) = visible(&self.views, copy.pane()) {
                let active = Some(copy.pane()) == focused;
                copy.follow(view, inner, &mut self.select, active);
            }
        }
        let mut rows = if relayout {
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
            if chrome {
                rows.extend_from_slice(self.outer.rows());
            }
            if !self.canvas.is_empty() {
                crate::protocol::surface_delta::apply_rows(&mut self.canvas, self.host.cols, &rows);
            }
            rows
        };
        let selected = self
            .select
            .pane()
            .and_then(|pane| visible(&self.views, pane));
        self.select.follow(selected);
        if chrome && self.outer.cover().is_some() {
            self.painted.cover = Rect::default();
        }
        if self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.stale(host))
        {
            self.painted.dialog = Rect::default();
        }
        self.paint(&mut rows);
        (rows, self.cursor())
    }

    fn overlays(&self) -> Overlays {
        let selection = self
            .select
            .shown()
            .and_then(|selection| {
                let (view, inner) = visible(&self.views, selection.pane_id)?;
                Some(selection.visible_rects(inner, drawn_scroll(view)))
            })
            .unwrap_or_default();
        let area = self.outer.bar_area(&self.areas);
        let copy = self
            .copy
            .as_ref()
            .and_then(|copy| {
                let (view, inner) = visible(&self.views, copy.pane())?;
                let active = !self.resizing && Some(copy.pane()) == self.session.focused_id();
                Some(copy.marks(inner, area, view.frame(), &self.select, active))
            })
            .unwrap_or_default();
        let resize = match self.resizing && !area.is_empty() {
            true => Rect::new(area.x, area.bottom() - 1, area.width, 1),
            false => Rect::default(),
        };
        Overlays {
            selection,
            copy,
            toast: self
                .toast
                .as_ref()
                .map_or_else(Rect::default, |toast| toast.image.0),
            cover: self
                .outer
                .cover()
                .map_or_else(Rect::default, |cover| cover.0),
            dialog: self
                .dialog
                .as_ref()
                .map_or_else(Rect::default, |_| self.host_area()),
            resize,
        }
    }

    fn paint(&mut self, rows: &mut Vec<PaneSurfacePatchRow>) {
        let overlays = self.overlays();
        let painted = std::mem::replace(&mut self.painted, overlays);
        let overlays = &self.painted;
        if painted == *overlays {
            if !overlays.cover.is_empty() || !overlays.dialog.is_empty() {
                rows.clear();
                return;
            }
            if *overlays == Overlays::default() {
                return;
            }
        }
        let mut ys: BTreeSet<u16> = rows
            .iter()
            .map(|row| row.y)
            .filter(|y| overlays.covers(*y))
            .collect();
        if painted != *overlays {
            for rect in painted.rects().chain(overlays.rects()) {
                ys.extend(rect.y..rect.bottom());
            }
        }
        rows.retain(|row| !ys.contains(&row.y));
        let cols = usize::from(self.canvas_cols);
        let copy = &overlays.copy;
        let bar = (!copy.bar.is_empty() && ys.contains(&copy.bar.y))
            .then(|| super::copy_mode::bar(copy, &self.palette));
        let resize = &overlays.resize;
        let resize_bar = (!resize.is_empty() && ys.contains(&resize.y))
            .then(|| resize_bar(resize.width, &self.palette));
        let mark = super::copy_mode::cursor_cell(&self.palette);
        let found = super::copy_mode::match_cell(&self.palette);
        for y in ys {
            let start = usize::from(y) * cols;
            let Some(cells) = self.canvas.get(start..start + cols) else {
                continue;
            };
            let mut cells = cells.to_vec();
            tint(&mut cells, &copy.matches, y, &found);
            for rect in overlays.selection.iter().filter(|rect| covers(rect, y)) {
                for cell in cells
                    .get_mut(usize::from(rect.x)..usize::from(rect.right()))
                    .into_iter()
                    .flatten()
                {
                    cell.fg = self.highlight.fg;
                    cell.bg = self.highlight.bg;
                    cell.modifier = self.highlight.modifier;
                }
            }
            tint(&mut cells, &copy.current, y, &mark);
            tint(&mut cells, std::slice::from_ref(&copy.cursor), y, &mark);
            if let Some(toast) = &self.toast {
                place(&mut cells, &toast.image, y);
            }
            if let (Some(to), Some(bar)) = (
                cells.get_mut(usize::from(copy.bar.x)..usize::from(copy.bar.right())),
                bar.as_deref().filter(|_| covers(&copy.bar, y)),
            ) {
                to.clone_from_slice(bar);
            }
            if let (Some(to), Some(bar)) = (
                cells.get_mut(usize::from(resize.x)..usize::from(resize.right())),
                resize_bar.as_deref().filter(|_| covers(resize, y)),
            ) {
                to.clone_from_slice(bar);
            }
            if let Some(switcher) = self.outer.cover().filter(|_| self.dialog.is_none()) {
                place(&mut cells, switcher, y);
            }
            rows.push(PaneSurfacePatchRow { x: 0, y, cells });
        }
        let host = self.host_area();
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        if dialog.stale(host) {
            let mut cells =
                vec![CellData::from_ratatui_cell(&Default::default()); host.area() as usize];
            crate::protocol::surface_delta::apply_rows(&mut cells, host.width, rows);
            let seed = crate::protocol::FrameData {
                cells,
                width: host.width,
                height: host.height,
                cursor: None,
                hyperlinks: Vec::new(),
                graphics: Vec::new(),
            }
            .to_ratatui_buffer()
            .unwrap_or_else(|| Buffer::empty(host));
            dialog.draw(seed, &self.keybinds, &self.palette);
        }
        if let Some(image) = dialog.image() {
            for row in rows.iter_mut() {
                place(&mut row.cells, image, row.y);
            }
        }
    }

    fn lay_out(
        &mut self,
        mut attach: impl FnMut(&str, PaneId, Geometry),
        mut leave: impl FnMut(PaneId, &View),
    ) {
        let areas = self.lay_out_chrome();
        if areas.pane_surface != self.areas.pane_surface {
            if let (Some(toast), Some(position)) = (&mut self.toast, self.toast_at) {
                toast.image = toast_image(areas.pane_surface, position, &self.palette);
            }
        }
        self.areas = areas;
        self.tiles = self.session.tiles(areas.pane_surface, self.chrome);
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
        crate::protocol::surface_delta::apply_rows(&mut cells, cols, self.outer.rows());
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
                    drawn_scroll(&shown.view),
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
        if let Some(dialog) = &self.dialog {
            return dialog.cursor();
        }
        if self.copy_mode() || self.switching() {
            return None;
        }
        let Shown {
            view,
            place: Place::Visible { inner, .. },
            ..
        } = self.views.get_mut(&self.session.focused_id()?)?
        else {
            return None;
        };
        let mut cursor = view
            .cursor()
            .filter(|cursor| cursor.x < inner.width && cursor.y < inner.height)?;
        cursor.x += inner.x;
        cursor.y += inner.y;
        (!covers(&self.painted.resize, cursor.y)).then_some(cursor)
    }
}

fn visible(views: &HashMap<PaneId, Shown>, id: PaneId) -> Option<(&View, Rect)> {
    match views.get(&id)? {
        Shown {
            view,
            place: Place::Visible { inner, .. },
            ..
        } => Some((view, *inner)),
        _ => None,
    }
}

fn tint(cells: &mut [CellData], rects: &[Rect], y: u16, style: &CellData) {
    for rect in rects.iter().filter(|rect| covers(rect, y)) {
        for cell in cells
            .get_mut(usize::from(rect.x)..usize::from(rect.right()))
            .into_iter()
            .flatten()
        {
            cell.fg = style.fg;
            cell.bg = style.bg;
            cell.modifier |= style.modifier;
        }
    }
}

fn covers(rect: &Rect, y: u16) -> bool {
    rect.y <= y && y < rect.bottom()
}

fn place(cells: &mut [CellData], (rect, image): &(Rect, Vec<CellData>), y: u16) {
    if !covers(rect, y) {
        return;
    }
    let width = usize::from(rect.width);
    let from = usize::from(y - rect.y) * width;
    if let (Some(to), Some(from)) = (
        cells.get_mut(usize::from(rect.x)..usize::from(rect.right())),
        image.get(from..from + width),
    ) {
        to.clone_from_slice(from);
    }
}

fn toast_image(
    area: Rect,
    position: ToastClipboardPosition,
    palette: &Palette,
) -> (Rect, Vec<CellData>) {
    let mut buffer = Buffer::empty(area);
    let feedback = crate::app::state::CopyFeedback {
        message: "copied to clipboard".to_owned(),
    };
    let rect =
        crate::ui::render_copy_feedback_buffer(&mut buffer, area, &feedback, 0, position, palette);
    let cells = (rect.y..rect.bottom())
        .flat_map(|y| (rect.x..rect.right()).map(move |x| (x, y)))
        .map(|position| CellData::from_ratatui_cell(&buffer[position]))
        .collect();
    (rect, cells)
}

fn resize_bar(width: u16, palette: &Palette) -> Vec<CellData> {
    let area = Rect::new(0, 0, width, 1);
    let mut buffer = Buffer::empty(area);
    let base = Style::default().fg(palette.overlay0).bg(palette.panel_bg);
    buffer.set_style(area, base);
    let key = Style::default()
        .fg(palette.accent)
        .bg(palette.panel_bg)
        .add_modifier(Modifier::BOLD);
    let mode = Style::default()
        .fg(crate::ui::chrome::panel_contrast_fg(palette))
        .bg(palette.mauve)
        .add_modifier(Modifier::BOLD);
    let mut x = 0;
    for (text, style) in [
        (" RESIZE ", mode),
        ("  ", base),
        ("h/l", key),
        (" width  ", base),
        ("j/k", key),
        (" height  ", base),
        ("esc", key),
        (" done", base),
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

fn drawn_scroll(view: &View) -> Option<ScrollMetrics> {
    view.frame().and_then(|frame| frame.scroll)
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
