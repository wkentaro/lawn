// Modified for Lawn.
use super::*;

pub(in crate::client::shell) use crate::ui::chrome::workspace_active_background;
use crate::ui::chrome::workspace_selection_background;

pub(in crate::client::shell) fn collapsed_sidebar_sections(
    area: Rect,
    mode: SidebarCollapsedModeConfig,
) -> (Rect, Option<u16>, Rect) {
    let content = Rect::new(
        area.x,
        area.y,
        area.width.saturating_sub(1),
        area.height
            .saturating_sub(u16::from(mode == SidebarCollapsedModeConfig::Compact)),
    );
    if content.is_empty() {
        return (Rect::default(), None, Rect::default());
    }
    (content, None, Rect::default())
}

pub(crate) fn render_collapsed_sidebar(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    parking: Option<&lawn::PaneParking>,
    selected_workspace_id: Option<&str>,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    let selection_background = workspace_selection_background(palette);
    let active_background = workspace_active_background(palette, selected_workspace_id.is_some());
    render_sidebar_background(buffer, area, palette);
    let (workspace_area, divider_y, detail_area) =
        collapsed_sidebar_sections(area, config.sidebar_collapsed_mode);
    let mut number = 0;
    for (index, entry) in displayed_workspace_entries(snapshot, parking)
        .into_iter()
        .take(workspace_area.height as usize)
        .enumerate()
    {
        let workspace = &snapshot.workspaces[entry.index];
        let rect = Rect::new(
            workspace_area.x,
            workspace_area.y + index as u16,
            workspace_area.width,
            1,
        );
        let selected = selected_workspace_id == Some(workspace.workspace_id.as_str());
        if selected {
            buffer.set_style(rect, Style::default().bg(selection_background));
        } else if workspace.focused {
            buffer.set_style(rect, Style::default().bg(active_background));
        }
        let number_style = if selected {
            Style::default()
                .fg(palette.overlay1)
                .bg(selection_background)
        } else if workspace.focused {
            Style::default().fg(palette.text).bg(active_background)
        } else {
            Style::default().fg(palette.overlay0)
        };
        let label = if entry.temporary {
            "()".to_owned()
        } else {
            number += 1;
            format!("{number:<2}")
        };
        put_text(
            buffer,
            rect.x,
            rect.y,
            rect.width.min(2),
            &label,
            number_style,
        );
        if !entry.temporary {
            hits.workspaces.push(WorkspaceHit {
                rect,
                endpoint_id: ClientEndpointId::Local,
                workspace_id: workspace.workspace_id.clone(),
            });
        }
    }

    if let Some(divider_y) = divider_y {
        put_text(
            buffer,
            workspace_area.x,
            divider_y,
            workspace_area.width,
            &"─".repeat(workspace_area.width as usize),
            Style::default().fg(palette.surface_dim),
        );
    }

    let detail_content = Rect::new(
        detail_area.x,
        detail_area.y,
        detail_area.width,
        detail_area.height.saturating_sub(1),
    );
    for (index, pane_id) in (!detail_content.is_empty())
        .then(|| super::ordered_agent_pane_ids(snapshot, config.agent_panel_sort))
        .into_iter()
        .flatten()
        .take(detail_content.height as usize)
        .enumerate()
    {
        let Some(agent) = snapshot
            .agents
            .iter()
            .find(|agent| agent.pane_id == pane_id)
        else {
            continue;
        };
        let rect = Rect::new(
            detail_content.x,
            detail_content.y + index as u16,
            detail_content.width,
            1,
        );
        if agent.focused {
            buffer.set_style(rect, Style::default().bg(palette.active_row_bg));
        }
        put_text(
            buffer,
            rect.x,
            rect.y,
            rect.width.min(2),
            &format!("{:<2}", index + 1),
            Style::default().fg(if agent.focused {
                palette.text
            } else {
                palette.overlay0
            }),
        );
        put_text(
            buffer,
            rect.x.saturating_add(2),
            rect.y,
            rect.width.saturating_sub(2),
            status_icon(agent.agent_status, config.status_indicators),
            Style::default().fg(status_color(agent.agent_status, palette)),
        );
        hits.agents.push((rect, pane_id));
    }
    hits.sidebar_toggle = if area.is_empty() || workspace_area.width == 0 {
        Rect::default()
    } else {
        Rect::new(
            workspace_area.x + workspace_area.width / 2,
            area.bottom().saturating_sub(1),
            1,
            1,
        )
    };
    put_text(
        buffer,
        hits.sidebar_toggle.x,
        hits.sidebar_toggle.y,
        hits.sidebar_toggle.width,
        "»",
        if super::super::global_menu::global_menu_attention(snapshot) {
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.overlay0)
        },
    );
}

