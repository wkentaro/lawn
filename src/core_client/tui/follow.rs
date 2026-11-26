use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::core_lifecycle::Lifecycle;
use crate::hc::{Core, Machine, Stream};
use crate::session_doc::{Layout, Loaded, Store};

pub(super) const RECONNECT: Duration = Duration::from_millis(500);

pub(super) fn follow<T: From<Loaded> + Send + 'static>(
    session: String,
    layouts: mpsc::UnboundedSender<T>,
) -> io::Result<()> {
    crate::thread_spawn::spawn_named("lawn-follow", move || {
        let follower = Arc::new(Follower {
            session,
            layouts,
            shown: Mutex::default(),
        });
        while follower.follow() {
            std::thread::sleep(RECONNECT);
        }
    })?;
    Ok(())
}

struct Follower<T> {
    session: String,
    layouts: mpsc::UnboundedSender<T>,
    shown: Mutex<Shown>,
}

#[derive(Default)]
struct Shown {
    near: Option<Loaded>,
    far: BTreeMap<String, Layout>,
    sent: Option<Loaded>,
    filled: bool,
}

#[derive(Default)]
struct Stale {
    changed: bool,
    every_machine: bool,
    machines: BTreeSet<String>,
    names: BTreeMap<String, String>,
    unread: Vec<u8>,
}

impl Stale {
    fn heard(&mut self, bytes: &[u8]) {
        let document = crate::session_doc::name();
        self.unread.extend_from_slice(bytes);
        while let Some(end) = self.unread.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.unread.drain(..=end).collect();
            let Ok(message) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            let params = &message["params"];
            match (message["method"].as_str(), params["type"].as_str()) {
                (Some("missed"), _) => (self.changed, self.every_machine) = (true, true),
                (Some("event"), Some("doc")) if params["data"]["name"] == document => {
                    self.changed = true;
                    if let Some(machine) = params["machine"].as_str() {
                        self.machines.insert(machine.to_owned());
                    }
                }
                _ => {}
            }
        }
    }
}

impl<T: From<Loaded> + Send + 'static> Follower<T> {
    fn follow(self: &Arc<Self>) -> bool {
        let core = match crate::core_client::core() {
            Ok(core) => core,
            Err(error) => {
                tracing::debug!(%error, "herdr core is unreachable");
                return true;
            }
        };
        let mut subscription = match core.subscribe(json!({"docs": [crate::session_doc::name()]})) {
            Ok(subscription) => subscription,
            Err(error) => {
                tracing::debug!(%error, "the session document cannot be followed");
                return true;
            }
        };
        self.shown().sent = None;
        let (events, heard) = std::sync::mpsc::channel();
        let (follower, far) = (self.clone(), core.clone());
        let linked = crate::thread_spawn::spawn_named("lawn-follow-linked", move || {
            follower.follow_linked(&far, &heard)
        });
        if let Err(error) = linked {
            tracing::warn!(%error, "the linked machines are not followed");
        }
        let store = Store::new(core.clone(), &self.session, None);
        loop {
            match store.load() {
                Ok(near) => {
                    let seen = {
                        let shown = self.shown();
                        let kept = shown.near.as_ref().map(|kept| kept.version);
                        shown.sent.is_some() && kept == Some(near.version)
                    };
                    if !seen && !self.show(&core, |shown| shown.near = Some(near)) {
                        return false;
                    }
                }
                Err(error) => tracing::warn!(%error, "the session layout did not load"),
            }
            let heard = |bytes: Vec<u8>| {
                core.credit(subscription.channel, credit(&bytes));
                let _ = events.send(bytes);
            };
            match subscription.stream.blocking_recv() {
                Some(Stream::Data(bytes)) => {
                    heard(bytes);
                    while let Ok(Stream::Data(bytes)) = subscription.stream.try_recv() {
                        heard(bytes);
                    }
                }
                Some(Stream::Lost | Stream::Closed(_)) | None => return true,
                Some(Stream::Resize { .. }) => {}
            }
        }
    }

    fn follow_linked(&self, core: &Arc<Core>, events: &std::sync::mpsc::Receiver<Vec<u8>>) {
        let mut stale = Stale {
            every_machine: true,
            ..Stale::default()
        };
        while self.reload(core, &mut stale) {
            while !stale.changed {
                let Ok(bytes) = events.recv() else {
                    return;
                };
                stale.heard(&bytes);
            }
            for bytes in events.try_iter() {
                stale.heard(&bytes);
            }
            stale.changed = false;
        }
    }

    fn reload(&self, core: &Arc<Core>, stale: &mut Stale) -> bool {
        let cores = match core.cores() {
            Ok(cores) => cores,
            Err(error) => {
                tracing::warn!(%error, "the linked machines did not load");
                return true;
            }
        };
        let machines = cores.split_last().map_or(&[][..], |(_, machines)| machines);
        let every_machine = std::mem::take(&mut stale.every_machine);
        let named = std::mem::take(&mut stale.machines);
        let reached: Vec<_> = machines
            .iter()
            .map(|machine| (machine, machine.id.as_str()))
            .filter(|(_, id)| crate::hc::exact_core_id(&cores, id).is_ok())
            .collect();
        let unread = |machine: &Machine, id: &str| {
            every_machine
                || named.contains(&machine.name)
                || stale.names.get(id) != Some(&machine.name)
        };
        let wanted: Vec<_> = reached
            .iter()
            .filter(|(machine, id)| machine.state == "connected" && unread(machine, id))
            .map(|(machine, id)| (&machine.name, *id))
            .collect();
        for (name, id) in wanted {
            match Store::new(core.clone(), &self.session, Some(id.to_owned())).load() {
                Ok(loaded) => {
                    let layout = loaded.layout.qualified(id);
                    let shown = self.show(core, |shown| {
                        shown.far.insert(id.to_owned(), layout);
                    });
                    if !shown {
                        return false;
                    }
                    stale.names.insert(id.to_owned(), name.clone());
                }
                Err(error) => {
                    tracing::warn!(%error, machine = %name, "a linked machine's layout did not load");
                    stale.machines.insert(name.clone());
                }
            }
        }
        let listed = |kept: &String| reached.iter().any(|(_, id)| id == kept);
        stale.names.retain(|id, _| listed(id));
        if self.shown().far.keys().all(listed) {
            return true;
        }
        self.show(core, |shown| shown.far.retain(|id, _| listed(id)))
    }

    fn shown(&self) -> MutexGuard<'_, Shown> {
        self.shown.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn show(&self, core: &Arc<Core>, change: impl FnOnce(&mut Shown)) -> bool {
        let closed_out = {
            let mut shown = self.shown();
            change(&mut shown);
            let Some(mut loaded) = shown.near.clone() else {
                return true;
            };
            for layout in shown.far.values() {
                loaded.layout.join(layout.clone());
            }
            if shown.sent.as_ref() == Some(&loaded) {
                return true;
            }
            let filled = !loaded.layout.workspaces.is_empty();
            let closed_out = std::mem::replace(&mut shown.filled, filled) && !filled;
            shown.sent = Some(loaded.clone());
            if self.layouts.send(loaded.into()).is_err() {
                return false;
            }
            closed_out
        };
        if closed_out {
            let home = crate::home_path::expand_tilde_path("~");
            if let Err(error) = Lifecycle::new(core.clone(), &self.session).start_first(&home, &[])
            {
                tracing::warn!(%error, "the closed-out session got no shell");
            }
        }
        true
    }
}

fn credit(bytes: &[u8]) -> u32 {
    u32::try_from(bytes.len()).unwrap_or(u32::MAX)
}

#[cfg(all(test, unix))]
mod tests;
