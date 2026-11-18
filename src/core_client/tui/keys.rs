use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

use crate::config::LiveKeybindConfig;
use crate::input::{
    ConsumedInputLease, InputLease, InputLeaseKey, InputLeaseTable, KeybindMatch, RepeatPlan,
    TerminalKey,
};
use crate::layout::PaneId;

#[derive(Debug)]
pub(super) enum Routed {
    Text(PaneId, String),
    Key(PaneId, TerminalKey),
    Binding(KeybindMatch),
    Copy,
    CopyMode(TerminalKey),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Panes,
    Copy,
    Prompt,
}

pub(super) struct Keys {
    config: LiveKeybindConfig,
    erase_is_ctrl_h: bool,
    reports_releases: bool,
    prefix: bool,
    leases: InputLeaseTable<(), (bool, Mode), PaneId>,
}

impl Keys {
    pub(super) fn new(
        config: LiveKeybindConfig,
        erase_is_ctrl_h: bool,
        reports_releases: bool,
    ) -> Self {
        Self {
            config,
            erase_is_ctrl_h,
            reports_releases,
            prefix: false,
            leases: InputLeaseTable::default(),
        }
    }

    pub(super) fn route_host(
        &mut self,
        key: TerminalKey,
        focused: Option<PaneId>,
        copyable: bool,
        mode: Mode,
        committed: impl for<'k> FnOnce(&'k TerminalKey) -> Option<&'k str>,
    ) -> Vec<Routed> {
        let key = key.with_host_erase(self.erase_is_ctrl_h);
        let mut routed = Vec::new();
        if !self.prefix && mode == Mode::Panes {
            if let Some(text) = committed(&key) {
                routed.extend(focused.map(|pane| Routed::Text(pane, text.to_owned())));
                return routed;
            }
        }
        let key = if self.reports_releases && key.generated_text.is_some() {
            key.with_physical_identity_hint(true)
        } else {
            key
        };
        let lease_key = InputLeaseKey::new((), &key);
        let key = self.leases.normalize_press(&lease_key, key);
        match key.kind {
            KeyEventKind::Press
                if copyable
                    && matches!(key.code, KeyCode::Char('c' | 'C'))
                    && matches!(key.modifiers, KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
            {
                self.leases
                    .insert_consumed(lease_key, ConsumedInputLease::SuppressRepeats);
                routed.push(Routed::Copy);
            }
            KeyEventKind::Press => {
                let initial = (self.prefix, mode);
                let target = self.press(&key, focused, mode, &mut routed);
                if let Some(target) = target {
                    routed.push(Routed::Key(target, key.clone()));
                }
                let plan = self.leases.complete_press(
                    lease_key,
                    &key,
                    Some(&initial),
                    Some(&(self.prefix, mode)),
                    target,
                );
                self.repeat(lease_key, key, plan, focused, mode, &mut routed);
            }
            KeyEventKind::Repeat => {
                let plan = self
                    .leases
                    .plan_repeat(lease_key, &key, Some(&(self.prefix, mode)));
                self.repeat(lease_key, key, plan, focused, mode, &mut routed);
            }
            KeyEventKind::Release => {
                if let Some(InputLease::Forwarded(lease)) =
                    self.take_release_lease(&lease_key, &key)
                {
                    let layout_chord =
                        key.code != lease.key.code && key.identity() == lease.key.identity();
                    let release = if key.physical_key_id().is_some() || layout_chord {
                        lease
                            .key
                            .with_modifiers(key.modifiers)
                            .with_kind(KeyEventKind::Release)
                    } else {
                        key
                    };
                    routed.push(Routed::Key(lease.target, release));
                }
            }
        }
        routed
    }

    pub(super) fn release(&mut self, pane: PaneId) -> Vec<TerminalKey> {
        let mut released = Vec::new();
        for lease in self.leases.remove_target(&pane) {
            if lease.key.committed_text(true).is_some() {
                continue;
            }
            self.leases.insert_consumed(
                InputLeaseKey::new((), &lease.key),
                ConsumedInputLease::SuppressRepeats,
            );
            released.push(lease.key.with_kind(KeyEventKind::Release));
        }
        released
    }

    fn repeat(
        &mut self,
        lease_key: InputLeaseKey<()>,
        key: TerminalKey,
        plan: RepeatPlan<(bool, Mode), PaneId>,
        focused: Option<PaneId>,
        mode: Mode,
        routed: &mut Vec<Routed>,
    ) {
        match plan {
            RepeatPlan::Forwarded(_) if mode != Mode::Panes => {}
            RepeatPlan::Forwarded(target) => routed.push(Routed::Key(target, key)),
            RepeatPlan::Reprocess {
                context,
                repetitions,
                tracked,
            } => {
                for _ in 0..repetitions {
                    if !self.leases.reprocess_allowed(
                        lease_key,
                        &context,
                        Some(&(self.prefix, mode)),
                        tracked,
                    ) {
                        break;
                    }
                    let repeated = key
                        .clone()
                        .with_repeat_count(1)
                        .with_kind(KeyEventKind::Repeat);
                    if let Some(target) = self.press(&repeated, focused, mode, routed) {
                        routed.push(Routed::Key(target, repeated));
                    }
                }
            }
            RepeatPlan::Ignore => {}
        }
    }

    fn press(
        &mut self,
        key: &TerminalKey,
        focused: Option<PaneId>,
        mode: Mode,
        routed: &mut Vec<Routed>,
    ) -> Option<PaneId> {
        if matches!(key.code, KeyCode::Modifier(_)) {
            return None;
        }
        let to_focus = |routed: &mut Vec<Routed>| {
            if mode != Mode::Panes {
                routed.push(Routed::CopyMode(key.clone()));
                return None;
            }
            focused
        };
        if mode == Mode::Prompt {
            self.prefix = false;
            return to_focus(routed);
        }
        if !std::mem::take(&mut self.prefix) {
            if let Some(binding) = (mode == Mode::Panes)
                .then(|| crate::input::resolve_direct_binding(&self.config.keybinds, key))
                .flatten()
            {
                routed.push(Routed::Binding(binding));
                return None;
            }
            if self.config.matches_prefix(key) {
                self.prefix = true;
                return None;
            }
            return to_focus(routed);
        }
        if self.config.matches_prefix(key) {
            return to_focus(routed);
        }
        if key.code == KeyCode::Esc {
            return None;
        }
        routed.extend(
            crate::input::resolve_prefix_binding(&self.config.keybinds, key).map(Routed::Binding),
        );
        None
    }

    fn take_release_lease(
        &mut self,
        lease_key: &InputLeaseKey<()>,
        key: &TerminalKey,
    ) -> Option<InputLease<(bool, Mode), PaneId>> {
        if let Some(lease) = self.leases.remove(lease_key) {
            return Some(lease);
        }
        let KeyCode::Char(c) = key.layout_key().map_or(key.code, KeyCode::Char) else {
            return None;
        };
        fn single(mut chars: impl Iterator<Item = char>) -> Option<char> {
            match (chars.next(), chars.next()) {
                (Some(only), None) => Some(only),
                _ => None,
            }
        }
        let other_case = if c.is_lowercase() {
            single(c.to_uppercase())
        } else {
            single(c.to_lowercase())
        }
        .filter(|other| *other != c);
        key.shifted_codepoint
            .and_then(char::from_u32)
            .into_iter()
            .chain(other_case)
            .find_map(|candidate| {
                self.leases.remove(&InputLeaseKey::new(
                    (),
                    &TerminalKey::new(KeyCode::Char(candidate), key.modifiers),
                ))
            })
            .or_else(|| self.leases.remove_sole_text_press(()))
    }
}

#[cfg(test)]
mod tests;