pub(crate) fn render_sidebar(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    state: &mut ShellRenderState<'_>,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    render_sidebar_background(buffer, area, palette);
    hits.sidebar_divider = if area.is_empty() {
        Rect::default()
    } else {
        Rect::new(area.right().saturating_sub(1), area.y, 1, area.height)
    };
    let (workspace_area, _, detail_area) =
        collapsed_sidebar_sections(area, config.sidebar_collapsed_mode);
    put_text(
        buffer,
        workspace_area.x,
        workspace_area.y,
        workspace_area.width,
        " spaces",
        Style::default()
            .fg(palette.overlay0)
            .add_modifier(Modifier::BOLD),
    );

    let entries = displayed_workspace_entries(
        snapshot,
        state
            .endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == state.active_endpoint_id)
            .map(|endpoint| &endpoint.parking),
    );
    let body = Rect::new(
        workspace_area.x,
        workspace_area.y.saturating_add(WORKSPACE_HEADER_ROWS),
        workspace_area.width,
        workspace_area
            .height
            .saturating_sub(WORKSPACE_HEADER_ROWS + 1 + state.navigation_height),
    );
    hits.workspace_body = body;
    let row_heights = entries
        .iter()
        .map(|entry| {
            displayed_workspace(snapshot, *entry)
                .map(|workspace| {
                    workspace_rows(&workspace, &config.spaces)
                        .len()
                        .max(1)
                        .min(u16::MAX as usize) as u16
                })
                .unwrap_or(1)
        })
        .collect::<Vec<_>>();
    let gaps = entries
        .iter()
        .enumerate()
        .map(|(index, _)| {
            if index + 1 < entries.len() {
                config.spaces.row_gap
            } else {
                0
            }
        })
        .collect::<Vec<_>>();
    let mut metrics = super::scroll::list_scroll_metrics(
        &row_heights,
        &gaps,
        body.height,
        *state.workspace_scroll,
    );
    if !body.is_empty() && std::mem::take(state.reveal_focused_workspace) {
        if let Some(target) = entries
            .iter()
            .position(|entry| snapshot.workspaces[entry.index].focused)
        {
            *state.workspace_scroll = super::scroll::list_scroll_start_to_reveal(
                &row_heights,
                &gaps,
                body.height,
                *state.workspace_scroll,
                target,
            );
            metrics = super::scroll::list_scroll_metrics(
                &row_heights,
                &gaps,
                body.height,
                *state.workspace_scroll,
            );
        }
    }
    hits.workspace_max_scroll = metrics.max_offset_from_bottom;
    hits.workspace_scroll_metrics = Some(metrics);
    *state.workspace_scroll = metrics
        .max_offset_from_bottom
        .saturating_sub(metrics.offset_from_bottom);
    let show_scrollbar = metrics.max_offset_from_bottom > 0 && body.width > 1;
    let content_width = body.width.saturating_sub(u16::from(show_scrollbar));
    let mut y = body.y;
    for (entry_position, entry) in entries.iter().enumerate().skip(*state.workspace_scroll) {
        let Some(workspace) = displayed_workspace(snapshot, *entry) else {
            continue;
        };
        let rows = workspace_rows(&workspace, &config.spaces);
        let row_height = (rows.len().max(1).min(u16::MAX as usize) as u16).min(body.height);
        if y.saturating_add(row_height) > body.bottom() {
            break;
        }
        let rect = Rect::new(body.x, y, content_width, row_height);
        let selected = state.selected_workspace_id.is_some_and(|target| {
            target.matches(state.active_endpoint_id, &workspace.workspace_id)
        });
        let dragged = state.dragged_workspace_id == Some(workspace.workspace_id.as_str());
        if selected {
            buffer.set_style(rect, Style::default().bg(palette.selection_bg));
        } else if dragged {
            buffer.set_style(rect, Style::default().bg(palette.surface1));
        } else if workspace.focused {
            buffer.set_style(rect, Style::default().bg(palette.active_row_bg));
        }
        render_workspace_rows(
            buffer,
            rect,
            workspace.agent_status,
            config.status_indicators,
            rows,
            workspace.focused,
            selected,
            state.selected_workspace_id.is_some(),
            dragged,
            palette,
        );
        if !entry.temporary {
            hits.workspaces.push(WorkspaceHit {
                rect,
                endpoint_id: ClientEndpointId::Local,
                workspace_id: workspace.workspace_id.clone(),
            });
        }
        let gap = gaps[entry_position];
        y = y.saturating_add(row_height + gap);
    }

    if show_scrollbar {
        let track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
        hits.workspace_scrollbar = track;
        super::scroll::render_list_scrollbar(buffer, track, metrics, palette);
    }

    if let Some(row) = state.workspace_drop_indicator_row.filter(|row| {
        *row >= workspace_area.y.saturating_add(1)
            && *row < workspace_area.bottom().saturating_sub(1)
    }) {
        put_text(
            buffer,
            body.x,
            row,
            body.width,
            &"─".repeat(body.width as usize),
            Style::default().fg(palette.accent),
        );
    }

    let footer_y = workspace_area.bottom().saturating_sub(1);
    if config.mouse_capture {
        hits.new_workspace = Rect::new(
            workspace_area.x,
            footer_y,
            5.min(workspace_area.width),
            u16::from(workspace_area.height > 0),
        );
        put_text(
            buffer,
            workspace_area.x,
            footer_y,
            workspace_area.width,
            " new",
            Style::default().fg(palette.overlay0),
        );
        let attention = super::super::global_menu::global_menu_attention(snapshot);
        let launcher_width = if attention { 8 } else { 6 }.min(workspace_area.width);
        hits.global_launcher = Rect::new(
            workspace_area.right().saturating_sub(launcher_width),
            footer_y,
            launcher_width,
            1,
        );
        if attention {
            let start_x = workspace_area.right().saturating_sub(6);
            put_text(
                buffer,
                start_x,
                footer_y,
                2,
                "● ",
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            );
            put_text(
                buffer,
                start_x.saturating_add(2),
                footer_y,
                4,
                "menu",
                Style::default().fg(palette.overlay0),
            );
        } else {
            put_right_text(
                buffer,
                workspace_area,
                footer_y,
                "menu",
                Style::default().fg(palette.overlay0),
            );
        }
    }

    super::render_agent_panel(
        buffer,
        detail_area,
        snapshot,
        config,
        state.agent_scroll,
        hits,
    );

    if config.sidebar_collapsed_mode == SidebarCollapsedModeConfig::Hidden {
        return;
    }
    hits.sidebar_toggle = Rect::new(
        area.right().saturating_sub(2),
        area.bottom().saturating_sub(1),
        u16::from(area.width > 1),
        u16::from(area.height > 0),
    );
    put_text(
        buffer,
        hits.sidebar_toggle.x,
        hits.sidebar_toggle.y,
        hits.sidebar_toggle.width,
        "«",
        Style::default().fg(palette.overlay0),
    );
}

