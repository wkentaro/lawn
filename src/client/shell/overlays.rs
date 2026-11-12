// Modified for Lawn.
use super::*;

mod settings_overlay;
mod worktree_overlays;

#[derive(Default)]
pub(crate) struct OverlayRender {
    pub(crate) area: Rect,
    pub(crate) menu_rows: Vec<(Rect, usize)>,
    pub(crate) primary: Rect,
    pub(crate) clear: Rect,
    pub(crate) cancel: Rect,
    pub(crate) navigator_popup: Rect,
    pub(crate) navigator_search: Rect,
    pub(crate) navigator_rows: Vec<(Rect, ClientNavigatorTarget)>,
    pub(crate) navigator_preview: Rect,
    pub(crate) navigator_content_preview: Rect,
    pub(crate) navigator_close: Rect,
    pub(crate) navigator_scrollbar: Rect,
    pub(crate) navigator_scroll_metrics: Option<crate::pane::ScrollMetrics>,
    pub(crate) worktree_search: Rect,
    pub(crate) worktree_rows: Vec<(Rect, usize)>,
    pub(crate) help_popup: Rect,
    pub(crate) help_scrollbar: Rect,
    pub(crate) help_scroll_metrics: Option<crate::pane::ScrollMetrics>,
    pub(crate) help_max_scroll: usize,
    pub(crate) settings_popup: Rect,
    pub(crate) settings_tabs: Vec<(Rect, ClientSettingsSection)>,
    pub(crate) settings_choices: Vec<(Rect, usize)>,
    pub(crate) product_announcement_scrollbar: Rect,
    pub(crate) product_announcement_scroll_metrics: Option<crate::pane::ScrollMetrics>,
    pub(crate) product_announcement_max_scroll: usize,
    pub(crate) release_notes_scrollbar: Rect,
    pub(crate) release_notes_scroll_metrics: Option<crate::pane::ScrollMetrics>,
    pub(crate) release_notes_max_scroll: usize,
    pub(crate) cursor: Option<crate::protocol::CursorState>,
}

pub(crate) fn render_client_overlay(
    b: &mut Buffer,
    o: &ClientShellOverlay,
    s: &ClientShellSnapshot,
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    k: &LiveKeybindConfig,
    preview: &navigator_preview::NavigatorPreview,
    p: &Palette,
) -> Option<OverlayRender> {
    if !matches!(
        o,
        ClientShellOverlay::Navigator(_)
            | ClientShellOverlay::ContextMenu(_)
            | ClientShellOverlay::GlobalMenu(_)
    ) {
        for y in b.area.y..b.area.bottom() {
            for x in b.area.x..b.area.right() {
                let c = &mut b[(x, y)];
                c.set_style(c.style().add_modifier(Modifier::DIM));
            }
        }
    }
    match o {
        ClientShellOverlay::Onboarding => render_onboarding_overlay(b, k, p),
        ClientShellOverlay::ProductAnnouncement(v) => render_product_announcement_overlay(b, v, p),
        ClientShellOverlay::ReleaseNotes(v) => {
            render_release_notes_overlay(b, v, &s.update_install_command, p)
        }
        ClientShellOverlay::Rename(v) => render_rename_overlay(b, v, p),
        ClientShellOverlay::ConfirmClose(v) => render_confirm_close_overlay(b, v, p),
        ClientShellOverlay::Help(v) => render_help_overlay(b, v, k, p),
        ClientShellOverlay::Navigator(v) => {
            render_navigator_overlay(b, v, endpoints, active_endpoint_id, preview, p)
        }
        ClientShellOverlay::Settings(v) => {
            settings_overlay::render_settings_overlay(b, v, s.integration_updates_available, p)
        }
        ClientShellOverlay::WorktreeCreate(v) => {
            worktree_overlays::render_worktree_create_overlay(b, v, p)
        }
        ClientShellOverlay::WorktreeOpen(v) => {
            worktree_overlays::render_worktree_open_overlay(b, v, p)
        }
        ClientShellOverlay::WorktreeRemove(v) => {
            worktree_overlays::render_worktree_remove_overlay(b, v, p)
        }
        ClientShellOverlay::ContextMenu(_) | ClientShellOverlay::GlobalMenu(_) => None,
    }
}

