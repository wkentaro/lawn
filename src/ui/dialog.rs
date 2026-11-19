mod help;
mod menu;
mod navigator;
mod settings;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

use crate::protocol::CursorState;
use crate::ui::chrome::{display_width, panel_contrast_fg, put_text};
use crate::ui::text_editor::{self, TextEditor};
use crate::ui::theme::Palette;

pub(crate) use help::render_help;
pub(crate) use menu::{render_context_menu, render_launcher_menu};
pub(crate) use navigator::{
    render_navigator, Navigator, NavigatorMove, NavigatorRow, NavigatorRowKind,
};
pub(crate) use settings::{
    integration_needs_install, normalized_theme_name, render_settings, Settings, SettingsSection,
};

#[derive(Debug, Default)]
pub(crate) struct Dialog {
    pub(crate) area: Rect,
    pub(crate) primary: Rect,
    pub(crate) clear: Rect,
    pub(crate) cancel: Rect,
    pub(crate) cursor: Option<CursorState>,
}

pub(crate) fn dim_backdrop(buffer: &mut Buffer) {
    for y in buffer.area.y..buffer.area.bottom() {
        for x in buffer.area.x..buffer.area.right() {
            let cell = &mut buffer[(x, y)];
            cell.set_style(cell.style().add_modifier(Modifier::DIM));
        }
    }
}

pub(crate) fn panel(buffer: &mut Buffer, area: Rect, border: Color, bg: Color) -> Option<Rect> {
    if area.width < 2 || area.height < 2 {
        return None;
    }
    let background = Style::default().bg(bg).remove_modifier(Modifier::DIM);
    let border = Style::default()
        .fg(border)
        .bg(bg)
        .remove_modifier(Modifier::DIM);
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            buffer[(x, y)].set_symbol(" ").set_style(background);
        }
    }
    for x in area.x..area.right() {
        buffer[(x, area.y)]
            .set_symbol(if x == area.x {
                "┌"
            } else if x + 1 == area.right() {
                "┐"
            } else {
                "─"
            })
            .set_style(border);
        let y = area.bottom() - 1;
        buffer[(x, y)]
            .set_symbol(if x == area.x {
                "└"
            } else if x + 1 == area.right() {
                "┘"
            } else {
                "─"
            })
            .set_style(border);
    }
    for y in area.y + 1..area.bottom() - 1 {
        buffer[(area.x, y)].set_symbol("│").set_style(border);
        buffer[(area.right() - 1, y)]
            .set_symbol("│")
            .set_style(border);
    }
    Some(Rect::new(
        area.x + 1,
        area.y + 1,
        area.width - 2,
        area.height - 2,
    ))
}

pub(crate) fn button(buffer: &mut Buffer, area: Rect, text: &str, style: Style) {
    buffer.set_style(area, style);
    let width = display_width(text).min(area.width);
    put_text(
        buffer,
        area.x + (area.width - width) / 2,
        area.y,
        width,
        text,
        style,
    )
}

pub(crate) fn button_row(inner: Rect, widths: &[u16], gap: u16, offset: u16) -> Vec<Rect> {
    let total = widths.iter().sum::<u16>() + gap * (widths.len().saturating_sub(1) as u16);
    let mut x = inner.x + inner.width.saturating_sub(total) / 2;
    widths
        .iter()
        .map(|width| {
            let rect = Rect::new(
                x,
                inner.y + offset.min(inner.height.saturating_sub(1)),
                (*width).min(inner.width.saturating_sub(x - inner.x)),
                1,
            );
            x += *width + gap;
            rect
        })
        .collect()
}

pub(crate) fn render_confirm(
    buffer: &mut Buffer,
    title: &str,
    detail: &str,
    palette: &Palette,
) -> Option<Dialog> {
    let area = crate::ui::centered_popup_rect(buffer.area, 64, 6)?;
    let inner = panel(buffer, area, palette.red, palette.panel_bg)?;
    put_text(
        buffer,
        inner.x,
        inner.y,
        inner.width,
        &format!(" {title}"),
        Style::default()
            .fg(palette.red)
            .bg(palette.panel_bg)
            .add_modifier(Modifier::BOLD),
    );
    put_text(
        buffer,
        inner.x,
        inner.y + 1,
        inner.width,
        &format!(" {detail}"),
        Style::default().fg(palette.text).bg(palette.panel_bg),
    );
    let [confirm, cancel] = button_row(inner, &[13, 12], 2, 3)[..] else {
        return None;
    };
    button(
        buffer,
        confirm,
        " ↵ confirm ",
        Style::default()
            .fg(panel_contrast_fg(palette))
            .bg(palette.red)
            .add_modifier(Modifier::BOLD),
    );
    button(
        buffer,
        cancel,
        " esc cancel ",
        Style::default()
            .fg(palette.text)
            .bg(palette.surface0)
            .add_modifier(Modifier::BOLD),
    );
    Some(Dialog {
        area,
        primary: confirm,
        cancel,
        ..Dialog::default()
    })
}

pub(crate) fn render_rename(
    buffer: &mut Buffer,
    title: &str,
    input: &TextEditor,
    palette: &Palette,
) -> Option<Dialog> {
    let area = crate::ui::centered_popup_rect(buffer.area, 56, 7)?;
    let inner = panel(buffer, area, palette.accent, palette.panel_bg)?;
    put_text(
        buffer,
        inner.x,
        inner.y,
        inner.width,
        title,
        Style::default()
            .fg(palette.text)
            .bg(palette.panel_bg)
            .add_modifier(Modifier::BOLD),
    );
    let field = Rect::new(inner.x, inner.y + 2, inner.width, 1);
    let field_style = Style::default().fg(palette.text).bg(palette.surface0);
    buffer.set_style(field, field_style);
    let cursor = text_editor::render(
        buffer,
        Rect::new(field.x + 1, field.y, field.width.saturating_sub(1), 1),
        input,
        field_style,
    );
    let [save, clear, cancel] = button_row(inner, &[8, 10, 12], 2, 3)[..] else {
        return None;
    };
    button(
        buffer,
        save,
        " ↵ save ",
        Style::default()
            .fg(panel_contrast_fg(palette))
            .bg(palette.accent)
            .add_modifier(Modifier::BOLD),
    );
    let secondary = Style::default()
        .fg(palette.text)
        .bg(palette.surface0)
        .add_modifier(Modifier::BOLD);
    button(buffer, clear, " ^c clear ", secondary);
    button(buffer, cancel, " esc cancel ", secondary);
    Some(Dialog {
        area,
        primary: save,
        clear,
        cancel,
        cursor,
    })
}
