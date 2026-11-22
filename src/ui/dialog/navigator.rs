use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

use super::{button, panel};
use crate::api::schema::AgentStatus;
use crate::pane::ScrollMetrics;
use crate::protocol::CursorState;
use crate::ui::chrome::{
    display_width, put_right_text, put_text, status_color, status_dot, status_text,
};
use crate::ui::text_editor::{self, TextEditor};
use crate::ui::theme::Palette;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NavigatorRowKind {
    Machine {
        signal: Option<(&'static str, &'static str, Color)>,
    },
    Workspace,
    Pane,
    Tab,
    NewWorkspace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NavigatorListing {
    Terminals,
    Tabs,
}

const WIDEST: u16 = 116;
const LIST_SHARE: u16 = 45;
const LIST_WIDTH: u16 = (WIDEST - 2) * LIST_SHARE / 100;

#[derive(Debug)]
pub(crate) struct NavigatorRow<'a> {
    pub(crate) kind: NavigatorRowKind,
    pub(crate) depth: u8,
    pub(crate) label: &'a str,
    pub(crate) meta: &'a str,
    pub(crate) detail: &'a str,
    pub(crate) agent: Option<&'a str>,
    pub(crate) status: Option<AgentStatus>,
    pub(crate) stale: bool,
    pub(crate) current: bool,
    pub(crate) parked: bool,
    pub(crate) moving: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct NavigatorMove<'a> {
    pub(crate) label: &'a str,
    pub(crate) tab: bool,
}

pub(crate) struct Navigator<'a> {
    pub(crate) listing: NavigatorListing,
    pub(crate) query: &'a TextEditor,
    pub(crate) parked: bool,
    pub(crate) moving: Option<NavigatorMove<'a>>,
    pub(crate) scroll: Option<usize>,
    pub(crate) selected: Option<usize>,
    pub(crate) rows: &'a [NavigatorRow<'a>],
    pub(crate) background: &'a dyn Fn(usize) -> u32,
}

#[derive(Debug, Default)]
pub(crate) struct NavigatorDialog {
    pub(crate) area: Rect,
    pub(crate) search: Rect,
    pub(crate) rows: Vec<(Rect, usize)>,
    pub(crate) preview: Rect,
    pub(crate) content_preview: Rect,
    pub(crate) close: Rect,
    pub(crate) scrollbar: Rect,
    pub(crate) scroll_metrics: Option<ScrollMetrics>,
    pub(crate) cursor: Option<CursorState>,
}

fn navigator_lists_rows(area: Rect, listing: NavigatorListing) -> bool {
    let narrowest = match listing {
        NavigatorListing::Terminals => 4,
        NavigatorListing::Tabs => 8,
    };
    area.width.saturating_sub(4) >= narrowest && area.height.saturating_sub(2).min(42) >= 6
}