pub(crate) fn render_global_menu(
    buffer: &mut Buffer,
    launcher: Rect,
    menu: &ClientGlobalMenuOverlay,
    snapshot: &ClientShellSnapshot,
    palette: &Palette,
) -> Option<OverlayRender> {
    let items = super::super::global_menu::global_menu_items(snapshot, false);
    let screen = buffer.area;
    let width = items
        .iter()
        .map(|(label, action)| {
            display_width(label)
                + u16::from(super::super::global_menu::global_menu_item_has_badge(
                    snapshot, *action,
                )) * 2
        })
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
    let rect = Rect::new(x, y, width, height);
    let inner = panel(buffer, rect, palette.accent, palette.panel_bg)?;
    let mut rows = Vec::new();
    for (index, (label, action)) in items.iter().enumerate() {
        let row_y = inner.y.saturating_add(index as u16);
        if row_y >= inner.bottom() {
            break;
        }
        let row = Rect::new(inner.x, row_y, inner.width, 1);
        let highlighted = index == menu.highlighted;
        let style = if highlighted {
            Style::default()
                .fg(panel_contrast_fg(palette))
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.text).bg(palette.panel_bg)
        };
        buffer.set_style(row, style);
        let has_badge = super::super::global_menu::global_menu_item_has_badge(snapshot, *action);
        if has_badge {
            let badge_style = if highlighted {
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
        rows.push((row, index));
    }
    Some(OverlayRender {
        area: rect,
        menu_rows: rows,
        ..OverlayRender::default()
    })
}

pub(crate) fn render_context_menu(
    buffer: &mut Buffer,
    menu: &ClientContextMenuOverlay,
    palette: &Palette,
) -> Option<OverlayRender> {
    let items = menu.items();
    let screen = buffer.area;
    let max_item_width = items
        .iter()
        .map(|item| display_width(item.label))
        .max()
        .unwrap_or(0);
    let width = max_item_width
        .saturating_add(4)
        .max(14)
        .min(screen.width.max(1));
    let height = (items.len() as u16)
        .saturating_add(2)
        .min(screen.height.max(1));
    let x = menu
        .x
        .min(screen.x.saturating_add(screen.width.saturating_sub(width)));
    let y = menu.y.min(
        screen
            .y
            .saturating_add(screen.height.saturating_sub(height)),
    );
    let rect = Rect::new(x, y, width, height);
    let inner = panel(buffer, rect, palette.accent, palette.panel_bg)?;
    let mut rows = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let row_y = inner.y.saturating_add(index as u16);
        if row_y >= inner.bottom() {
            break;
        }
        let row = Rect::new(inner.x, row_y, inner.width, 1);
        let highlighted = index == menu.highlighted;
        let style = if highlighted {
            Style::default()
                .fg(panel_contrast_fg(palette))
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.text).bg(palette.panel_bg)
        };
        buffer.set_style(row, style);
        put_text(buffer, row.x, row.y, row.width, item.label, style);
        rows.push((row, index));
    }
    Some(OverlayRender {
        area: rect,
        menu_rows: rows,
        ..OverlayRender::default()
    })
}

fn panel(
    b: &mut Buffer,
    a: Rect,
    c: ratatui::style::Color,
    bg: ratatui::style::Color,
) -> Option<Rect> {
    if a.width < 2 || a.height < 2 {
        return None;
    }
    let background = Style::default().bg(bg).remove_modifier(Modifier::DIM);
    let border = Style::default().fg(c).bg(bg).remove_modifier(Modifier::DIM);
    for y in a.y..a.bottom() {
        for x in a.x..a.right() {
            b[(x, y)].set_symbol(" ").set_style(background);
        }
    }
    for x in a.x..a.right() {
        b[(x, a.y)]
            .set_symbol(if x == a.x {
                "┌"
            } else if x + 1 == a.right() {
                "┐"
            } else {
                "─"
            })
            .set_style(border);
        let y = a.bottom() - 1;
        b[(x, y)]
            .set_symbol(if x == a.x {
                "└"
            } else if x + 1 == a.right() {
                "┘"
            } else {
                "─"
            })
            .set_style(border);
    }
    for y in a.y + 1..a.bottom() - 1 {
        b[(a.x, y)].set_symbol("│").set_style(border);
        b[(a.right() - 1, y)].set_symbol("│").set_style(border);
    }
    Some(Rect::new(a.x + 1, a.y + 1, a.width - 2, a.height - 2))
}
fn popup(a: Rect, w: u16, h: u16) -> Option<Rect> {
    popup_with_width_cap(a, w, h, 4)
}

