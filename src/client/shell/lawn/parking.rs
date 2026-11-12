use super::parking_store::ParkingStore;
use super::*;
use std::{collections::BTreeSet, time::Duration};

#[derive(Clone, Debug, Default)]
pub(in crate::client::shell) struct PaneParking {
    pub(in crate::client::shell) supported: bool,
    store: ParkingStore,
    protected: BTreeSet<String>,
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
    prune_candidates: BTreeSet<String>,
    pane_tabs: HashMap<String, String>,
    pane_workspaces: HashMap<String, String>,
    focused_workspace: Option<String>,
    all_tabs: HashSet<String>,
    all_workspaces: HashSet<String>,
    pub(super) hidden_panes: HashSet<String>,
    visible_tabs: HashSet<String>,
    visible_workspaces: HashSet<String>,
    pub(in crate::client::shell) departed_workspace: Option<WorkspaceDeparture>,
    terminal_panes: HashMap<String, String>,
    dirty: bool,
    failed: bool,
    last_poll: Option<Instant>,
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
    pub(in crate::client::shell) fn new(path: Option<PathBuf>) -> Self {
        Self {
            store: ParkingStore::new(path),
            ..Self::default()
        }
    }

    pub(in crate::client::shell) fn set_path(&mut self, path: Option<PathBuf>) {
        if self.store.path != path {
            *self = Self::new(path);
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
                self.protected.clear();
                self.hidden_panes.clear();
                self.visible_tabs.clear();
                self.visible_workspaces.clear();
                self.terminal_panes.clear();
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
        self.supported && self.hidden_panes.contains(pane)
    }

    pub(in crate::client::shell) fn shows_tab(&self, tab: &str) -> bool {
        !self.supported || self.visible_tabs.contains(tab)
    }

    pub(in crate::client::shell) fn shows_workspace(&self, workspace: &str) -> bool {
        !self.supported
            || !self.all_workspaces.contains(workspace)
            || self.visible_workspaces.contains(workspace)
    }

    pub(in crate::client::shell) fn accepts_input(&self, pane: &str) -> bool {
        self.ready() && self.preview.is_none() && !self.hides_pane(pane)
    }

    pub(in crate::client::shell) fn preview_pane(&self) -> Option<&str> {
        self.preview
            .as_ref()
            .and_then(|id| self.terminal_panes.get(id))
            .map(String::as_str)
    }

    pub(in crate::client::shell) fn parked_terminals(&self) -> impl Iterator<Item = &String> {
        self.store.terminals.iter().chain(
            self.protected
                .iter()
                .filter(|id| !self.store.terminals.contains(*id)),
        )
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
            self.hidden_panes.clear();
            self.visible_tabs.clear();
            self.visible_workspaces.clear();
            self.pane_workspaces.clear();
            self.focused_workspace = None;
            self.departed_workspace = None;
            self.preview_return = None;
            self.last_poll = None;
            self.prune_candidates.clear();
            self.failed = false;
            self.dirty = true;
        }
        self.revision = snapshot.revision;
        let pane_workspaces: HashMap<_, _> = snapshot
            .panes
            .iter()
            .map(|pane| (pane.pane_id.clone(), pane.workspace_id.clone()))
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
        self.all_workspaces = snapshot
            .workspaces
            .iter()
            .map(|workspace| workspace.workspace_id.clone())
            .collect();
        self.pane_tabs = snapshot
            .panes
            .iter()
            .map(|pane| (pane.pane_id.clone(), pane.tab_id.clone()))
            .collect();
        self.all_tabs = snapshot.tabs.iter().map(|tab| tab.tab_id.clone()).collect();
        self.recompute_visibility();
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
        let (_, ids) = self.pending_identities.take().expect("matched identities");
        self.repaint = true;
        let live: BTreeSet<String> = ids.pane_terminals.values().cloned().collect();
        let closed: BTreeSet<String> = self
            .identities
            .iter()
            .flat_map(|(_, previous)| previous.pane_terminals.values())
            .chain(self.prune_candidates.iter())
            .filter(|id| self.store.terminals.contains(*id) && !live.contains(*id))
            .cloned()
            .collect();
        self.prune_candidates.clear();
        if !closed.is_empty() {
            if let Err(error) = self.store.prune(&closed, &live) {
                self.record_error(error);
            }
        }
        self.protected.retain(|id| live.contains(id));
        self.preview = self.preview.take().filter(|id| live.contains(id));
        self.focus = self.focus.take().filter(|id| live.contains(id));
        let focused_terminal = snapshot
            .focused_pane_id
            .as_ref()
            .and_then(|pane| ids.pane_terminals.get(pane));
        if let Some(request) = &self.request {
            if request.preview_terminal.is_some()
                && self.preview.as_ref() == request.preview_terminal.as_ref()
                && focused_terminal != self.preview.as_ref()
            {
                self.preview = None;
            }
            if self.focus.as_ref() == request.focus_terminal.as_ref() {
                self.focus = None;
            }
            let added = self
                .store
                .terminals
                .iter()
                .chain(&self.protected)
                .any(|id| request.hidden_terminals.binary_search(id).is_err());
            let restored = live.iter().any(|id| {
                request.hidden_terminals.binary_search(id).is_ok()
                    && !self.store.terminals.contains(id)
                    && !self.protected.contains(id)
            });
            if added || restored || request.preview_terminal != self.preview || self.focus.is_some()
            {
                self.dirty = true;
            }
        }
        self.identities = Some((generation, ids));
        if self.preview.is_none() {
            self.preview_return = None;
        }
        self.recompute_visibility();
    }

