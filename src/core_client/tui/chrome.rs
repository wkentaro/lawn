use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use time::format_description::OwnedFormatItem;

use self::mobile::{Switcher, Target};
use super::session::Session;
use crate::app::tab_bar_status::{
    format_local_datetime, sanitize_literal_text, sanitize_separator, sanitize_status_text,
};
use crate::client::preferences::{self, ClientChromePreferences};
use crate::client::scroll;
use crate::config::{
    Config, SidebarCollapsedModeConfig, SpacesSidebarConfig, StatusIndicatorStyle,
    TabBarPositionConfig, TabBarRightEntryConfig,
};
use crate::input::KeybindAction;
use crate::pane::ScrollMetrics;
use crate::protocol::{CellData, PaneSurfacePatchRow};
use crate::ui::chrome::{self as paint, Layout, LayoutSettings, StatusSegment, TabBarState};
use crate::ui::theme::Palette;

mod mobile;

const HEADER_ROWS: u16 = 2;
const DOUBLE_CLICK: Duration = Duration::from_millis(350);

enum Entry {
    Zoom,
    Text(Option<String>),
    Clock(Clock),
}

struct Clock {
    format: OwnedFormatItem,
    period: Duration,
    text: Option<String>,
    due: Instant,
}

#[derive(PartialEq)]
struct Drawn {
    host: Rect,
    layout: Layout,
    collapsed: bool,
    workspaces: Vec<Label>,
    focused_workspace: Option<usize>,
    tabs: Vec<Label>,
    focused_tab: Option<usize>,
    zoomed: bool,
    status: Vec<(String, bool)>,
    tab_statuses: Vec<String>,
    switching: bool,
}

#[derive(PartialEq)]
struct Label {
    id: String,
    text: String,
    custom: bool,
}

#[derive(Default)]
struct Hits {
    toggle: Rect,
    divider: Rect,
    body: Rect,
    list: Option<ScrollMetrics>,
    track: Rect,
    workspaces: Vec<(Rect, usize)>,
    new_workspace: Rect,
    tabs: Vec<(Rect, usize)>,
    new_tab: Rect,
    scroll_left: Rect,
    scroll_right: Rect,
    switch: Rect,
    close: Rect,
    targets: Vec<(Rect, Target)>,
    switcher_max: usize,
}

#[derive(PartialEq)]
enum Press {
    Workspace(String),
    Tab(String),
    Thumb(u16),
    Width,
}

impl Drawn {
    fn focused_workspace_id(&self) -> Option<&str> {
        let label = self.workspaces.get(self.focused_workspace?)?;
        Some(&label.id)
    }

    fn status(&self) -> impl Iterator<Item = StatusSegment<'_>> + Clone {
        self.status.iter().map(|(text, accent)| StatusSegment {
            text,
            accent: *accent,
        })
    }
}

pub(super) struct Chrome {
    settings: LayoutSettings,
    width: u16,
    default_width: u16,
    width_manual: bool,
    spaces: SpacesSidebarConfig,
    indicators: StatusIndicatorStyle,
    entries: Vec<Entry>,
    separator: String,
    collapsed: bool,
    collapsed_manual: bool,
    divider_click: Option<Instant>,
    preferences: Option<PathBuf>,
    ticked: bool,
    redraw: bool,
    tab_bar: TabBarState,
    workspace_scroll: usize,
    reveal_workspace: bool,
    drawn: Option<Drawn>,
    rows: Vec<PaneSurfacePatchRow>,
    hits: Hits,
    press: Option<Press>,
    switcher: Option<Switcher>,
    cover: Option<(Rect, Vec<CellData>)>,
}

