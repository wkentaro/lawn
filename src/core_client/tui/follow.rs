use std::io;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc;

use crate::core_lifecycle::Lifecycle;
use crate::hc::{Core, Stream};
use crate::session_doc::{Loaded, Store};

pub(super) const RECONNECT: Duration = Duration::from_millis(500);

pub(super) fn follow(session: String) -> io::Result<mpsc::UnboundedReceiver<Loaded>> {
    let (layouts, received) = mpsc::unbounded_channel();
    crate::thread_spawn::spawn_named("lawn-follow", move || {
        let mut follower = Follower {
            session,
            shown: false,
        };
        while follower.follow(&layouts) {
            std::thread::sleep(RECONNECT);
        }
    })?;
    Ok(received)
}

struct Follower {
    session: String,
    shown: bool,
}

impl Follower {
    fn follow(&mut self, layouts: &mpsc::UnboundedSender<Loaded>) -> bool {
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
        let store = Store::new(core.clone(), &self.session, None);
        let mut version = None;
        loop {
            match store.load() {
                Ok(loaded) if version != Some(loaded.version) => {
                    self.closed_out(&core, &loaded.layout);
                    version = Some(loaded.version);
                    if layouts.send(loaded).is_err() {
                        return false;
                    }
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "the session layout did not load"),
            }
            match subscription.stream.blocking_recv() {
                Some(Stream::Data(bytes)) => {
                    core.credit(subscription.channel, credit(&bytes));
                    while let Ok(Stream::Data(bytes)) = subscription.stream.try_recv() {
                        core.credit(subscription.channel, credit(&bytes));
                    }
                }
                Some(Stream::Lost | Stream::Closed(_)) | None => return true,
                Some(Stream::Resize { .. }) => {}
            }
        }
    }

    fn closed_out(&mut self, core: &Arc<Core>, layout: &crate::session_doc::Layout) {
        let shown = !layout.workspaces.is_empty();
        if std::mem::replace(&mut self.shown, shown) && !shown {
            let home = crate::home_path::expand_tilde_path("~");
            if let Err(error) = Lifecycle::new(core.clone(), &self.session).start_first(&home, &[])
            {
                tracing::warn!(%error, "the closed-out session got no shell");
            }
        }
    }
}

fn credit(bytes: &[u8]) -> u32 {
    u32::try_from(bytes.len()).unwrap_or(u32::MAX)
}

#[cfg(all(test, unix))]
mod tests;
