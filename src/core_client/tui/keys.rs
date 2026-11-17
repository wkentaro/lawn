use crossterm::event::{KeyCode, KeyEventKind};

use crate::config::LiveKeybindConfig;
use crate::input::{
    InputLease, InputLeaseKey, InputLeaseTable, KeybindMatch, RepeatPlan, TerminalKey,
};
use crate::layout::PaneId;

#[derive(Debug)]
pub(super) enum Routed {
    Text(PaneId, String),
    Key(PaneId, TerminalKey),
    Binding(KeybindMatch),
}

pub(super) struct Keys {
    config: LiveKeybindConfig,
    erase_is_ctrl_h: bool,
    reports_releases: bool,
    prefix: bool,
    leases: InputLeaseTable<(), bool, PaneId>,
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
        committed: impl for<'k> FnOnce(&'k TerminalKey) -> Option<&'k str>,
    ) -> Vec<Routed> {
        let key = key.with_host_erase(self.erase_is_ctrl_h);
        let mut routed = Vec::new();
        if !self.prefix {
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
            KeyEventKind::Press => {
                let initial = self.prefix;
                let target = self.press(&key, focused, &mut routed);
                if let Some(target) = target {
                    routed.push(Routed::Key(target, key.clone()));
                }
                let plan = self.leases.complete_press(
                    lease_key,
                    &key,
                    Some(&initial),
                    Some(&self.prefix),
                    target,
                );
                self.repeat(lease_key, key, plan, focused, &mut routed);
            }
            KeyEventKind::Repeat => {
                let plan = self.leases.plan_repeat(lease_key, &key, Some(&self.prefix));
                self.repeat(lease_key, key, plan, focused, &mut routed);
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
        self.leases
            .remove_target(&pane)
            .into_iter()
            .filter(|lease| lease.key.committed_text(true).is_none())
            .map(|lease| lease.key.with_kind(KeyEventKind::Release))
            .collect()
    }

    fn repeat(
        &mut self,
        lease_key: InputLeaseKey<()>,
        key: TerminalKey,
        plan: RepeatPlan<bool, PaneId>,
        focused: Option<PaneId>,
        routed: &mut Vec<Routed>,
    ) {
        match plan {
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
                        Some(&self.prefix),
                        tracked,
                    ) {
                        break;
                    }
                    let repeated = key
                        .clone()
                        .with_repeat_count(1)
                        .with_kind(KeyEventKind::Repeat);
                    if let Some(target) = self.press(&repeated, focused, routed) {
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
        routed: &mut Vec<Routed>,
    ) -> Option<PaneId> {
        if matches!(key.code, KeyCode::Modifier(_)) {
            return None;
        }
        if !std::mem::take(&mut self.prefix) {
            if let Some(binding) = crate::input::resolve_direct_binding(&self.config.keybinds, key)
            {
                routed.push(Routed::Binding(binding));
                return None;
            }
            if self.config.matches_prefix(key) {
                self.prefix = true;
                return None;
            }
            return focused;
        }
        if self.config.matches_prefix(key) {
            return focused;
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
    ) -> Option<InputLease<bool, PaneId>> {
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
