use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};

use super::{button, panel};
use crate::config::LiveKeybindConfig;
use crate::pane::ScrollMetrics;
use crate::protocol::CursorState;
use crate::ui::chrome::{panel_contrast_fg, put_text};
use crate::ui::text_editor::{self, TextEditor};
use crate::ui::theme::Palette;

#[derive(Debug)]
pub(crate) struct HelpDialog {
    pub(crate) area: Rect,
    pub(crate) close: Rect,
    pub(crate) scrollbar: Rect,
    pub(crate) scroll_metrics: ScrollMetrics,
    pub(crate) max_scroll: usize,
    pub(crate) cursor: Option<CursorState>,
}

fn help_lines(
    keybinds: &LiveKeybindConfig,
    query: &str,
    palette: &Palette,
) -> Vec<(usize, Line<'static>)> {
    let groups = crate::input::filter_keybind_help_groups(
        crate::input::keybind_help_groups(&keybinds.keybinds, &keybinds.prefix),
        query,
    );
    let key_width = groups
        .iter()
        .flat_map(|(_, entries)| entries.iter().map(|(key, _)| key.chars().count()))
        .max()
        .unwrap_or(8);
    if groups.is_empty() {
        let message = " no matching keybinds";
        return vec![(
            message.chars().count(),
            Line::from(Span::styled(
                message,
                Style::default().fg(palette.overlay1).bg(palette.panel_bg),
            )),
        )];
    }

    let mut lines = Vec::new();
    for (group, entries) in groups {
        lines.push((
            group.len() + 1,
            Line::from(Span::styled(
                format!(" {group}"),
                Style::default()
                    .fg(palette.accent)
                    .bg(palette.panel_bg)
                    .add_modifier(Modifier::BOLD),
            )),
        ));
        for (key, label) in entries {
            let padded_key = format!(" {key:<key_width$} ");
            let width = padded_key.chars().count() + label.chars().count();
            lines.push((
                width,
                Line::from(vec![
                    Span::styled(
                        padded_key,
                        Style::default()
                            .fg(palette.mauve)
                            .bg(palette.panel_bg)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        label.into_owned(),
                        Style::default().fg(palette.text).bg(palette.panel_bg),
                    ),
                ]),
            ));
        }
        lines.push((0, Line::raw("")));
    }
    lines
}

pub(crate) fn render_help(
    buffer: &mut Buffer,
    query: &TextEditor,
    search_focused: bool,
    scroll: usize,
    keybinds: &LiveKeybindConfig,
    palette: &Palette,
) -> Option<HelpDialog> {
    let area = crate::ui::centered_popup_rect(buffer.area, 76, 22)?;
    let inner = panel(buffer, area, palette.accent, palette.panel_bg)?;
    if inner.width < 20 || inner.height < 6 {
        return None;
    }
    put_text(
        buffer,
        inner.x,
        inner.y,
        inner.width,
        "keybinds",
        Style::default()
            .fg(palette.text)
            .bg(palette.panel_bg)
            .add_modifier(Modifier::BOLD),
    );
    let close = Rect::new(inner.right() - 13, inner.y, 13, 1);
    button(
        buffer,
        close,
        if search_focused {
            " esc back "
        } else {
            " esc close "
        },
        Style::default()
            .fg(panel_contrast_fg(palette))
            .bg(palette.accent)
            .add_modifier(Modifier::BOLD),
    );
    let search_y = inner.y + 1;
    put_text(
        buffer,
        inner.x,
        search_y,
        inner.width,
        if search_focused {
            " / "
        } else {
            " / press / to filter by command or shortcut"
        },
        Style::default()
            .fg(if search_focused {
                palette.text
            } else {
                palette.overlay0
            })
            .bg(palette.panel_bg),
    );
    let cursor = if search_focused {
        text_editor::render(
            buffer,
            Rect::new(inner.x + 3, search_y, inner.width.saturating_sub(3), 1),
            query,
            Style::default().fg(palette.text).bg(palette.panel_bg),
        )
    } else {
        None
    };

    let body = Rect::new(
        inner.x,
        inner.y + 3,
        inner.width,
        inner.height.saturating_sub(5),
    );
    let lines = help_lines(keybinds, query, palette);
    let viewport_rows = usize::from(body.height.max(1));
    let wrapped_rows = |width: u16| {
        let width = usize::from(width.max(1));
        lines
            .iter()
            .map(|(line_width, _)| line_width.max(&1).div_ceil(width))
            .sum::<usize>()
    };
    let needs_scrollbar = wrapped_rows(body.width) > viewport_rows;
    let text_area = if needs_scrollbar {
        Rect::new(body.x, body.y, body.width.saturating_sub(1), body.height)
    } else {
        body
    };
    let total_rows = wrapped_rows(text_area.width);
    let max_scroll = total_rows.saturating_sub(viewport_rows);
    let scroll = scroll.min(max_scroll);
    let scroll_metrics = ScrollMetrics {
        offset_from_bottom: max_scroll.saturating_sub(scroll),
        max_offset_from_bottom: max_scroll,
        viewport_rows,
    };
    let scrollbar = needs_scrollbar.then_some(Rect::new(
        body.right().saturating_sub(1),
        body.y,
        1,
        body.height,
    ));
    Widget::render(
        Paragraph::new(lines.into_iter().map(|(_, line)| line).collect::<Vec<_>>())
            .wrap(Wrap { trim: false })
            .scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0)),
        text_area,
        buffer,
    );
    if let Some(track) = scrollbar {
        if let Some(thumb) = crate::ui::scrollbar_thumb(scroll_metrics, track) {
            for y in track.y..track.bottom() {
                buffer[(track.x, y)]
                    .set_symbol("▐")
                    .set_style(Style::default().fg(palette.overlay0).bg(palette.panel_bg));
            }
            for y in thumb.top..thumb.top.saturating_add(thumb.len) {
                buffer[(track.x, y)]
                    .set_symbol("▐")
                    .set_style(Style::default().fg(palette.overlay1).bg(palette.panel_bg));
            }
        }
    }

    put_text(
        buffer,
        inner.x,
        inner.bottom() - 1,
        inner.width,
        if search_focused {
            " edit ←→/home/end · kill ^u/^k · yank ^y · scroll ↑↓ · back esc"
        } else {
            " search / · scroll j/k/↑↓/pgup/pgdn · close esc/enter"
        },
        Style::default().fg(palette.overlay0).bg(palette.panel_bg),
    );
    Some(HelpDialog {
        area,
        close,
        scrollbar: scrollbar.unwrap_or_default(),
        scroll_metrics,
        max_scroll,
        cursor,
    })
}
