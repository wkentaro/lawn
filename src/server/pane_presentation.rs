use std::collections::{HashMap, HashSet};

use crate::app::AppState;
use crate::layout::{PaneId, PresentedLayout};
use crate::protocol::endpoint::pane_presentation::SetPanePresentation;
use crate::ui::TabSurfaceTarget;
use crate::workspace::{Tab, UnparkPlan};
use ratatui::layout::Rect;

#[derive(Clone, Copy)]
pub(crate) struct PaneLocation {
    pub(crate) target: TabSurfaceTarget,
    pub(crate) pane_id: PaneId,
    pub(crate) parked: bool,
}

pub(crate) fn terminal_locations(state: &AppState) -> HashMap<&str, PaneLocation> {
    state
        .workspaces
        .iter()
        .enumerate()
        .flat_map(|(workspace_index, workspace)| {
            workspace
                .tabs
                .iter()
                .enumerate()
                .flat_map(move |(tab_index, tab)| {
                    tab.panes.iter().map(move |(&pane_id, pane)| {
                        (
                            pane.attached_terminal_id.as_str(),
                            PaneLocation {
                                target: TabSurfaceTarget {
                                    workspace_index,
                                    tab_index,
                                },
                                pane_id,
                                parked: tab.parked_from.is_some(),
                            },
                        )
                    })
                })
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RestoreOrigin {
    pub(crate) target: TabSurfaceTarget,
    pub(crate) parked: PaneId,
}

#[derive(Clone)]
pub(crate) struct ClientPanePresentation {
    pub(crate) request: SetPanePresentation,
    pub(crate) restore_preview: bool,
    hidden: HashSet<PaneId>,
    read_only: HashSet<PaneId>,
    preview: Option<PaneLocation>,
    focus: Option<PaneId>,
    peek: Option<PaneLocation>,
    pub(crate) preview_active: bool,
    focus_active: bool,
}

impl ClientPanePresentation {
    pub(crate) fn new(
        request: SetPanePresentation,
        restore_preview: bool,
        locations: &HashMap<&str, PaneLocation>,
    ) -> Self {
        let mut result = Self {
            preview_active: request.preview_terminal.is_some(),
            focus_active: request.focus_terminal.is_some(),
            request,
            restore_preview,
            hidden: HashSet::new(),
            read_only: HashSet::new(),
            preview: None,
            focus: None,
            peek: None,
        };
        result.refresh(locations);
        result
    }

    pub(crate) fn refresh(&mut self, locations: &HashMap<&str, PaneLocation>) {
        self.hidden.clear();
        self.hidden.extend(
            self.request
                .hidden_terminals
                .iter()
                .filter_map(|id| locations.get(id.as_str()).map(|location| location.pane_id)),
        );
        self.read_only.clear();
        self.read_only.extend(
            self.request
                .read_only_terminals
                .iter()
                .chain(self.request.preview_terminal.iter())
                .filter_map(|id| locations.get(id.as_str()).map(|location| location.pane_id)),
        );
        self.preview = self
            .request
            .preview_terminal
            .as_deref()
            .and_then(|id| locations.get(id))
            .copied();
        if let Some(id) = self.request.focus_terminal.as_deref() {
            self.focus = locations.get(id).map(|location| location.pane_id);
        }
        self.peek = self
            .requested_location(locations)
            .filter(|location| location.parked)
            .copied();
    }

    fn requested_location<'a>(
        &self,
        locations: &'a HashMap<&str, PaneLocation>,
    ) -> Option<&'a PaneLocation> {
        self.request
            .preview_terminal
            .as_ref()
            .or(self.request.focus_terminal.as_ref())
            .and_then(|id| locations.get(id.as_str()))
    }

    /// A parked pane this client is checking in on; it overrides the shared
    /// location without moving it, so ending the peek returns to the same tab.
    pub(crate) fn peek_target(&self, state: &AppState) -> Option<TabSurfaceTarget> {
        let peek = self.active_peek()?;
        Some(
            self.restore_origin(state)
                .map_or(peek.target, |origin| origin.target),
        )
    }

    pub(crate) fn active_peek(&self) -> Option<PaneLocation> {
        self.peek
            .filter(|_| self.preview_active || self.focus_active)
    }

    pub(crate) fn restore_origin(&self, state: &AppState) -> Option<RestoreOrigin> {
        if !self.restore_preview {
            return None;
        }
        let peek = self.active_peek()?;
        let workspace_index = peek.target.workspace_index;
        let workspace = state.workspaces.get(workspace_index)?;
        let UnparkPlan::Split(split) = workspace.unpark_plan(peek.pane_id)? else {
            return None;
        };
        let tab_index = workspace
            .visible_tabs()
            .iter()
            .position(|tab| tab.number == split.tab_number)?;
        Some(RestoreOrigin {
            target: TabSurfaceTarget {
                workspace_index,
                tab_index,
            },
            parked: peek.pane_id,
        })
    }

    pub(crate) fn restore_layout(
        &self,
        state: &AppState,
        origin: RestoreOrigin,
        area: Rect,
    ) -> Option<PresentedLayout> {
        let (_, layout) = state
            .workspaces
            .get(origin.target.workspace_index)?
            .unpark_layout(origin.parked)?;
        let mut presented = layout.presented(area, Some(origin.parked), |id| self.presents(id));
        presented.unzoomed = true;
        Some(presented)
    }

    pub(crate) fn focused_in(&self, state: &AppState, tab: &Tab) -> Option<PaneId> {
        if let Some(origin) = self.restore_origin(state) {
            if state
                .workspaces
                .get(origin.target.workspace_index)
                .and_then(|workspace| workspace.tabs.get(origin.target.tab_index))
                .is_some_and(|origin_tab| std::ptr::eq(origin_tab, tab))
            {
                return Some(origin.parked);
            }
        }
        self.focused(tab)
    }

    pub(crate) fn presents(&self, pane_id: PaneId) -> bool {
        if self.restore_preview
            && self
                .active_peek()
                .is_some_and(|peek| peek.pane_id == pane_id)
        {
            return true;
        }
        if self.preview_active && !self.restore_preview {
            self.preview
                .is_some_and(|preview| preview.pane_id == pane_id)
        } else {
            !self.hidden.contains(&pane_id)
        }
    }

    pub(crate) fn accepts_input(&self, pane_id: PaneId) -> bool {
        self.presents(pane_id) && !self.read_only.contains(&pane_id)
    }

    pub(crate) fn focused(&self, tab: &Tab) -> Option<PaneId> {
        if self.focus_active {
            if let Some(pane_id) = self
                .focus
                .filter(|id| tab.panes.contains_key(id) && self.presents(*id))
            {
                return Some(pane_id);
            }
        }
        let native = tab.layout.focused();
        if self.presents(native) {
            Some(native)
        } else {
            tab.layout.first_matching_pane(|id| self.presents(id))
        }
    }

    pub(crate) fn layout(&self, tab: &Tab, area: Rect) -> Option<PresentedLayout> {
        if self.hidden.is_empty() && !self.preview_active && !self.focus_active {
            return None;
        }
        Some(
            tab.layout
                .presented(area, self.focused(tab), |id| self.presents(id)),
        )
    }

    pub(crate) fn deactivate_focus(&mut self) -> bool {
        let changed = self.focus_active || self.preview_active;
        self.focus_active = false;
        self.preview_active = false;
        changed
    }

    pub(crate) fn preserve_focus(&mut self, pane_id: PaneId) {
        self.focus = Some(pane_id);
        self.focus_active = true;
    }

    pub(crate) fn requested_target(
        &self,
        locations: &HashMap<&str, PaneLocation>,
    ) -> Option<TabSurfaceTarget> {
        self.requested_location(locations)
            .filter(|location| !location.parked)
            .map(|location| location.target)
    }

    pub(crate) fn reflow_target(
        &self,
        state: &AppState,
        target: TabSurfaceTarget,
    ) -> TabSurfaceTarget {
        if self.preview_active {
            if let Some(preview) = self.preview.filter(|preview| !preview.parked) {
                return preview.target;
            }
        }
        let Some(workspace) = state.workspaces.get(target.workspace_index) else {
            return target;
        };
        let visible = |index: usize| {
            workspace.visible_tabs().get(index).is_some_and(|tab| {
                tab.layout
                    .first_matching_pane(|id| self.presents(id))
                    .is_some()
            })
        };
        if visible(target.tab_index) {
            return target;
        }
        (target.tab_index.saturating_add(1)..workspace.visible_tab_count())
            .chain((0..target.tab_index).rev())
            .find(|&index| visible(index))
            .map_or(target, |tab_index| TabSurfaceTarget {
                tab_index,
                ..target
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> SetPanePresentation {
        SetPanePresentation {
            boot_id: "boot".into(),
            sequence: 1,
            hidden_terminals: vec!["one".into()],
            read_only_terminals: Vec::new(),
            preview_terminal: None,
            focus_terminal: None,
        }
    }

    #[test]
    fn hidden_identity_follows_placement_changes_and_preview_remains_read_only() {
        let first = PaneId::alloc();
        let second = PaneId::alloc();
        let target = TabSurfaceTarget {
            workspace_index: 0,
            tab_index: 0,
        };
        let mut locations = HashMap::from([(
            "one",
            PaneLocation {
                target,
                pane_id: first,
                parked: false,
            },
        )]);
        let mut policy = ClientPanePresentation::new(request(), false, &locations);
        assert!(!policy.presents(first));
        locations.insert(
            "one",
            PaneLocation {
                target,
                pane_id: second,
                parked: false,
            },
        );
        policy.refresh(&locations);
        assert!(policy.presents(first));
        assert!(!policy.presents(second));
        let mut preview = request();
        preview.preview_terminal = Some("one".into());
        let mut policy = ClientPanePresentation::new(preview, false, &locations);
        assert!(policy.presents(second));
        assert!(!policy.accepts_input(second));
        assert!(!policy.presents(first));
        policy.deactivate_focus();
        assert!(!policy.accepts_input(second));
    }
}