fn popup_with_width_cap(a: Rect, w: u16, h: u16, width_cap: u16) -> Option<Rect> {
    let w = w.min(a.width.saturating_sub(width_cap));
    let h = h.min(a.height.saturating_sub(2));
    if w < 4 || h < 4 {
        return None;
    }
    Some(Rect::new(
        a.x + (a.width - w) / 2,
        a.y + (a.height - h) / 2,
        w,
        h,
    ))
}
fn button(b: &mut Buffer, r: Rect, t: &str, s: Style) {
    b.set_style(r, s);
    let w = display_width(t).min(r.width);
    put_text(b, r.x + (r.width - w) / 2, r.y, w, t, s)
}
fn row(i: Rect, ws: &[u16], gap: u16, off: u16) -> Vec<Rect> {
    let total = ws.iter().sum::<u16>() + gap * (ws.len().saturating_sub(1) as u16);
    let mut x = i.x + i.width.saturating_sub(total) / 2;
    ws.iter()
        .map(|w| {
            let r = Rect::new(
                x,
                i.y + off.min(i.height.saturating_sub(1)),
                (*w).min(i.width.saturating_sub(x - i.x)),
                1,
            );
            x += *w + gap;
            r
        })
        .collect()
}
fn contrast(p: &Palette) -> ratatui::style::Color {
    match p.panel_bg {
        ratatui::style::Color::Reset => p.surface_dim,
        c => c,
    }
}
fn render_release_notes_overlay(
    b: &mut Buffer,
    notes: &crate::app::state::ReleaseNotesState,
    install_command: &str,
    p: &Palette,
) -> Option<OverlayRender> {
    let outer = popup(
        b.area,
        crate::ui::RELEASE_NOTES_MODAL_SIZE.0,
        crate::ui::RELEASE_NOTES_MODAL_SIZE.1,
    )?;
    let inner = panel(b, outer, p.accent, p.panel_bg)?;
    if inner.height < 8 || inner.width < 20 {
        return Some(OverlayRender {
            area: outer,
            ..OverlayRender::default()
        });
    }

    let stack = crate::ui::modal_stack_areas(inner, 2, 1, 0, 1);
    let title_area = Rect::new(
        stack.header.x.saturating_add(1),
        stack.header.y,
        stack.header.width.saturating_sub(2),
        1,
    );
    let subtitle_area = Rect::new(
        stack.header.x.saturating_add(1),
        stack.header.y.saturating_add(1),
        stack.header.width.saturating_sub(2),
        1,
    );
    let base = Style::default()
        .bg(p.panel_bg)
        .remove_modifier(Modifier::DIM);
    put_text(
        b,
        title_area.x,
        title_area.y,
        title_area.width,
        &format!("v{}", notes.version),
        base.fg(p.text).add_modifier(Modifier::BOLD),
    );
    put_text(
        b,
        subtitle_area.x,
        subtitle_area.y,
        subtitle_area.width,
        if notes.preview {
            "update ready"
        } else {
            "what's new in this release"
        },
        base.fg(p.overlay1),
    );
    let close = crate::ui::release_notes_close_button_rect(Rect::new(
        stack.header.x,
        stack.header.y,
        stack.header.width,
        1,
    ));
    button(
        b,
        close,
        " esc close ",
        Style::default()
            .fg(contrast(p))
            .bg(p.accent)
            .add_modifier(Modifier::BOLD)
            .remove_modifier(Modifier::DIM),
    );

    let body = stack.content;
    let lines = crate::ui::release_notes_display_lines(notes, install_command, p);
    let metrics = crate::ui::release_notes_scroll_metrics(notes, install_command, body, p);
    let max_scroll = metrics.max_offset_from_bottom;
    let scroll = usize::from(notes.scroll).min(max_scroll);
    let track = crate::ui::release_notes_scrollbar_rect(body, metrics);
    let text_area = track
        .map(|_| Rect::new(body.x, body.y, body.width.saturating_sub(1), body.height))
        .unwrap_or(body);
    let paragraph = ratatui::widgets::Paragraph::new(
        lines.into_iter().map(|(_, line)| line).collect::<Vec<_>>(),
    )
    .wrap(ratatui::widgets::Wrap { trim: false })
    .scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0));
    ratatui::widgets::Widget::render(paragraph, text_area, b);
    if let Some(track) = track {
        crate::ui::render_scrollbar_buffer(b, metrics, track, p.overlay0, p.overlay1, "▐");
    }

    if let Some(footer_area) = stack.footer {
        let footer_line = ratatui::text::Line::from(vec![
            ratatui::text::Span::styled(" scroll ", base.fg(p.overlay0)),
            ratatui::text::Span::styled("wheel ↑↓", base.fg(p.text)),
            ratatui::text::Span::styled("  ·  ", base.fg(p.overlay0)),
            ratatui::text::Span::styled("close", base.fg(p.overlay0)),
            ratatui::text::Span::styled(" esc / enter ", base.fg(p.text)),
        ]);
        ratatui::widgets::Widget::render(
            ratatui::widgets::Paragraph::new(footer_line),
            footer_area,
            b,
        );
    }

    Some(OverlayRender {
        area: outer,
        primary: close,
        release_notes_scrollbar: track.unwrap_or_default(),
        release_notes_scroll_metrics: Some(metrics),
        release_notes_max_scroll: max_scroll,
        ..OverlayRender::default()
    })
}

fn render_product_announcement_overlay(
    b: &mut Buffer,
    announcement: &crate::app::state::ProductAnnouncementState,
    p: &Palette,
) -> Option<OverlayRender> {
    let outer = popup(
        b.area,
        crate::ui::PRODUCT_ANNOUNCEMENT_MODAL_SIZE.0,
        crate::ui::PRODUCT_ANNOUNCEMENT_MODAL_SIZE.1,
    )?;
    let inner = panel(b, outer, p.accent, p.panel_bg)?;
    if inner.height < 8 || inner.width < 20 {
        return Some(OverlayRender {
            area: outer,
            ..OverlayRender::default()
        });
    }

    let stack = crate::ui::modal_stack_areas(inner, 2, 1, 0, 1);
    let title_area = Rect::new(
        stack.header.x.saturating_add(1),
        stack.header.y,
        stack.header.width.saturating_sub(2),
        1,
    );
    let subtitle_area = Rect::new(
        stack.header.x.saturating_add(1),
        stack.header.y.saturating_add(1),
        stack.header.width.saturating_sub(2),
        1,
    );
    let base = Style::default()
        .bg(p.panel_bg)
        .remove_modifier(Modifier::DIM);
    put_text(
        b,
        title_area.x,
        title_area.y,
        title_area.width,
        &announcement.title,
        base.fg(p.text).add_modifier(Modifier::BOLD),
    );
    let subtitle = if announcement.preview {
        "product announcement preview"
    } else {
        "product announcement"
    };
    put_text(
        b,
        subtitle_area.x,
        subtitle_area.y,
        subtitle_area.width,
        &format!("{subtitle} · v{}", announcement.version),
        base.fg(p.overlay1),
    );
    let close = crate::ui::release_notes_close_button_rect(Rect::new(
        stack.header.x,
        stack.header.y,
        stack.header.width,
        1,
    ));
    button(
        b,
        close,
        " esc close ",
        Style::default()
            .fg(contrast(p))
            .bg(p.accent)
            .add_modifier(Modifier::BOLD)
            .remove_modifier(Modifier::DIM),
    );

    let body = stack.content;
    let lines = crate::ui::product_announcement_display_lines(announcement, p);
    let metrics = crate::ui::product_announcement_scroll_metrics(announcement, body, p);
    let max_scroll = metrics.max_offset_from_bottom;
    let scroll = usize::from(announcement.scroll).min(max_scroll);
    let track = crate::ui::release_notes_scrollbar_rect(body, metrics);
    let text_area = track
        .map(|_| Rect::new(body.x, body.y, body.width.saturating_sub(1), body.height))
        .unwrap_or(body);
    let paragraph = ratatui::widgets::Paragraph::new(
        lines.into_iter().map(|(_, line)| line).collect::<Vec<_>>(),
    )
    .wrap(ratatui::widgets::Wrap { trim: false })
    .scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0));
    ratatui::widgets::Widget::render(paragraph, text_area, b);
    if let Some(track) = track {
        crate::ui::render_scrollbar_buffer(b, metrics, track, p.overlay0, p.overlay1, "▐");
    }

    if let Some(footer_area) = stack.footer {
        let footer_line = ratatui::text::Line::from(vec![
            ratatui::text::Span::styled(" scroll ", base.fg(p.overlay0)),
            ratatui::text::Span::styled("wheel ↑↓", base.fg(p.text)),
            ratatui::text::Span::styled("  ·  ", base.fg(p.overlay0)),
            ratatui::text::Span::styled("close", base.fg(p.overlay0)),
            ratatui::text::Span::styled(" esc / enter ", base.fg(p.text)),
        ]);
        ratatui::widgets::Widget::render(
            ratatui::widgets::Paragraph::new(footer_line),
            footer_area,
            b,
        );
    }

    Some(OverlayRender {
        area: outer,
        primary: close,
        product_announcement_scrollbar: track.unwrap_or_default(),
        product_announcement_scroll_metrics: Some(metrics),
        product_announcement_max_scroll: max_scroll,
        ..OverlayRender::default()
    })
}

