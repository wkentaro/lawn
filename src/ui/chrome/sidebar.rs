use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Paragraph, Widget},
};

use super::{status_color, status_icon};
use crate::ui::theme::Palette;

pub(crate) struct WorkspaceRow<'a> {
    pub(crate) status: crate::api::schema::AgentStatus,
    pub(crate) rows: &'a [Vec<crate::ui::ResolvedToken>],
    pub(crate) focused: bool,
    pub(crate) selected: bool,
    pub(crate) navigating: bool,
    pub(crate) dragged: bool,
}

pub(crate) struct AgentRow<'a> {
    pub(crate) status: crate::api::schema::AgentStatus,
    pub(crate) rows: &'a [Vec<crate::ui::ResolvedToken>],
    pub(crate) focused: bool,
}

pub(crate) fn workspace_selection_background(palette: &Palette) -> ratatui::style::Color {
    if palette.selection_bg == ratatui::style::Color::Reset {
        palette.active_row_bg
    } else {
        palette.selection_bg
    }
}

pub(crate) fn workspace_active_background(
    palette: &Palette,
    navigating: bool,
) -> ratatui::style::Color {
    if navigating && palette.selection_bg == ratatui::style::Color::Reset {
        palette.sidebar_bg
    } else {
        palette.active_row_bg
    }
}

pub(crate) fn render_workspace_rows(
    buffer: &mut Buffer,
    area: Rect,
    row: WorkspaceRow<'_>,
    indicators: crate::config::StatusIndicatorStyle,
    palette: &Palette,
) {
    for (row_index, tokens) in row.rows.iter().enumerate() {
        let y = area.y + row_index as u16;
        if y >= area.bottom() {
            break;
        }
        let x = area.x.saturating_add(if row_index == 0 { 1 } else { 3 });
        let highlighted = row.focused || row.dragged;
        let workspace_style = Style::default()
            .fg(if highlighted {
                palette.text
            } else {
                palette.subtext0
            })
            .add_modifier(if highlighted {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });
        let secondary_style = Style::default().fg(if row.focused {
            palette.mauve
        } else {
            palette.overlay0
        });
        let spans = crate::ui::resolved_token_spans(
            tokens,
            (
                status_icon(row.status, indicators),
                Style::default().fg(status_color(row.status, palette)),
            ),
            Style::default().fg(status_color(row.status, palette)),
            workspace_style,
            secondary_style,
            Style::default().fg(palette.overlay1),
            palette,
            area.right().saturating_sub(2).saturating_sub(x) as usize,
        );
        Paragraph::new(Line::from(spans)).render(
            Rect::new(x, y, area.right().saturating_sub(2).saturating_sub(x), 1),
            buffer,
        );
    }

    let background = if row.selected {
        Some(workspace_selection_background(palette))
    } else if row.dragged {
        Some(palette.surface1)
    } else if row.focused {
        Some(workspace_active_background(palette, row.navigating))
    } else {
        None
    };
    if let Some(background) = background {
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                buffer[(x, y)].set_bg(background);
            }
        }
    }
}

pub(crate) fn render_agent_row(
    buffer: &mut Buffer,
    rect: Rect,
    row: AgentRow<'_>,
    indicators: crate::config::StatusIndicatorStyle,
    palette: &Palette,
) {
    let row_style = if row.focused {
        Style::default().bg(palette.active_row_bg)
    } else {
        Style::default()
    };
    let name_style = if row.focused {
        Style::default()
            .fg(palette.text)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(palette.subtext0)
            .add_modifier(Modifier::BOLD)
    };
    let status_style = Style::default().fg(status_color(row.status, palette));
    let secondary = Style::default().fg(palette.overlay0);
    let icon = (
        status_icon(row.status, indicators),
        Style::default().fg(status_color(row.status, palette)),
    );
    let fallback = [crate::ui::ResolvedToken {
        kind: crate::ui::ResolvedTokenKind::StateIcon,
        style: Default::default(),
    }];
    let rows = row
        .rows
        .iter()
        .map(Vec::as_slice)
        .chain(row.rows.is_empty().then_some(fallback.as_slice()));
    for (index, tokens) in rows.take(rect.height as usize).enumerate() {
        let indent = if index == 0 { 1 } else { 3 };
        let mut spans = vec![ratatui::text::Span::raw(" ".repeat(indent))];
        spans.extend(crate::ui::resolved_token_spans(
            tokens,
            icon,
            status_style,
            name_style,
            secondary,
            secondary,
            palette,
            rect.width.saturating_sub(indent as u16) as usize,
        ));
        Paragraph::new(Line::from(spans)).style(row_style).render(
            Rect::new(rect.x, rect.y + index as u16, rect.width, 1),
            buffer,
        );
    }
}
