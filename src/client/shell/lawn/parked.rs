use super::super::render::{display_width, put_right_text, put_text};
use super::super::sidebar::collapsed_sidebar_sections;
use super::*;

#[derive(Debug, Default)]
pub(in crate::client::shell) struct ParkedState {
    pub(in crate::client::shell) expanded: bool,
    pub(in crate::client::shell) collapsed: HashSet<ClientNavigatorTarget>,
    pub(in crate::client::shell) scroll: usize,
    pub(in crate::client::shell) resize_pending: bool,
}

pub(in crate::client::shell) fn rows(
    endpoints: &[ClientShellEndpoint],
    active: &ClientEndpointId,
    parked: &ParkedState,
    visible: bool,
) -> Vec<ClientNavigatorRow> {
    if !visible || !parked.expanded {
        return Vec::new();
    }
    let navigator = ClientNavigatorOverlay {
        query: TextEditor::default(),
        search_focused: false,
        selected: None,
        scroll: 0,
        filter: None,
        parked: true,
        machine: None,
        machine_scroll: 0,
        workspace: None,
    };
    let mut hidden_below = None;
    super::super::aggregate_navigation::legacy_navigator_rows(endpoints, active, &navigator)
        .into_iter()
        .filter(|row| {
            if hidden_below.is_some_and(|depth| row.depth > depth) {
                return false;
            }
            hidden_below = parked.collapsed.contains(&row.target).then_some(row.depth);
            true
        })
        .collect()
}

pub(in crate::client::shell) fn height(
    area: Rect,
    collapsed: bool,
    parked: &ParkedState,
    row_count: usize,
) -> u16 {
    if collapsed || area.width < 6 || area.height < 6 {
        0
    } else if parked.expanded {
        area.height.saturating_sub(4).div_ceil(2).max(2).min(
            row_count
                .saturating_add(1)
                .max(2)
                .min(usize::from(u16::MAX)) as u16,
        )
    } else {
        1
    }
}

pub(in crate::client::shell) fn render(
    buffer: &mut Buffer,
    sidebar: Rect,
    height: u16,
    endpoints: &[ClientShellEndpoint],
    rows: &[ClientNavigatorRow],
    parked: &mut ParkedState,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) {
    if height == 0 {
        return;
    }
    let (content, _, _) = collapsed_sidebar_sections(sidebar, config.sidebar_collapsed_mode);
    let area = Rect::new(
        content.x,
        content.bottom().saturating_sub(1 + height),
        content.width,
        height,
    );
    let p = &config.palette;
    buffer.set_style(area, Style::default().bg(p.sidebar_bg));
    let count: usize = endpoints
        .iter()
        .map(|endpoint| endpoint.parking.parked_terminals().count())
        .sum();
    let marker = if parked.expanded { "▾" } else { "▸" };
    put_text(
        buffer,
        area.x,
        area.y,
        area.width,
        &format!(" {marker} parked"),
        Style::default().fg(p.overlay0),
    );
    put_right_text(
        buffer,
        area,
        area.y,
        &format!("{count} "),
        Style::default().fg(p.overlay0),
    );
    hits.parked_toggle = Rect::new(area.x, area.y, area.width, 1);
    if !parked.expanded {
        return;
    }
    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );
    hits.parked_body = body;
    hits.parked_max_scroll = rows.len().saturating_sub(usize::from(body.height));
    parked.scroll = parked.scroll.min(hits.parked_max_scroll);
    if rows.is_empty() {
        put_text(
            buffer,
            body.x,
            body.y,
            body.width,
            " Nothing parked",
            Style::default().fg(p.overlay0),
        );
    }
    for (index, row) in rows
        .iter()
        .skip(parked.scroll)
        .take(usize::from(body.height))
        .enumerate()
    {
        let rect = Rect::new(body.x, body.y + index as u16, body.width, 1);
        let mut x = rect.x.saturating_add(1 + u16::from(row.depth) * 2);
        let leaf = matches!(row.target, ClientNavigatorTarget::Pane { .. });
        let style = Style::default().fg(if row.stale { p.overlay0 } else { p.overlay1 });
        if row.current && leaf {
            buffer.set_style(rect, Style::default().bg(p.active_row_bg));
        }
        if !leaf {
            let marker = if parked.collapsed.contains(&row.target) {
                "▸ "
            } else {
                "▾ "
            };
            put_text(
                buffer,
                x,
                rect.y,
                rect.right().saturating_sub(x),
                marker,
                style,
            );
            x = x.saturating_add(2);
        } else if let Some(status) = row.status {
            let icon = status_icon(status, config.status_indicators);
            put_text(
                buffer,
                x,
                rect.y,
                rect.right().saturating_sub(x),
                icon,
                Style::default().fg(status_color(status, p)),
            );
            x = x.saturating_add(display_width(icon) + 1);
        }
        put_text(
            buffer,
            x,
            rect.y,
            rect.right().saturating_sub(x),
            &row.label,
            style,
        );
        hits.parked_rows.push((rect, row.target.clone()));
    }
}

impl ClientShellState {
    pub(in crate::client::shell) fn handle_parked_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        use crossterm::event::{MouseButton, MouseEventKind};
        let point = (mouse.column, mouse.row);
        if matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) && contains(self.hits.parked_body, point)
        {
            self.parked.scroll = if mouse.kind == MouseEventKind::ScrollUp {
                self.parked.scroll.saturating_sub(3)
            } else {
                self.parked
                    .scroll
                    .saturating_add(3)
                    .min(self.hits.parked_max_scroll)
            };
            outcome.repaint = true;
            return true;
        }
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            if contains(self.hits.dismiss_parking_preview, point) {
                self.dismiss_parking_preview(outcome);
                return true;
            }
            if contains(self.hits.new_workspace_empty, point) {
                self.record_binding(
                    crate::input::KeybindMatch::Action(crate::input::KeybindAction::NewWorkspace),
                    outcome,
                );
                return true;
            }
            if contains(self.hits.restore_pane, point) {
                self.focused_parking_action(ParkingAction::Restore, outcome);
                return true;
            }
            if contains(self.hits.open_parked, point) {
                self.open_parked();
                outcome.repaint = true;
                return true;
            }
            if contains(self.hits.parked_toggle, point) {
                self.parked.expanded = !self.parked.expanded;
                outcome.repaint = true;
                return true;
            }
        }
        let target = self
            .hits
            .parked_rows
            .iter()
            .find(|(rect, _)| contains(*rect, point))
            .map(|(_, target)| target.clone());
        let Some(target) = target else {
            return false;
        };
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                match target {
                    ClientNavigatorTarget::Pane {
                        endpoint_id,
                        pane_id,
                    } => {
                        self.parking_action(endpoint_id, pane_id, ParkingAction::Preview, outcome);
                    }
                    target => {
                        if !self.parked.collapsed.remove(&target) {
                            self.parked.collapsed.insert(target);
                        }
                    }
                }
                outcome.repaint = true;
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if let ClientNavigatorTarget::Pane {
                    endpoint_id,
                    pane_id,
                } = target
                {
                    self.open_parked_context_menu(endpoint_id, pane_id, mouse.column, mouse.row);
                    outcome.repaint = true;
                }
            }
            _ => {}
        }
        true
    }
}