fn render_onboarding_overlay(
    b: &mut Buffer,
    k: &LiveKeybindConfig,
    p: &Palette,
) -> Option<OverlayRender> {
    let prefix = k.primary_prefix_label();
    let hint_width = display_width("  ")
        + display_width(&prefix)
        + display_width(crate::ui::ONBOARDING_PREFIX_SUFFIX)
        + display_width(crate::ui::ONBOARDING_HELP_LABEL)
        + display_width(crate::ui::ONBOARDING_HELP_SUFFIX);
    let outer = popup_with_width_cap(b.area, (hint_width + 5).max(64), 16, 3)?;
    let inner = panel(b, outer, p.accent, p.panel_bg)?;
    if inner.height < 11 {
        return Some(OverlayRender {
            area: outer,
            ..OverlayRender::default()
        });
    }
    let stack = crate::ui::modal_stack_areas(inner, 2, 0, 1, 1);
    let base = Style::default()
        .bg(p.panel_bg)
        .remove_modifier(Modifier::DIM);
    let title = base.fg(p.text).add_modifier(Modifier::BOLD);
    let muted = base.fg(p.overlay0);
    let text = base.fg(p.overlay1);
    let accent = base.fg(p.accent).add_modifier(Modifier::BOLD);

    put_text(
        b,
        stack.header.x,
        stack.header.y,
        stack.header.width,
        crate::ui::ONBOARDING_TITLE,
        title,
    );
    put_text(
        b,
        stack.header.x,
        stack.header.y.saturating_add(1),
        stack.header.width,
        crate::ui::ONBOARDING_SUBTITLE,
        muted,
    );

    let content = stack.content;
    for (offset, line) in crate::ui::ONBOARDING_DESCRIPTION.iter().enumerate() {
        put_text(
            b,
            content.x,
            content.y.saturating_add(offset as u16),
            content.width,
            line,
            text,
        );
    }

    let key_y = content.y.saturating_add(4);
    let mut key_x = content.x;
    for (value, style) in [
        ("  ", base),
        (prefix.as_str(), accent),
        (crate::ui::ONBOARDING_PREFIX_SUFFIX, text),
        (crate::ui::ONBOARDING_HELP_LABEL, accent),
        (crate::ui::ONBOARDING_HELP_SUFFIX, text),
    ] {
        let width = display_width(value);
        put_text(
            b,
            key_x,
            key_y,
            content.right().saturating_sub(key_x),
            value,
            style,
        );
        key_x = key_x.saturating_add(width);
    }
    put_text(
        b,
        content.x,
        content.y.saturating_add(5),
        content.width,
        crate::ui::ONBOARDING_NEXT,
        text,
    );

    let primary = crate::ui::onboarding_welcome_continue_rect(stack.actions.unwrap_or_default());
    button(
        b,
        primary,
        " ↵ continue ",
        Style::default()
            .fg(contrast(p))
            .bg(p.accent)
            .add_modifier(Modifier::BOLD)
            .remove_modifier(Modifier::DIM),
    );
    Some(OverlayRender {
        area: outer,
        primary,
        ..OverlayRender::default()
    })
}

fn render_rename_overlay(
    b: &mut Buffer,
    v: &ClientRenameOverlay,
    p: &Palette,
) -> Option<OverlayRender> {
    let q = popup(b.area, 56, 7)?;
    let i = panel(b, q, p.accent, p.panel_bg)?;
    put_text(
        b,
        i.x,
        i.y,
        i.width,
        v.title,
        Style::default()
            .fg(p.text)
            .bg(p.panel_bg)
            .add_modifier(Modifier::BOLD),
    );
    let input = Rect::new(i.x, i.y + 2, i.width, 1);
    b.set_style(input, Style::default().fg(p.text).bg(p.surface0));
    let cursor = text_editor::render(
        b,
        Rect::new(input.x + 1, input.y, input.width.saturating_sub(1), 1),
        &v.input,
        Style::default().fg(p.text).bg(p.surface0),
    );
    let rs = row(i, &[8, 10, 12], 2, 3);
    let [save, clear, cancel] = rs.as_slice() else {
        return None;
    };
    button(
        b,
        *save,
        " ↵ save ",
        Style::default()
            .fg(contrast(p))
            .bg(p.accent)
            .add_modifier(Modifier::BOLD),
    );
    let n = Style::default()
        .fg(p.text)
        .bg(p.surface0)
        .add_modifier(Modifier::BOLD);
    button(b, *clear, " ^c clear ", n);
    button(b, *cancel, " esc cancel ", n);
    Some(OverlayRender {
        area: q,
        primary: *save,
        clear: *clear,
        cancel: *cancel,
        navigator_popup: Rect::default(),
        navigator_search: Rect::default(),
        navigator_rows: Vec::new(),
        worktree_search: Rect::default(),
        worktree_rows: Vec::new(),
        cursor,
        ..OverlayRender::default()
    })
}