    fn recompute_visibility(&mut self) {
        self.hidden_panes.clear();
        self.terminal_panes.clear();
        if let Some((_, ids)) = &self.identities {
            for (pane, terminal) in &ids.pane_terminals {
                self.terminal_panes.insert(terminal.clone(), pane.clone());
                if self.store.terminals.contains(terminal) || self.protected.contains(terminal) {
                    self.hidden_panes.insert(pane.clone());
                }
            }
        }
        let occupied: HashSet<_> = self.pane_tabs.values().cloned().collect();
        self.visible_tabs = self.all_tabs.difference(&occupied).cloned().collect();
        self.visible_tabs.extend(
            self.pane_tabs
                .iter()
                .filter(|(pane, _)| !self.hidden_panes.contains(*pane))
                .map(|(_, tab)| tab.clone()),
        );
        let occupied: HashSet<_> = self.pane_workspaces.values().cloned().collect();
        self.visible_workspaces = self.all_workspaces.difference(&occupied).cloned().collect();
        self.visible_workspaces.extend(
            self.pane_workspaces
                .iter()
                .filter(|(pane, _)| !self.hidden_panes.contains(*pane))
                .map(|(_, workspace)| workspace.clone()),
        );
    }

    pub(in crate::client::shell) fn poll(&mut self, now: Instant) -> Option<SetPanePresentation> {
        if !self.supported || self.failed {
            return None;
        }
        if self
            .last_poll
            .is_none_or(|last| now.saturating_duration_since(last) >= Duration::from_millis(500))
        {
            self.last_poll = Some(now);
            match self.store.reload() {
                Ok(changed) => {
                    self.dirty |= changed;
                    self.last_error = None;
                    if changed {
                        self.repaint = true;
                        self.recompute_visibility();
                    }
                }
                Err(error) => self.record_error(error),
            }
        }
        if !self.dirty {
            return None;
        }
        let boot_id = self.boot_id.clone()?;
        self.sequence = self.sequence.checked_add(1)?;
        let hidden: BTreeSet<_> = self
            .store
            .terminals
            .union(&self.protected)
            .cloned()
            .collect();
        let known = |id: &&String| self.terminal_panes.contains_key(*id);
        let request = SetPanePresentation {
            boot_id,
            sequence: self.sequence,
            hidden_terminals: hidden.into_iter().collect(),
            read_only_terminals: self.protected.iter().cloned().collect(),
            preview_terminal: self.preview.as_ref().filter(known).cloned(),
            focus_terminal: self.focus.as_ref().filter(known).cloned(),
        };
        self.prune_candidates = self.store.terminals.clone();
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

    pub(in crate::client::shell) fn park(&mut self, pane: &str) -> Result<(), String> {
        if !self.available() {
            return Err("Parking is unavailable on this server".into());
        }
        let terminal = self
            .terminal_for_pane(pane)
            .ok_or("This pane is no longer available")?
            .to_owned();
        self.store.park(&terminal)?;
        self.preview = None;
        self.preview_return = None;
        self.focus = None;
        self.dirty = true;
        self.recompute_visibility();
        Ok(())
    }

    #[cfg(test)]
    pub(in crate::client::shell) fn preview(&mut self, terminal: &str) -> Result<(), String> {
        if !self.available() || !self.terminal_panes.contains_key(terminal) {
            return Err("This pane is no longer available".into());
        }
        self.protected.insert(terminal.into());
        self.preview = Some(terminal.into());
        self.focus = None;
        self.dirty = true;
        self.recompute_visibility();
        Ok(())
    }

    pub(in crate::client::shell) fn restore(&mut self, terminal: &str) -> Result<(), String> {
        if !self.available() || !self.terminal_panes.contains_key(terminal) {
            return Err("This pane is no longer available".into());
        }
        self.store.restore(terminal)?;
        self.protected.remove(terminal);
        self.preview = None;
        self.preview_return = None;
        self.focus = Some(terminal.into());
        self.dirty = true;
        self.recompute_visibility();
        Ok(())
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

    fn acknowledgement(
        model: &mut PaneParking,
        snapshot: &mut ClientShellSnapshot,
        sequence: u64,
        focused: Option<&str>,
    ) {
        snapshot.revision += 1;
        snapshot.focused_pane_id = focused.map(str::to_owned);
        model.receive_identities(
            Some(7),
            PaneIdentities {
                boot_id: snapshot.boot_id.clone(),
                revision: snapshot.revision,
                sequence,
                pane_terminals: [("pane_1".into(), "terminal-one".into())]
                    .into_iter()
                    .collect(),
            },
        );
        model.observe_snapshot(Some(7), snapshot);
    }

    fn connected(path: Option<PathBuf>) -> (PaneParking, ClientShellSnapshot, Instant) {
        let mut model = PaneParking::new(path);
        let mut snapshot = crate::client::shell::tests::snapshot();
        let now = Instant::now();
        model.set_supported(true);
        model.observe_snapshot(Some(7), &snapshot);
        let request = model.poll(now).unwrap();
        acknowledgement(&mut model, &mut snapshot, request.sequence, Some("pane_1"));
        (model, snapshot, now)
    }

    fn path() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lawn-parking-model-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("parking.json")
    }

    #[test]
    fn unsupported_server_keeps_normal_input_and_rejects_only_parking() {
        let mut model = PaneParking::default();
        assert!(model.ready());
        assert!(model.accepts_input("pane_1"));
        assert!(model.shows_tab("tab_1"));
        assert!(model.park("pane_1").is_err());
        assert!(model.poll(Instant::now()).is_none());
    }

    #[test]
    fn visibility_and_input_wait_for_the_requested_generation_boot_revision_and_sequence() {
        let (mut model, mut snapshot, now) = connected(None);
        assert!(model.available());
        model.park("pane_1").unwrap();
        assert!(!model.ready());
        assert!(!model.accepts_input("pane_1"));
        assert!(!model.shows_tab("tab_1"));
        let request = model.poll(now).unwrap();
        assert_eq!(request.hidden_terminals, ["terminal-one"]);
        snapshot.revision += 1;
        let valid = PaneIdentities {
            boot_id: snapshot.boot_id.clone(),
            revision: snapshot.revision,
            sequence: request.sequence,
            pane_terminals: [("pane_1".into(), "terminal-one".into())]
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
        assert!(!model.accepts_input("pane_1"));
        assert!(!model.shows_tab("tab_1"));
    }

    #[test]
    fn another_window_restoring_the_record_does_not_make_a_preview_writable() {
        let path = path();
        let (mut model, mut snapshot, now) = connected(Some(path.clone()));
        model.park("pane_1").unwrap();
        let request = model.poll(now).unwrap();
        acknowledgement(&mut model, &mut snapshot, request.sequence, None);
        model.preview("terminal-one").unwrap();
        let request = model.poll(now).unwrap();
        assert_eq!(request.preview_terminal.as_deref(), Some("terminal-one"));
        acknowledgement(&mut model, &mut snapshot, request.sequence, Some("pane_1"));
        ParkingStore::new(Some(path.clone()))
            .restore("terminal-one")
            .unwrap();
        let request = model.poll(now + Duration::from_millis(500)).unwrap();
        assert_eq!(request.read_only_terminals, ["terminal-one"]);
        acknowledgement(&mut model, &mut snapshot, request.sequence, Some("pane_1"));
        assert_eq!(model.preview_pane(), Some("pane_1"));
        assert_eq!(model.parked_terminals().count(), 1);
        assert!(!model.accepts_input("pane_1"));
        assert!(!model.accepts_input("another-visible-pane"));
        model.restore("terminal-one").unwrap();
        let request = model.poll(now + Duration::from_millis(500)).unwrap();
        assert!(request.hidden_terminals.is_empty());
        assert!(request.read_only_terminals.is_empty());
        assert_eq!(request.focus_terminal.as_deref(), Some("terminal-one"));
        acknowledgement(&mut model, &mut snapshot, request.sequence, Some("pane_1"));
        assert!(model.accepts_input("pane_1"));
        assert!(model.shows_tab("tab_1"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn pruning_waits_for_a_fresh_policy_ack_and_preserves_newer_file_additions() {
        let path = path();
        ParkingStore::new(Some(path.clone()))
            .park("closed-terminal")
            .unwrap();
        let mut model = PaneParking::new(Some(path.clone()));
        let mut snapshot = crate::client::shell::tests::snapshot();
        let now = Instant::now();
        model.set_supported(true);
        model.observe_snapshot(Some(7), &snapshot);
        let request = model.poll(now).unwrap();
        assert!(model.store.terminals.contains("closed-terminal"));
        ParkingStore::new(Some(path.clone()))
            .park("later-addition")
            .unwrap();
        acknowledgement(&mut model, &mut snapshot, request.sequence, Some("pane_1"));
        assert!(!model.store.terminals.contains("closed-terminal"));
        assert!(model.store.terminals.contains("later-addition"));
        let refreshed = model
            .poll(now)
            .expect("newer additions require a newer authoritative map");
        assert_eq!(refreshed.hidden_terminals, ["later-addition"]);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn closing_a_previewed_terminal_sends_a_clearing_policy_before_input_resumes() {
        let (mut model, mut snapshot, now) = connected(None);
        let mut second = snapshot.panes[0].clone();
        second.pane_id = "pane_2".into();
        second.focused = false;
        snapshot.panes.push(second);
        let reply = |model: &mut PaneParking,
                     snapshot: &mut ClientShellSnapshot,
                     sequence,
                     focused: Option<&str>| {
            snapshot.revision += 1;
            snapshot.focused_pane_id = focused.map(str::to_owned);
            model.receive_identities(
                Some(7),
                PaneIdentities {
                    boot_id: snapshot.boot_id.clone(),
                    revision: snapshot.revision,
                    sequence,
                    pane_terminals: snapshot
                        .panes
                        .iter()
                        .map(|pane| {
                            (
                                pane.pane_id.clone(),
                                if pane.pane_id == "pane_1" {
                                    "terminal-one".into()
                                } else {
                                    "terminal-two".into()
                                },
                            )
                        })
                        .collect(),
                },
            );
            model.observe_snapshot(Some(7), snapshot);
        };
        model.park("pane_1").unwrap();
        let parked = model.poll(now).unwrap();
        reply(&mut model, &mut snapshot, parked.sequence, Some("pane_2"));
        model.preview("terminal-one").unwrap();
        let preview = model.poll(now).unwrap();
        reply(&mut model, &mut snapshot, preview.sequence, Some("pane_1"));
        assert!(!model.accepts_input("pane_2"));

        snapshot.panes.retain(|pane| pane.pane_id != "pane_1");
        reply(&mut model, &mut snapshot, preview.sequence, None);
        assert!(!model.ready());
        assert!(!model.accepts_input("pane_2"));
        let cleared = model
            .poll(now)
            .expect("the deleted preview must be cleared on the server");
        assert!(cleared.preview_terminal.is_none());
        assert!(cleared.hidden_terminals.is_empty());
        assert!(cleared.read_only_terminals.is_empty());
        reply(&mut model, &mut snapshot, cleared.sequence, Some("pane_2"));
        assert!(model.available());
        assert!(model.shows_tab("tab_1"));
        assert!(model.accepts_input("pane_2"));
    }
}
