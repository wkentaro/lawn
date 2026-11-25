use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

#[cfg(test)]
mod tests;

use bytes::Bytes;
use tracing::{error, warn};

use super::{exit_reason, follow_core_terminal, PaneOutput};
use crate::hc::{Attached, Core, HcError, Stream};
use crate::platform::ChildExitReason;

struct Attachment {
    core: Arc<Core>,
    channel: u32,
}

struct Routing {
    current: Option<Arc<Attachment>>,
    events_channel: Option<u32>,
    size: (u16, u16),
    stopped: bool,
}

struct Connection {
    routing: Mutex<Routing>,
    changed: Condvar,
}

impl Connection {
    fn lock(&self) -> MutexGuard<'_, Routing> {
        self.routing
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn current(&self) -> Option<Arc<Attachment>> {
        self.lock()
            .current
            .as_ref()
            .filter(|attachment| !attachment.core.is_closed())
            .cloned()
    }

    fn is_current(routing: &Routing, attachment: &Arc<Attachment>) -> bool {
        !routing.stopped
            && !attachment.core.is_closed()
            && routing
                .current
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, attachment))
    }

    fn wait_current(&self, attachment: &Arc<Attachment>, delay: Duration) -> bool {
        let (routing, _) = self
            .changed
            .wait_timeout_while(self.lock(), delay, |routing| {
                Self::is_current(routing, attachment)
            })
            .unwrap_or_else(|error| error.into_inner());
        Self::is_current(&routing, attachment)
    }

    fn detach(&self, stopped: bool) {
        let (attachment, events) = {
            let mut routing = self.lock();
            routing.stopped |= stopped;
            (routing.current.take(), routing.events_channel.take())
        };
        self.changed.notify_all();
        if let Some(attachment) = attachment {
            attachment.core.detach(attachment.channel);
            if let Some(channel) = events {
                attachment.core.detach(channel);
            }
        }
    }

    fn follow(
        &self,
        attachment: &Arc<Attachment>,
        output: &PaneOutput,
        address: &str,
    ) -> Option<std::thread::JoinHandle<()>> {
        if !Self::is_current(&self.lock(), attachment) {
            return None;
        }
        match follow_core_terminal(
            &attachment.core,
            address,
            output.pane_id,
            output.reported_cwd.clone(),
            output.events.clone(),
        ) {
            Ok((channel, worker)) => {
                let mut routing = self.lock();
                if Self::is_current(&routing, attachment) {
                    routing.events_channel = Some(channel);
                } else {
                    drop(routing);
                    attachment.core.detach(channel);
                }
                Some(worker)
            }
            Err(error) => {
                warn!(pane = output.pane_id.raw(), %error, "cannot follow the core terminal");
                None
            }
        }
    }

    fn reconnect(
        &self,
        address: &str,
        by_core_id: bool,
    ) -> Result<Option<(Arc<Attachment>, Attached)>, HcError> {
        let mut delay = Duration::from_millis(100);
        loop {
            let size = {
                let routing = self.lock();
                if routing.stopped {
                    return Ok(None);
                }
                routing.size
            };
            let result = crate::hc::backend()
                .unwrap_or_else(|| Err(HcError::Unavailable("core panes are disabled".into())))
                .and_then(|core| {
                    if let (true, Ok((machine, _))) = (by_core_id, crate::hc::located(address)) {
                        core.holds(machine)?;
                    }
                    let attached = match core.attach(serde_json::json!({
                        "terminal": address,
                        "rows": size.0,
                        "cols": size.1,
                        "resumed": true,
                    })) {
                        Err(error) if error.kind() == Some("finished") => core.finished(address),
                        attached => attached,
                    }?;
                    Ok((core, attached))
                });
            match result {
                Ok((core, attached)) => {
                    let attachment = Arc::new(Attachment {
                        core,
                        channel: attached.channel,
                    });
                    let mut routing = self.lock();
                    if routing.stopped {
                        drop(routing);
                        attachment.core.detach(attachment.channel);
                        return Ok(None);
                    }
                    if routing.size != size {
                        attachment
                            .core
                            .resize(attachment.channel, routing.size.1, routing.size.0);
                    }
                    routing.current = Some(attachment.clone());
                    return Ok(Some((attachment, attached)));
                }
                Err(error) if error.kind() == Some("not_found") => return Err(error),
                Err(error) => {
                    warn!(%address, %error, "cannot reattach the core terminal");
                }
            }
            let (routing, _) = self
                .changed
                .wait_timeout_while(self.lock(), delay, |routing| !routing.stopped)
                .unwrap_or_else(|error| error.into_inner());
            if routing.stopped {
                return Ok(None);
            }
            delay = (delay * 2).min(Duration::from_secs(2));
        }
    }
}