/// The workspaces lists show, skipping parked ones.
pub(crate) fn workspace_entries(snapshot: &ClientShellSnapshot) -> Vec<WorkspaceEntry> {
    displayed_workspace_entries(snapshot, None)
}

pub(in crate::client::shell) fn displayed_workspace_entries(
    snapshot: &ClientShellSnapshot,
    parking: Option<&lawn::PaneParking>,
) -> Vec<WorkspaceEntry> {
    let temporary = parking
        .and_then(|parking| parking.restore_preview(snapshot))
        .filter(|preview| preview.temporary_workspace)
        .map(|preview| preview.pane.workspace_id.as_str());
    snapshot
        .workspaces
        .iter()
        .enumerate()
        .filter(|(_, workspace)| {
            !workspace.parked || Some(workspace.workspace_id.as_str()) == temporary
        })
        .map(|(index, workspace)| WorkspaceEntry {
            index,
            temporary: workspace.parked,
        })
        .collect()
}

pub(in crate::client::shell) fn displayed_workspace(
    snapshot: &ClientShellSnapshot,
    entry: WorkspaceEntry,
) -> Option<std::borrow::Cow<'_, ClientShellWorkspace>> {
    let workspace = snapshot.workspaces.get(entry.index)?;
    Some(if entry.temporary {
        std::borrow::Cow::Owned(ClientShellWorkspace {
            label: format!("({})", workspace.label),
            ..workspace.clone()
        })
    } else {
        std::borrow::Cow::Borrowed(workspace)
    })
}

pub(in crate::client::shell) fn workspace_rows(
    workspace: &ClientShellWorkspace,
    config: &SpacesSidebarConfig,
) -> Vec<Vec<crate::ui::ResolvedToken>> {
    let token_values = workspace.tokens.iter().cloned().collect::<HashMap<_, _>>();
    let mut rows = crate::ui::sidebar_space_rows(
        config,
        crate::ui::SpaceTokenContext {
            workspace: &workspace.label,
            state_text: status_text(workspace.agent_status),
            tokens: &token_values,
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
}

pub(in crate::client::shell) fn render_workspace_rows(
    buffer: &mut Buffer,
    area: Rect,
    status: crate::api::schema::AgentStatus,
    indicators: crate::config::StatusIndicatorStyle,
    rows: Vec<Vec<crate::ui::ResolvedToken>>,
    focused: bool,
    selected: bool,
    navigating: bool,
    dragged: bool,
    palette: &Palette,
) {
    crate::ui::chrome::render_workspace_rows(
        buffer,
        area,
        crate::ui::chrome::WorkspaceRow {
            status,
            rows: &rows,
            focused,
            selected,
            navigating,
            dragged,
        },
        indicators,
        palette,
    );
}
