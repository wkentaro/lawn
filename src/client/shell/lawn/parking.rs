use super::parking_store;
use super::*;
use std::collections::BTreeSet;

/// Client view of server-owned parking: which panes are parked, and the
/// presentation policy this client uses to peek at one of them.
#[derive(Clone, Debug, Default)]
pub(in crate::client::shell) struct PaneParking {
    pub(in crate::client::shell) supported: bool,
    /// Pre-server `parking.json`. It is removed once a snapshot shows all
    /// of its live terminals parked; until then each connection retries.
    legacy_path: Option<PathBuf>,
    legacy_terminals: Option<BTreeSet<String>>,
    migration_sent: bool,
    preview: Option<String>,
    pub(super) preview_return: Option<WorkspaceNavigationTarget>,
    focus: Option<String>,
    generation: Option<u64>,
    boot_id: Option<String>,
    revision: u64,
    sequence: u64,
    request: Option<SetPanePresentation>,
    identities: Option<(Option<u64>, PaneIdentities)>,
    pending_identities: Option<(Option<u64>, PaneIdentities)>,
    pane_workspaces: HashMap<String, String>,
    focused_workspace: Option<String>,
    /// Workspaces whose every pane is parked.
    parked_workspaces: HashSet<String>,
    pub(in crate::client::shell) hidden_panes: HashSet<String>,
    /// Parks sent to the server but not yet in a snapshot's `parked_panes`.
    pending_parks: HashSet<String>,
    pub(in crate::client::shell) departed_workspace: Option<WorkspaceDeparture>,
    terminal_panes: HashMap<String, String>,
    dirty: bool,
    failed: bool,
    last_error: Option<String>,
    notice: Option<String>,
    repaint: bool,
}

#[derive(Clone, Debug)]
pub(in crate::client::shell) struct WorkspaceDeparture {
    pub(in crate::client::shell) workspace_id: String,
    pub(in crate::client::shell) focused_workspace: Option<String>,
    pub(in crate::client::shell) origin: Option<Box<ClientShellSnapshot>>,
}

impl PaneParking {
    pub(in crate::client::shell) fn set_path(&mut self, path: Option<PathBuf>) {
        if self.legacy_path != path {
            self.legacy_path = path;
            self.legacy_terminals = None;
            self.migration_sent = false;
        }
    }

    pub(in crate::client::shell) fn set_supported(&mut self, supported: bool) {
        if self.supported != supported {
            self.supported = supported;
            self.boot_id = None;
            self.request = None;
            self.identities = None;
            self.pending_identities = None;
            self.failed = false;
            self.dirty = supported;
            self.repaint = true;
            if !supported {
                self.preview = None;
                self.preview_return = None;
                self.focus = None;
                self.terminal_panes.clear();
                self.hidden_panes.clear();
                self.pending_parks.clear();
                self.parked_workspaces.clear();
            }
        }
    }

    pub(in crate::client::shell) fn ready(&self) -> bool {
        !self.supported
            || self.failed
            || (!self.dirty
                && self.identities.as_ref().is_some_and(|(generation, ids)| {
                    *generation == self.generation
                        && Some(ids.boot_id.as_str()) == self.boot_id.as_deref()
                        && ids.revision == self.revision
                        && ids.sequence == self.sequence
                        && self.request.is_some()
                }))
    }

    pub(in crate::client::shell) fn available(&self) -> bool {
        self.supported && !self.failed && self.ready()
    }

    pub(in crate::client::shell) fn hides_pane(&self, pane: &str) -> bool {
        self.hidden_panes.contains(pane)
    }

    pub(in crate::client::shell) fn shows_workspace(&self, workspace: &str) -> bool {
        !self.parked_workspaces.contains(workspace)
    }

    pub(in crate::client::shell) fn accepts_input(&self, pane: &str) -> bool {
        self.ready()
            && match self.preview_pane() {
                Some(preview) => preview == pane,
                None => !self.hides_pane(pane),
            }
    }

