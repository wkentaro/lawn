use super::*;
use crate::protocol::endpoint::pane_presentation::{self as contract, PanePresentationError};
use crate::server::pane_presentation::{terminal_locations, ClientPanePresentation};

pub(super) struct ClientPaneRequest {
    pub(super) client_id: u64,
    pub(super) deactivate_focus: bool,
    pub(super) layout: Option<crate::layout::PresentedLayout>,
    pub(super) location_before: Option<crate::server::clients::ClientShellLocation>,
}

impl HeadlessServer {
    pub(super) fn deactivate_client_pane_focus(&mut self, client_id: u64) {
        if let Some(client) = self.clients.get_mut(&client_id) {
            if client
                .pane_presentation
                .as_mut()
                .is_some_and(|presentation| presentation.deactivate_focus())
            {
                client.shell_snapshot = None;
            }
        }
    }

    pub(super) fn resolve_client_pane_target(
        &self,
        client_id: u64,
        method: &mut api::schema::Method,
    ) -> bool {
        if self
            .clients
            .get(&client_id)
            .is_none_or(|client| client.pane_presentation.is_none())
        {
            return true;
        }
        let focus = self.shell_focus_target(client_id);
        let workspace = self
            .shell_target_for_client(client_id)
            .map(|target| self.app.public_workspace_id(target.workspace_index));
        let slot = match method {
            api::schema::Method::PaneSplit(params)
                if params
                    .workspace_id
                    .as_ref()
                    .is_none_or(|id| Some(id) == workspace.as_ref()) =>
            {
                &mut params.target_pane_id
            }
            api::schema::Method::PaneSwap(params) if params.direction.is_some() => {
                &mut params.pane_id
            }
            api::schema::Method::PaneZoom(params) => &mut params.pane_id,
            api::schema::Method::PaneFocusDirection(params) => &mut params.pane_id,
            api::schema::Method::PaneResize(params) => &mut params.pane_id,
            api::schema::Method::LayoutSetSplitRatio(params) if params.tab_id.is_none() => {
                &mut params.pane_id
            }
            _ => return true,
        };
        if slot.is_none() {
            *slot = focus.and_then(|focus| {
                self.app
                    .public_pane_id(focus.workspace_index, focus.pane_id)
            });
        }
        slot.is_some()
    }

    fn pane_presentation_error(
        &mut self,
        client_id: u64,
        sequence: Option<u64>,
        code: &str,
        message: &str,
    ) -> bool {
        if let Ok(message) = contract::error_message(&PanePresentationError {
            boot_id: self.client_shell_boot_id.clone(),
            sequence,
            code: code.into(),
            message: message.into(),
        }) {
            self.send_to_client(client_id, message);
        }
        false
    }