enum Input {
    Bytes(Bytes),
    Stop,
    Submit {
        text: Bytes,
        enter: Bytes,
        delay: Duration,
        reply: mpsc::Sender<std::io::Result<()>>,
    },
}

pub(in crate::pane) struct CoreIo {
    connection: Arc<Connection>,
    address: String,
    followed: bool,
    input: mpsc::Sender<(Arc<Attachment>, Input)>,
}

impl CoreIo {
    pub(in crate::pane) fn new(
        core: Arc<Core>,
        channel: u32,
        address: String,
        followed: bool,
        rows: u16,
        cols: u16,
    ) -> std::io::Result<Self> {
        let connection = Arc::new(Connection {
            routing: Mutex::new(Routing {
                current: Some(Arc::new(Attachment { core, channel })),
                events_channel: None,
                size: (rows, cols),
                stopped: false,
            }),
            changed: Condvar::new(),
        });
        let (input, queue) = mpsc::channel::<(Arc<Attachment>, Input)>();
        let writer = connection.clone();
        crate::thread_spawn::spawn_named("hc-input", move || {
            while let Ok((attachment, item)) = queue.recv() {
                let connected = Connection::is_current(&writer.lock(), &attachment);
                match item {
                    Input::Bytes(bytes) => {
                        if connected {
                            attachment.core.input(attachment.channel, &bytes);
                        }
                    }
                    Input::Stop => writer.detach(true),
                    Input::Submit {
                        text,
                        enter,
                        delay,
                        reply,
                    } => {
                        let sent = if connected {
                            attachment
                                .core
                                .write_input(attachment.channel, &text)
                                .map_err(std::io::Error::other)
                                .and_then(|()| {
                                    if !writer.wait_current(&attachment, delay) {
                                        return Err(std::io::Error::other(
                                            "core connection changed; submission may be incomplete",
                                        ));
                                    }
                                    attachment
                                        .core
                                        .write_input(attachment.channel, &enter)
                                        .map_err(std::io::Error::other)
                                })
                        } else {
                            Err(std::io::Error::other(
                                "core connection changed before submission",
                            ))
                        };
                        let _ = reply.send(sent);
                    }
                }
            }
        })?;
        Ok(Self {
            connection,
            address,
            followed,
            input,
        })
    }

