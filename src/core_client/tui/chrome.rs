use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use time::format_description::OwnedFormatItem;

use super::session::Session;
use crate::app::tab_bar_status::{
    format_local_datetime, sanitize_literal_text, sanitize_separator, sanitize_status_text,
};
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

const HEADER_ROWS: u16 = 2;

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
    body: Rect,
    list: Option<ScrollMetrics>,
    track: Rect,
    workspaces: Vec<(Rect, usize)>,
    new_workspace: Rect,
    tabs: Vec<(Rect, usize)>,
    new_tab: Rect,
    scroll_left: Rect,
    scroll_right: Rect,
}

#[derive(PartialEq)]
enum Press {
    Workspace(String),
    Tab(String),
    Thumb(u16),
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
    spaces: SpacesSidebarConfig,
    indicators: StatusIndicatorStyle,
    entries: Vec<Entry>,
    separator: String,
    collapsed: bool,
    ticked: bool,
    scrolled: bool,
    tab_bar: TabBarState,
    workspace_scroll: usize,
    reveal_workspace: bool,
    drawn: Option<Drawn>,
    rows: Vec<PaneSurfacePatchRow>,
    hits: Hits,
    press: Option<Press>,
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
        Self {
            settings: LayoutSettings {
                sidebar_min_width: ui.sidebar_min_width,
                sidebar_max_width: ui.sidebar_max_width,
                mobile_width_threshold: ui.mobile_width_threshold,
                tab_bar_position: ui.tab_bar_position,
                hide_tab_bar_when_single_tab: ui.hide_tab_bar_when_single_tab,
                sidebar_collapsed_mode: ui.sidebar_collapsed_mode,
            },
            width: ui.sidebar_width,
            spaces: ui.sidebar.spaces.clone(),
            indicators: ui.status_indicators,
            entries,
            separator: sanitize_separator(&ui.tab_bar_right_separator),
            collapsed: ui.sidebar_start_collapsed,
            ticked: false,
            scrolled: false,
            tab_bar: TabBarState::default(),
            workspace_scroll: 0,
            reveal_workspace: true,
            drawn: None,
            rows: Vec::new(),
            hits: Hits::default(),
            press: None,
        }
    }

    pub(super) fn layout(&self, cols: u16, rows: u16, session: &Session) -> Layout {
        if cols <= self.settings.mobile_width_threshold {
            return Layout {
                pane_surface: Rect::new(0, 0, cols, rows),
                ..Layout::default()
            };
        }
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
    }

    pub(super) fn claim(&mut self, event: MouseEvent, hidden: Rect) -> bool {
        if matches!(event.kind, MouseEventKind::Down(_)) {
            self.press = None;
        }
        let point = Position::new(event.column, event.row);
        let held = self.press.is_some()
            && matches!(
                event.kind,
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
            );
        held || !hidden.contains(point)
            && self.drawn.as_ref().is_some_and(|drawn| {
                drawn.layout.sidebar.contains(point) || drawn.layout.tab_bar.contains(point)
            })
    }

    pub(super) fn mouse(&mut self, event: MouseEvent, session: &Session) -> Option<KeybindAction> {
        let point = Position::new(event.column, event.row);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => self.press_at(point),
            MouseEventKind::Drag(MouseButton::Left) => {
                if let (Some(Press::Thumb(grab)), Some(metrics)) = (&self.press, self.hits.list) {
                    let offset = crate::ui::scrollbar_offset_from_drag_row(
                        metrics,
                        self.hits.track,
                        point.y,
                        *grab,
                    );
                    self.scroll_list(metrics.max_offset_from_bottom.saturating_sub(offset));
                }
                None
            }
            MouseEventKind::Up(MouseButton::Left) => self.release(point, session),
            MouseEventKind::ScrollUp => self.wheel(point, true),
            MouseEventKind::ScrollDown => self.wheel(point, false),
            _ => None,
        }
    }

    fn press_at(&mut self, point: Position) -> Option<KeybindAction> {
        if self.hits.toggle.contains(point) {
            return Some(KeybindAction::ToggleSidebar);
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
        let press = self.press.take()?;
        if self.over(point).as_ref() != Some(&press) {
            return None;
        }
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
            Press::Thumb(_) => None,
        }
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
            self.scrolled = true;
        }
    }

    fn scroll_tabs(&mut self, start: usize) {
        if start != self.tab_bar.scroll {
            self.tab_bar.scroll = start;
            self.scrolled = true;
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
        self.ticked || self.scrolled
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
        let scrolled = std::mem::take(&mut self.scrolled);
        let drawn = self.drawn(host, layout, session);
        let previous = self.drawn.take();
        if !scrolled && previous.as_ref() == Some(&drawn) {
            self.drawn = previous;
            return false;
        }
        if previous
            .as_ref()
            .is_none_or(|previous| previous.focused_workspace_id() != drawn.focused_workspace_id())
        {
            self.reveal_workspace = true;
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
                    text: tab.name.clone().unwrap_or_else(|| (index + 1).to_string()),
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
        }
    }

    fn draw(&mut self, drawn: &Drawn, palette: &Palette) -> Vec<PaneSurfacePatchRow> {
        self.hits = Hits::default();
        let mut buffer = Buffer::empty(drawn.host);
        let Layout {
            sidebar, tab_bar, ..
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
        [sidebar, tab_bar]
            .into_iter()
            .flat_map(|rect| (rect.y..rect.bottom()).map(move |y| (rect, y)))
            .map(|(rect, y)| PaneSurfacePatchRow {
                x: rect.x,
                y,
                cells: (rect.x..rect.right())
                    .map(|x| CellData::from_ratatui_cell(&buffer[(x, y)]))
                    .collect(),
            })
            .collect()
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
