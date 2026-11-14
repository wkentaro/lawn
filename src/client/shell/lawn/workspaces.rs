use super::*;
use crate::api::schema::Method;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::client::shell) enum EmptyWorkspaceAction {
    NewTab,
    OpenParked,
    Close,
}

impl ClientShellState {
    /// A parked pane or its hidden tab never closes on its own. Closing its
    /// workspace closes it along with the rest.
    pub(in crate::client::shell) fn preserve_parked_close(&self, method: &Method) -> bool {
        let Some(parking) = self.active_parking() else {
            return false;
        };
        match method {
            Method::PaneClose(target) => parking.hides_pane(&target.pane_id),
            // A peek or an empty parked view focuses a parked pane's hidden tab.
            Method::TabClose(target) => self.snapshot.as_deref().is_some_and(|snapshot| {
                !snapshot.tabs.iter().any(|tab| tab.tab_id == target.tab_id)
                    && snapshot
                        .parked_panes
                        .iter()
                        .any(|pane| pane.tab_id == target.tab_id)
            }),
            _ => false,
        }
    }

    /// Draws the actions of a focused workspace without an active pane.
    /// Returns whether the pane surface is empty.
    pub(super) fn render_empty_workspace(
        &mut self,
        buffer: &mut Buffer,
        layout: ClientShellLayout,
    ) -> bool {
        let (Some(snapshot), Some(parking)) = (self.snapshot.as_deref(), self.active_parking())
        else {
            return false;
        };
        // A workspace whose every pane is parked or closed stays until the user closes it.
        let Some(workspace) = snapshot
            .focused_workspace_id
            .as_deref()
            .filter(|workspace| {
                !snapshot.panes.iter().any(|pane| {
                    &pane.workspace_id == workspace && !parking.hides_pane(&pane.pane_id)
                })
            })
        else {
            return false;
        };
        let area = layout.pane_surface;
        if area.is_empty() {
            return true;
        }
        let p = &self.config.palette;
        let parked = snapshot
            .parked_panes
            .iter()
            .any(|pane| pane.workspace_id == workspace);
        let actions: Vec<_> = [
            Some((
                "New tab",
                EmptyWorkspaceAction::NewTab,
                Style::default().fg(p.accent).bg(p.surface0),
            )),
            parked.then_some((
                "Open parked",
                EmptyWorkspaceAction::OpenParked,
                Style::default().fg(p.accent),
            )),
            Some((
                "Close workspace",
                EmptyWorkspaceAction::Close,
                Style::default().fg(p.overlay0),
            )),
        ]
        .into_iter()
        .flatten()
        .collect();
        let count = actions.len() as u16;
        let gap = if area.height > count * 2 { 2 } else { 1 };
        let label_width = actions
            .iter()
            .map(|(label, ..)| render::display_width(label))
            .max()
            .unwrap_or(0);
        let width = label_width.saturating_add(4).min(area.width);
        let x = area.x + area.width.saturating_sub(width) / 2;
        let top = area.y + area.height.saturating_sub((count - 1) * gap + 1) / 2;
        for (index, (label, action, style)) in actions.into_iter().enumerate() {
            let y = top + index as u16 * gap;
            if y >= area.bottom() {
                break;
            }
            let rect = Rect::new(x, y, width, 1);
            render::put_text(
                buffer,
                rect.x,
                rect.y,
                rect.width,
                &format!("  {label:<pad$}  ", pad = usize::from(label_width)),
                style,
            );
            self.hits.empty_workspace.push((rect, action));
        }
        true
    }

    pub(super) fn empty_workspace_action(
        &mut self,
        action: EmptyWorkspaceAction,
        outcome: &mut ClientShellInput,
    ) {
        match action {
            EmptyWorkspaceAction::NewTab => self.record_binding(
                crate::input::KeybindMatch::Action(crate::input::KeybindAction::NewTab),
                outcome,
            ),
            EmptyWorkspaceAction::OpenParked => self.open_parked(),
            // The workspace on screen, not a sidebar highlight.
            EmptyWorkspaceAction::Close => {
                if let Some(workspace) = self
                    .snapshot
                    .as_deref()
                    .and_then(|snapshot| snapshot.focused_workspace_id.clone())
                {
                    self.request_workspace_close(workspace, outcome);
                }
            }
        }
        outcome.repaint = true;
    }
}