impl Chrome {
    pub(super) fn new(config: &Config) -> Self {
        let ui = &config.ui;
        let now = Instant::now();
        let entries = ui
            .tab_bar_right
            .iter()
            .take(crate::config::MAX_TAB_BAR_RIGHT_ENTRIES)
            .filter_map(|entry| match entry {
                TabBarRightEntryConfig::Zoom => Some(Entry::Zoom),
                TabBarRightEntryConfig::Hostname => Some(Entry::Text(sanitize_status_text(
                    crate::platform::hostname().as_deref().unwrap_or_default(),
                ))),
                TabBarRightEntryConfig::Datetime { format } => {
                    let format = crate::config::parse_tab_bar_datetime_format(format).ok()?;
                    let period = period(&format);
                    let due = next_change(period, now);
                    Some(Entry::Clock(Clock {
                        text: format_local_datetime(&format),
                        format,
                        period,
                        due,
                    }))
                }
                TabBarRightEntryConfig::Text { text } => {
                    Some(Entry::Text(sanitize_literal_text(text)))
                }
                TabBarRightEntryConfig::Command { .. } => None,
            })
            .collect();
        let mut chrome = Self {
            settings: LayoutSettings {
                sidebar_min_width: ui.sidebar_min_width,
                sidebar_max_width: ui.sidebar_max_width,
                mobile_width_threshold: ui.mobile_width_threshold,
                tab_bar_position: ui.tab_bar_position,
                hide_tab_bar_when_single_tab: ui.hide_tab_bar_when_single_tab,
                sidebar_collapsed_mode: ui.sidebar_collapsed_mode,
            },
            width: ui.sidebar_width,
            default_width: ui.sidebar_width,
            width_manual: false,
            spaces: ui.sidebar.spaces.clone(),
            indicators: ui.status_indicators,
            entries,
            separator: sanitize_separator(&ui.tab_bar_right_separator),
            collapsed: ui.sidebar_start_collapsed,
            collapsed_manual: false,
            divider_click: None,
            preferences: None,
            ticked: false,
            redraw: false,
            tab_bar: TabBarState::default(),
            workspace_scroll: 0,
            reveal_workspace: true,
            drawn: None,
            rows: Vec::new(),
            hits: Hits::default(),
            press: None,
            switcher: None,
            cover: None,
        };
        chrome.width = chrome.bounded(chrome.width);
        chrome
    }

    pub(super) fn remembering(mut self, path: PathBuf) -> Self {
        let saved = preferences::load(&path).unwrap_or_default();
        if let Some(width) = saved.sidebar_width {
            self.width = self.bounded(width);
            self.width_manual = true;
        }
        if let Some(collapsed) = saved.sidebar_collapsed {
            self.collapsed = collapsed;
            self.collapsed_manual = true;
        }
        self.preferences = Some(path);
        self
    }

    pub(super) fn layout(&self, cols: u16, rows: u16, session: &Session) -> Layout {
        let single_tab = session
            .focused_workspace()
            .is_some_and(|workspace| workspace.tabs.len() == 1);
        self.settings
            .layout(cols, rows, self.collapsed, single_tab, self.width)
    }

    pub(super) fn bar_area(&self, layout: &Layout) -> Rect {
        if self.settings.tab_bar_position == TabBarPositionConfig::Bottom
            && !layout.tab_bar.is_empty()
        {
            layout.tab_bar
        } else {
            layout.pane_surface
        }
    }

    pub(super) fn toggle(&mut self) {
        self.collapsed = !self.collapsed;
        self.collapsed_manual = true;
        self.save();
    }

    fn bounded(&self, width: u16) -> u16 {
        let (min, max) = crate::config::validated_sidebar_bounds(
            self.settings.sidebar_min_width,
            self.settings.sidebar_max_width,
        )
        .unwrap_or((18, 36));
        width.clamp(min, max)
    }

    fn resize_to(&mut self, column: u16) {
        let width = self.bounded(column.saturating_add(1));
        if width != self.width {
            self.width = width;
            self.width_manual = true;
        }
    }

