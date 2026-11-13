use super::super::render::{display_width, put_right_text, put_text};
use super::super::sidebar::collapsed_sidebar_sections;
use super::*;

#[derive(Debug, Default)]
pub(in crate::client::shell) struct ParkedState {
    pub(in crate::client::shell) resize_pending: bool,
}

pub(in crate::client::shell) fn height(area: Rect, collapsed: bool) -> u16 {
    2 * u16::from(!collapsed && area.width >= 6 && area.height >= 6)
}

pub(in crate::client::shell) fn render(
    buffer: &mut Buffer,
    sidebar: Rect,
    height: u16,
    endpoints: &[ClientShellEndpoint],
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) {
    if height == 0 {
        return;
    }
    let (content, _, _) = collapsed_sidebar_sections(sidebar, config.sidebar_collapsed_mode);
    let active_count: usize = endpoints
        .iter()
        .filter_map(|endpoint| {
            endpoint
                .snapshot
                .as_deref()
                .map(|snapshot| (endpoint, snapshot))
        })
        .map(|(endpoint, snapshot)| {
            snapshot
                .panes
                .iter()
                .filter(|pane| !endpoint.parking.hides_pane(&pane.pane_id))
                .count()
        })
        .sum();
    let parked_count: usize = endpoints
        .iter()
        .map(|endpoint| endpoint.parking.parked_terminals().count())
        .sum();
    let p = &config.palette;
    for (offset, label, count) in [(3, " active", active_count), (2, " parked", parked_count)] {
        let area = Rect::new(
            content.x,
            content.bottom().saturating_sub(offset),
            content.width,
            1,
        );
        buffer.set_style(area, Style::default().bg(p.sidebar_bg));
        let count = format!("{count} ");
        put_text(
            buffer,
            area.x,
            area.y,
            area.width.saturating_sub(display_width(&count)),
            label,
            Style::default().fg(p.overlay0),
        );
        put_right_text(
            buffer,
            area,
            area.y,
            &count,
            Style::default().fg(p.overlay0),
        );
        if offset == 3 {
            hits.active_launcher = area;
        } else {
            hits.parked_launcher = area;
        }
    }
}

impl ClientShellState {
    pub(in crate::client::shell) fn handle_parked_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        use crossterm::event::{MouseButton, MouseEventKind};
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return false;
        }
        let point = (mouse.column, mouse.row);
        if contains(self.hits.dismiss_parking_preview, point) {
            self.dismiss_parking_preview(outcome);
        } else if let Some(action) = self
            .hits
            .empty_actions
            .iter()
            .find(|(rect, _)| contains(*rect, point))
            .map(|(_, action)| *action)
        {
            let focused = self
                .snapshot
                .as_deref()
                .and_then(|snapshot| snapshot.focused_workspace_id.clone());
            match (action, focused) {
                // Close the workspace on screen, not a navigation highlight.
                (crate::input::KeybindAction::CloseWorkspace, Some(workspace)) => {
                    self.request_workspace_close(workspace, None, outcome);
                    // The confirmation overlay needs a frame to appear.
                    outcome.repaint = true;
                }
                _ => self.record_binding(crate::input::KeybindMatch::Action(action), outcome),
            }
        } else if contains(self.hits.restore_pane, point) {
            self.focused_parking_action(ParkingAction::Restore, outcome);
        } else if contains(self.hits.parked_launcher, point) {
            self.open_parked();
            outcome.repaint = true;
        } else {
            return false;
        }
        true
    }
}