pub(crate) fn render_navigator_overlay(
    b: &mut Buffer,
    n: &ClientNavigatorOverlay,
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    preview: &navigator_preview::NavigatorPreview,
    p: &Palette,
) -> Option<OverlayRender> {
    let a = b.area;
    let width = a.width.saturating_sub(4).min(116);
    let minimum_height = 6;
    let rows = super::aggregate_navigation::navigator_rows(endpoints, active_endpoint_id, n);
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
    put_text(
        b,
        q.x + 2,
        q.y,
        q.width.saturating_sub(4),
        if n.parked { " parked " } else { " Go to " },
        Style::default().fg(p.accent).bg(p.panel_bg),
    );
    if height < minimum_height {
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
        return Some(OverlayRender {
            area: q,
            navigator_popup: q,
            navigator_close: close,
            ..OverlayRender::default()
        });
    }
    let terminal_count = rows
        .iter()
        .filter(|row| matches!(row.target, ClientNavigatorTarget::Pane { .. }))
        .count();
    let count = format!(
        "{terminal_count} {}",
        if terminal_count == 1 {
            "terminal"
        } else {
            "terminals"
        }
    );
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
        &n.query,
        Style::default().fg(p.text).bg(p.panel_bg),
    );
    put_right_text(
        b,
        i,
        i.y,
        &count,
        Style::default().fg(p.overlay0).bg(p.panel_bg),
    );
    // In short windows, give results priority over the divider and context footer.
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
    let (body, preview_panel) = if content.width >= 90 && content.height >= 3 {
        let list_width = content.width * 45 / 100;
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
    let selected_index = super::aggregate_navigation::navigator_selected_index(&rows, n);
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
    let metrics = crate::pane::ScrollMetrics {
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
        row_hits.push((rect, r.target.clone()));
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
                .fg(
                    if matches!(r.target, ClientNavigatorTarget::Machine { .. }) {
                        p.subtext0
                    } else {
                        p.text
                    },
                )
                .bg(p.panel_bg)
        };
        let is_pane = matches!(r.target, ClientNavigatorTarget::Pane { .. });
        let connector = if !is_pane {
            ""
        } else if rows
            .get(ix + 1)
            .is_some_and(|next| matches!(next.target, ClientNavigatorTarget::Pane { .. }))
        {
            "├─ "
        } else {
            "└─ "
        };
        let padding = u16::from(r.depth.saturating_sub(u8::from(is_pane))) * 2 + 1;
        let connector_x = rect.x + padding;
        let indent = format!("{:width$}{connector}", "", width = usize::from(padding));
        let current = if r.current { "◆ " } else { "" };
        let status = if n.parked && r.stale {
            ""
        } else {
            r.status.map(status_dot).unwrap_or_default()
        };
        let status_separator = if status.is_empty() { "" } else { " " };
        let label = if n.parked && is_pane && r.stale && rect.width < 45 {
            format!("{indent}cached · {}", r.label)
        } else {
            format!("{indent}{current}{status}{status_separator}{}", r.label)
        };
        let st = if !is_pane {
            st.add_modifier(Modifier::BOLD)
        } else {
            st
        };
        b.set_style(rect, st);
        let columns = if is_pane {
            if rect.width >= 64 {
                24
            } else if rect.width >= 36 {
                12
            } else {
                0
            }
        } else {
            0
        };
        let marker = if columns > 0 {
            if let ClientNavigatorTarget::Pane {
                endpoint_id,
                pane_id,
            } = &r.target
            {
                endpoints
                    .iter()
                    .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
                    .map(|endpoint| preview.background_count(endpoint, pane_id))
                    .filter(|count| *count > 0)
                    .map(|count| format!("bg {count}"))
                    .filter(|marker| {
                        display_width(&label)
                            .saturating_add(display_width(marker))
                            .saturating_add(2)
                            <= rect.width.saturating_sub(columns)
                    })
            } else {
                None
            }
        } else {
            None
        };
        let marker_width = marker
            .as_ref()
            .map_or(0, |marker| display_width(marker) + 1);
        put_text(
            b,
            rect.x,
            rect.y,
            rect.width.saturating_sub(columns + marker_width),
            &label,
            if selected_index == Some(ix) {
                st.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                st
            },
        );
        if is_pane {
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
                    11,
                    if n.parked && r.stale && columns == 12 {
                        "cached"
                    } else if r.parked && columns == 12 {
                        "parked"
                    } else {
                        r.agent.as_deref().unwrap_or("terminal")
                    },
                    meta_style,
                );
            }
            if columns == 24 {
                put_text(
                    b,
                    rect.right() - 11,
                    rect.y,
                    11,
                    if n.parked && r.stale {
                        "cached"
                    } else if r.parked {
                        "parked"
                    } else if r.agent.is_some() {
                        r.status.map(status_text).unwrap_or("unknown")
                    } else {
                        "shell"
                    },
                    meta_style,
                );
            }
        }
        let machine_status = match &r.target {
            ClientNavigatorTarget::Machine { endpoint_id } if !endpoint_id.is_local() => endpoints
                .iter()
                .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
                .filter(|endpoint| endpoint.status != ClientEndpointStatus::Online)
                .map(|endpoint| endpoint.status),
            _ => None,
        };
        if let Some(status) = machine_status {
            let (glyph, state, color) = endpoint_status_presentation(status, p);
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
        } else if !is_pane && !r.meta.is_empty() {
            let label_width = display_width(&label).min(rect.width);
            let meta = Rect::new(
                rect.x.saturating_add(label_width).saturating_add(1),
                rect.y,
                rect.width.saturating_sub(label_width.saturating_add(1)),
                1,
            );
            put_right_text(b, meta, rect.y, &r.meta, st)
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
    let action_label = if selected_row.is_some_and(|row| row.parked) {
        if i.width >= 38 {
            " Unpark & open Enter"
        } else {
            " Unpark Enter"
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
    let preview = if selected_row.is_some_and(|row| !row.stale) {
        action_area
    } else {
        Rect::default()
    };
    let close = Rect::new(i.right() - close_width, i.bottom() - 1, close_width, 1);
    put_text(
        b,
        action_area.x,
        action_area.y,
        action_area.width,
        action_label,
        Style::default()
            .fg(if preview.is_empty() {
                p.overlay0
            } else {
                p.accent
            })
            .bg(p.panel_bg),
    );
    let hint = Rect::new(
        action_area.right() + 1,
        close.y,
        close.x.saturating_sub(action_area.right() + 1),
        1,
    );
    put_text(
        b,
        hint.x,
        hint.y,
        hint.width,
        "↑↓ select · type to search",
        Style::default().fg(p.overlay0).bg(p.panel_bg),
    );
    put_right_text(
        b,
        close,
        close.y,
        close_label,
        Style::default().fg(p.overlay0).bg(p.panel_bg),
    );
    Some(OverlayRender {
        area: q,
        primary: Rect::default(),
        clear: Rect::default(),
        cancel: Rect::default(),
        navigator_popup: q,
        navigator_search: Rect::new(i.x, i.y, i.width, 1),
        navigator_rows: row_hits,
        navigator_preview: preview,
        navigator_content_preview: preview_content,
        navigator_close: close,
        navigator_scrollbar: scrollbar.unwrap_or_default(),
        navigator_scroll_metrics: Some(metrics),
        worktree_search: Rect::default(),
        worktree_rows: Vec::new(),
        cursor,
        ..OverlayRender::default()
    })
}

pub(in crate::client::shell) fn render_navigator_preview(
    buffer: &mut Buffer,
    area: Rect,
    preview: PreviewDisplay<'_>,
    palette: &Palette,
) {
    if area.is_empty() {
        return;
    }
    if let PreviewDisplay::Background { screen, background } = preview {
        let count = background_rows(background).count();
        let section_height = background_section_height(background, count, area.height);
        let screen_area = Rect::new(
            area.x,
            area.y,
            area.width,
            area.height.saturating_sub(section_height),
        );
        render_navigator_preview(buffer, screen_area, *screen, palette);
        render_background_section(
            buffer,
            Rect::new(area.x, screen_area.bottom(), area.width, section_height),
            background,
            count,
            palette,
        );
        return;
    }
    if let PreviewDisplay::Cells(snapshot) = preview {
        let rows = snapshot.height.min(area.height);
        let cols = snapshot.width.min(area.width);
        let default_fg = crate::protocol::u32_to_color(snapshot.default_fg);
        let default_bg = crate::protocol::u32_to_color(snapshot.default_bg);
        for y in 0..area.height {
            for x in 0..area.width {
                let cell = &mut buffer[(area.x + x, area.y + y)];
                cell.reset();
                cell.fg = default_fg;
                cell.bg = default_bg;
                if y >= rows || x >= cols {
                    continue;
                }
                let source =
                    &snapshot.cells[usize::from(y + snapshot.height.saturating_sub(area.height))
                        * usize::from(snapshot.width)
                        + usize::from(x)];
                let clipped = unicode_width::UnicodeWidthStr::width(source.symbol.as_str())
                    > usize::from(cols - x);
                cell.set_symbol(if clipped { " " } else { &source.symbol });
                cell.fg = crate::protocol::u32_to_color(source.fg);
                cell.bg = crate::protocol::u32_to_color(source.bg);
                cell.modifier = Modifier::from_bits_retain(source.modifier);
                cell.skip = source.skip;
            }
        }
        return;
    }
    let (text, color) = match preview {
        PreviewDisplay::Loading => ("Loading…", palette.overlay0),
        PreviewDisplay::Message(message) => (message, palette.overlay0),
        PreviewDisplay::None => ("Select a terminal", palette.overlay0),
        PreviewDisplay::Text("") => ("No output", palette.overlay0),
        PreviewDisplay::Text(text) => (text, palette.text),
        PreviewDisplay::Cells(_) | PreviewDisplay::Background { .. } => {
            unreachable!("structured preview rendered above")
        }
    };
    let skip = text
        .lines()
        .count()
        .saturating_sub(usize::from(area.height));
    for (offset, line) in text
        .lines()
        .skip(skip)
        .take(usize::from(area.height))
        .enumerate()
    {
        put_text(
            buffer,
            area.x,
            area.y + offset as u16,
            area.width,
            line,
            Style::default().fg(color).bg(palette.panel_bg),
        );
    }
}

struct BackgroundRow<'a> {
    kind: crate::terminal::background::BackgroundTaskKind,
    id: &'a str,
    status: Option<&'a str>,
    detail: &'a str,
    receipt: Option<(&'static str, u64)>,
}

fn background_rows(
    background: &crate::api::schema::background::BackgroundSnapshot,
) -> impl Iterator<Item = BackgroundRow<'_>> {
    use crate::terminal::background::{BackgroundTaskKind as Kind, ShellObservation};
    let shell = background.shell_observations.as_ref();
    let snapshot_tasks = shell
        .and_then(|shell| shell.last_received_snapshot.as_ref())
        .and_then(|record| match &record.observation {
            ShellObservation::SnapshotObserved { tasks, .. } => Some(tasks.as_slice()),
            _ => None,
        })
        .unwrap_or_default();
    let tasks = snapshot_tasks.iter().map(|task| BackgroundRow {
        kind: Kind::Shell,
        id: &task.task_id,
        status: Some(&task.reported_status),
        detail: task.command.as_deref().unwrap_or_default(),
        receipt: None,
    });
    let bash = shell
        .into_iter()
        .flat_map(|shell| &shell.bash_reports)
        .filter_map(move |record| {
            let ShellObservation::BashBackgroundObserved {
                task_id, command, ..
            } = &record.observation
            else {
                return None;
            };
            Some(BackgroundRow {
                kind: Kind::Shell,
                id: task_id,
                status: None,
                detail: command.as_deref().unwrap_or_default(),
                receipt: None,
            })
        });
    let agents = background.records.iter().map(|record| BackgroundRow {
        kind: Kind::Subagent,
        id: &record.task_id,
        status: None,
        detail: record.task_type.as_deref().unwrap_or_default(),
        receipt: record
            .stop_observed_at_ms
            .map(|at| ("stop seen", at))
            .or_else(|| record.start_observed_at_ms.map(|at| ("start seen", at))),
    });
    let waits = background
        .wait_declarations
        .iter()
        .filter(move |wait| match wait.kind {
            Kind::Subagent => !background
                .records
                .iter()
                .any(|record| record.task_id == wait.task_id),
            Kind::Shell => !shell.is_some_and(|shell| shell.contains_task(&wait.task_id)),
        })
        .map(|wait| BackgroundRow {
            kind: wait.kind,
            id: &wait.task_id,
            status: None,
            detail: "",
            receipt: None,
        });
    let mut shell_ids = std::collections::BTreeSet::new();
    tasks
        .chain(bash)
        .filter(move |row| shell_ids.insert(row.id))
        .chain(agents)
        .chain(waits)
}

impl navigator_preview::NavigatorPreview {
    pub(in crate::client::shell) fn background_has_rows(
        background: &crate::api::schema::background::BackgroundSnapshot,
    ) -> bool {
        background_rows(background).next().is_some()
    }
}

fn background_section_height(
    background: &crate::api::schema::background::BackgroundSnapshot,
    count: usize,
    height: u16,
) -> u16 {
    let footer = usize::from(
        background
            .shell_observations
            .as_ref()
            .is_some_and(|shell| shell.last_received_snapshot.is_some()),
    );
    if height / 2 < 3 {
        return 0;
    }
    u16::try_from(count + 2 + footer)
        .unwrap_or(u16::MAX)
        .min(height / 2)
}

fn background_age(now_ms: u64, received_at_ms: u64) -> String {
    let seconds = now_ms.saturating_sub(received_at_ms) / 1000;
    if seconds < 60 {
        format!("{seconds}s ago")
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else {
        format!("{}h ago", seconds / 3600)
    }
}

fn render_background_section(
    buffer: &mut Buffer,
    area: Rect,
    background: &crate::api::schema::background::BackgroundSnapshot,
    count: usize,
    palette: &Palette,
) {
    use crate::terminal::background::{BackgroundTaskKind as Kind, ShellObservation};
    if area.is_empty() || area.height < 3 {
        return;
    }
    let base = Style::default().fg(palette.subtext0).bg(palette.panel_bg);
    buffer.set_style(area, base);
    put_text(
        buffer,
        area.x,
        area.y,
        area.width,
        &"─".repeat(usize::from(area.width)),
        base.fg(palette.surface1),
    );
    put_text(buffer, area.x, area.y + 1, area.width, "Background", base);
    put_text(
        buffer,
        area.x + 11.min(area.width),
        area.y + 1,
        area.width.saturating_sub(11),
        "current state unknown",
        base.add_modifier(Modifier::DIM),
    );
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        });
    let footer = background
        .shell_observations
        .as_ref()
        .and_then(|shell| shell.last_received_snapshot.as_ref())
        .filter(|_| area.height > 3);
    let available = usize::from(area.height.saturating_sub(2 + u16::from(footer.is_some())));
    let shown = if count > available {
        available.saturating_sub(1)
    } else {
        count
    };
    for (index, row) in background_rows(background).take(shown).enumerate() {
        let kind = match row.kind {
            Kind::Shell => "shell",
            Kind::Subagent => "agent",
        };
        let status = row
            .status
            .map(|status| format!("  \"{status}\""))
            .unwrap_or_default();
        let detail = row.detail.lines().next().unwrap_or_default();
        let receipt = row
            .receipt
            .map(|(event, at)| format!("  {event} {}", background_age(now_ms, at)))
            .unwrap_or_default();
        let text = format!("{kind}  {}{status}  {detail}{receipt}", row.id);
        let waiting = background
            .wait_declarations
            .iter()
            .any(|wait| wait.kind == row.kind && wait.task_id == row.id);
        let y = area.y + 2 + index as u16;
        let wait_width = if waiting { 8.min(area.width) } else { 0 };
        put_text(
            buffer,
            area.x,
            y,
            area.width.saturating_sub(wait_width),
            &text,
            base,
        );
        if waiting {
            put_right_text(buffer, area, y, "waiting", base.fg(palette.yellow));
        }
    }
    if shown < count && available > 0 {
        put_text(
            buffer,
            area.x,
            area.y + 2 + shown as u16,
            area.width,
            &format!("+{} more", count - shown),
            base.add_modifier(Modifier::DIM),
        );
    }
    if let Some(record) = footer {
        if let ShellObservation::SnapshotObserved { hook_event, .. } = &record.observation {
            let hook = format!("{hook_event:?}");
            put_text(
                buffer,
                area.x,
                area.bottom() - 1,
                area.width,
                &format!(
                    "last shell report {} · {hook}",
                    background_age(now_ms, record.received_at_ms)
                ),
                base.add_modifier(Modifier::DIM),
            );
        }
    }
}