    fn save(&self) {
        let Some(path) = &self.preferences else {
            return;
        };
        let preferences = ClientChromePreferences {
            sidebar_width: self.width_manual.then_some(self.width),
            sidebar_collapsed: self.collapsed_manual.then_some(self.collapsed),
            ..preferences::load(path).unwrap_or_default()
        };
        if let Err(error) = preferences::store(path, preferences) {
            tracing::warn!(%error, path = %path.display(), "the sidebar preferences were not saved");
        }
    }

    pub(super) fn claim(&mut self, event: MouseEvent, hidden: Rect) -> bool {
        if event.kind == MouseEventKind::Down(MouseButton::Left) {
            self.press = None;
        }
        let point = Position::new(event.column, event.row);
        let held = self.press.is_some()
            && matches!(
                event.kind,
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
            );
        held || self.switcher.is_some()
            || !hidden.contains(point)
                && self.drawn.as_ref().is_some_and(|drawn| {
                    let Layout {
                        sidebar,
                        tab_bar,
                        mobile_header,
                        ..
                    } = drawn.layout;
                    [sidebar, tab_bar, mobile_header]
                        .iter()
                        .any(|rect| rect.contains(point))
                })
    }

    pub(super) fn switching(&self) -> bool {
        self.switcher.is_some()
    }

    pub(super) fn close(&mut self) {
        if self.switcher.take().is_some() {
            self.redraw = true;
        }
    }

    pub(super) fn cover(&self) -> Option<&(Rect, Vec<CellData>)> {
        self.switcher.as_ref().and(self.cover.as_ref())
    }