    pub(in crate::pane) fn read(
        &self,
        mut stream: tokio::sync::mpsc::UnboundedReceiver<Stream>,
        output: PaneOutput,
        ended: Arc<std::sync::atomic::AtomicBool>,
    ) -> std::io::Result<()> {
        let connection = self.connection.clone();
        let address = self.address.clone();
        let followed = self.followed;
        let Some(mut attachment) = connection.lock().current.clone() else {
            return Err(std::io::Error::other(
                "core connection stopped before reading",
            ));
        };
        crate::thread_spawn::spawn_named("hc-pane", move || {
            let follow = |attachment: &Arc<Attachment>| {
                followed
                    .then(|| connection.follow(attachment, &output, &address))
                    .flatten()
            };
            let mut follower = follow(&attachment);
            let reason = loop {
                match stream.blocking_recv() {
                    Some(Stream::Data(bytes)) => {
                        let result = output.process(&bytes);
                        for response in result.terminal_responses {
                            attachment
                                .core
                                .automatic_input(attachment.channel, &response);
                        }
                        attachment.core.credit(
                            attachment.channel,
                            u32::try_from(bytes.len()).unwrap_or(u32::MAX),
                        );
                    }
                    Some(Stream::Resize { .. }) => {}
                    Some(item @ Stream::Closed(_)) => break exit_reason(&item),
                    _ => {
                        connection.detach(false);
                        if let Some(worker) = follower.take() {
                            let _ = worker.join();
                        }
                        match connection.reconnect(&address, !followed) {
                            Ok(Some((replacement, attached))) => {
                                attachment = replacement;
                                stream = attached.stream;
                                follower = follow(&attachment);
                            }
                            Ok(None) => return,
                            Err(_) => break ChildExitReason::WaitFailed,
                        }
                    }
                }
            };
            if connection.lock().stopped {
                return;
            }
            connection.detach(false);
            if let Some(worker) = follower {
                let _ = worker.join();
            }
            ended.store(true, std::sync::atomic::Ordering::Release);
            if let Err(error) = output
                .events
                .blocking_send(crate::events::AppEvent::PaneDied {
                    pane_id: output.pane_id,
                    exit_reason: reason,
                })
            {
                error!(pane = output.pane_id.raw(), %error, "failed to send PaneDied event");
            }
        })?;
        Ok(())
    }

    pub(in crate::pane) fn send(
        &self,
        bytes: Bytes,
    ) -> Result<(), tokio::sync::mpsc::error::TrySendError<Bytes>> {
        let Some(attachment) = self.connection.current() else {
            return Err(tokio::sync::mpsc::error::TrySendError::Closed(bytes));
        };
        self.input
            .send((attachment, Input::Bytes(bytes.clone())))
            .map_err(|_| tokio::sync::mpsc::error::TrySendError::Closed(bytes))
    }

    pub(in crate::pane) fn validate_input(&self) -> std::io::Result<()> {
        self.connection
            .current()
            .map(|_| ())
            .ok_or_else(|| std::io::Error::other("core pane is disconnected"))
    }

    pub(in crate::pane) fn submit(
        &self,
        text: Bytes,
        enter: Bytes,
        delay: Duration,
    ) -> std::io::Result<mpsc::Receiver<std::io::Result<()>>> {
        let attachment = self
            .connection
            .current()
            .ok_or_else(|| std::io::Error::other("core pane is disconnected"))?;
        let (reply, done) = mpsc::channel();
        self.input
            .send((
                attachment,
                Input::Submit {
                    text,
                    enter,
                    delay,
                    reply,
                },
            ))
            .map_err(|_| std::io::Error::other("core pane input stopped"))?;
        Ok(done)
    }

    pub(in crate::pane) fn address(&self) -> &str {
        &self.address
    }

    pub(in crate::pane) fn respond(&self, bytes: &[u8]) {
        if let Some(attachment) = self.connection.current() {
            attachment.core.automatic_input(attachment.channel, bytes);
        }
    }

    pub(in crate::pane) fn resize(&self, cols: u16, rows: u16) {
        let mut routing = self.connection.lock();
        routing.size = (rows, cols);
        if let Some(attachment) = routing.current.as_ref() {
            attachment.core.resize(attachment.channel, cols, rows);
        }
    }

    pub(in crate::pane) fn kill(&self) {
        let current = self.connection.current();
        let address = self.address.clone();
        if let Err(error) = crate::thread_spawn::spawn_named("hc-kill", move || {
            let core = current
                .map(|attachment| Ok(attachment.core.clone()))
                .or_else(crate::hc::backend);
            if let Some(core) = core {
                if let Err(error) = core.and_then(|core| {
                    core.call("terminal.kill", serde_json::json!({"terminal": address}))
                }) {
                    warn!(%address, %error, "cannot end the core terminal");
                }
            }
        }) {
            warn!(address = %self.address, %error, "cannot end the core terminal");
        }
    }
}

impl Drop for CoreIo {
    fn drop(&mut self) {
        let queued = self
            .connection
            .current()
            .is_some_and(|attachment| self.input.send((attachment, Input::Stop)).is_ok());
        if !queued {
            self.connection.detach(true);
        }
    }
}
