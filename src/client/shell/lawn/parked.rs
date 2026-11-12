use super::super::render::{put_right_text, put_text};
use super::super::sidebar::collapsed_sidebar_sections;
use super::*;

#[derive(Debug, Default)]
pub(in crate::client::shell) struct ParkedState {
    pub(in crate::client::shell) resize_pending: bool,
}

pub(in crate::client::shell) fn height(area: Rect, collapsed: bool) -> u16 {
    u16::from(!collapsed && area.width >= 6 && area.height >= 6)
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
    let area = Rect::new(
        content.x,
        content.bottom().saturating_sub(2),
        content.width,
        1,
    );
    let p = &config.palette;
    buffer.set_style(area, Style::default().bg(p.sidebar_bg));
    let count: usize = endpoints
        .iter()
        .map(|endpoint| endpoint.parking.parked_terminals().count())
        .sum();
    put_text(
        buffer,
        area.x,
        area.y,
        area.width,
        " parked",
        Style::default().fg(p.overlay0),
    );
    put_right_text(
        buffer,
        area,
        area.y,
        &format!("{count} "),
        Style::default().fg(p.overlay0),
    );
    hits.parked_launcher = area;
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
        } else if contains(self.hits.new_workspace_empty, point) {
            self.record_binding(
                crate::input::KeybindMatch::Action(crate::input::KeybindAction::NewWorkspace),
                outcome,
            );
        } else if contains(self.hits.restore_pane, point) {
            self.focused_parking_action(ParkingAction::Restore, outcome);
        } else if contains(self.hits.open_parked, point)
            || contains(self.hits.parked_launcher, point)
        {
            self.open_parked();
            outcome.repaint = true;
        } else {
            return false;
        }
        true
    }
}