fn help_lines(
    keybinds: &LiveKeybindConfig,
    query: &str,
    palette: &Palette,
) -> Vec<(usize, ratatui::text::Line<'static>)> {
    use ratatui::text::{Line, Span};

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

fn render_help_overlay(
    b: &mut Buffer,
    h: &ClientHelpOverlay,
    k: &LiveKeybindConfig,
    p: &Palette,
) -> Option<OverlayRender> {
    use ratatui::widgets::{Paragraph, Widget, Wrap};

    let q = popup(b.area, 76, 22)?;
    let i = panel(b, q, p.accent, p.panel_bg)?;
    if i.width < 20 || i.height < 6 {
        return None;
    }
    put_text(
        b,
        i.x,
        i.y,
        i.width,
        "keybinds",
        Style::default()
            .fg(p.text)
            .bg(p.panel_bg)
            .add_modifier(Modifier::BOLD),
    );
    let close = Rect::new(i.right() - 13, i.y, 13, 1);
    button(
        b,
        close,
        if h.search_focused {
            " esc back "
        } else {
            " esc close "
        },
        Style::default()
            .fg(contrast(p))
            .bg(p.accent)
            .add_modifier(Modifier::BOLD),
    );
    let sy = i.y + 1;
    put_text(
        b,
        i.x,
        sy,
        i.width,
        &if h.search_focused {
            " / ".to_owned()
        } else {
            " / press / to filter by command or shortcut".to_owned()
        },
        Style::default()
            .fg(if h.search_focused { p.text } else { p.overlay0 })
            .bg(p.panel_bg),
    );
    let cursor = if h.search_focused {
        text_editor::render(
            b,
            Rect::new(i.x + 3, sy, i.width.saturating_sub(3), 1),
            &h.query,
            Style::default().fg(p.text).bg(p.panel_bg),
        )
    } else {
        None
    };

    let body = Rect::new(i.x, i.y + 3, i.width, i.height.saturating_sub(5));
    let lines = help_lines(k, &h.query, p);
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
    let scroll = h.scroll.min(max_scroll);
    let metrics = crate::pane::ScrollMetrics {
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
        b,
    );
    if let Some(track) = scrollbar {
        if let Some(thumb) = crate::ui::scrollbar_thumb(metrics, track) {
            for y in track.y..track.bottom() {
                b[(track.x, y)]
                    .set_symbol("▐")
                    .set_style(Style::default().fg(p.overlay0).bg(p.panel_bg));
            }
            for y in thumb.top..thumb.top.saturating_add(thumb.len) {
                b[(track.x, y)]
                    .set_symbol("▐")
                    .set_style(Style::default().fg(p.overlay1).bg(p.panel_bg));
            }
        }
    }

    put_text(
        b,
        i.x,
        i.bottom() - 1,
        i.width,
        if h.search_focused {
            " edit ←→/home/end · kill ^u/^k · yank ^y · scroll ↑↓ · back esc"
        } else {
            " search / · scroll j/k/↑↓/pgup/pgdn · close esc/enter"
        },
        Style::default().fg(p.overlay0).bg(p.panel_bg),
    );
    Some(OverlayRender {
        area: q,
        cancel: close,
        help_popup: q,
        help_scrollbar: scrollbar.unwrap_or_default(),
        help_scroll_metrics: Some(metrics),
        help_max_scroll: max_scroll,
        cursor,
        ..OverlayRender::default()
    })
}
fn render_confirm_close_overlay(
    b: &mut Buffer,
    c: &ClientConfirmCloseOverlay,
    p: &Palette,
) -> Option<OverlayRender> {
    let q = popup(b.area, 64, 6)?;
    let i = panel(b, q, p.red, p.panel_bg)?;
    put_text(
        b,
        i.x,
        i.y,
        i.width,
        &format!(" {}", c.title),
        Style::default()
            .fg(p.red)
            .bg(p.panel_bg)
            .add_modifier(Modifier::BOLD),
    );
    put_text(
        b,
        i.x,
        i.y + 1,
        i.width,
        &format!(" {}", c.detail),
        Style::default().fg(p.text).bg(p.panel_bg),
    );
    let rs = row(i, &[13, 12], 2, 3);
    let [ok, cancel] = rs.as_slice() else {
        return None;
    };
    button(
        b,
        *ok,
        " ↵ confirm ",
        Style::default()
            .fg(contrast(p))
            .bg(p.red)
            .add_modifier(Modifier::BOLD),
    );
    button(
        b,
        *cancel,
        " esc cancel ",
        Style::default()
            .fg(p.text)
            .bg(p.surface0)
            .add_modifier(Modifier::BOLD),
    );
    Some(OverlayRender {
        area: q,
        primary: *ok,
        clear: Rect::default(),
        cancel: *cancel,
        navigator_popup: Rect::default(),
        navigator_search: Rect::default(),
        navigator_rows: Vec::new(),
        worktree_search: Rect::default(),
        worktree_rows: Vec::new(),
        cursor: None,
        ..OverlayRender::default()
    })
}