    pub(in crate::client::shell) fn preview_pane(&self) -> Option<&str> {
        self.preview
            .as_ref()
            .and_then(|id| self.terminal_panes.get(id))
            .map(String::as_str)
    }

    pub(in crate::client::shell) fn terminal_for_pane(&self, pane: &str) -> Option<&str> {
        self.identities
            .as_ref()?
            .1
            .pane_terminals
            .get(pane)
            .map(String::as_str)
    }

    fn record_error(&mut self, message: String) {
        if self.last_error.as_ref() != Some(&message) {
            self.notice = Some(message.clone());
        }
        self.last_error = Some(message);
    }

    pub(in crate::client::shell) fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }

    pub(in crate::client::shell) fn take_repaint(&mut self) -> bool {
        std::mem::take(&mut self.repaint)
    }

    /// Panes from the legacy `parking.json` to park on the server, once per
    /// connection.
    pub(in crate::client::shell) fn take_migration(&mut self) -> Vec<String> {
        if self.migration_sent || !self.available() {
            return Vec::new();
        }
        let Some(path) = self.legacy_path.as_deref() else {
            return Vec::new();
        };
        self.migration_sent = true;
        let terminals = match parking_store::read(path) {
            Ok(terminals) if terminals.is_empty() => {
                self.legacy_path = None;
                return Vec::new();
            }
            Ok(terminals) => terminals,
            Err(error) => {
                self.record_error(error);
                return Vec::new();
            }
        };
        let panes = self
            .identities
            .iter()
            .flat_map(|(_, ids)| &ids.pane_terminals)
            .filter(|(pane, terminal)| {
                terminals.contains(*terminal) && !self.hidden_panes.contains(*pane)
            })
            .map(|(pane, _)| pane.clone())
            .collect();
        self.legacy_terminals = Some(terminals);
        panes
    }

    fn finish_migration(&mut self, snapshot: &ClientShellSnapshot) {
        let (Some(terminals), Some((_, ids))) = (&self.legacy_terminals, &self.identities) else {
            return;
        };
        let unparked = ids.pane_terminals.iter().any(|(pane, terminal)| {
            terminals.contains(terminal)
                && !snapshot
                    .parked_panes
                    .iter()
                    .any(|parked| &parked.pane_id == pane)
        });
        if unparked {
            return;
        }
        self.legacy_terminals = None;
        if let Some(path) = self.legacy_path.take() {
            if let Err(error) = parking_store::remove(&path) {
                self.record_error(error);
            }
        }
    }

    pub(in crate::client::shell) fn receive_identities(
        &mut self,
        generation: Option<u64>,
        ids: PaneIdentities,
    ) {
        if self
            .boot_id
            .as_ref()
            .is_some_and(|boot| self.generation != generation || *boot != ids.boot_id)
        {
            return;
        }
        if self
            .pending_identities
            .as_ref()
            .is_some_and(|(old_generation, old)| {
                *old_generation == generation
                    && old.boot_id == ids.boot_id
                    && old.sequence == ids.sequence
                    && old.revision > ids.revision
            })
        {
            return;
        }
        self.pending_identities = Some((generation, ids));
    }

    pub(in crate::client::shell) fn observe_snapshot(
        &mut self,
        generation: Option<u64>,
        snapshot: &ClientShellSnapshot,
    ) {
        if !self.supported {
            return;
        }
        if self.generation != generation || self.boot_id.as_deref() != Some(&snapshot.boot_id) {
            self.generation = generation;
            self.boot_id = Some(snapshot.boot_id.clone());
            self.sequence = 0;
            self.request = None;
            self.identities = None;
            self.terminal_panes.clear();
            self.pane_workspaces.clear();
            self.focused_workspace = None;
            self.departed_workspace = None;
            self.preview_return = None;
            self.pending_parks.clear();
            self.migration_sent = false;
            self.failed = false;
            self.dirty = true;
        }
        self.revision = snapshot.revision;
        let parked: HashSet<&str> = snapshot
            .parked_panes
            .iter()
            .map(|pane| pane.pane_id.as_str())
            .collect();
        // A pending park ends once the server parks or closes its pane.
        self.pending_parks.retain(|pending| {
            !parked.contains(pending.as_str())
                && snapshot.panes.iter().any(|pane| &pane.pane_id == pending)
        });
        self.hidden_panes = parked
            .iter()
            .map(|pane| (*pane).to_owned())
            .chain(self.pending_parks.iter().cloned())
            .collect();
        self.finish_migration(snapshot);
        let pane_workspaces: HashMap<_, _> = snapshot
            .panes
            .iter()
            .map(|pane| (pane.pane_id.clone(), pane.workspace_id.clone()))
            .collect();
        let occupied: HashSet<&str> = snapshot
            .panes
            .iter()
            .filter(|pane| !parked.contains(pane.pane_id.as_str()))
            .map(|pane| pane.workspace_id.as_str())
            .collect();
        self.parked_workspaces = snapshot
            .parked_panes
            .iter()
            .filter(|pane| !occupied.contains(pane.workspace_id.as_str()))
            .map(|pane| pane.workspace_id.clone())
            .collect();
        let removed_workspace = self.focused_workspace.as_ref().filter(|workspace| {
            snapshot.focused_workspace_id.as_ref() != Some(*workspace)
                && !snapshot
                    .workspaces
                    .iter()
                    .any(|candidate| &candidate.workspace_id == *workspace)
        });
        if let Some(workspace) = snapshot
            .focused_workspace_id
            .as_ref()
            .into_iter()
            .chain(removed_workspace)
            .find(|workspace| {
                self.pane_workspaces.iter().any(|(pane, owner)| {
                    owner == *workspace
                        && !self.hides_pane(pane)
                        && !pane_workspaces.contains_key(pane)
                })
            })
        {
            // A park changes visibility; only a vanished active pane triggers departure.
            // The server may already have selected a parked-only neighbor after a close.
            self.departed_workspace = Some(WorkspaceDeparture {
                workspace_id: workspace.clone(),
                focused_workspace: snapshot.focused_workspace_id.clone(),
                origin: None,
            });
        }
        self.focused_workspace = snapshot.focused_workspace_id.clone();
        self.pane_workspaces = pane_workspaces;
        let matches = self
            .pending_identities
            .as_ref()
            .is_some_and(|(received_generation, ids)| {
                *received_generation == generation
                    && ids.boot_id == snapshot.boot_id
                    && ids.revision == snapshot.revision
                    && ids.sequence == self.sequence
                    && self.request.is_some()
            });
        if !matches {
            return;
        }
        let Some((_, ids)) = self.pending_identities.take() else {
            return;
        };
        self.repaint = true;
        self.terminal_panes = ids
            .pane_terminals
            .iter()
            .map(|(pane, terminal)| (terminal.clone(), pane.clone()))
            .collect();
        // A peek ends once its pane is gone or restored by any client.
        let parked = |id: &String| {
            self.terminal_panes
                .get(id)
                .is_some_and(|pane| self.hidden_panes.contains(pane))
        };
        self.preview = self.preview.take().filter(parked);
        self.focus = self
            .focus
            .take()
            .filter(|id| self.terminal_panes.contains_key(id));
        let focused_terminal = snapshot
            .focused_pane_id
            .as_ref()
            .and_then(|pane| ids.pane_terminals.get(pane));
        if let Some(request) = &self.request {
            if self.preview.is_some()
                && self.preview.as_ref() == request.focus_terminal.as_ref()
                && focused_terminal != self.preview.as_ref()
            {
                self.preview = None;
            }
            if self.focus.as_ref() == request.focus_terminal.as_ref() {
                self.focus = None;
            }
            // A settled restore may keep its focus; a stale peek must not.
            let stale = match &self.preview {
                Some(preview) => request.focus_terminal.as_ref() != Some(preview),
                None => request.focus_terminal.as_ref().is_some_and(parked),
            };
            if stale || self.focus.is_some() {
                self.dirty = true;
            }
        }
        if self.preview.is_none() {
            self.preview_return = None;
        }
        self.identities = Some((generation, ids));
    }

    fn wanted_focus(&self) -> Option<String> {
        self.preview
            .as_ref()
            .or(self.focus.as_ref())
            .filter(|id| self.terminal_panes.contains_key(*id))
            .cloned()
    }

    pub(in crate::client::shell) fn poll(&mut self) -> Option<SetPanePresentation> {
        if !self.supported || self.failed || !self.dirty {
            return None;
        }
        let boot_id = self.boot_id.clone()?;
        self.sequence = self.sequence.checked_add(1)?;
        let request = SetPanePresentation {
            boot_id,
            sequence: self.sequence,
            hidden_terminals: Vec::new(),
            read_only_terminals: Vec::new(),
            preview_terminal: None,
            focus_terminal: self.wanted_focus(),
        };
        self.request = Some(request.clone());
        self.dirty = false;
        Some(request)
    }

    pub(in crate::client::shell) fn receive_error(
        &mut self,
        generation: Option<u64>,
        error: PanePresentationError,
    ) {
        if self.generation != generation
            || (error.code != "wrong_boot"
                && self.boot_id.as_deref() != Some(error.boot_id.as_str()))
            || error
                .sequence
                .is_some_and(|sequence| sequence != self.sequence)
        {
            return;
        }
        self.record_error(error.message);
        match error.code.as_str() {
            "pane_not_found" => {
                self.preview = None;
                self.preview_return = None;
                self.focus = None;
                self.dirty = true;
            }
            "wrong_boot" => {
                self.preview_return = None;
                self.boot_id = Some(error.boot_id);
                self.sequence = 0;
                self.identities = None;
                self.request = None;
                self.dirty = true;
            }
            _ => {
                self.failed = true;
            }
        }
    }

    /// Hides the pane until the server confirms it parked.
    pub(in crate::client::shell) fn park(&mut self, pane: &str) {
        self.hidden_panes.insert(pane.to_owned());
        self.pending_parks.insert(pane.to_owned());
        self.preview = None;
        self.preview_return = None;
        self.focus = None;
        self.dirty = true;
    }

    /// The server parked the pane. It stays hidden until the next snapshot,
    /// which already reflects the park or any later unpark.
    pub(in crate::client::shell) fn finish_park(&mut self, pane: &str) {
        self.pending_parks.remove(pane);
    }

    /// The server rejected a park, so the pane stays visible.
    pub(in crate::client::shell) fn cancel_park(&mut self, pane: &str) {
        if self.pending_parks.remove(pane) {
            self.hidden_panes.remove(pane);
            self.dirty = true;
            self.repaint = true;
        }
    }

    pub(in crate::client::shell) fn preview(&mut self, terminal: &str) -> Result<(), String> {
        if !self.available() || !self.terminal_panes.contains_key(terminal) {
            return Err("This pane is no longer available".into());
        }
        self.preview = Some(terminal.into());
        self.focus = None;
        self.dirty = true;
        Ok(())
    }

    pub(in crate::client::shell) fn restore(&mut self, terminal: &str) -> Result<(), String> {
        let pane = self
            .terminal_panes
            .get(terminal)
            .filter(|_| self.available())
            .ok_or("This pane is no longer available")?
            .clone();
        self.hidden_panes.remove(&pane);
        self.preview = None;
        self.preview_return = None;
        self.focus = Some(terminal.into());
        self.dirty = true;
        Ok(())
    }

    #[cfg(test)]
    pub(in crate::client::shell) fn current_request(&self) -> Option<&SetPanePresentation> {
        self.request.as_ref()
    }

    pub(in crate::client::shell) fn leave_preview(&mut self) {
        self.preview_return = None;
        if self.preview.take().is_some() {
            self.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str) -> crate::protocol::ClientShellPane {
        let mut pane = crate::client::shell::tests::snapshot().panes[0].clone();
        pane.pane_id = id.into();
        pane.focused = false;
        pane
    }

    /// Mirrors what the client receives: every pane stays in `panes`, and
    /// `parked_panes` names the parked ones.
    fn acknowledge(
        model: &mut PaneParking,
        snapshot: &mut ClientShellSnapshot,
        sequence: u64,
        parked: &[&str],
        focused: Option<&str>,
    ) {
        snapshot.revision += 1;
        snapshot.focused_pane_id = focused.map(str::to_owned);
        snapshot.parked_panes = snapshot
            .panes
            .iter()
            .filter(|pane| parked.contains(&pane.pane_id.as_str()))
            .cloned()
            .collect();
        model.receive_identities(
            Some(7),
            PaneIdentities {
                boot_id: snapshot.boot_id.clone(),
                revision: snapshot.revision,
                sequence,
                pane_terminals: snapshot
                    .panes
                    .iter()
                    .map(|pane| (pane.pane_id.clone(), format!("terminal-{}", pane.pane_id)))
                    .collect(),
            },
        );
        model.observe_snapshot(Some(7), snapshot);
    }

    fn connected(legacy: Option<PathBuf>) -> (PaneParking, ClientShellSnapshot) {
        let mut model = PaneParking::default();
        model.set_path(legacy);
        let mut snapshot = crate::client::shell::tests::snapshot();
        snapshot.panes.push(pane("pane_2"));
        model.set_supported(true);
        model.observe_snapshot(Some(7), &snapshot);
        let request = model.poll().unwrap();
        acknowledge(
            &mut model,
            &mut snapshot,
            request.sequence,
            &[],
            Some("pane_1"),
        );
        (model, snapshot)
    }

    #[test]
    fn unsupported_server_keeps_normal_input() {
        let mut model = PaneParking::default();
        assert!(model.ready());
        assert!(!model.available());
        assert!(model.accepts_input("pane_1"));
        assert!(model.shows_workspace("ws_1"));
        assert!(model.poll().is_none());
    }

    #[test]
    fn input_waits_for_the_requested_generation_boot_revision_and_sequence() {
        let (mut model, mut snapshot) = connected(None);
        let sequence = model.sequence;
        acknowledge(
            &mut model,
            &mut snapshot,
            sequence,
            &["pane_1"],
            Some("pane_2"),
        );
        assert!(model.available());
        model.preview("terminal-pane_1").unwrap();
        assert!(!model.ready());
        let request = model.poll().unwrap();
        snapshot.revision += 1;
        snapshot.focused_pane_id = Some("pane_1".into());
        let valid = PaneIdentities {
            boot_id: snapshot.boot_id.clone(),
            revision: snapshot.revision,
            sequence: request.sequence,
            pane_terminals: [("pane_1".into(), "terminal-pane_1".into())]
                .into_iter()
                .collect(),
        };
        model.receive_identities(
            Some(7),
            PaneIdentities {
                sequence: request.sequence - 1,
                ..valid.clone()
            },
        );
        model.observe_snapshot(Some(7), &snapshot);
        assert!(!model.ready());
        model.receive_identities(Some(7), valid.clone());
        model.receive_identities(Some(6), valid.clone());
        model.receive_identities(
            Some(7),
            PaneIdentities {
                boot_id: "retired-boot".into(),
                ..valid
            },
        );
        model.observe_snapshot(Some(7), &snapshot);
        assert!(model.ready());
    }

    #[test]
    fn server_parked_panes_hide_their_workspace_only_when_nothing_else_is_visible() {
        let (mut model, mut snapshot) = connected(None);
        model.park("pane_1");
        assert!(model.hides_pane("pane_1"));
        assert!(!model.accepts_input("pane_1"));
        let sequence = model.poll().unwrap().sequence;
        acknowledge(&mut model, &mut snapshot, sequence, &["pane_1"], None);
        assert!(model.hides_pane("pane_1"));
        assert!(!model.accepts_input("pane_1"));
        assert!(model.accepts_input("pane_2"));
        assert!(model.shows_workspace("ws_1"));
        let sequence = model.sequence;
        acknowledge(
            &mut model,
            &mut snapshot,
            sequence,
            &["pane_1", "pane_2"],
            None,
        );
        assert!(!model.shows_workspace("ws_1"));
    }

    #[test]
    fn a_peek_focuses_the_parked_terminal_and_ends_when_any_client_restores_it() {
        let (mut model, mut snapshot) = connected(None);
        let sequence = model.sequence;
        acknowledge(
            &mut model,
            &mut snapshot,
            sequence,
            &["pane_1"],
            Some("pane_2"),
        );
        model.preview("terminal-pane_1").unwrap();
        let peek = model.poll().unwrap();
        assert_eq!(peek.focus_terminal.as_deref(), Some("terminal-pane_1"));
        assert!(peek.hidden_terminals.is_empty());
        acknowledge(
            &mut model,
            &mut snapshot,
            peek.sequence,
            &["pane_1"],
            Some("pane_1"),
        );
        assert_eq!(model.preview_pane(), Some("pane_1"));
        assert!(model.accepts_input("pane_1"));
        assert!(!model.accepts_input("pane_2"));
        assert!(model.poll().is_none());

        acknowledge(
            &mut model,
            &mut snapshot,
            peek.sequence,
            &[],
            Some("pane_1"),
        );
        assert_eq!(model.preview_pane(), None);
        assert!(model.poll().is_none(), "the restored pane keeps its focus");
        assert!(model.available());
        assert!(model.accepts_input("pane_1"));
        assert!(model.accepts_input("pane_2"));
    }

    #[test]
    fn restoring_focuses_the_terminal_once_the_server_unparks_it() {
        let (mut model, mut snapshot) = connected(None);
        let sequence = model.sequence;
        acknowledge(
            &mut model,
            &mut snapshot,
            sequence,
            &["pane_1"],
            Some("pane_2"),
        );
        model.restore("terminal-pane_1").unwrap();
        assert!(!model.hides_pane("pane_1"));
        let focus = model.poll().unwrap();
        assert_eq!(focus.focus_terminal.as_deref(), Some("terminal-pane_1"));
        acknowledge(
            &mut model,
            &mut snapshot,
            focus.sequence,
            &[],
            Some("pane_1"),
        );
        assert!(model.accepts_input("pane_1"));
        assert!(model.poll().is_none(), "focus is requested once");
    }

    #[test]
    fn legacy_parking_file_is_kept_until_the_server_holds_its_parks() {
        let dir = std::env::temp_dir().join(format!(
            "lawn-parking-model-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("parking.json");
        std::fs::write(
            &path,
            r#"{"version":1,"terminals":["terminal-pane_2","closed-terminal"]}"#,
        )
        .unwrap();
        let (mut model, mut snapshot) = connected(Some(path.clone()));
        assert_eq!(model.take_migration(), ["pane_2"]);
        assert!(model.take_migration().is_empty());

        // The park never landed: a new connection retries it.
        assert!(path.exists());
        snapshot.boot_id = "restarted".into();
        model.observe_snapshot(Some(7), &snapshot);
        let request = model.poll().unwrap();
        acknowledge(
            &mut model,
            &mut snapshot,
            request.sequence,
            &[],
            Some("pane_1"),
        );
        assert_eq!(model.take_migration(), ["pane_2"]);

        // The file goes once the server holds the park.
        acknowledge(
            &mut model,
            &mut snapshot,
            request.sequence,
            &["pane_2"],
            Some("pane_1"),
        );
        assert!(!path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
