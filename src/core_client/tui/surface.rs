use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Direction, Position, Rect};
use ratatui::style::{Modifier, Style};

use super::chrome::Chrome;
use super::copy_mode::{CopyMode, Marks, Step};
use super::dialog::{
    Aim, Change, Chosen, Dialog, Item, Menu, Modal, Move, Outcome, Preferences, Rename,
};
use super::gate::Gate;
use super::keys::Navigation;
use super::notices::{Effect, Notices, Notifying};
use super::select::{Copying, Select};
use super::session::{Operation, PaneChrome, Request, Session, Tile, Tiles};
use super::terminals::Report;
use crate::config::{Config, LiveKeybindConfig, ToastClipboardPosition};
use crate::core_client::view::{Geometry, Placing, Scrollback, View};
use crate::input::{KeybindAction, TerminalKey};
use crate::layout::{PaneId, SplitBorder};
use crate::pane::{ScrollMetrics, MIN_PANE_COLS, MIN_PANE_ROWS};
use crate::protocol::{CellData, CursorState, PaneSurfacePatchRow};
use crate::raw_input::RawInputEvent;
use crate::session_doc::{Arrangement, Destination, Layout, Loaded};
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
    unreported: KeyModifiers,
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
    notice: Rect,
    shows: u64,
    toast: Rect,
    cover: Rect,
    dialog: Rect,
    resize: Rect,
    bar: Rect,
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
                &self.notice,
                &self.toast,
                &self.cover,
                &self.dialog,
                &self.resize,
                &self.bar,
            ])
    }

    fn covers(&self, y: u16) -> bool {
        self.rects().any(|rect| covers(rect, y))
    }

    fn hides(&self, point: ratatui::layout::Position) -> bool {
        [self.cover, self.dialog]
            .iter()
            .any(|rect| rect.contains(point))
    }

    fn toasts(&self) -> (Rect, u64, Rect) {
        (self.notice, self.shows, self.toast)
    }

    fn bars(&self, point: ratatui::layout::Position) -> bool {
        [self.copy.bar, self.resize, self.bar]
            .iter()
            .any(|rect| rect.contains(point))
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum Pointed {
    Copied(String),
    Act(KeybindAction),
    Run(Operation),
    Save(crate::config::ConfigEdit<'static>),
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

pub(super) enum Arrival {
    Layout(Loaded),
    Report(Report),
}

impl From<Loaded> for Arrival {
    fn from(loaded: Loaded) -> Self {
        Self::Layout(loaded)
    }
}

impl From<Report> for Arrival {
    fn from(report: Report) -> Self {
        Self::Report(report)
    }
}

pub(super) struct Settings {
    pub(super) chrome: PaneChrome,
    pub(super) palette: Palette,
    pub(super) scrollbars: bool,
    pub(super) copying: Copying,
    pub(super) keybinds: LiveKeybindConfig,
    pub(super) confirm_close: bool,
    pub(super) right_click_passthrough: Option<KeyModifiers>,
    pub(super) scrollback: Scrollback,
    pub(super) chosen: Chosen,
}

impl From<&Config> for Settings {
    fn from(config: &Config) -> Self {
        let ui = &config.ui;
        Self {
            chrome: PaneChrome {
                borders: ui.pane_borders,
                gaps: ui.pane_gaps,
                outer_borders: ui.pane_outer_borders,
            },
            palette: crate::ui::theme::client_palette_from_config(config),
            scrollbars: ui.pane_scrollbars,
            copying: Copying {
                on_select: ui.copy_on_select,
                toast: ui
                    .toast
                    .clipboard
                    .enabled
                    .then_some(ui.toast.clipboard.position),
            },
            keybinds: config
                .live_keybinds_with_diagnostics()
                .map(|(keybinds, _diagnostics)| keybinds)
                .unwrap_or_else(|_diagnostics| LiveKeybindConfig {
                    prefix: config.prefix_keys(),
                    keybinds: config.keybinds(),
                }),
            confirm_close: ui.confirm_close,
            right_click_passthrough: ui.right_click_passthrough_modifiers(),
            scrollback: Scrollback::from(config),
            chosen: Chosen::from(config),
        }
    }
}

pub(super) fn reloaded(
    running: &mut Config,
    mut next: Config,
    invalid: &[String],
) -> (Config, Vec<String>) {
    let mut diagnostics = Vec::new();
    let failed = |section: &str| invalid.iter().any(|invalid| invalid == section);
    let bad_keys = match next.live_keybinds_with_diagnostics() {
        Ok((_, warnings)) => {
            diagnostics.extend(warnings);
            false
        }
        Err(errors) => {
            diagnostics.extend(
                errors
                    .into_iter()
                    .map(|error| format!("{error}; kept current keybinds")),
            );
            true
        }
    };
    let bounds = next.invalid_sidebar_bounds_diagnostic();
    let bad_bounds = bounds.is_some();
    diagnostics.extend(bounds.map(|bounds| format!("{bounds}; keeping previous [ui] settings")));
    if !failed("ui") && !bad_bounds {
        diagnostics.extend(next.ui.sidebar.spaces.diagnostics());
        diagnostics.extend(crate::config::tab_bar_right_diagnostics(
            &next.ui.tab_bar_right,
        ));
    }
    if failed("theme") {
        next.theme = std::mem::take(&mut running.theme);
    }
    if failed("keys") || bad_keys {
        next.keys = std::mem::take(&mut running.keys);
    }
    if failed("ui") || bad_bounds {
        next.ui = std::mem::take(&mut running.ui);
    }
    if failed("advanced") {
        next.advanced = std::mem::take(&mut running.advanced);
    }
    (next, diagnostics)
}

fn highlight(palette: &Palette) -> CellData {
    let mut highlight = ratatui::buffer::Cell::default();
    highlight.set_style(crate::ui::automatic_selection_style(
        palette,
        crate::terminal_theme::TerminalTheme::default(),
    ));
    CellData::from_ratatui_cell(&highlight)
}

pub(super) struct Surface {
    session: Session,
    views: HashMap<PaneId, Shown>,
    attaching: HashMap<PaneId, bool>,
    owned: HashMap<PaneId, String>,
    targets: HashSet<PaneId>,
    ended: Vec<PaneId>,
    presented: Option<HashSet<PaneId>>,
    host_focused: bool,
    tiles: Tiles,
    chrome: PaneChrome,
    outer: Chrome,
    areas: crate::ui::chrome::Layout,
    palette: Palette,
    unpreviewed: Option<Palette>,
    chosen: Chosen,
    host: Geometry,
    scrollbars: bool,
    gesture: Option<Gesture>,
    splitting: Option<Splitting>,
    select: Select,
    copy: Option<CopyMode>,
    dialog: Option<Modal>,
    confirm_close: bool,
    kitty: bool,
    right_click_passthrough: Option<KeyModifiers>,
    resizing: bool,
    prefixed: bool,
    keybinds: LiveKeybindConfig,
    toast_at: Option<ToastClipboardPosition>,
    toast: Option<Toast>,
    notices: Notices,
    notice: Option<(Rect, Vec<CellData>)>,
    placed: (u64, Rect, bool, Rect),
    scrollback: Scrollback,
    highlight: CellData,
    painted: Overlays,
    relayout: bool,
    laid_for: Option<Rect>,
    #[cfg(test)]
    retargets: usize,
    canvas: Vec<CellData>,
    canvas_cols: u16,
}

impl Surface {
    pub(super) fn new(host: Geometry, outer: Chrome, settings: Settings) -> Self {
        let Settings {
            chrome,
            palette,
            scrollbars,
            copying,
            keybinds,
            confirm_close,
            right_click_passthrough,
            scrollback,
            chosen,
        } = settings;
        Self {
            session: Session::default(),
            views: HashMap::new(),
            attaching: HashMap::new(),
            owned: HashMap::new(),
            targets: HashSet::new(),
            ended: Vec::new(),
            presented: None,
            host_focused: true,
            tiles: Tiles::default(),
            chrome,
            outer,
            areas: crate::ui::chrome::Layout::default(),
            highlight: highlight(&palette),
            palette,
            unpreviewed: None,
            chosen,
            host,
            scrollbars,
            gesture: None,
            splitting: None,
            select: Select::new(copying.on_select),
            copy: None,
            dialog: None,
            confirm_close,
            kitty: false,
            right_click_passthrough,
            resizing: false,
            prefixed: false,
            keybinds,
            toast_at: copying.toast,
            toast: None,
            notices: Notices::default(),
            notice: None,
            placed: Default::default(),
            scrollback,
            painted: Overlays::default(),
            relayout: true,
            laid_for: None,
            #[cfg(test)]
            retargets: 0,
            canvas: Vec::new(),
            canvas_cols: 0,
        }
    }

    pub(super) fn notifying(mut self, notifying: Notifying) -> Self {
        self.notices = Notices::new(notifying);
        self
    }

    pub(super) fn configure(&mut self, config: &Config) {
        let Settings {
            chrome,
            palette,
            scrollbars,
            copying,
            keybinds,
            confirm_close,
            right_click_passthrough,
            scrollback,
            chosen,
        } = Settings::from(config);
        self.outer.configure(config);
        self.chrome = chrome;
        self.unpreviewed = None;
        self.chosen = chosen;
        self.scrollbars = scrollbars;
        self.select.copy_on_select(copying.on_select);
        self.toast_at = copying.toast;
        self.toast = None;
        self.keybinds = keybinds;
        self.confirm_close = confirm_close;
        self.right_click_passthrough = right_click_passthrough;
        self.scrollback = scrollback;
        self.notices.configure(Notifying::from_config(config));
        for shown in self.views.values_mut() {
            shown.view.scroll_by(scrollback.wheel_lines);
        }
        self.restyle(palette);
    }

    fn restyle(&mut self, palette: Palette) {
        self.highlight = highlight(&palette);
        self.palette = palette;
        self.outer.restyle();
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.restyle();
        }
        self.relayout = true;
    }

    fn preview(&mut self, theme: usize) {
        let Some(palette) = self.chosen.palette(theme) else {
            return;
        };
        self.unpreviewed.get_or_insert_with(|| self.palette.clone());
        self.chosen.preview(Some(theme));
        self.restyle(palette);
    }

    fn unpreview(&mut self) {
        if let Some(palette) = self.unpreviewed.take() {
            self.chosen.preview(None);
            self.restyle(palette);
        }
    }

    pub(super) fn saved(&mut self, written: bool) {
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        if dialog.saved(&self.chosen, written) == Outcome::Close {
            self.dialog = None;
        }
        self.unpreview();
    }

    pub(super) fn keybinds(&self) -> &LiveKeybindConfig {
        &self.keybinds
    }

    pub(super) fn scrollback(&self) -> Scrollback {
        self.scrollback
    }

    pub(super) fn confirms_close(&self) -> bool {
        self.confirm_close
    }

    pub(super) fn on_a_kitty_host(mut self, kitty: bool) -> Self {
        self.kitty = kitty;
        self
    }

    #[cfg(test)]
    pub(super) fn confirming_close(mut self, confirm_close: bool) -> Self {
        self.confirm_close = confirm_close;
        self
    }

    pub(super) fn holds(&self, layout: &Layout) -> bool {
        self.session.holds(layout)
    }

    pub(super) fn update(&mut self, layout: Layout) {
        self.session.update(layout);
        self.owned = self.session.owned();
        self.reflow();
        self.session_changed(Change::Layout);
    }

    fn session_changed(&mut self, change: Change) {
        let session = &self.session;
        if !self
            .dialog
            .as_mut()
            .is_none_or(|dialog| dialog.session_changed(session, change))
        {
            self.dialog = None;
        }
    }

    pub(super) fn act(&mut self, action: KeybindAction, confirm_close: bool) -> Option<Operation> {
        if action == KeybindAction::ToggleSidebar {
            self.outer.toggle();
            self.reflow();
            return None;
        }
        if action == KeybindAction::Help {
            self.open(Dialog::Help(Default::default()));
            return None;
        }
        if action == KeybindAction::Settings {
            self.open(Dialog::Settings(Preferences::new(&self.chosen)));
            return None;
        }
        if action == KeybindAction::EnterResizeMode {
            self.outer.leave();
            self.resizing = true;
            return None;
        }
        if matches!(
            action,
            KeybindAction::RenameWorkspace | KeybindAction::RenameTab | KeybindAction::RenamePane
        ) {
            if let Some(rename) = Rename::of(action, &self.session, self.outer.selected()) {
                self.open(Dialog::Rename(rename));
            }
            return None;
        }
        if action == KeybindAction::MovePane {
            if let Some(moving) = self
                .session
                .focused()
                .and_then(|terminal| Move::of(&self.session, terminal))
            {
                self.open(Dialog::Move(moving));
            }
            return None;
        }
        if action == KeybindAction::WorkspacePicker {
            self.outer.navigate(&self.session);
            if self.outer.switching() {
                self.let_go();
            }
            return None;
        }
        let area = self.lay_out_chrome().pane_surface;
        if self.session.navigate(action, area, self.chrome) {
            self.reflow();
            self.session_changed(Change::Layout);
        }
        if let Some(arrangement) = self.session.arrangement(action, area) {
            return Some(Operation::Arrange(arrangement));
        }
        let request = self
            .session
            .operation(action, confirm_close, self.outer.selected());
        self.request(request)
    }

    fn request(&mut self, request: Option<Request>) -> Option<Operation> {
        match request? {
            Request::Run(operation) => Some(operation),
            Request::Confirm(closing) => {
                self.open(Dialog::Confirm(closing));
                None
            }
        }
    }

    fn choose(&mut self, aim: Aim, item: Item) -> Option<Operation> {
        let confirm_close = self.confirm_close;
        let (terminal, action) = match (aim, item) {
            (Aim::Workspace(id), Item::Rename) => {
                let workspace = self.session.workspaces().iter().find(|w| w.id == id)?;
                let rename = Rename::workspace(&self.session, workspace);
                self.open(Dialog::Rename(rename));
                return None;
            }
            (Aim::Workspace(id), Item::Close) => {
                let request =
                    self.session
                        .operation(KeybindAction::CloseWorkspace, confirm_close, Some(&id));
                return self.request(request);
            }
            (Aim::Tab(id), item) => {
                let action = match item {
                    Item::NewTab => KeybindAction::NewTab,
                    Item::Rename => KeybindAction::RenameTab,
                    Item::Close => KeybindAction::CloseTab,
                    _ => return None,
                };
                let index = self
                    .session
                    .focused_workspace()?
                    .tabs
                    .iter()
                    .position(|tab| tab.id == id)?;
                self.act(KeybindAction::SwitchTab(index), confirm_close);
                return self.act(action, confirm_close);
            }
            (Aim::Pane(terminal), item) => match item {
                Item::RenamePane => {
                    if let Some(rename) = Rename::pane(&self.session, &terminal) {
                        self.open(Dialog::Rename(rename));
                    }
                    return None;
                }
                Item::Move => {
                    if let Some(moving) = Move::of(&self.session, &terminal) {
                        self.open(Dialog::Move(moving));
                    }
                    return None;
                }
                Item::ClearPaneName => {
                    return Some(Operation::Name {
                        terminal,
                        name: String::new(),
                    });
                }
                Item::SwapWithFocused(focused) => {
                    return Some(Operation::Arrange(Arrangement::SwapWith {
                        terminal: focused,
                        with: terminal,
                    }));
                }
                Item::Zoom => {
                    if self.session.focus(&terminal) {
                        self.relayout = true;
                        self.sync_focus();
                    }
                    if self.session.focused() != Some(terminal.as_str()) {
                        return None;
                    }
                    return self.act(KeybindAction::Zoom, confirm_close);
                }
                Item::Park => (terminal, KeybindAction::ParkPane),
                Item::SplitRight => (terminal, KeybindAction::SplitVertical),
                Item::SplitDown => (terminal, KeybindAction::SplitHorizontal),
                Item::ClosePane => (terminal, KeybindAction::ClosePane),
                Item::Rename
                | Item::Close
                | Item::NewTab
                | Item::Settings
                | Item::Keybinds
                | Item::ReloadConfig
                | Item::Detach => return None,
            },
            (Aim::Workspace(_) | Aim::Global, _) => return None,
        };
        let request = self.session.operation_on(&terminal, action, confirm_close);
        self.request(request)
    }

    fn open(&mut self, dialog: Dialog) {
        self.let_go();
        self.outer.leave();
        self.dialog = Some(Modal::new(dialog));
    }

    fn open_menu(&mut self, menu: Menu) {
        if menu.on_screen(&self.session) {
            self.open(Dialog::Menu(menu));
        }
    }

    fn let_go(&mut self) {
        self.end_gesture(None);
        self.end_drags();
        self.outer.let_go();
    }

    pub(super) fn dialog_open(&self) -> bool {
        self.dialog.is_some()
    }

    pub(super) fn wants_all_keys(&self) -> bool {
        self.focused().is_some_and(View::reports_all_keys)
            || self.kitty
                && self
                    .dialog
                    .as_ref()
                    .is_some_and(Modal::leads_to_a_confirmation)
    }

    pub(super) fn dialog_input(&mut self, event: &RawInputEvent) -> bool {
        let Some(dialog) = self.dialog.as_mut() else {
            return false;
        };
        match event {
            RawInputEvent::Text(text) => dialog.text(text.as_str(), &self.session),
            RawInputEvent::Paste(text) => dialog.text(text, &self.session),
            _ => {}
        }
        true
    }

    pub(super) fn dialog_key(&mut self, key: &TerminalKey) -> Option<Pointed> {
        let dialog = self.dialog.as_mut()?;
        let outcome = dialog.key(key, &self.keybinds, &self.session, &self.chosen);
        let pointed = self.settle(outcome);
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.after(key);
        }
        pointed
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

    fn settle(&mut self, outcome: Outcome) -> Option<Pointed> {
        if outcome == Outcome::Stay {
            return None;
        }
        if let Outcome::Save(edit) = outcome {
            return Some(Pointed::Save(edit));
        }
        if let Outcome::Preview(theme) = outcome {
            self.preview(theme);
            return None;
        }
        let dialog = self.dialog.take()?;
        self.unpreview();
        match outcome {
            Outcome::Accept => match dialog.chosen(&self.session) {
                Some((Aim::Global, item)) => item.action().map(Pointed::Act),
                Some((aim, item)) => self.choose(aim, item).map(Pointed::Run),
                None => dialog.accepted(&self.session).map(Pointed::Run),
            },
            Outcome::Stay | Outcome::Close | Outcome::Save(_) | Outcome::Preview(_) => None,
        }
    }

    pub(super) fn navigate(&mut self, step: Navigation) -> Option<KeybindAction> {
        match step {
            Navigation::Move(by) => {
                let mobile = !self.lay_out_chrome().mobile_header.is_empty();
                self.outer.select(by, mobile, &self.session);
            }
            Navigation::Accept => return self.outer.accept(&self.session),
            Navigation::Cycle(step) => {
                self.outer.leave();
                return (!self.session.zoomed()).then_some(if step < 0 {
                    KeybindAction::CyclePanePrevious
                } else {
                    KeybindAction::CyclePaneNext
                });
            }
            Navigation::Jump(action) => {
                let reaches = match action {
                    KeybindAction::SwitchWorkspace(index) => {
                        self.session.workspaces().get(index).is_some()
                    }
                    KeybindAction::SwitchTab(index) => self
                        .session
                        .focused_workspace()
                        .is_some_and(|workspace| workspace.tabs.get(index).is_some()),
                    _ => true,
                };
                if reaches {
                    self.outer.leave();
                    return Some(action);
                }
            }
        }
        None
    }

    fn hear(&mut self, report: Report) {
        let change = match report {
            Report::Status(..) | Report::Removed(_) => None,
            Report::Names(_) | Report::Name(..) => Some(Change::Names),
            Report::Snapshot { .. } | Report::Folder(..) => Some(Change::Folders),
        };
        if let Some(change) = change {
            self.session_changed(change);
        }
        let names = change == Some(Change::Names);
        let tiles = &self.tiles.tiles;
        if self.session.heard(report, |terminal| {
            tiles.iter().any(|tile| {
                tile.terminal == terminal && crate::ui::pane_border_shows_title(&tile.info)
            })
        }) {
            if names {
                self.relayout = true;
            } else {
                self.outer.recheck();
            }
        }
    }

    fn judge(&mut self, now: Instant) -> Vec<Effect> {
        for moved in self.session.moves() {
            self.notices.hear(moved, &self.session, now);
        }
        self.notify(now)
    }

    fn notify(&mut self, now: Instant) -> Vec<Effect> {
        let effects = self.notices.deliver(&self.session, self.host_focused, now);
        self.place_toasts();
        effects
    }

    pub(super) fn open_notice(&mut self, now: Instant) {
        if self.painted.shows != self.notices.shows() {
            return;
        }
        let opened = self.notices.open(&self.session, now);
        self.place_toasts();
        if opened.is_some_and(|terminal| self.session.reveal(&terminal)) {
            self.relayout = true;
            self.sync_focus();
        }
    }

    fn place_toasts(&mut self) {
        let host = Rect::new(0, 0, self.host.cols, self.host.rows);
        let mobile = !self.areas.mobile_header.is_empty();
        let placed = (self.notices.shows(), host, mobile, self.areas.pane_surface);
        if std::mem::replace(&mut self.placed, placed) == placed {
            return;
        }
        let position = self.notices.position();
        self.notice = self
            .notices
            .shown()
            .map(|notice| notice.image(host, mobile, position, &self.palette));
        if let (Some(toast), Some(position)) = (&mut self.toast, self.toast_at) {
            toast.image = toast_image(
                self.areas.pane_surface,
                position,
                self.notice
                    .as_ref()
                    .map_or_else(Rect::default, |notice| notice.0),
                &self.palette,
            );
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

    pub(super) fn started(&mut self, request: u64, terminal: Option<String>) {
        if self.session.started(request, terminal) {
            self.reflow();
            self.session_changed(Change::Layout);
        }
    }

    pub(super) fn host_focus(&mut self, focused: bool) {
        self.host_focused = focused;
        if focused {
            self.outer.recheck();
        } else {
            self.end_gesture(None);
            self.end_drags();
            if let Some(dialog) = self.dialog.as_mut() {
                dialog.host_unfocused();
            }
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
        for id in &self.targets {
            if let Some(shown) = self.views.get_mut(id) {
                shown.tell_focus(focused == Some(*id));
            }
        }
    }

    fn reflow(&mut self) {
        self.relayout = true;
        let area = self.lay_out_chrome().pane_surface;
        self.tiles = self.session.tiles(area, self.chrome);
        self.laid_for = Some(area);
        if self
            .gesture
            .is_some_and(|gesture| !self.placed(gesture.pane))
        {
            self.end_gesture(None);
        }
        self.retarget();
    }

    fn placed(&self, id: PaneId) -> bool {
        let Some(Shown {
            place: Place::Visible { inner, gutter, .. },
            ..
        }) = self.views.get(&id)
        else {
            return false;
        };
        let track = gutter.as_ref().map(|gutter| gutter.track);
        self.tiles.tiles.iter().any(|tile| {
            tile.info.id == id
                && crate::ui::scrollbar_gutter_for(tile.info.inner_rect, gutter.is_some())
                    == (*inner, track)
        })
    }

    fn retarget(&mut self) {
        #[cfg(test)]
        {
            self.retargets += 1;
        }
        let blank: HashSet<PaneId> = self
            .tiles
            .tiles
            .iter()
            .filter(|tile| tile.info.inner_rect.is_empty())
            .map(|tile| tile.info.id)
            .collect();
        let targets: HashSet<PaneId> = self
            .session
            .reachable()
            .into_iter()
            .filter(|id| !blank.contains(id))
            .collect();
        for id in self.targets.difference(&targets) {
            if let Some(shown) = self.views.get_mut(id) {
                if shown.told_focus == Some(true) {
                    shown.tell_focus(false);
                }
                shown.told_focus = None;
                self.ended.push(*id);
            }
        }
        self.targets = targets;
        self.sync_focus();
    }

    pub(super) fn end_input(&mut self, mut leave: impl FnMut(PaneId, &View)) {
        for id in std::mem::take(&mut self.ended) {
            if let Some(shown) = self.views.get(&id) {
                leave(id, &shown.view);
            }
        }
    }

    pub(super) fn resize(&mut self, host: Geometry) {
        self.host = host;
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.forget();
        }
        self.reflow();
    }

    fn lay_out_chrome(&self) -> crate::ui::chrome::Layout {
        self.outer
            .layout(self.host.cols, self.host.rows, &self.session)
    }

    pub(super) fn copyable(&self) -> bool {
        self.dialog.is_none() && !self.copy_mode() && self.select.copyable()
    }

    pub(super) fn navigating(&self) -> bool {
        self.outer.navigating()
    }

    pub(super) fn navigate_session(&self) -> Option<u64> {
        self.outer.navigate_session()
    }

    pub(super) fn hold_prefix(&mut self, prefixed: bool) {
        self.prefixed = prefixed;
    }

    pub(super) fn selection_vanished(&self) -> bool {
        self.outer.vanished(&self.session)
    }

    pub(super) fn leave_navigate(&mut self) {
        self.outer.leave();
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
                image: toast_image(
                    self.areas.pane_surface,
                    position,
                    self.notice
                        .as_ref()
                        .map_or_else(Rect::default, |notice| notice.0),
                    &self.palette,
                ),
            });
        }
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.select
            .deadline()
            .into_iter()
            .chain(self.toast.as_ref().map(|toast| toast.until))
            .chain(self.notices.deadline())
            .chain(self.outer.deadline())
            .chain(self.splitting.as_ref().and_then(Splitting::due))
            .min()
    }

    pub(super) fn received(&mut self, arrival: Arrival, gate: &mut Gate) {
        match arrival {
            Arrival::Layout(loaded) => {
                let redraws = !self.holds(&loaded.layout);
                gate.arrived(loaded.version, redraws);
                if redraws {
                    self.update(loaded.layout);
                }
            }
            Arrival::Report(report) => self.hear(report),
        }
    }

    pub(super) fn caught_up(
        &mut self,
        gate: &mut Gate,
        arrivals: &mut tokio::sync::mpsc::UnboundedReceiver<Arrival>,
        now: Instant,
    ) -> (Option<Operation>, Vec<Effect>) {
        while let Ok(arrival) = arrivals.try_recv() {
            self.received(arrival, gate);
        }
        let operation = self.tick(now, gate.holding());
        (operation, self.judge(now))
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
        let on_screen = self
            .tiles
            .tiles
            .iter()
            .any(|tile| tile.info.id == id && !tile.info.inner_rect.is_empty());
        match view {
            Ok(mut view) if !detached && self.terminal(id).is_some() => {
                view.scroll_by(self.scrollback.wheel_lines);
                self.relayout |= on_screen;
                self.views.insert(
                    id,
                    Shown {
                        view,
                        place: Place::Hidden(Vec::new()),
                        geometry,
                        told_focus: None,
                    },
                );
            }
            Ok(_) => self.relayout |= on_screen,
            Err(error) => {
                self.relayout |= on_screen;
                tracing::warn!(%error, terminal = self.terminal(id), "the pane did not attach");
            }
        }
        let focused = self.reporting_focus() == Some(id);
        if let Some(shown) = self
            .views
            .get_mut(&id)
            .filter(|_| self.targets.contains(&id))
        {
            shown.tell_focus(focused);
        }
    }

    pub(super) fn ended(&self, id: PaneId) -> Option<Operation> {
        Some(Operation::Close(vec![self.terminal(id)?.to_owned()]))
    }

    pub(super) fn presented(&mut self) -> Option<HashSet<PaneId>> {
        self.presented.take()
    }

    fn terminal(&self, id: PaneId) -> Option<&str> {
        self.owned.get(&id).map(String::as_str)
    }

    #[cfg(test)]
    pub(super) fn focused_id(&self) -> Option<PaneId> {
        self.session.focused_id()
    }

    pub(super) fn input_focus(&self) -> Option<PaneId> {
        self.session
            .focused_id()
            .filter(|id| self.view(*id).is_some())
    }

    pub(super) fn focused(&self) -> Option<&View> {
        self.view(self.session.focused_id()?)
    }

    pub(super) fn folder(&self, operation: &Operation) -> Option<PathBuf> {
        let terminal = match operation {
            Operation::Start(_, Destination::Split { beside, .. }) => beside,
            _ => self.session.focused()?,
        };
        self.session
            .id_of(terminal)
            .and_then(|id| self.view(id)?.cwd())
            .or_else(|| self.session.folder(terminal).cloned())
    }

    pub(super) fn view(&self, id: PaneId) -> Option<&View> {
        self.targets
            .contains(&id)
            .then(|| self.views.get(&id).map(|shown| &shown.view))?
    }

    pub(super) fn mouse(&mut self, event: MouseEvent, now: Instant) -> Option<Pointed> {
        let press = event.kind == MouseEventKind::Down(MouseButton::Left);
        if let Some(dialog) = self.dialog.as_mut() {
            let point = Position::new(event.column, event.row);
            if press && dialog.launched() && self.outer.on_launcher(point) {
                self.dialog = None;
                return None;
            }
            let outcome = dialog.mouse(event, &self.session, &self.chosen);
            return self.settle(outcome);
        }
        if press {
            self.splitting = None;
        }
        if press && !self.outer.switching() && self.selection_vanished() {
            self.outer.leave();
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
                            let mut event = event;
                            event.modifiers.remove(gesture.unreported);
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
        let point = Position::new(event.column, event.row);
        if !self.outer.switching() && self.painted.hides(point) {
            return None;
        }
        if press
            && !self.outer.switching()
            && !self.copy_mode()
            && !self.resizing
            && !self.outer.navigating()
            && !self.painted.bars(point)
            && self.painted.notice.contains(point)
        {
            self.open_notice(now);
            return None;
        }
        let hidden = match self.painted.resize.is_empty() {
            true => self.painted.copy.bar,
            false => self.painted.resize,
        };
        let right = event.kind == MouseEventKind::Down(MouseButton::Right);
        if right && self.outer.pressing() {
            return None;
        }
        let opens = !self.copy_mode() && !self.resizing;
        let menu = right && opens;
        if self.outer.claim(event, hidden) {
            if press && self.outer.on_launcher(point) {
                self.select.press(None, event, now);
                if opens && !self.outer.switching() {
                    self.open_menu(Menu::launcher());
                }
                return None;
            }
            let chrome_menu = if !opens || self.outer.switching() {
                None
            } else if right {
                self.outer.aimed(point).map(|aim| Menu::new(aim, point))
            } else {
                None
            };
            if let Some(menu) = chrome_menu {
                self.open_menu(menu);
                return None;
            }
            if press {
                self.select.press(None, event, now);
            }
            let before = self.lay_out_chrome();
            let pointed = self.outer.mouse(
                event,
                &self.session,
                now,
                (!self.copy_mode() || self.resizing) && !self.prefixed,
            );
            if let Some(terminal) = self.outer.revealed() {
                if self.session.reveal(&terminal) {
                    self.reflow();
                }
            }
            self.resizing &= !self.outer.navigating();
            if self.lay_out_chrome() != before {
                self.reflow();
            }
            return pointed;
        }
        if matches!(event.kind, MouseEventKind::Down(_)) && self.painted.bars(point) {
            if press {
                self.select.press(None, event, now);
            }
            return None;
        }
        if right && self.pass_right_click(event, point) {
            return None;
        }
        if menu {
            if let Some(tile) = self
                .tiles
                .tiles
                .iter()
                .find(|tile| tile.info.rect.contains(point))
                .filter(|_| !self.relayout)
            {
                let aim = Aim::Pane(tile.terminal.clone());
                self.open_menu(Menu::new(aim, point));
            }
            return None;
        }
        if press {
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
            self.reflow();
        }
        if !self.placed(id) {
            if press {
                self.select.press(None, event, now);
            }
            return None;
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
        if press && on_bar && self.bar(id).is_some() {
            self.outer.leave();
        }
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
            unreported: KeyModifiers::NONE,
        });
        None
    }

    fn pass_right_click(&mut self, mut event: MouseEvent, point: Position) -> bool {
        let Some(unreported) = self
            .right_click_passthrough
            .filter(|modifiers| *modifiers == event.modifiers)
        else {
            return false;
        };
        let Some(tile) = self
            .tiles
            .tiles
            .iter()
            .find(|tile| tile.info.inner_rect.contains(point))
            .filter(|tile| {
                self.placed(tile.info.id)
                    && visible(&self.views, tile.info.id)
                        .is_some_and(|(_, inner)| inner.contains(point))
            })
        else {
            return false;
        };
        let (id, terminal) = (tile.info.id, tile.terminal.clone());
        event.modifiers.remove(unreported);
        let Some((last, true)) = self.to_terminal(id, event) else {
            return false;
        };
        if self.session.focus(&terminal) {
            self.reflow();
        }
        self.gesture = Some(Gesture {
            pane: id,
            button: MouseButton::Right,
            owner: Owner::Terminal { last },
            unreported,
        });
        true
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
        attach: impl FnMut(&str, PaneId, Placing),
        leave: impl FnMut(PaneId, &View),
    ) -> (Vec<PaneSurfacePatchRow>, Option<CursorState>) {
        let focused = self.reporting_focus();
        for tile in &self.tiles.tiles {
            let id = tile.info.id;
            let Some(shown) = self
                .views
                .get_mut(&id)
                .filter(|_| dirty.contains(&id) && self.targets.contains(&id))
            else {
                continue;
            };
            shown.tell_focus(focused == Some(id));
            self.relayout = self.relayout || shown.gutter_is_stale(self.scrollbars);
        }
        let relayout = std::mem::take(&mut self.relayout);
        if relayout {
            self.lay_out(attach, leave);
        }
        if (relayout || self.outer.stale()) && self.look() {
            self.outer.recheck();
        }
        let host = self.host_area();
        let switching = self.outer.switching();
        let chrome = (relayout || self.outer.stale())
            && self
                .outer
                .refresh(host, self.areas, &self.session, &self.palette);
        if !switching && self.outer.switching() {
            self.let_go();
        }
        if self.outer.launcher().is_none() && self.dialog.as_ref().is_some_and(Modal::launched) {
            self.dialog = None;
        }
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
            for tile in &self.tiles.tiles {
                let id = tile.info.id;
                if !dirty.contains(&id) {
                    continue;
                }
                if let Some(Shown {
                    view,
                    place: Place::Visible { inner, gutter, .. },
                    ..
                }) = self.views.get_mut(&id)
                {
                    rows.extend(placed(view.draw(), *inner));
                    if let Some(gutter) = gutter {
                        rows.extend(gutter.redraw(view, &self.palette, focused == Some(id)));
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
                let active = !self.resizing
                    && !self.outer.navigating()
                    && Some(copy.pane()) == self.session.focused_id();
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
            notice: self
                .notice
                .as_ref()
                .map_or_else(Rect::default, |notice| notice.0),
            shows: self.notice.as_ref().map_or(0, |_| self.notices.shows()),
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
            bar: self.outer.bar().map_or_else(Rect::default, |bar| bar.0),
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
        } else if let Some(dialog) = self
            .dialog
            .as_mut()
            .filter(|_| painted.toasts() != overlays.toasts())
        {
            dialog.redraw();
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
            let toasts = self
                .notice
                .iter()
                .chain(self.toast.as_ref().map(|toast| &toast.image));
            for image in toasts {
                place(&mut cells, image, y);
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
            for image in self.outer.cover().into_iter().chain(self.outer.bar()) {
                place(&mut cells, image, y);
            }
            rows.push(PaneSurfacePatchRow { x: 0, y, cells });
        }
        let host = self.host_area();
        let launcher = self.outer.launcher();
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
            dialog.draw(
                seed,
                launcher,
                &self.keybinds,
                &self.chosen,
                &self.palette,
                &self.session,
            );
        }
        if let Some(image) = dialog.image() {
            for row in rows.iter_mut() {
                place(&mut row.cells, image, row.y);
            }
        }
    }

    fn lay_out(
        &mut self,
        mut attach: impl FnMut(&str, PaneId, Placing),
        mut leave: impl FnMut(PaneId, &View),
    ) {
        let areas = self.lay_out_chrome();
        self.end_input(&mut leave);
        self.areas = areas;
        self.place_toasts();
        if self.laid_for.take() != Some(areas.pane_surface) {
            self.tiles = self.session.tiles(areas.pane_surface, self.chrome);
        }
        self.presented = Some(
            self.tiles
                .tiles
                .iter()
                .filter(|tile| !tile.info.inner_rect.is_empty())
                .map(|tile| tile.info.id)
                .collect(),
        );
        let mut views = std::mem::take(&mut self.views);
        for tile in &self.tiles.tiles {
            if tile.info.inner_rect.is_empty() {
                continue;
            }
            let alternate = views
                .get(&tile.info.id)
                .is_some_and(|shown| shown.view.alternate_screen());
            let (inner, track, geometry) = fitted(
                tile.info.inner_rect,
                self.scrollbars && !alternate,
                self.host,
            );
            let Some(mut shown) = views.remove(&tile.info.id) else {
                if let Entry::Vacant(attempt) = self.attaching.entry(tile.info.id) {
                    attempt.insert(false);
                    attach(
                        &tile.terminal,
                        tile.info.id,
                        Placing {
                            geometry,
                            shown: true,
                        },
                    );
                }
                continue;
            };
            let retained = match std::mem::replace(&mut shown.place, Place::Hidden(Vec::new())) {
                Place::Visible { inner: was, .. } => {
                    if shown.geometry == geometry {
                        shown.view.invalidate();
                    } else {
                        shown.view.resize(geometry);
                    }
                    (was.width == inner.width && was.height == inner.height)
                        .then_some(Retained::At(was))
                }
                Place::Hidden(image) => {
                    let kept = (shown.geometry.cols == inner.width
                        && shown.geometry.rows == inner.height)
                        .then_some(Retained::Image(image));
                    shown.view.show(geometry);
                    kept
                }
            };
            shown.place = Place::Visible {
                inner,
                alternate,
                gutter: track.map(|track| Gutter { track, thumb: None }),
                retained,
            };
            shown.geometry = geometry;
            self.views.insert(tile.info.id, shown);
        }
        let needs: HashSet<PaneId> = self
            .owned
            .keys()
            .copied()
            .filter(|id| {
                !views.contains_key(id)
                    && !self.views.contains_key(id)
                    && !self.attaching.contains_key(id)
            })
            .collect();
        for tile in self.offstage_tiles(areas.pane_surface, &needs) {
            if !needs.contains(&tile.info.id) {
                continue;
            }
            if let Entry::Vacant(attempt) = self.attaching.entry(tile.info.id) {
                attempt.insert(false);
                let geometry = fitted(tile.info.inner_rect, self.scrollbars, self.host).2;
                attach(
                    &tile.terminal,
                    tile.info.id,
                    Placing {
                        geometry: Geometry {
                            cols: geometry.cols.max(MIN_PANE_COLS),
                            rows: geometry.rows.max(MIN_PANE_ROWS),
                            ..geometry
                        },
                        shown: false,
                    },
                );
            }
        }
        for (id, mut shown) in views {
            self.end_gesture(Some((id, &shown.view)));
            if !self.owned.contains_key(&id) {
                continue;
            }
            if let Place::Visible { inner, .. } = shown.place {
                shown.place = Place::Hidden(cut(&self.canvas, self.canvas_cols, inner));
            }
            self.views.insert(id, shown);
        }
    }

    fn offstage_tiles(&mut self, area: Rect, needs: &HashSet<PaneId>) -> Vec<Tile> {
        if needs.is_empty() {
            return Vec::new();
        }
        let (cols, rows, outer) = (self.host.cols, self.host.rows, &self.outer);
        let mut tiles: Vec<Tile> = self
            .tiles
            .tiles
            .iter()
            .filter(|tile| tile.info.inner_rect.is_empty())
            .cloned()
            .collect();
        tiles.extend(self.session.offstage(
            |workspace| outer.layout_for(cols, rows, Some(workspace)).pane_surface,
            self.chrome,
            |id| needs.contains(&id),
        ));
        if self.tiles.hidden.iter().any(|(id, _)| needs.contains(id)) {
            tiles.extend(self.session.unzoomed(area, self.chrome).tiles);
        }
        tiles
    }

    fn named_tiles(&self) -> HashMap<PaneId, &str> {
        self.tiles
            .tiles
            .iter()
            .filter(|tile| crate::ui::pane_border_shows_title(&tile.info))
            .filter_map(|tile| Some((tile.info.id, self.session.name(&tile.terminal)?)))
            .collect()
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
        let names = self.named_tiles();
        crate::ui::paint_pane_borders(
            &mut buffer,
            &infos,
            &self.tiles.splits,
            self.chrome.gaps,
            &self.palette,
            |id| names.get(&id).map(|name| (*name).to_owned()),
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
        if self.copy_mode() || self.outer.cover().is_some() {
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
        let bar = self.outer.bar().map(|bar| bar.0.y);
        (!covers(&self.painted.resize, cursor.y) && Some(cursor.y) != bar).then_some(cursor)
    }
}

fn fitted(rect: Rect, gutter: bool, host: Geometry) -> (Rect, Option<Rect>, Geometry) {
    let (inner, track) = crate::ui::scrollbar_gutter_for(rect, gutter);
    let geometry = Geometry {
        cols: inner.width,
        rows: inner.height,
        ..host
    };
    (inner, track, geometry)
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
    notice: Rect,
    palette: &Palette,
) -> (Rect, Vec<CellData>) {
    let mut buffer = Buffer::empty(area);
    let feedback = crate::app::state::CopyFeedback {
        message: "copied to clipboard".to_owned(),
    };
    let offset = crate::ui::copy_feedback_offset_for_toast(area, &feedback, 0, position, notice);
    let rect = crate::ui::render_copy_feedback_buffer(
        &mut buffer,
        area,
        &feedback,
        offset,
        position,
        palette,
    );
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
