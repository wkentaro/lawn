use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

use super::panel;
use crate::ui::chrome::{display_width, panel_contrast_fg, put_text};
use crate::ui::theme::Palette;

#[derive(Debug)]
pub(crate) struct Menu {
    pub(crate) area: Rect,
    pub(crate) rows: Vec<(Rect, usize)>,
}

fn row_style(highlighted: bool, palette: &Palette) -> Style {
    if highlighted {
        Style::default()
            .fg(panel_contrast_fg(palette))
            .bg(palette.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(palette.text).bg(palette.panel_bg)
    }
}

fn render_rows(
    buffer: &mut Buffer,
    rect: Rect,
    count: usize,
    highlighted: usize,
    palette: &Palette,
    mut draw: impl FnMut(&mut Buffer, usize, Rect, Style),
) -> Option<Menu> {
    let inner = panel(buffer, rect, palette.accent, palette.panel_bg)?;
    let mut rows = Vec::new();
    for index in 0..count {
        let row_y = inner.y.saturating_add(index as u16);
        if row_y >= inner.bottom() {
            break;
        }
        let row = Rect::new(inner.x, row_y, inner.width, 1);
        let style = row_style(index == highlighted, palette);
        buffer.set_style(row, style);
        draw(buffer, index, row, style);
        rows.push((row, index));
    }
    Some(Menu { area: rect, rows })
}

pub(crate) fn render_context_menu(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    labels: &[&str],
    highlighted: usize,
    palette: &Palette,
) -> Option<Menu> {
    let screen = buffer.area;
    let max_item_width = labels
        .iter()
        .map(|label| display_width(label))
        .max()
        .unwrap_or(0);
    let width = max_item_width
        .saturating_add(4)
        .max(14)
        .min(screen.width.max(1));
    let height = (labels.len() as u16)
        .saturating_add(2)
        .min(screen.height.max(1));
    let x = x.min(screen.x.saturating_add(screen.width.saturating_sub(width)));
    let y = y.min(
        screen
            .y
            .saturating_add(screen.height.saturating_sub(height)),
    );
    render_rows(
        buffer,
        Rect::new(x, y, width, height),
        labels.len(),
        highlighted,
        palette,
        |buffer, index, row, style| {
            put_text(buffer, row.x, row.y, row.width, labels[index], style);
        },
    )
}

pub(crate) fn render_launcher_menu(
    buffer: &mut Buffer,
    launcher: Rect,
    items: &[(&str, bool)],
    highlighted: usize,
    palette: &Palette,
) -> Option<Menu> {
    let screen = buffer.area;
    let width = items
        .iter()
        .map(|(label, badge)| display_width(label) + u16::from(*badge) * 2)
        .max()
        .unwrap_or(8)
        .saturating_add(4)
        .min(screen.width.max(1));
    let height = (items.len() as u16)
        .saturating_add(2)
        .min(screen.height.max(1));
    let x = launcher
        .right()
        .saturating_sub(width)
        .min(screen.right().saturating_sub(width));
    let y = launcher.y.saturating_sub(height).max(screen.y);
    render_rows(
        buffer,
        Rect::new(x, y, width, height),
        items.len(),
        highlighted,
        palette,
        |buffer, index, row, style| {
            let (label, badge) = items[index];
            if badge {
                let badge_style = if index == highlighted {
                    style
                } else {
                    Style::default()
                        .fg(palette.accent)
                        .bg(palette.panel_bg)
                        .add_modifier(Modifier::BOLD)
                };
                put_text(buffer, row.x, row.y, row.width.min(2), " ●", badge_style);
                put_text(
                    buffer,
                    row.x.saturating_add(2),
                    row.y,
                    row.width.saturating_sub(2),
                    &format!(" {label}"),
                    style,
                );
            } else {
                put_text(buffer, row.x, row.y, row.width, &format!(" {label}"), style);
            }
        },
    )
}