    pub(super) fn mouse(
        &mut self,
        event: MouseEvent,
        session: &Session,
        now: Instant,
        copying: bool,
    ) -> Option<KeybindAction> {
        if self.switcher.is_some() {
            return self.switcher_mouse(event, session);
        }
        let point = Position::new(event.column, event.row);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if self.hits.switch.contains(point) => {
                if !copying {
                    self.switcher = Some(Switcher::default());
                    self.redraw = true;
                }
                None
            }
            MouseEventKind::Down(MouseButton::Left) => self.press_at(point, now),
            MouseEventKind::Drag(MouseButton::Left) => {
                match (&self.press, self.hits.list) {
                    (Some(Press::Width), _) => self.resize_to(point.x),
                    (Some(Press::Thumb(grab)), Some(metrics)) => {
                        let offset = crate::ui::scrollbar_offset_from_drag_row(
                            metrics,
                            self.hits.track,
                            point.y,
                            *grab,
                        );
                        self.scroll_list(metrics.max_offset_from_bottom.saturating_sub(offset));
                    }
                    _ => {}
                }
                None
            }
            MouseEventKind::Up(MouseButton::Left) => self.release(point, session),
            MouseEventKind::ScrollUp => self.wheel(point, true),
            MouseEventKind::ScrollDown => self.wheel(point, false),
            _ => None,
        }
    }

    fn press_at(&mut self, point: Position, now: Instant) -> Option<KeybindAction> {
        if self.hits.toggle.contains(point) {
            return Some(KeybindAction::ToggleSidebar);
        }
        if self.hits.divider.contains(point) {
            let double = self
                .divider_click
                .is_some_and(|last| now.duration_since(last) <= DOUBLE_CLICK);
            self.divider_click = Some(now);
            if double {
                self.width = self.default_width;
                self.width_manual = false;
                self.save();
            } else {
                self.press = Some(Press::Width);
                self.resize_to(point.x);
            }
            return None;
        }
        if self.hits.track.contains(point) {
            if let Some(metrics) = self.hits.list {
                let track = self.hits.track;
                match crate::ui::scrollbar_thumb_grab_offset(metrics, track, point.y) {
                    Some(grab) => self.press = Some(Press::Thumb(grab)),
                    None => {
                        let offset = crate::ui::scrollbar_offset_from_row(metrics, track, point.y);
                        self.scroll_list(metrics.max_offset_from_bottom.saturating_sub(offset));
                    }
                }
            }
            return None;
        }
        if self.hits.new_workspace.contains(point) {
            return Some(KeybindAction::NewWorkspace);
        }
        if self.hits.new_tab.contains(point) {
            return Some(KeybindAction::NewTab);
        }
        if self.hits.scroll_left.contains(point) {
            self.scroll_tabs(self.tab_bar.scroll.saturating_sub(1));
            return None;
        }
        if self.hits.scroll_right.contains(point) {
            let last = self
                .drawn
                .as_ref()
                .map_or(0, |drawn| drawn.tabs.len().saturating_sub(1));
            self.scroll_tabs(self.tab_bar.scroll.saturating_add(1).min(last));
            return None;
        }
        self.press = self.over(point);
        None
    }

    fn over(&self, point: Position) -> Option<Press> {
        let drawn = self.drawn.as_ref()?;
        let id = |labels: &[Label], index: usize| Some(labels.get(index)?.id.clone());
        hit(&self.hits.workspaces, point)
            .and_then(|index| id(&drawn.workspaces, index))
            .map(Press::Workspace)
            .or_else(|| {
                hit(&self.hits.tabs, point)
                    .and_then(|index| id(&drawn.tabs, index))
                    .map(Press::Tab)
            })
    }

    fn release(&mut self, point: Position, session: &Session) -> Option<KeybindAction> {
        match self.press.take()? {
            Press::Width => {
                self.save();
                None
            }
            Press::Thumb(_) => None,
            press if self.over(point).as_ref() != Some(&press) => None,
            press => focusing(press, session),
        }
    }

    fn switcher_mouse(&mut self, event: MouseEvent, session: &Session) -> Option<KeybindAction> {
        let switcher = self.switcher.as_mut()?;
        let scroll = match event.kind {
            MouseEventKind::ScrollUp => switcher.scroll.saturating_sub(mobile::WHEEL_ROWS),
            MouseEventKind::ScrollDown => switcher
                .scroll
                .saturating_add(mobile::WHEEL_ROWS)
                .min(self.hits.switcher_max),
            MouseEventKind::Down(MouseButton::Left) => {
                return self.choose(Position::new(event.column, event.row), session);
            }
            _ => return None,
        };
        if scroll != switcher.scroll {
            switcher.scroll = scroll;
            self.redraw = true;
        }
        None
    }

    fn choose(&mut self, point: Position, session: &Session) -> Option<KeybindAction> {
        if self.hits.close.contains(point) {
            self.close();
            return None;
        }
        let target = self
            .hits
            .targets
            .iter()
            .find(|(rect, _)| rect.contains(point))?
            .1;
        let drawn = self.drawn.as_ref()?;
        let id = |labels: &[Label], index: usize| Some(labels.get(index)?.id.clone());
        let action = match target {
            Target::NewWorkspace => KeybindAction::NewWorkspace,
            Target::NewTab => KeybindAction::NewTab,
            Target::Workspace(index) => {
                focusing(Press::Workspace(id(&drawn.workspaces, index)?), session)?
            }
            Target::Tab(index) => focusing(Press::Tab(id(&drawn.tabs, index)?), session)?,
        };
        self.close();
        Some(action)
    }

    fn wheel(&mut self, point: Position, up: bool) -> Option<KeybindAction> {
        let hits = &self.hits;
        if hit(&hits.tabs, point).is_some()
            || [hits.scroll_left, hits.scroll_right, hits.new_tab]
                .iter()
                .any(|rect| rect.contains(point))
        {
            return Some(if up {
                KeybindAction::PreviousTab
            } else {
                KeybindAction::NextTab
            });
        }
        if hits.body.contains(point) {
            let last = hits
                .list
                .map_or(0, |metrics| metrics.max_offset_from_bottom);
            self.scroll_list(if up {
                self.workspace_scroll.saturating_sub(1)
            } else {
                self.workspace_scroll.saturating_add(1).min(last)
            });
        }
        None
    }

    fn scroll_list(&mut self, start: usize) {
        if start != self.workspace_scroll {
            self.workspace_scroll = start;
            self.redraw = true;
        }
    }

    fn scroll_tabs(&mut self, start: usize) {
        if start != self.tab_bar.scroll {
            self.tab_bar.scroll = start;
            self.redraw = true;
        }
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.clocks().map(|clock| clock.due).min()
    }

    pub(super) fn tick(&mut self, now: Instant) {
        let mut ticked = false;
        for entry in &mut self.entries {
            let Entry::Clock(clock) = entry else {
                continue;
            };
            if now < clock.due {
                continue;
            }
            clock.due = next_change(clock.period, now);
            let text = format_local_datetime(&clock.format);
            ticked |= text != clock.text;
            clock.text = text;
        }
        self.ticked |= ticked;
    }

    pub(super) fn stale(&self) -> bool {
        self.ticked || self.redraw
    }

    pub(super) fn rows(&self) -> &[PaneSurfacePatchRow] {
        &self.rows
    }

    pub(super) fn refresh(
        &mut self,
        host: Rect,
        layout: Layout,
        session: &Session,
        palette: &Palette,
    ) -> bool {
        self.ticked = false;
        if layout.mobile_header.is_empty() {
            self.switcher = None;
        }
        let redraw = std::mem::take(&mut self.redraw);
        let drawn = self.drawn(host, layout, session);
        let previous = self.drawn.take();
        if !redraw && previous.as_ref() == Some(&drawn) {
            self.drawn = previous;
            return false;
        }
        let refocused = previous
            .as_ref()
            .is_none_or(|previous| previous.focused_workspace_id() != drawn.focused_workspace_id());
        self.reveal_workspace |= refocused;
        if let (Some(switcher), Some(previous)) = (&mut self.switcher, &previous) {
            switcher.reveal |= previous.switching && (refocused || previous.host != drawn.host);
        }
        if previous.as_ref().is_none_or(|previous| {
            previous.tabs != drawn.tabs
                || previous.focused_tab != drawn.focused_tab
                || previous.zoomed != drawn.zoomed
                || previous.layout.tab_bar.width != drawn.layout.tab_bar.width
                || paint::tab_bar_status_width(previous.status(), &self.separator)
                    != paint::tab_bar_status_width(drawn.status(), &self.separator)
        }) {
            self.tab_bar.reveal_focused = true;
        }
        self.rows = self.draw(&drawn, palette);
        self.drawn = Some(drawn);
        true
    }

    fn clocks(&self) -> impl Iterator<Item = &Clock> {
        self.entries.iter().filter_map(|entry| match entry {
            Entry::Clock(clock) => Some(clock),
            _ => None,
        })
    }

    fn drawn(&self, host: Rect, layout: Layout, session: &Session) -> Drawn {
        let workspaces = session.workspaces();
        let focused = session.focused_workspace();
        let tabs = focused.map_or(&[][..], |workspace| &workspace.tabs);
        let focused_tab = session.focused_tab();
        let zoomed = session.zoomed();
        Drawn {
            host,
            layout,
            collapsed: self.collapsed,
            workspaces: workspaces
                .iter()
                .map(|workspace| Label {
                    id: workspace.id.clone(),
                    text: workspace
                        .name
                        .clone()
                        .unwrap_or_else(|| "workspace".to_owned()),
                    custom: workspace.name.is_some(),
                })
                .collect(),
            focused_workspace: focused.and_then(|focused| {
                workspaces
                    .iter()
                    .position(|workspace| workspace.id == focused.id)
            }),
            tabs: tabs
                .iter()
                .enumerate()
                .map(|(index, tab)| Label {
                    id: tab.id.clone(),
                    text: tab_label(index, tab),
                    custom: tab.name.is_some(),
                })
                .collect(),
            focused_tab: focused_tab
                .and_then(|focused| tabs.iter().position(|tab| tab.id == focused)),
            zoomed,
            status: self
                .entries
                .iter()
                .filter_map(|entry| match entry {
                    Entry::Zoom => zoomed.then(|| ("ZOOM".to_owned(), true)),
                    Entry::Text(text) | Entry::Clock(Clock { text, .. }) => text
                        .clone()
                        .filter(|text| !text.is_empty())
                        .map(|text| (text, false)),
                })
                .collect(),
            tab_statuses: if layout.mobile_header.is_empty() {
                Vec::new()
            } else {
                workspaces
                    .iter()
                    .map(|workspace| {
                        let active = session.active_tab(workspace).unwrap_or(0);
                        let label = workspace
                            .tabs
                            .get(active)
                            .map_or_else(|| "1".to_owned(), |tab| tab_label(active, tab));
                        mobile::tab_status(&label, active, workspace.tabs.len())
                    })
                    .collect()
            },
            switching: self.switcher.is_some(),
        }
    }

    fn draw(&mut self, drawn: &Drawn, palette: &Palette) -> Vec<PaneSurfacePatchRow> {
        self.hits = Hits::default();
        let mut buffer = Buffer::empty(drawn.host);
        let Layout {
            sidebar,
            tab_bar,
            mobile_header,
            ..
        } = drawn.layout;
        if !sidebar.is_empty() {
            if self.collapsed {
                self.draw_collapsed(&mut buffer, drawn, palette);
            } else {
                self.draw_sidebar(&mut buffer, drawn, palette);
            }
        }
        if !tab_bar.is_empty() {
            self.draw_tab_bar(&mut buffer, drawn, palette);
        }
        mobile::draw_header(&mut buffer, drawn, palette, &mut self.hits);
        let rows = [sidebar, tab_bar, mobile_header]
            .into_iter()
            .flat_map(|rect| (rect.y..rect.bottom()).map(move |y| (rect, y)))
            .map(|(rect, y)| PaneSurfacePatchRow {
                x: rect.x,
                y,
                cells: (rect.x..rect.right())
                    .map(|x| CellData::from_ratatui_cell(&buffer[(x, y)]))
                    .collect(),
            })
            .collect();
        self.cover = self.switcher.as_mut().map(|switcher| {
            mobile::draw_switcher(&mut buffer, drawn, switcher, palette, &mut self.hits);
            (
                drawn.host,
                buffer
                    .content
                    .iter()
                    .map(CellData::from_ratatui_cell)
                    .collect(),
            )
        });
        rows
    }

    fn list_area(&self, sidebar: Rect) -> Rect {
        let list = Rect::new(
            sidebar.x,
            sidebar.y,
            sidebar.width.saturating_sub(1),
            sidebar.height.saturating_sub(u16::from(
                self.settings.sidebar_collapsed_mode == SidebarCollapsedModeConfig::Compact,
            )),
        );
        if list.is_empty() {
            Rect::default()
        } else {
            list
        }
    }

    fn draw_sidebar(&mut self, buffer: &mut Buffer, drawn: &Drawn, palette: &Palette) {
        let area = drawn.layout.sidebar;
        let quiet = Style::default().fg(palette.overlay0);
        paint::render_sidebar_background(buffer, area, palette);
        self.hits.divider = Rect::new(area.right().saturating_sub(1), area.y, 1, area.height);
        let list = self.list_area(area);
        paint::put_text(
            buffer,
            list.x,
            list.y,
            list.width,
            " spaces",
            quiet.add_modifier(Modifier::BOLD),
        );
        let body = Rect::new(
            list.x,
            list.y.saturating_add(HEADER_ROWS),
            list.width,
            list.height.saturating_sub(HEADER_ROWS + 1),
        );
        let tokens = HashMap::new();
        let rows: Vec<_> = drawn
            .workspaces
            .iter()
            .map(|label| {
                let mut rows = crate::ui::sidebar_space_rows(
                    &self.spaces,
                    crate::ui::SpaceTokenContext {
                        workspace: &label.text,
                        state_text: "",
                        tokens: &tokens,
                    },
                );
                for row in &mut rows {
                    row.retain(|token| {
                        !matches!(
                            token.kind,
                            crate::ui::ResolvedTokenKind::StateIcon
                                | crate::ui::ResolvedTokenKind::StateText(_)
                        )
                    });
                }
                rows.retain(|row| !row.is_empty());
                rows
            })
            .collect();
        let heights: Vec<u16> = rows
            .iter()
            .map(|rows| rows.len().clamp(1, usize::from(u16::MAX)) as u16)
            .collect();
        let gaps: Vec<u16> = (0..rows.len())
            .map(|index| {
                if index + 1 < rows.len() {
                    self.spaces.row_gap
                } else {
                    0
                }
            })
            .collect();
        let mut metrics =
            scroll::list_scroll_metrics(&heights, &gaps, body.height, self.workspace_scroll);
        if !body.is_empty() && std::mem::take(&mut self.reveal_workspace) {
            if let Some(target) = drawn.focused_workspace {
                self.workspace_scroll = scroll::list_scroll_start_to_reveal(
                    &heights,
                    &gaps,
                    body.height,
                    self.workspace_scroll,
                    target,
                );
                metrics = scroll::list_scroll_metrics(
                    &heights,
                    &gaps,
                    body.height,
                    self.workspace_scroll,
                );
            }
        }
        self.workspace_scroll = metrics
            .max_offset_from_bottom
            .saturating_sub(metrics.offset_from_bottom);
        self.hits.body = body;
        self.hits.list = Some(metrics);
        let scrollbar = metrics.max_offset_from_bottom > 0 && body.width > 1;
        let width = body.width.saturating_sub(u16::from(scrollbar));
        let mut y = body.y;
        for (index, rows) in rows.iter().enumerate().skip(self.workspace_scroll) {
            let height = heights[index].min(body.height);
            if y.saturating_add(height) > body.bottom() {
                break;
            }
            let rect = Rect::new(body.x, y, width, height);
            self.hits.workspaces.push((rect, index));
            paint::render_workspace_rows(
                buffer,
                rect,
                paint::WorkspaceRow {
                    status: crate::api::schema::AgentStatus::Unknown,
                    rows,
                    focused: drawn.focused_workspace == Some(index),
                    selected: false,
                    navigating: false,
                    dragged: false,
                },
                self.indicators,
                palette,
            );
            y = y.saturating_add(height + gaps[index]);
        }
        if scrollbar {
            self.hits.track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
            scroll::render_list_scrollbar(buffer, self.hits.track, metrics, palette);
        }
        let footer = list.bottom().saturating_sub(1);
        self.hits.new_workspace = Rect::new(
            list.x,
            footer,
            list.width.min(5),
            u16::from(list.height > 0),
        );
        paint::put_text(buffer, list.x, footer, list.width, " new", quiet);
        if self.settings.sidebar_collapsed_mode != SidebarCollapsedModeConfig::Hidden {
            self.hits.toggle = Rect::new(
                area.right().saturating_sub(2),
                area.bottom().saturating_sub(1),
                u16::from(area.width > 1),
                u16::from(area.height > 0),
            );
            paint::put_text(
                buffer,
                self.hits.toggle.x,
                self.hits.toggle.y,
                self.hits.toggle.width,
                "«",
                quiet,
            );
        }
    }

    fn draw_collapsed(&mut self, buffer: &mut Buffer, drawn: &Drawn, palette: &Palette) {
        let area = drawn.layout.sidebar;
        paint::render_sidebar_background(buffer, area, palette);
        let list = self.list_area(area);
        let active = paint::workspace_active_background(palette, false);
        for index in 0..drawn.workspaces.len().min(usize::from(list.height)) {
            let rect = Rect::new(list.x, list.y + index as u16, list.width, 1);
            self.hits.workspaces.push((rect, index));
            let style = if drawn.focused_workspace == Some(index) {
                buffer.set_style(rect, Style::default().bg(active));
                Style::default().fg(palette.text).bg(active)
            } else {
                Style::default().fg(palette.overlay0)
            };
            paint::put_text(
                buffer,
                rect.x,
                rect.y,
                rect.width.min(2),
                &format!("{:<2}", index + 1),
                style,
            );
        }
        if !list.is_empty() {
            self.hits.toggle = Rect::new(
                list.x + list.width / 2,
                area.bottom().saturating_sub(1),
                1,
                1,
            );
            paint::put_text(
                buffer,
                self.hits.toggle.x,
                self.hits.toggle.y,
                1,
                "»",
                Style::default().fg(palette.overlay0),
            );
        }
    }

    fn draw_tab_bar(&mut self, buffer: &mut Buffer, drawn: &Drawn, palette: &Palette) {
        let area = drawn.layout.tab_bar;
        buffer.set_style(area, Style::default().bg(palette.panel_bg));
        let tabs: Vec<_> = drawn
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let focused = drawn.focused_tab == Some(index);
                paint::Tab {
                    key: &tab.id,
                    label: &tab.text,
                    focused,
                    custom_label: tab.custom,
                    zoomed: focused && drawn.zoomed,
                    interactive: true,
                    status: None,
                }
            })
            .collect();
        let content = paint::tab_bar_content_area(
            area,
            paint::tab_bar_status_width(drawn.status(), &self.separator),
        );
        let mut hits = Vec::new();
        let outcome = paint::render_tab_strip(
            buffer,
            content,
            &tabs,
            paint::TabBarStyle {
                palette,
                indicators: self.indicators,
                mouse: true,
            },
            self.tab_bar,
            None,
            |rect, key| {
                if let Some(index) = drawn.tabs.iter().position(|tab| tab.id == key) {
                    hits.push((rect, index));
                }
            },
        );
        self.tab_bar = outcome.state;
        self.hits.tabs = hits;
        self.hits.new_tab = outcome.new_tab;
        self.hits.scroll_left = outcome.tab_scroll_left;
        self.hits.scroll_right = outcome.tab_scroll_right;
        paint::render_tab_bar_status(buffer, area, drawn.status(), &self.separator, palette);
    }
}

