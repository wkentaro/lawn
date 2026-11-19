use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Widget};

use super::{Drawn, Hits};
use crate::api::schema::AgentStatus;
use crate::config::StatusIndicatorStyle;
use crate::ui::chrome::{display_width, put_text, status_color, status_icon};
use crate::ui::theme::Palette;

const BUTTON_WIDTH: u16 = 10;
pub(super) const WHEEL_ROWS: usize = 2;

#[derive(Default)]
pub(super) struct Switcher {
    pub(super) scroll: usize,
    pub(super) reveal: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Target {
    Agent(usize),
    NewWorkspace,
    Workspace(usize),
    NewTab,
    Tab(usize),
}

struct Item {
    lines: Vec<Line<'static>>,
    background: Color,
    target: Option<Target>,
}

impl Item {
    fn section(label: &str, palette: &Palette) -> Self {
        Self {
            lines: vec![Line::from(Span::styled(
                format!(" {label} "),
                Style::default()
                    .fg(palette.overlay1)
                    .bg(palette.panel_bg)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            ))],
            background: palette.panel_bg,
            target: None,
        }
    }

    fn action(label: &'static str, target: Target, palette: &Palette) -> Self {
        Self {
            lines: vec![Line::from(Span::styled(
                label,
                Style::default()
                    .fg(palette.accent)
                    .bg(palette.panel_bg)
                    .add_modifier(Modifier::BOLD),
            ))],
            background: palette.panel_bg,
            target: Some(target),
        }
    }
}

pub(super) fn tab_status(label: &str, active: usize, count: usize) -> String {
    if count <= 1 {
        format!("tab {label}")
    } else {
        format!("tab {label} · {}/{count}", active + 1)
    }
}

pub(super) fn draw_header(
    buffer: &mut Buffer,
    drawn: &Drawn,
    indicators: StatusIndicatorStyle,
    palette: &Palette,
    hits: &mut Hits,
) {
    let area = drawn.layout.mobile_header;
    if area.is_empty() {
        return;
    }
    buffer.set_style(area, Style::default().bg(palette.panel_bg));
    let width = BUTTON_WIDTH.min(area.width);
    let button = Rect::new(area.right() - width, area.y, width, area.height);
    hits.switch = button;
    let status = Rect::new(
        area.x,
        area.y,
        button.x.saturating_sub(area.x).saturating_sub(1),
        area.height,
    );
    draw_status(buffer, status, drawn, indicators, palette);
    draw_button(buffer, button, palette);
    let label_y = if button.height > 1 {
        button.y + 1
    } else {
        button.y
    };
    put_text(
        buffer,
        centered(button, display_width("switch")),
        label_y,
        button.width.saturating_sub(1),
        "switch",
        Style::default()
            .fg(palette.text)
            .bg(palette.surface0)
            .add_modifier(Modifier::BOLD),
    );
    if drawn
        .agents
        .iter()
        .any(|agent| agent.status == AgentStatus::Blocked)
    {
        put_text(
            buffer,
            button.right() - 1,
            button.y,
            1,
            status_icon(AgentStatus::Blocked, indicators),
            Style::default().fg(palette.red).bg(palette.surface0),
        );
    }
}

fn draw_status(
    buffer: &mut Buffer,
    area: Rect,
    drawn: &Drawn,
    indicators: StatusIndicatorStyle,
    palette: &Palette,
) {
    if area.is_empty() {
        return;
    }
    let focused = drawn.focused_workspace.and_then(|index| {
        Some((
            &drawn.workspaces.get(index)?.text,
            drawn.tab_statuses.get(index)?,
            *drawn.statuses.get(index)?,
        ))
    });
    let Some((name, tabs, status)) = focused else {
        put_text(
            buffer,
            area.x,
            area.y,
            area.width,
            " no workspace",
            Style::default().fg(palette.text).bg(palette.panel_bg),
        );
        return;
    };
    let tab_width = display_width(tabs).saturating_add(1).min(area.width);
    let name_width = area.width - tab_width;
    put_text(
        buffer,
        area.x,
        area.y,
        name_width.min(3),
        &format!(" {} ", status_icon(status, indicators)),
        Style::default()
            .fg(status_color(status, palette))
            .bg(palette.panel_bg),
    );
    put_text(
        buffer,
        area.x.saturating_add(3),
        area.y,
        name_width.saturating_sub(3),
        &crate::ui::truncate_end(name, usize::from(name_width.saturating_sub(4))),
        Style::default()
            .fg(palette.text)
            .bg(palette.panel_bg)
            .add_modifier(Modifier::BOLD),
    );
    put_text(
        buffer,
        area.right().saturating_sub(tab_width).saturating_add(1),
        area.y,
        tab_width.saturating_sub(1),
        tabs,
        Style::default().fg(palette.overlay1).bg(palette.panel_bg),
    );
    if area.height > 1 {
        draw_summary(
            buffer,
            Rect::new(area.x, area.y + 1, area.width, 1),
            drawn,
            indicators,
            palette,
        );
    }
}

fn draw_summary(
    buffer: &mut Buffer,
    area: Rect,
    drawn: &Drawn,
    indicators: StatusIndicatorStyle,
    palette: &Palette,
) {
    let counts = [
        (AgentStatus::Blocked, "blocked"),
        (AgentStatus::Done, "done"),
        (AgentStatus::Working, "working"),
        (AgentStatus::Idle, "idle"),
    ]
    .map(|(status, label)| {
        let count = drawn
            .agents
            .iter()
            .filter(|agent| agent.status == status)
            .count();
        (status, label, count)
    });
    let quiet = Style::default().fg(palette.overlay1).bg(palette.panel_bg);
    if counts.iter().all(|(.., count)| *count == 0) {
        put_text(buffer, area.x, area.y, area.width, " no agents", quiet);
        return;
    }
    if counts[..3].iter().all(|(.., count)| *count == 0) {
        put_text(buffer, area.x, area.y, area.width, " all idle", quiet);
        return;
    }
    let separator = Style::default().fg(palette.overlay0).bg(palette.panel_bg);
    let mut x = area.x.saturating_add(1);
    let shown = counts.into_iter().filter(|(.., count)| *count > 0);
    for (index, (status, label, count)) in shown.enumerate() {
        let symbol = match (indicators, status) {
            (StatusIndicatorStyle::Dots, AgentStatus::Blocked) => Some("◉"),
            (StatusIndicatorStyle::Dots, AgentStatus::Done) => Some("●"),
            (StatusIndicatorStyle::Dots, _) => None,
            _ => Some(status_icon(status, indicators)),
        };
        let text = symbol.map_or_else(
            || format!("{count} {label}"),
            |symbol| format!("{symbol} {count} {label}"),
        );
        let gap = if index == 0 { "" } else { " · " };
        if area.right().saturating_sub(x) < display_width(gap) + display_width(&text) {
            if area.right().saturating_sub(x) >= 2 {
                put_text(buffer, x, area.y, 2, " …", separator);
            }
            return;
        }
        put_text(buffer, x, area.y, display_width(gap), gap, separator);
        x += display_width(gap);
        let style = if index == 0 {
            Style::default()
                .fg(match status {
                    AgentStatus::Done => palette.blue,
                    _ => status_color(status, palette),
                })
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.overlay1)
        };
        put_text(
            buffer,
            x,
            area.y,
            display_width(&text),
            &text,
            style.bg(palette.panel_bg),
        );
        x += display_width(&text);
    }
}

