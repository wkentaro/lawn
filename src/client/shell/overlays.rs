// Modified for Lawn.
use super::*;
use crate::ui::dialog::{self, button, panel};

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
        dialog::dim_backdrop(b);
    }
    match o {
        ClientShellOverlay::Onboarding => render_onboarding_overlay(b, k, p),
        ClientShellOverlay::ProductAnnouncement(v) => render_product_announcement_overlay(b, v, p),
        ClientShellOverlay::ReleaseNotes(v) => {
            render_release_notes_overlay(b, v, &s.update_install_command, p)
        }
        ClientShellOverlay::Rename(v) => {
            dialog::render_rename(b, v.title, &v.input, p).map(dialog_render)
        }
        ClientShellOverlay::ConfirmClose(v) => {
            dialog::render_confirm(b, &v.title, &v.detail, p).map(dialog_render)
        }
        ClientShellOverlay::Help(v) => render_help_overlay(b, v, k, p),
        ClientShellOverlay::Navigator(v) => {
            render_navigator_overlay(b, v, endpoints, active_endpoint_id, preview, p)
        }
        ClientShellOverlay::Settings(v) => render_settings_overlay(b, v, s, p),
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
    let items = super::super::global_menu::global_menu_items(snapshot, false)
        .into_iter()
        .map(|(label, action)| {
            (
                label,
                super::super::global_menu::global_menu_item_has_badge(snapshot, action),
            )
        })
        .collect::<Vec<_>>();
    let rendered =
        dialog::render_launcher_menu(buffer, launcher, &items, menu.highlighted, palette)?;
    Some(OverlayRender {
        area: rendered.area,
        menu_rows: rendered.rows,
        ..OverlayRender::default()
    })
}

pub(crate) fn render_context_menu(
    buffer: &mut Buffer,
    menu: &ClientContextMenuOverlay,
    palette: &Palette,
) -> Option<OverlayRender> {
    let labels = menu
        .items()
        .iter()
        .map(|item| item.label)
        .collect::<Vec<_>>();
    let rendered =
        dialog::render_context_menu(buffer, menu.x, menu.y, &labels, menu.highlighted, palette)?;
    Some(OverlayRender {
        area: rendered.area,
        menu_rows: rendered.rows,
        ..OverlayRender::default()
    })
}

fn dialog_render(rendered: dialog::Dialog) -> OverlayRender {
    OverlayRender {
        area: rendered.area,
        primary: rendered.primary,
        clear: rendered.clear,
        cancel: rendered.cancel,
        cursor: rendered.cursor,
        ..OverlayRender::default()
    }
}

fn render_settings_overlay(
    buffer: &mut Buffer,
    settings: &ClientSettingsOverlay,
    snapshot: &ClientShellSnapshot,
    palette: &Palette,
) -> Option<OverlayRender> {
    let rendered = dialog::render_settings(
        buffer,
        &dialog::Settings {
            section: settings.section,
            selected: settings.selected,
            original_theme_name: &settings.original_theme_name,
            integrations: &settings.integrations,
            integration_messages: &settings.integration_messages,
            loading_integrations: settings.loading_integrations,
            installing_integrations: settings.installing_integrations,
            integration_updates_available: snapshot.integration_updates_available,
        },
        palette,
    )?;
    Some(OverlayRender {
        area: rendered.area,
        primary: rendered.primary,
        cancel: rendered.close,
        settings_popup: rendered.area,
        settings_tabs: rendered.tabs,
        settings_choices: rendered.choices,
        ..OverlayRender::default()
    })
}