pub(super) fn preferences_path(session: &str) -> PathBuf {
    let name = Some(session).filter(|name| *name != crate::session::DEFAULT_SESSION_NAME);
    preferences::path_for_local_endpoint(&crate::session::client_socket_path_for(name))
}

fn tab_label(index: usize, tab: &crate::session_doc::Tab) -> String {
    tab.name.clone().unwrap_or_else(|| (index + 1).to_string())
}

fn focusing(press: Press, session: &Session) -> Option<KeybindAction> {
    match press {
        Press::Workspace(id) => session
            .workspaces()
            .iter()
            .position(|workspace| workspace.id == id)
            .map(KeybindAction::SwitchWorkspace),
        Press::Tab(id) => session
            .focused_workspace()?
            .tabs
            .iter()
            .position(|tab| tab.id == id)
            .map(KeybindAction::SwitchTab),
        Press::Thumb(_) | Press::Width => None,
    }
}

fn hit(hits: &[(Rect, usize)], point: Position) -> Option<usize> {
    hits.iter()
        .find(|(rect, _)| rect.contains(point))
        .map(|(_, index)| *index)
}

fn period(format: &OwnedFormatItem) -> Duration {
    let at = |seconds| {
        time::PrimitiveDateTime::MIN
            .saturating_add(time::Duration::seconds(seconds))
            .format(format)
            .ok()
    };
    if at(0) == at(1) {
        Duration::from_secs(60)
    } else {
        Duration::from_secs(1)
    }
}

fn next_change(period: Duration, now: Instant) -> Instant {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let into = since.as_nanos() % period.as_nanos();
    now + period - Duration::from_nanos(u64::try_from(into).unwrap_or_default())
}