fn draw_button(buffer: &mut Buffer, area: Rect, palette: &Palette) {
    buffer.set_style(area, Style::default().bg(palette.surface0));
    for y in area.y..area.bottom() {
        put_text(
            buffer,
            area.x,
            y,
            1,
            "│",
            Style::default()
                .fg(palette.surface_dim)
                .bg(palette.surface0),
        );
    }
}

fn centered(button: Rect, label: u16) -> u16 {
    button
        .x
        .saturating_add(1)
        .saturating_add(button.width.saturating_sub(1 + label) / 2)
}

pub(super) fn draw_switcher(
    buffer: &mut Buffer,
    drawn: &Drawn,
    switcher: &mut Switcher,
    indicators: StatusIndicatorStyle,
    palette: &Palette,
    hits: &mut Hits,
) {
    let area = drawn.host;
    if area.is_empty() {
        return;
    }
    Clear.render(area, buffer);
    buffer.set_style(area, Style::default().bg(palette.panel_bg));
    hits.switch = Rect::default();
    let rule = |buffer: &mut Buffer, y| {
        put_text(
            buffer,
            area.x,
            y,
            area.width,
            &"─".repeat(usize::from(area.width)),
            Style::default()
                .fg(palette.surface_dim)
                .bg(palette.panel_bg),
        );
    };
    if area.height <= 2 {
        switcher.scroll = 0;
        rule(buffer, area.y);
        return;
    }
    let header = area.height.min(2);
    let width = BUTTON_WIDTH.min(area.width);
    let close = Rect::new(area.right() - width, area.y, width, header);
    hits.close = close;
    put_text(
        buffer,
        area.x,
        area.y,
        close.x - area.x,
        " switch",
        Style::default()
            .fg(palette.text)
            .bg(palette.panel_bg)
            .add_modifier(Modifier::BOLD),
    );
    draw_close(buffer, close, palette);
    rule(buffer, area.y + header);
    let viewport = Rect::new(
        area.x,
        area.y + header + 1,
        area.width,
        area.height.saturating_sub(header + 1),
    );
    if viewport.is_empty() {
        switcher.scroll = 0;
        return;
    }
    let items = items(drawn, indicators, palette, viewport.width.saturating_sub(1));
    let total: usize = items.iter().map(|item| item.lines.len()).sum();
    let height = usize::from(viewport.height);
    let max = total.saturating_sub(height);
    switcher.scroll = switcher.scroll.min(max);
    if std::mem::take(&mut switcher.reveal) {
        reveal(&items, drawn.selected, height, max, &mut switcher.scroll);
    }
    hits.switcher_max = max;
    if max > 0 {
        draw_scrollbar(buffer, viewport, total, switcher.scroll, palette);
    }
    if viewport.width <= 1 {
        return;
    }
    let content = Rect::new(
        viewport.x + 1,
        viewport.y,
        viewport.width - 1,
        viewport.height,
    );
    let (start, end) = (switcher.scroll, switcher.scroll + height);
    let mut row = 0;
    for item in items {
        let (first, last) = (row, row + item.lines.len());
        row = last;
        let (from, to) = (first.max(start), last.min(end));
        if from >= to {
            continue;
        }
        let y = viewport.y + (from - start) as u16;
        let rect = Rect::new(content.x, y, content.width, (to - from) as u16);
        buffer.set_style(rect, Style::default().bg(item.background));
        let lines = item.lines.into_iter().skip(from - first).take(to - from);
        for (line, y) in lines.zip(y..) {
            Paragraph::new(line).render(Rect::new(content.x, y, content.width, 1), buffer);
        }
        if let Some(target) = item.target {
            hits.targets.push((rect, target));
        }
    }
}