    pub(super) fn set_client_pane_presentation(&mut self, client_id: u64, data: &str) -> bool {
        let request = match contract::decode_set(data) {
            Ok(request) => request,
            Err(message) => {
                return self.pane_presentation_error(client_id, None, "invalid_request", message)
            }
        };
        let sequence = Some(request.sequence);
        if self.handoff_in_progress
            || !self
                .clients
                .get(&client_id)
                .is_some_and(ClientConnection::is_shell_client)
        {
            return self.pane_presentation_error(
                client_id,
                sequence,
                "unavailable",
                "pane presentation requires an available shell connection",
            );
        }
        if request.boot_id != self.client_shell_boot_id {
            return self.pane_presentation_error(
                client_id,
                sequence,
                "wrong_boot",
                "pane presentation belongs to a different server boot",
            );
        }
        let previous = self
            .clients
            .get(&client_id)
            .and_then(|client| client.pane_presentation.as_ref());
        if previous.is_some_and(|previous| {
            request.sequence < previous.request.sequence
                || (request.sequence == previous.request.sequence && request != previous.request)
        }) {
            return self.pane_presentation_error(
                client_id,
                sequence,
                "stale_sequence",
                "pane presentation sequence is stale",
            );
        }
        if previous.is_some_and(|previous| request == previous.request) {
            let client = self.clients.get_mut(&client_id).expect("validated shell");
            client.shell_snapshot = None;
            client.request_recompute();
            return true;
        }
        let locations = terminal_locations(&self.app.state);
        if request
            .preview_terminal
            .iter()
            .chain(request.focus_terminal.iter())
            .any(|id| !locations.contains_key(id.as_str()))
        {
            return self.pane_presentation_error(
                client_id,
                sequence,
                "pane_not_found",
                "the selected terminal no longer has a pane",
            );
        }
        let presentation = ClientPanePresentation::new(request, &locations);
        let target = presentation
            .requested_target(&locations)
            .or_else(|| self.shell_target_for_client(client_id))
            .map(|target| presentation.reflow_target(&self.app.state, target));
        let location = target.and_then(|target| {
            self.app
                .public_tab_id(target.workspace_index, target.tab_index)
                .map(|tab_id| (self.app.public_workspace_id(target.workspace_index), tab_id))
        });
        let focus_before = self.shell_focus_targets();
        let focused_before = self.focused_shell_panes();
        let app = &self.app;
        let held = self
            .clients
            .get_mut(&client_id)
            .expect("validated shell")
            .drain_shell_inputs_matching(|target| match target {
                ClientShellInputTarget::Pane(pane_id) => app
                    .parse_pane_id(pane_id)
                    .is_none_or(|(_, pane_id)| !presentation.accepts_input(pane_id)),
                ClientShellInputTarget::Popup(_) => presentation.preview_active,
            });
        self.release_client_shell_inputs(client_id, held);
        let client = self.clients.get_mut(&client_id).expect("validated shell");
        client.pane_presentation = Some(presentation);
        if let Some((workspace_id, tab_id)) = location {
            client
                .shell_location
                .get_or_insert_with(Default::default)
                .focus_tab(workspace_id, tab_id);
        }
        client.shell_snapshot = None;
        client.shell_pane_identities = None;
        client.request_recompute();
        self.finish_shell_location_reconciliation(focus_before, &focused_before);
        if !self.claim_shell_tab_geometry(client_id, false) {
            self.resize_shell_tab_if_controller(client_id, false);
        }
        true
    }

    pub(super) fn pane_layout_for_client(
        &self,
        client_id: u64,
        target: Option<crate::ui::TabSurfaceTarget>,
        area: Rect,
    ) -> Option<crate::layout::PresentedLayout> {
        let presentation = self.clients.get(&client_id)?.pane_presentation.as_ref()?;
        let target = target?;
        let tab = self
            .app
            .state
            .workspaces
            .get(target.workspace_index)?
            .tabs
            .get(target.tab_index)?;
        presentation.layout(tab, area)
    }

    pub(super) fn focused_pane_for_client(
        &self,
        client_id: u64,
        tab: &crate::workspace::Tab,
    ) -> Option<crate::layout::PaneId> {
        match self.clients.get(&client_id)?.pane_presentation.as_ref() {
            Some(presentation) => presentation.focused(tab),
            None => Some(tab.layout.focused()),
        }
    }

    pub(super) fn shell_client_shows_popup(&self, client_id: u64) -> bool {
        self.clients.get(&client_id).is_some_and(|client| {
            client
                .pane_presentation
                .as_ref()
                .is_none_or(|presentation| !presentation.preview_active)
        }) && self.popup_owner_tab_id == self.shell_tab_id_for_client(client_id)
    }

    pub(super) fn shell_client_accepts_pane_input(
        &self,
        client_id: u64,
        workspace_index: usize,
        pane_id: crate::layout::PaneId,
    ) -> bool {
        self.shell_client_views_pane(client_id, workspace_index, pane_id)
            && self.clients.get(&client_id).is_some_and(|client| {
                client
                    .pane_presentation
                    .as_ref()
                    .is_none_or(|presentation| presentation.accepts_input(pane_id))
            })
    }
}