pub(crate) fn render_navigator(
    b: &mut Buffer,
    n: &Navigator<'_>,
    p: &Palette,
) -> Option<NavigatorDialog> {
    let a = b.area;
    let width = a.width.saturating_sub(4).min(match n.listing {
        NavigatorListing::Terminals => WIDEST,
        NavigatorListing::Tabs => LIST_WIDTH + 2,
    });
    let rows = n.rows;
    let height = a.height.saturating_sub(2).min(42);
    if width < 4 || height < 3 {
        return None;
    }
    let q = Rect::new(
        a.x + (a.width - width) / 2,
        a.y + (a.height - height) / 2,
        width,
        height,
    )
    .intersection(a);
    let i = panel(b, q, p.accent, p.panel_bg)?;
    let title = match &n.moving {
        Some(moving) => format!(" move {} ", moving.label),
        None if n.parked => " parked ".into(),
        None => " active ".into(),
    };
    put_text(
        b,
        q.x + 2,
        q.y,
        q.width.saturating_sub(4),
        &title,
        Style::default().fg(p.accent).bg(p.panel_bg),
    );
    if !navigator_lists_rows(a, n.listing) {
        if i.height > 1 {
            put_text(
                b,
                i.x,
                i.y,
                i.width,
                " Enlarge terminal",
                Style::default().fg(p.overlay0).bg(p.panel_bg),
            );
        }
        let close = Rect::new(i.x, i.bottom() - 1, i.width, 1);
        button(
            b,
            close,
            "Close",
            Style::default().fg(p.accent).bg(p.panel_bg),
        );
        return Some(NavigatorDialog {
            area: q,
            close,
            ..NavigatorDialog::default()
        });
    }
    let (counted, one, many) = match n.listing {
        NavigatorListing::Terminals => (NavigatorRowKind::Pane, "terminal", "terminals"),
        NavigatorListing::Tabs => (NavigatorRowKind::Tab, "tab", "tabs"),
    };
    let listed = rows.iter().filter(|row| row.kind == counted).count();
    let count = format!("{listed} {}", if listed == 1 { one } else { many });
    put_text(
        b,
        i.x,
        i.y,
        i.width.saturating_sub(display_width(&count) + 1),
        " / ",
        Style::default().fg(p.text).bg(p.panel_bg),
    );
    let cursor = text_editor::render(
        b,
        Rect::new(
            i.x + 3,
            i.y,
            i.width.saturating_sub(4 + display_width(&count)),
            1,
        ),
        n.query,
        Style::default().fg(p.text).bg(p.panel_bg),
    );
    put_right_text(
        b,
        i,
        i.y,
        &count,
        Style::default().fg(p.overlay0).bg(p.panel_bg),
    );
    let search_height = if i.height < 7 { 1 } else { 2 };
    let header_height = search_height;
    let footer_height = if i.height < 7 { 1 } else { 3 };
    if search_height == 2 {
        put_text(
            b,
            i.x,
            i.y + 1,
            i.width,
            &"─".repeat(i.width as usize),
            Style::default().fg(p.surface1).bg(p.panel_bg),
        );
    }
    let mut row_hits = Vec::new();
    let body_y = i.y + header_height;
    let content = Rect::new(
        i.x,
        body_y,
        i.width,
        i.bottom().saturating_sub(body_y + footer_height),
    );
    let previews = n.listing == NavigatorListing::Terminals;
    let (body, preview_panel) = if !previews {
        (content, Rect::default())
    } else if content.width >= 90 && content.height >= 3 {
        let list_width = content.width * LIST_SHARE / 100;
        for y in content.y..content.bottom() {
            put_text(
                b,
                content.x + list_width,
                y,
                1,
                "│",
                Style::default().fg(p.surface1).bg(p.panel_bg),
            );
        }
        (
            Rect::new(content.x, content.y, list_width, content.height),
            Rect::new(
                content.x + list_width + 1,
                content.y,
                content.width - list_width - 1,
                content.height,
            ),
        )
    } else if content.height >= 6 {
        let list_height = content.height / 2;
        put_text(
            b,
            content.x,
            content.y + list_height,
            content.width,
            &"─".repeat(usize::from(content.width)),
            Style::default().fg(p.surface1).bg(p.panel_bg),
        );
        (
            Rect::new(content.x, content.y, content.width, list_height),
            Rect::new(
                content.x,
                content.y + list_height + 1,
                content.width,
                content.height - list_height - 1,
            ),
        )
    } else {
        (content, Rect::default())
    };
    let preview_content = if preview_panel.is_empty() {
        Rect::default()
    } else {
        put_text(
            b,
            preview_panel.x + 1,
            preview_panel.y,
            preview_panel.width.saturating_sub(2),
            "Preview · read-only",
            Style::default().fg(p.overlay0).bg(p.panel_bg),
        );
        Rect::new(
            preview_panel.x + 1,
            preview_panel.y + 1,
            preview_panel.width.saturating_sub(2),
            preview_panel.height.saturating_sub(1),
        )
    };
    let selected_index = n.selected;
    let max = rows.len().saturating_sub(body.height as usize);
    let scroll = n.scroll.unwrap_or_default();
    let scroll = selected_index
        .map(|selected| {
            scroll
                .max(selected.saturating_sub(body.height.saturating_sub(1) as usize))
                .min(selected)
        })
        .unwrap_or(scroll)
        .min(max);
    let metrics = ScrollMetrics {
        offset_from_bottom: max.saturating_sub(scroll),
        max_offset_from_bottom: max,
        viewport_rows: usize::from(body.height),
    };
    let scrollbar =
        (max > 0 && body.width > 1).then_some(Rect::new(body.right() - 1, body.y, 1, body.height));
    let row_width = body.width.saturating_sub(u16::from(scrollbar.is_some()));
    if rows.is_empty() {
        put_text(
            b,
            body.x,
            body.y,
            body.width,
            " No matching agents or terminals",
            Style::default().fg(p.overlay0).bg(p.panel_bg),
        );
    }
    for (ix, r) in rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(body.height as usize)
    {
        let rect = Rect::new(body.x, body.y + (ix - scroll) as u16, row_width, 1);
        let st = if r.stale {
            Style::default()
                .fg(p.overlay0)
                .bg(if selected_index == Some(ix) {
                    p.surface0
                } else {
                    p.panel_bg
                })
                .add_modifier(Modifier::DIM)
        } else if selected_index == Some(ix) {
            Style::default()
                .fg(p.text)
                .bg(p.panel_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(if matches!(r.kind, NavigatorRowKind::Machine { .. }) {
                    p.subtext0
                } else {
                    p.text
                })
                .bg(p.panel_bg)
        };
        let is_pane = matches!(r.kind, NavigatorRowKind::Pane);
        let nested = matches!(r.kind, NavigatorRowKind::Pane | NavigatorRowKind::Tab);
        let connector = if !nested {
            ""
        } else if rows.get(ix + 1).is_some_and(|next| next.kind == r.kind) {
            "├─ "
        } else {
            "└─ "
        };
        let padding = u16::from(r.depth.saturating_sub(u8::from(nested))) * 2 + 1;
        let connector_x = rect.x + padding;
        let indent = format!("{:width$}{connector}", "", width = usize::from(padding));
        let current = if r.current { "◆ " } else { "" };
        let status = if n.parked && r.stale {
            ""
        } else {
            r.status.map(status_dot).unwrap_or_default()
        };
        let status_separator = if status.is_empty() { "" } else { " " };
        let prefix = if n.parked && is_pane && r.stale && rect.width < 45 {
            format!("{indent}cached · ")
        } else {
            format!("{indent}{current}{status}{status_separator}")
        };
        let label = format!("{prefix}{}", r.label);
        let st = if !nested {
            st.add_modifier(Modifier::BOLD)
        } else {
            st
        };
        b.set_style(rect, st);
        let wide = rect.width >= 64;
        let tag = if n.parked && r.stale {
            Some("cached")
        } else if r.moving {
            Some("moving")
        } else if r.parked {
            Some("parked")
        } else {
            None
        };
        let agent_text = match tag {
            Some(tag) if !wide => tag,
            _ => r.agent.unwrap_or("terminal"),
        };
        let columns = if !is_pane {
            0
        } else if wide {
            24
        } else if rect.width >= 36 {
            display_width(agent_text).min(11) + 1
        } else {
            0
        };
        let marker = Some(columns)
            .filter(|columns| *columns > 0)
            .map(|_| (n.background)(ix))
            .filter(|count| *count > 0)
            .map(|count| format!("bg {count}"))
            .filter(|marker| {
                display_width(&label)
                    .saturating_add(display_width(marker))
                    .saturating_add(2)
                    <= rect.width.saturating_sub(columns)
            });
        let marker_width = marker
            .as_ref()
            .map_or(0, |marker| display_width(marker) + 1);
        let label_x = rect.x.saturating_add(display_width(&prefix));
        if drawn_past(
            b,
            (rect.x, rect.y),
            rect.width.saturating_sub(columns + marker_width),
            &label,
            if selected_index == Some(ix) {
                st.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                st
            },
        ) > label_x
        {
            row_hits.push((rect, ix));
        }
        if nested {
            put_text(
                b,
                connector_x,
                rect.y,
                rect.right().saturating_sub(connector_x).min(2),
                connector,
                if r.stale || selected_index == Some(ix) {
                    st
                } else {
                    st.fg(p.overlay0)
                }
                .remove_modifier(Modifier::UNDERLINED),
            );
        }
        if !n.parked || !r.stale {
            if let Some(status) = r.status {
                let prefix = format!("{indent}{current}");
                let status_style = if r.stale {
                    st
                } else {
                    st.fg(status_color(status, p))
                };
                put_text(
                    b,
                    rect.x.saturating_add(display_width(&prefix)),
                    rect.y,
                    display_width(status_dot(status)),
                    status_dot(status),
                    status_style.remove_modifier(Modifier::UNDERLINED),
                );
            }
        }
        if let Some(marker) = marker {
            put_text(
                b,
                rect.right() - columns - display_width(&marker),
                rect.y,
                display_width(&marker),
                &marker,
                st.fg(p.yellow).remove_modifier(Modifier::UNDERLINED),
            );
        }
        if is_pane {
            let meta_style = if r.stale || selected_index == Some(ix) {
                st
            } else {
                st.fg(p.overlay0)
            };
            if columns > 0 {
                put_text(
                    b,
                    rect.right() - columns + 1,
                    rect.y,
                    if wide { 11 } else { columns - 1 },
                    agent_text,
                    meta_style,
                );
            }
            if wide {
                put_text(
                    b,
                    rect.right() - 11,
                    rect.y,
                    11,
                    tag.unwrap_or(if r.agent.is_some() {
                        r.status.map(status_text).unwrap_or("unknown")
                    } else {
                        "shell"
                    }),
                    meta_style,
                );
            }
        }
        if let NavigatorRowKind::Machine {
            signal: Some((glyph, state, color)),
        } = r.kind
        {
            let signal = format!("{glyph} {state}");
            let signal_style = if selected_index == Some(ix) {
                st
            } else {
                Style::default()
                    .fg(color)
                    .bg(p.panel_bg)
                    .add_modifier(if r.stale {
                        Modifier::DIM
                    } else {
                        Modifier::empty()
                    })
            };
            put_right_text(b, rect, rect.y, &signal, signal_style);
        }
    }
    if let Some(track) = scrollbar {
        crate::ui::render_scrollbar_buffer(b, metrics, track, p.overlay0, p.overlay1, "▐");
    }
    let selected_row = selected_index.and_then(|index| rows.get(index));
    if let Some(r) = selected_row.filter(|_| footer_height == 3) {
        put_text(
            b,
            i.x,
            i.bottom() - 3,
            i.width,
            &format!(" {}", r.detail),
            Style::default().fg(p.subtext0).bg(p.panel_bg),
        );
        put_text(
            b,
            i.x,
            i.bottom() - 2,
            i.width,
            &format!(" {}", r.meta),
            Style::default().fg(p.overlay0).bg(p.panel_bg),
        );
    }
    let action_label = if let Some(moving) = &n.moving {
        match selected_row {
            Some(row) if row.moving => " Move Enter",
            Some(NavigatorRow {
                kind: NavigatorRowKind::Workspace,
                ..
            }) if moving.tab => " Move here Enter",
            Some(NavigatorRow {
                kind: NavigatorRowKind::Workspace,
                ..
            }) => " New tab Enter",
            Some(NavigatorRow {
                kind: NavigatorRowKind::NewWorkspace,
                ..
            }) => " New workspace Enter",
            _ => " Split right Enter",
        }
    } else {
        " Open Enter"
    };
    let close_label = "Esc close";
    let close_width = display_width(close_label).min(i.width);
    let action_area = Rect::new(
        i.x,
        i.bottom() - 1,
        display_width(action_label).min(i.width.saturating_sub(close_width + 1)),
        1,
    );
    let enabled = selected_row.is_some_and(|row| !row.stale && !row.moving);
    let close = Rect::new(i.right() - close_width, i.bottom() - 1, close_width, 1);
    let action_text = action_label.trim_start();
    let action_x = action_area.x.saturating_add(display_width(
        &action_label[..action_label.len() - action_text.len()],
    ));
    let action_drawn = drawn_past(
        b,
        (action_area.x, action_area.y),
        action_area.width,
        action_label,
        Style::default()
            .fg(if enabled { p.accent } else { p.overlay0 })
            .bg(p.panel_bg),
    ) > action_x;
    let preview = if enabled && action_drawn {
        action_area
    } else {
        Rect::default()
    };
    let hint = Rect::new(
        action_area.right() + 1,
        close.y,
        close.x.saturating_sub(action_area.right() + 1),
        1,
    );
    let hint_text = "↑↓ select · type to search";
    if previews || display_width(hint_text) < hint.width {
        put_text(
            b,
            hint.x,
            hint.y,
            hint.width,
            hint_text,
            Style::default().fg(p.overlay0).bg(p.panel_bg),
        );
    }
    put_right_text(
        b,
        close,
        close.y,
        close_label,
        Style::default().fg(p.overlay0).bg(p.panel_bg),
    );
    Some(NavigatorDialog {
        area: q,
        search: Rect::new(i.x, i.y, i.width, 1),
        rows: row_hits,
        preview,
        content_preview: preview_content,
        close,
        scrollbar: scrollbar.unwrap_or_default(),
        scroll_metrics: Some(metrics),
        cursor,
    })
}

fn drawn_past(b: &mut Buffer, (x, y): (u16, u16), width: u16, text: &str, style: Style) -> u16 {
    if width == 0 || y >= b.area.bottom() || x >= b.area.right() {
        return x;
    }
    b.set_stringn(x, y, text, usize::from(width), style).0
}

#[cfg(test)]
mod tests;