fn render_release_notes_overlay(
    b: &mut Buffer,
    notes: &crate::app::state::ReleaseNotesState,
    install_command: &str,
    p: &Palette,
) -> Option<OverlayRender> {
    let outer = crate::ui::centered_popup_rect(
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
            .fg(panel_contrast_fg(p))
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
    let outer = crate::ui::centered_popup_rect(
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
            .fg(panel_contrast_fg(p))
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
    let outer =
        crate::ui::centered_popup_rect_with_width_margin(b.area, (hint_width + 5).max(64), 16, 3)?;
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
            .fg(panel_contrast_fg(p))
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

pub(crate) fn render_navigator_overlay(
    b: &mut Buffer,
    n: &ClientNavigatorOverlay,
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    preview: &navigator_preview::NavigatorPreview,
    p: &Palette,
) -> Option<OverlayRender> {
    let rows = super::aggregate_navigation::navigator_rows(endpoints, active_endpoint_id, n);
    let endpoint = |endpoint_id: &ClientEndpointId| {
        endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
    };
    let views = rows
        .iter()
        .map(|row| dialog::NavigatorRow {
            kind: match &row.target {
                ClientNavigatorTarget::Machine { endpoint_id } => {
                    dialog::NavigatorRowKind::Machine {
                        signal: endpoint(endpoint_id)
                            .filter(|endpoint| {
                                !endpoint_id.is_local()
                                    && endpoint.status != ClientEndpointStatus::Online
                            })
                            .map(|endpoint| endpoint_status_presentation(endpoint.status, p)),
                    }
                }
                ClientNavigatorTarget::Workspace { .. } => dialog::NavigatorRowKind::Workspace,
                ClientNavigatorTarget::Pane { .. } => dialog::NavigatorRowKind::Pane,
                ClientNavigatorTarget::NewWorkspace => dialog::NavigatorRowKind::NewWorkspace,
            },
            depth: row.depth,
            label: &row.label,
            meta: &row.meta,
            detail: &row.detail,
            agent: row.agent.as_deref(),
            status: row.status,
            stale: row.stale,
            current: row.current,
            parked: row.parked,
            moving: n.moves(&row.target),
        })
        .collect::<Vec<_>>();
    let rendered = dialog::render_navigator(
        b,
        &dialog::Navigator {
            query: &n.query,
            parked: n.parked,
            moving: n.moving.as_ref().map(|moving| dialog::NavigatorMove {
                label: &moving.label,
                tab: matches!(moving.source, ClientMoveSource::Tab { .. }),
            }),
            scroll: n.scroll,
            selected: super::aggregate_navigation::navigator_selected_index(&rows, n),
            rows: &views,
            background: &|index| match rows.get(index).map(|row| &row.target) {
                Some(ClientNavigatorTarget::Pane {
                    endpoint_id,
                    pane_id,
                }) => endpoint(endpoint_id)
                    .map_or(0, |endpoint| preview.background_count(endpoint, pane_id)),
                _ => 0,
            },
        },
        p,
    )?;
    Some(OverlayRender {
        area: rendered.area,
        navigator_popup: rendered.area,
        navigator_search: rendered.search,
        navigator_rows: rendered
            .rows
            .into_iter()
            .filter_map(|(rect, index)| rows.get(index).map(|row| (rect, row.target.clone())))
            .collect(),
        navigator_preview: rendered.preview,
        navigator_content_preview: rendered.content_preview,
        navigator_close: rendered.close,
        navigator_scrollbar: rendered.scrollbar,
        navigator_scroll_metrics: rendered.scroll_metrics,
        cursor: rendered.cursor,
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

fn render_help_overlay(
    b: &mut Buffer,
    h: &ClientHelpOverlay,
    k: &LiveKeybindConfig,
    p: &Palette,
) -> Option<OverlayRender> {
    let rendered = dialog::render_help(b, &h.query, h.search_focused, h.scroll, k, p)?;
    Some(OverlayRender {
        area: rendered.area,
        cancel: rendered.close,
        help_popup: rendered.area,
        help_scrollbar: rendered.scrollbar,
        help_scroll_metrics: Some(rendered.scroll_metrics),
        help_max_scroll: rendered.max_scroll,
        cursor: rendered.cursor,
        ..OverlayRender::default()
    })
}