fn reveal(items: &[Item], selected: Option<usize>, height: usize, max: usize, scroll: &mut usize) {
    let mut start = 0;
    for item in items {
        let end = start + item.lines.len();
        if selected.is_some_and(|selected| item.target == Some(Target::Workspace(selected))) {
            if start < *scroll {
                *scroll = start;
            } else if end > *scroll + height {
                *scroll = (end - height).min(max);
            }
            return;
        }
        start = end;
    }
}

fn draw_close(buffer: &mut Buffer, area: Rect, palette: &Palette) {
    draw_button(buffer, area, palette);
    put_text(
        buffer,
        centered(area, 5),
        area.y,
        area.width.saturating_sub(1),
        "close",
        Style::default()
            .fg(palette.overlay1)
            .bg(palette.surface0)
            .add_modifier(Modifier::BOLD),
    );
    if area.height > 1 {
        put_text(
            buffer,
            area.x.saturating_add(area.width / 2),
            area.y + 1,
            1,
            "×",
            Style::default()
                .fg(palette.text)
                .bg(palette.surface0)
                .add_modifier(Modifier::BOLD),
        );
    }
}

fn marked(
    status: AgentStatus,
    name: &str,
    indicators: StatusIndicatorStyle,
    palette: &Palette,
    background: Color,
    width: u16,
) -> Line<'static> {
    Line::from(vec![
        Span::styled("  ", Style::default().fg(palette.overlay0).bg(background)),
        Span::styled(
            status_icon(status, indicators),
            Style::default()
                .fg(status_color(status, palette))
                .bg(background),
        ),
        Span::styled(" ", Style::default().bg(background)),
        Span::styled(
            crate::ui::truncate_end(name, usize::from(width.saturating_sub(5))),
            Style::default()
                .fg(palette.text)
                .bg(background)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn items(
    drawn: &Drawn,
    indicators: StatusIndicatorStyle,
    palette: &Palette,
    width: u16,
) -> Vec<Item> {
    let mut items = Vec::new();
    if !drawn.agents.is_empty() {
        items.push(Item::section("agents", palette));
    }
    for (index, agent) in drawn.agents.iter().enumerate() {
        let background = if agent.focused {
            palette.surface_dim
        } else {
            palette.panel_bg
        };
        let name = drawn
            .workspaces
            .get(agent.workspace)
            .map_or("agent", |workspace| &workspace.text);
        items.push(Item {
            lines: vec![
                marked(agent.status, name, indicators, palette, background, width),
                Line::from(Span::styled(
                    crate::ui::truncate_end(&format!("  {}", agent.detail), usize::from(width)),
                    Style::default().fg(palette.overlay0).bg(background),
                )),
            ],
            background,
            target: Some(Target::Agent(index)),
        });
    }
    items.push(Item::section("spaces", palette));
    items.push(Item::action(
        "  + new workspace",
        Target::NewWorkspace,
        palette,
    ));
    let highlight = if palette.surface0 == Color::Reset {
        palette.active_row_bg
    } else {
        palette.surface0
    };
    for (index, (workspace, tabs)) in drawn.workspaces.iter().zip(&drawn.tab_statuses).enumerate() {
        let background = if drawn.selected == Some(index) {
            highlight
        } else if drawn.focused_workspace == Some(index)
            && (drawn.selected.is_none() || palette.surface0 != Color::Reset)
        {
            palette.surface_dim
        } else {
            palette.panel_bg
        };
        let status = drawn
            .statuses
            .get(index)
            .copied()
            .unwrap_or(AgentStatus::Unknown);
        items.push(Item {
            lines: vec![
                marked(
                    status,
                    &workspace.text,
                    indicators,
                    palette,
                    background,
                    width,
                ),
                Line::from(Span::styled(
                    crate::ui::truncate_end(&format!("  {tabs}"), usize::from(width)),
                    Style::default().fg(palette.overlay0).bg(background),
                )),
            ],
            background,
            target: Some(Target::Workspace(index)),
        });
    }
    if drawn.focused_workspace.is_none() {
        return items;
    }
    items.push(Item::section("tabs", palette));
    items.push(Item::action("  + new tab", Target::NewTab, palette));
    for (index, tab) in drawn.tabs.iter().enumerate() {
        let background = if drawn.focused_tab == Some(index) {
            palette.surface_dim
        } else {
            palette.panel_bg
        };
        let label = if tab.custom {
            format!("{} · {}", index + 1, tab.text)
        } else {
            format!("tab {}", tab.text)
        };
        items.push(Item {
            lines: vec![Line::from(Span::styled(
                format!(
                    "  {}",
                    crate::ui::truncate_end(&label, usize::from(width.saturating_sub(3)))
                ),
                Style::default()
                    .fg(palette.text)
                    .bg(background)
                    .add_modifier(Modifier::BOLD),
            ))],
            background,
            target: Some(Target::Tab(index)),
        });
    }
    items
}

fn draw_scrollbar(
    buffer: &mut Buffer,
    viewport: Rect,
    total: usize,
    scroll: usize,
    palette: &Palette,
) {
    let height = usize::from(viewport.height);
    let max = total.saturating_sub(height);
    let metrics = crate::pane::ScrollMetrics {
        offset_from_bottom: max.saturating_sub(scroll),
        max_offset_from_bottom: max,
        viewport_rows: height,
    };
    let track = Rect::new(viewport.x, viewport.y, 1, viewport.height);
    for y in track.y..track.bottom() {
        put_text(
            buffer,
            track.x,
            y,
            1,
            "│",
            Style::default()
                .fg(palette.surface_dim)
                .bg(palette.panel_bg),
        );
    }
    if let Some(thumb) = crate::ui::scrollbar_thumb(metrics, track) {
        for y in thumb.top..thumb.top.saturating_add(thumb.len) {
            put_text(
                buffer,
                track.x,
                y,
                1,
                "▌",
                Style::default().fg(palette.accent).bg(palette.panel_bg),
            );
        }
    }
}
