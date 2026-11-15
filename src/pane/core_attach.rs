//! Panes whose program runs in a herdr-core terminal. Lawn keeps its own
//! terminal engine for drawing and feeds it the core's output stream; the
//! program lives on in the core when the pane goes away.

use super::*;
use crate::hc::{Attached, Core, Stream};

enum Input {
    Bytes(Bytes),
    /// Text, a pause, then Enter, with nothing else sent in between.
    Submit {
        text: Bytes,
        enter: Bytes,
        delay: std::time::Duration,
        reply: std::sync::mpsc::Sender<std::io::Result<()>>,
    },
}

pub(super) struct CoreIo {
    core: Arc<Core>,
    channel: u32,
    /// The core terminal's address (`t7`), saved so a restart reattaches.
    address: String,
    /// Keys and submissions go through one queue so a submission's pause
    /// never lets other input in between its text and its Enter.
    input: std::sync::mpsc::Sender<Input>,
    /// The events stream that follows the program's cwd, when it opened.
    events_channel: Option<u32>,
}

impl CoreIo {
    fn new(core: Arc<Core>, channel: u32, address: String) -> std::io::Result<Self> {
        let (input, queue) = std::sync::mpsc::channel();
        let writer = core.clone();
        crate::thread_spawn::spawn_named("hc-input", move || {
            while let Ok(item) = queue.recv() {
                match item {
                    Input::Bytes(bytes) => writer.input(channel, &bytes),
                    Input::Submit {
                        text,
                        enter,
                        delay,
                        reply,
                    } => {
                        // The pause starts once the text is out, so a slow
                        // core can't deliver text and Enter together.
                        let sent = writer.write_input(channel, &text).and_then(|()| {
                            std::thread::sleep(delay);
                            writer.write_input(channel, &enter)
                        });
                        let _ = reply.send(sent.map_err(std::io::Error::other));
                    }
                }
            }
        })?;
        Ok(Self {
            core,
            channel,
            address,
            input,
            events_channel: None,
        })
    }

    pub(super) fn send(&self, bytes: Bytes) {
        let _ = self.input.send(Input::Bytes(bytes));
    }

    pub(super) fn submit(
        &self,
        text: Bytes,
        enter: Bytes,
        delay: std::time::Duration,
    ) -> std::io::Result<std::sync::mpsc::Receiver<std::io::Result<()>>> {
        let (reply, done) = std::sync::mpsc::channel();
        self.input
            .send(Input::Submit {
                text,
                enter,
                delay,
                reply,
            })
            .map_err(|_| std::io::Error::other("core pane input stopped"))?;
        Ok(done)
    }

    pub(super) fn address(&self) -> &str {
        &self.address
    }

    pub(super) fn respond(&self, bytes: &[u8]) {
        self.core.automatic_input(self.channel, bytes);
    }

    pub(super) fn resize(&self, cols: u16, rows: u16) {
        self.core.resize(self.channel, cols, rows);
    }
}

impl CoreIo {
    /// Ends the program, for a pane that closes. Dropping only detaches.
    pub(super) fn kill(&self) {
        let core = self.core.clone();
        let address = self.address.clone();
        // The caller may be the app loop; a slow core must not stall it.
        let killer = crate::thread_spawn::spawn_named("hc-kill", move || {
            if let Err(err) = core.call("terminal.kill", serde_json::json!({"terminal": address})) {
                warn!(%address, %err, "cannot end the core terminal");
            }
        });
        if let Err(err) = killer {
            warn!(address = %self.address, %err, "cannot end the core terminal");
        }
    }
}

impl Drop for CoreIo {
    fn drop(&mut self) {
        self.core.detach(self.channel);
        if let Some(channel) = self.events_channel {
            self.core.detach(channel);
        }
    }
}

/// The cwd of a terminal record's foreground process.
fn foreground_cwd(record: &serde_json::Value) -> Option<std::path::PathBuf> {
    record
        .pointer("/foreground/cwd")
        .and_then(|cwd| cwd.as_str())
        .map(std::path::PathBuf::from)
}

/// What the program in a core terminal says it is doing (OSC 7501). A core
/// pane has no screen detector; this stands in for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreStatus {
    /// The agent the program names itself as, when Lawn knows it.
    pub agent: Option<Agent>,
    pub state: AgentState,
    /// It finished a turn, with a result or an error to look at.
    pub finished: bool,
}

/// The status in a terminal record or a `status` event. No status, or a
/// state Lawn does not know, is no agent.
fn core_status(status: &serde_json::Value) -> CoreStatus {
    let (state, finished) = match status["state"].as_str() {
        Some("working") => (AgentState::Working, false),
        Some("blocked") => (AgentState::Blocked, false),
        Some("idle") => (AgentState::Idle, false),
        Some("done" | "error") => (AgentState::Idle, true),
        _ => (AgentState::Unknown, false),
    };
    let app = status["app"]
        .as_str()
        .filter(|_| state != AgentState::Unknown);
    CoreStatus {
        agent: app.and_then(crate::detect::parse_agent_label),
        state,
        finished,
    }
}

/// The program isn't our child, so Lawn can't read its cwd from `/proc` or
/// watch its screen for an agent. The core samples the foreground process
/// and keeps the status the program reports; publish the cwd the way an
/// OSC 7 report would be, for shells that don't send one, and the status as
/// the pane's agent state. Returns the events channel to close with the pane.
fn follow_core_terminal(
    core: &Arc<Core>,
    address: &str,
    pane_id: PaneId,
    reported_cwd: Arc<Mutex<Option<std::path::PathBuf>>>,
    events: mpsc::Sender<AppEvent>,
) -> std::io::Result<u32> {
    let subscription = core
        .subscribe(serde_json::json!({"terminal": address}))
        .map_err(std::io::Error::other)?;
    let channel = subscription.channel;
    let stream = subscription.stream;
    let status_events = events.clone();
    let publish = move |cwd: Option<std::path::PathBuf>| {
        if let Some(cwd) = cwd {
            publish_reported_cwd(pane_id, cwd, &reported_cwd, &events);
        }
    };
    let publish_status = move |status: &serde_json::Value| {
        let status = core_status(status);
        let _ = status_events.blocking_send(AppEvent::CoreStatusReported { pane_id, status });
    };
    // A snapshot here holds the one terminal.
    fn snapshot_status(snapshot: &serde_json::Value) -> &serde_json::Value {
        &snapshot["terminals"][0]["status"]
    }
    publish(snapshot_cwd(&subscription.snapshot));
    let snapshot = subscription.snapshot;
    let reader = core.clone();
    crate::thread_spawn::spawn_named("hc-pane", move || {
        // Sending waits for room, which the caller's thread must not do.
        publish_status(snapshot_status(&snapshot));
        read_events(&reader, channel, stream, |message| {
            let params = &message["params"];
            match (message["method"].as_str(), params["type"].as_str()) {
                (Some("event"), Some("foreground")) => {
                    publish(params["data"]["cwd"].as_str().map(Into::into));
                }
                (Some("event"), Some("status")) => publish_status(&params["data"]["status"]),
                // Events were dropped; the fresh snapshot has the record.
                (Some("missed"), _) => {
                    publish(snapshot_cwd(params));
                    publish_status(snapshot_status(params));
                }
                _ => {}
            }
        });
    })?;
    Ok(channel)
}

/// Hands each message of an `events.subscribe` stream to `on_message`, until
/// the stream ends.
fn read_events(
    core: &Core,
    channel: u32,
    mut stream: mpsc::UnboundedReceiver<Stream>,
    mut on_message: impl FnMut(&serde_json::Value),
) {
    let mut pending = Vec::new();
    while let Some(Stream::Data(bytes)) = stream.blocking_recv() {
        core.credit(channel, u32::try_from(bytes.len()).unwrap_or(u32::MAX));
        pending.extend_from_slice(&bytes);
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            if let Ok(message) = serde_json::from_slice(&line) {
                on_message(&message);
            }
        }
    }
}

/// A core terminal that no pane shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreTerminal {
    pub address: String,
    /// The terminal whose program started this one.
    pub parent: Option<String>,
    pub name: Option<String>,
    pub cwd: std::path::PathBuf,
    pub rows: u16,
    pub cols: u16,
}

/// The `lawn` label on every terminal this session starts: the session's
/// name, which keeps one session from parking another's panes.
fn session_label() -> String {
    crate::session::active_name().unwrap_or_else(|| crate::session::DEFAULT_SESSION_NAME.into())
}

/// The terminal in `record`, when Lawn should park it: a live one on this
/// machine that no Lawn session started and Lawn doesn't run in. One this
/// session started itself counts once it `lost_pane`.
fn terminal_to_park(
    record: &serde_json::Value,
    session: &str,
    own: Option<&str>,
    lost_pane: bool,
) -> Option<CoreTerminal> {
    let text = |key: &str| record[key].as_str().filter(|text| !text.is_empty());
    let address = text("address")?;
    let started_by = record["labels"]["lawn"].as_str();
    // shortcut: skips terminals of other machines (`machine/t7`), whose cwd
    // is no folder here; list them once a pane can live on another machine.
    if record["state"] != "live"
        || address.contains('/')
        || own == Some(address)
        || !(started_by.is_none() || (lost_pane && started_by == Some(session)))
    {
        return None;
    }
    let size = |key: &str, default: u16| {
        record[key]
            .as_u64()
            .and_then(|size| u16::try_from(size).ok())
            .unwrap_or(default)
    };
    Some(CoreTerminal {
        address: address.to_owned(),
        parent: text("parent").map(str::to_owned),
        name: text("name").map(str::to_owned),
        cwd: foreground_cwd(record).or_else(|| text("cwd").map(Into::into))?,
        rows: size("rows", 24),
        cols: size("cols", 80),
    })
}

/// Whether `session` may park the terminal in `record`. A terminal belongs
/// to the session of the nearest terminal above it that has one: a helper
/// goes where the pane that started it is. Above a terminal with no session
/// anywhere, the topmost one decides for all below it, and the first session
/// to write its name in that terminal's document gets them for good.
// shortcut: walks up again for every terminal of a family, and leaves one
// small document per family in the core; remember the answer per snapshot
// once families get large, and drop the documents once the core can.
fn claim(core: &Core, record: &serde_json::Value, session: &str) -> crate::hc::HcResult<bool> {
    let mut top = std::borrow::Cow::Borrowed(record);
    // Parents never loop; the bound is for a core that says they do.
    for _ in 0..64 {
        if let Some(owner) = top["labels"]["lawn"].as_str() {
            return Ok(owner == session);
        }
        let Some(parent) = top["parent"].as_str() else {
            break;
        };
        match core.call("terminal.get", serde_json::json!({"terminal": parent})) {
            Ok(above) => top = std::borrow::Cow::Owned(above),
            // The core forgot it, so the one below is the topmost left.
            Err(err) if err.kind() == Some("not_found") => break,
            Err(err) => return Err(err),
        }
    }
    let owner = format!("lawn-owner-{}", top["address"].as_str().unwrap_or_default());
    // Version zero is a document never written, so only one write gets in.
    match core.doc_set(&owner, None, &session.into(), 0) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == Some("conflict") => {
            Ok(core.doc_get(&owner, None)?.value.as_str() == Some(session))
        }
        Err(err) => Err(err),
    }
}

/// Reports the core terminals Lawn should park: those already running, then
/// each one started later.
// shortcut: ends with the connection; watch again once panes survive a core
// that restarts.
pub(crate) fn watch_core_terminals(
    core: &Arc<Core>,
    events: mpsc::Sender<AppEvent>,
) -> std::io::Result<()> {
    let subscription = core
        .subscribe(serde_json::json!({}))
        .map_err(std::io::Error::other)?;
    let session = session_label();
    let own = std::env::var("HC_TERMINAL_ID").ok();
    let core = core.clone();
    crate::thread_spawn::spawn_named("hc-terminals", move || {
        let report = |record: &serde_json::Value, lost_pane: bool| {
            let Some(terminal) = terminal_to_park(record, &session, own.as_deref(), lost_pane)
            else {
                return;
            };
            match claim(&core, record, &session) {
                Ok(true) => {
                    let _ = events.blocking_send(AppEvent::CoreTerminalFound(terminal));
                }
                // Another session parks it.
                Ok(false) => {}
                Err(err) => {
                    warn!(address = %terminal.address, %err, "cannot take the core terminal to park");
                }
            }
        };
        let report_all = |snapshot: &serde_json::Value, lost_pane: bool| {
            for record in snapshot["terminals"].as_array().into_iter().flatten() {
                report(record, lost_pane);
            }
        };
        // Before this session starts any, one of its own has lost its pane.
        report_all(&subscription.snapshot, true);
        read_events(
            &core,
            subscription.channel,
            subscription.stream,
            |message| {
                let params = &message["params"];
                match (message["method"].as_str(), params["type"].as_str()) {
                    (Some("event"), Some("created")) => report(&params["data"], false),
                    // A kept terminal has a program again. It has no pane
                    // if it was found while it had none to attach to.
                    (Some("event"), Some("restarted")) => report(&params["data"], true),
                    // Events were dropped; the fresh snapshot has every record.
                    (Some("missed"), _) => report_all(params, false),
                    _ => {}
                }
            },
        );
    })?;
    Ok(())
}

/// Marks a terminal a pane shows again as this session's, so no other
/// session parks it or its helpers.
fn mark_ours(core: &Core, address: &str) {
    let labels = serde_json::json!({"lawn": session_label()});
    let params = serde_json::json!({"terminal": address, "labels": labels});
    if let Err(err) = core.call("terminal.label", params) {
        warn!(%address, %err, "cannot mark the core terminal as this session's");
    }
}

/// The foreground cwd in an `events.subscribe` snapshot of one terminal.
fn snapshot_cwd(snapshot: &serde_json::Value) -> Option<std::path::PathBuf> {
    snapshot["terminals"]
        .as_array()?
        .iter()
        .find_map(foreground_cwd)
}

/// Why the core ended a pane's stream.
fn exit_reason(item: &Stream) -> crate::platform::ChildExitReason {
    match item {
        // The program ended for good; other reasons (a lost kept terminal,
        // a stopping core) may bring the terminal back.
        Stream::Closed(reason)
            if matches!(
                reason.get("reason").and_then(|r| r.as_str()),
                Some("exited" | "killed")
            ) =>
        {
            crate::platform::ChildExitReason::Exited
        }
        _ => crate::platform::ChildExitReason::WaitFailed,
    }
}

/// `terminal.attach` for a terminal Lawn shows again, with all the history
/// the core keeps; the pane's own scrollback limit trims the rest.
fn reattach_params(address: &str, rows: u16, cols: u16) -> serde_json::Value {
    serde_json::json!({"terminal": address, "cols": cols, "rows": rows, "history": u32::MAX})
}

/// `NotFound` when the saved program is gone for good: the core forgot the
/// terminal or it finished. Anything else may pass, so the caller keeps it.
fn reattach_error(err: crate::hc::HcError) -> std::io::Error {
    let kind = match err.kind() {
        Some("not_found" | "finished") => std::io::ErrorKind::NotFound,
        _ => std::io::ErrorKind::Other,
    };
    std::io::Error::new(kind, err)
}

/// The hidden command a core pane starts through:
/// `lawn pane-exec <names> <program> [args…]`, with `<names>` joined by `=`.
/// The core adds a pane's `env` on top of its own environment, so something
/// has to drop the rest before the program starts.
pub(crate) const PANE_EXEC: &str = "pane-exec";

/// Whether a pane keeps `name`: Lawn listed it in `names`, or it is the
/// core's own `HC_*` or `PWD`.
#[cfg(unix)]
fn pane_keeps(names: &str, name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name.starts_with("HC_") || name == "PWD" || names.split('=').any(|kept| kept == name)
    })
}

/// Runs `lawn pane-exec`: becomes the pane's program, holding only the
/// variables the pane keeps. Returns only when the program can't start.
#[cfg(unix)]
pub(crate) fn pane_exec(args: &[String]) -> std::io::Error {
    let [names, program, rest @ ..] = args else {
        return std::io::Error::other("pane-exec needs the kept names and a program");
    };
    let mut command = std::process::Command::new(program);
    command
        .args(rest)
        .env_clear()
        .envs(std::env::vars_os().filter(|(name, _)| pane_keeps(names, name)));
    let err = crate::platform::exec(&mut command);
    std::io::Error::new(err.kind(), format!("cannot start {program}: {err}"))
}

/// The `lawn` whose `pane-exec` starts a pane's program.
fn launcher() -> std::io::Result<std::path::PathBuf> {
    #[cfg(not(test))]
    return crate::platform::launch_executable();
    // The test binary has no `pane-exec`; the `lawn` cargo builds beside its
    // folder for the integration tests does.
    #[cfg(test)]
    Ok(std::env::current_exe()?.with_file_name("../lawn"))
}

/// `terminal.run` for a command Lawn built for a PTY: same program, folder
/// and environment, started through `launcher` and owned by no terminal so
/// it outlives Lawn.
fn run_params(
    launcher: &std::path::Path,
    cmd: &CommandBuilder,
    rows: u16,
    cols: u16,
) -> serde_json::Value {
    let argv: Vec<String> = if cmd.is_default_prog() {
        // Lawn picked the shell; a login shell is what portable-pty would start.
        cmd.get_env("SHELL")
            .map(|shell| vec![shell.to_string_lossy().into_owned(), "-l".to_owned()])
            .unwrap_or_default()
    } else {
        cmd.get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    };
    let env: serde_json::Map<String, serde_json::Value> = cmd
        .iter_full_env_as_str()
        // The core names the terminal itself.
        .filter(|(key, _)| !key.starts_with("HC_"))
        .map(|(key, value)| (key.to_owned(), value.into()))
        .collect();
    let argv = if argv.is_empty() {
        argv
    } else {
        let mut launched = vec![
            launcher.to_string_lossy().into_owned(),
            PANE_EXEC.to_owned(),
            env.keys().map(String::as_str).collect::<Vec<_>>().join("="),
        ];
        launched.extend(argv);
        launched
    };
    serde_json::json!({
        "argv": argv,
        "cwd": cmd.get_cwd().map(|cwd| cwd.to_string_lossy()),
        "env": env,
        "size": {"cols": cols, "rows": rows},
        "labels": {"lawn": session_label()},
        "no_parent": true,
        "no_notify": true,
        "keep": true,
    })
}

/// Starts `cmd` in a new core terminal and attaches to it.
pub(super) fn start(
    core: &Core,
    pane_id: PaneId,
    cmd: &CommandBuilder,
    rows: u16,
    cols: u16,
) -> std::io::Result<Attached> {
    let address = core
        .run(run_params(&launcher()?, cmd, rows, cols))
        .map_err(std::io::Error::other)?;
    info!(pane = pane_id.raw(), %address, "pane runs in herdr-core");
    attach_new(core, &address, rows, cols)
}

/// Attaches to a terminal `start` just made.
fn attach_new(core: &Core, address: &str, rows: u16, cols: u16) -> std::io::Result<Attached> {
    match core.attach(serde_json::json!({"terminal": address, "cols": cols, "rows": rows})) {
        // A short command can end before the attach. It still ran, so the
        // pane shows what it left and then exits like any other.
        Err(err) if err.kind() == Some("finished") => core.finished(address),
        attached => attached,
    }
    .map_err(std::io::Error::other)
}

impl PaneRuntime {
    /// The herdr-core terminal this pane shows, if it shows one.
    pub(crate) fn core_address(&self) -> Option<&str> {
        match &self.io {
            PaneRuntimeIo::Core(io) => Some(io.address()),
            _ => None,
        }
    }

    /// Shows an existing core terminal again, as after a restart.
    // Same construction inputs as `attach_core`, with an address for the attach.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn reattach_core(
        pane_id: PaneId,
        address: &str,
        rows: u16,
        cols: u16,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
    ) -> std::io::Result<Self> {
        let core = crate::hc::backend()
            .ok_or_else(|| std::io::Error::other("lawn is not running on herdr-core"))?
            .map_err(std::io::Error::other)?;
        let attached = core
            .attach(reattach_params(address, rows, cols))
            .map_err(reattach_error)?;
        mark_ours(&core, address);
        Self::attach_core(
            pane_id,
            core,
            attached,
            rows,
            cols,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            events,
            render_notify,
            render_dirty,
            // The core paints its own history, asked for in the attach.
            None,
        )
    }

    /// A pane showing a core terminal through an attach made at `rows`×`cols`.
    // Same construction inputs as `spawn_command_builder`, minus the command.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn attach_core(
        pane_id: PaneId,
        core: Arc<Core>,
        attached: Attached,
        rows: u16,
        cols: u16,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
        history_ansi: Option<&str>,
    ) -> std::io::Result<Self> {
        let Attached {
            channel,
            mut stream,
            terminal: record,
        } = attached;
        let address = record
            .get("address")
            .and_then(|address| address.as_str())
            .unwrap_or_default()
            .to_owned();
        // From here on, failing drops `io`, which detaches the stream.
        let io =
            CoreIo::new(core.clone(), channel, address).inspect_err(|_| core.detach(channel))?;
        let (response_tx, _response_rx) = mpsc::channel::<Bytes>(1);
        let pane_terminal = new_pane_terminal(
            rows,
            cols,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            &response_tx,
        )?;
        if let Some(ansi) = history_ansi {
            // The core's first paint clears the screen, so scroll the saved
            // lines up into history first.
            let scroll = "\r\n".repeat(usize::from(rows));
            pane_terminal.seed_history_ansi(&format!("{ansi}{scroll}"));
        }
        let terminal = Arc::new(PaneTerminal::new(pane_terminal));
        let compression = TerminalCompressionTask::spawn(pane_id, terminal.clone());
        // The program isn't our child: no pid, so nothing here signals it.
        let child_pid = Arc::new(AtomicU32::new(0));
        let reported_cwd = Arc::new(Mutex::new(None));
        let mut io = io;
        io.events_channel = follow_core_terminal(
            &core,
            &io.address,
            pane_id,
            reported_cwd.clone(),
            events.clone(),
        )
        .inspect_err(|err| warn!(pane = pane_id.raw(), %err, "cannot follow the core terminal"))
        .ok();
        let stream_ended = Arc::new(AtomicBool::new(false));
        let content_seq = Arc::new(AtomicU64::new(0));
        let content_write_lock = Arc::new(Mutex::new(()));

        let output = PaneOutput {
            pane_id,
            terminal: terminal.clone(),
            response_writer: response_tx,
            render_notify,
            render_dirty,
            content_seq: content_seq.clone(),
            content_write_lock: content_write_lock.clone(),
            detection_content_seq: None,
            child_pid: child_pid.clone(),
            events: events.clone(),
            reported_cwd: reported_cwd.clone(),
            compression_wake: compression.notifier(),
            rt: tokio::runtime::Handle::current(),
        };
        let reader_core = core.clone();
        let ended = stream_ended.clone();
        // Parsing is blocking work, like the PTY actor's reader thread.
        crate::thread_spawn::spawn_named("hc-pane", move || {
            let reason = loop {
                match stream.blocking_recv() {
                    Some(Stream::Data(bytes)) => {
                        let result = output.process(&bytes);
                        for response in result.terminal_responses {
                            reader_core.automatic_input(channel, &response);
                        }
                        reader_core.credit(channel, u32::try_from(bytes.len()).unwrap_or(u32::MAX));
                    }
                    // The core sends RESIZE only to observing viewers. For a typing
                    // viewer like this one, the core sizes the grid to whichever viewer
                    // typed, resized or focused last, so our next keystroke or layout
                    // resize takes it back.
                    Some(Stream::Resize { .. }) => {}
                    Some(item) => break exit_reason(&item),
                    None => break crate::platform::ChildExitReason::WaitFailed,
                }
            };
            ended.store(true, Ordering::Release);
            if let Err(err) = events.blocking_send(AppEvent::PaneDied {
                pane_id,
                exit_reason: reason,
            }) {
                error!(pane = pane_id.raw(), err = %err, "failed to send PaneDied event");
            }
        })?;

        Ok(Self {
            pane_id,
            terminal,
            io: PaneRuntimeIo::Core(io),
            current_size: Cell::new((rows, cols, 0, 0)),
            child_pid,
            reported_cwd,
            persistence_cwd: Mutex::new(None),
            cwd_process_exited: stream_ended,
            child_wait_completed: None,
            kitty_keyboard_flags: Arc::new(AtomicU16::new(0)),
            content_seq,
            content_write_lock,
            detection_content_seq: Arc::new(AtomicU64::new(0)),
            full_lifecycle_authority_active: Arc::new(AtomicBool::new(false)),
            self_reported_agent_active: Arc::new(AtomicBool::new(false)),
            detect_reset_notify: Arc::new(Notify::new()),
            pending_release: Arc::new(Mutex::new(None)),
            preserve_processes_on_drop: true,
            compression,
            detect_handle: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_reported_exit_counts_as_exited() {
        use crate::platform::ChildExitReason;
        assert_eq!(
            exit_reason(&Stream::Closed(json!({"reason": "exited", "code": 0}))),
            ChildExitReason::Exited
        );
        assert_eq!(
            exit_reason(&Stream::Closed(json!({"reason": "removed"}))),
            ChildExitReason::WaitFailed
        );
        assert_eq!(exit_reason(&Stream::Lost), ChildExitReason::WaitFailed);
    }

    #[test]
    fn run_params_keep_the_command_and_drop_core_identity() {
        let mut cmd = CommandBuilder::new("claude");
        cmd.arg("--resume");
        cmd.cwd("/work");
        cmd.env("HC_TERMINAL_ID", "t9");
        cmd.env("LAWN_KEEP", "yes");
        let params = run_params(std::path::Path::new("/bin/lawn"), &cmd, 24, 80);
        let argv: Vec<String> = serde_json::from_value(params["argv"].clone()).unwrap();
        assert_eq!(argv[..2], ["/bin/lawn", PANE_EXEC]);
        assert_eq!(argv[3..], ["claude", "--resume"]);
        assert_eq!(params["cwd"], "/work");
        assert_eq!(params["env"]["LAWN_KEEP"], "yes");
        assert!(params["env"].get("HC_TERMINAL_ID").is_none());
        assert_eq!(params["size"], json!({"cols": 80, "rows": 24}));
        assert_eq!(
            (params["no_parent"].clone(), params["keep"].clone()),
            (json!(true), json!(true))
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_params_start_lawns_login_shell_with_only_the_pane_env() {
        let mut cmd = CommandBuilder::new_default_prog();
        cmd.env("SHELL", "/bin/fish");
        cmd.env("LAWN_KEEP", "yes");
        cmd.env_remove("PATH");
        let params = run_params(std::path::Path::new("/bin/lawn"), &cmd, 24, 80);
        let argv: Vec<String> = serde_json::from_value(params["argv"].clone()).unwrap();
        assert_eq!(argv[3..], ["/bin/fish", "-l"]);
        let keeps = |name: &str| pane_keeps(&argv[2], std::ffi::OsStr::new(name));
        assert!(keeps("LAWN_KEEP"));
        assert!(!keeps("PATH"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_first_session_to_claim_a_terminal_gets_all_below_it() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        let run = |params: serde_json::Value| test.core.run(params).expect("run");
        let record = |address: &str| {
            test.core
                .call("terminal.get", json!({"terminal": address}))
                .expect("terminal.get")
        };
        let claims = |address: &str, session: &str| {
            claim(&test.core, &record(address), session).expect("claim")
        };
        let top = run(json!({"argv": ["sleep", "30"], "no_parent": true}));
        let helper = run(json!({"argv": ["sleep", "30"], "parent": top}));
        let below = run(json!({"argv": ["sleep", "30"], "parent": helper}));

        // Two sessions find all three at once and keep asking. Whichever
        // gets one gets every one, every time.
        let asked = |session: &'static str| {
            let (claims, addresses) = (&claims, [&top, &below, &helper]);
            move || -> Vec<bool> {
                let ask = |address: &&String| claims(address, session);
                (0..10).flat_map(|_| addresses.iter().map(ask)).collect()
            }
        };
        let (core, work) = std::thread::scope(|scope| {
            let core = scope.spawn(asked("core"));
            let work = scope.spawn(asked("core-work"));
            (core.join().expect("core"), work.join().expect("core-work"))
        });
        let winner = if core[0] { "core" } else { "core-work" };
        assert!(
            core.iter().all(|won| *won == (winner == "core")),
            "{core:?}"
        );
        assert!(
            work.iter().all(|won| *won == (winner != "core")),
            "{work:?}"
        );

        // A pane of the other session in between owns what it starts.
        let other = if core[0] { "core-work" } else { "core" };
        test.core
            .call(
                "terminal.label",
                json!({"terminal": helper, "labels": {"lawn": other}}),
            )
            .expect("terminal.label");
        assert!(claims(&below, other));
        assert!(!claims(&below, winner));
        assert!(claims(&top, winner));
    }

    #[test]
    fn core_status_maps_the_programs_words_to_agent_states() {
        let status = |word: &str| {
            let status = core_status(&json!({"state": word, "app": "claude"}));
            (status.agent, status.state, status.finished)
        };
        let claude = Some(Agent::Claude);
        assert_eq!(status("working"), (claude, AgentState::Working, false));
        assert_eq!(status("blocked"), (claude, AgentState::Blocked, false));
        assert_eq!(status("idle"), (claude, AgentState::Idle, false));
        assert_eq!(status("done"), (claude, AgentState::Idle, true));
        assert_eq!(status("error"), (claude, AgentState::Idle, true));
        // A state Lawn does not know, or no status, is no agent.
        assert_eq!(status("napping"), (None, AgentState::Unknown, false));
        let none = core_status(&json!(null));
        assert_eq!((none.agent, none.state, none.finished), status("napping"));
        // A program Lawn does not know still has a state.
        assert_eq!(
            core_status(&json!({"state": "working", "app": "mytool"})),
            CoreStatus {
                agent: None,
                state: AgentState::Working,
                finished: false,
            }
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn core_pane_reports_the_status_its_program_sends() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        let (events, mut event_rx) = mpsc::channel(8);
        let script = r"printf '\033]7501;state=working:app=claude\033\\'; read line; \
            printf '\033]7501;state=done:app=claude\033\\'; sleep 30";
        let (_address, runtime) = attach(&test.core, &["sh", "-c", script], events);
        let mut next = async || loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(10), event_rx.recv());
            match event.await.expect("a status").expect("event") {
                AppEvent::CoreStatusReported { status, .. } => {
                    return (status.agent, status.state, status.finished)
                }
                _ => continue,
            }
        };
        let claude = Some(Agent::Claude);
        assert_eq!(next().await, (claude, AgentState::Working, false));
        runtime.try_send_bytes(Bytes::from_static(b"\r")).unwrap();
        assert_eq!(next().await, (claude, AgentState::Idle, true));
        runtime.shutdown();
    }

    #[test]
    fn only_terminals_outside_every_lawn_session_are_parked() {
        let record = |extra: serde_json::Value| {
            let mut record = json!({
                "address": "t7",
                "state": "live",
                "cwd": "/start",
                "foreground": {"cwd": "/now"},
                "parent": "t3",
                "name": "build",
                "cols": 120,
                "rows": 40,
            });
            record
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            record
        };
        let park = |record: &serde_json::Value, lost_pane| {
            terminal_to_park(record, "core", Some("t1"), lost_pane)
        };
        assert_eq!(
            park(&record(json!({})), false),
            Some(CoreTerminal {
                address: "t7".into(),
                parent: Some("t3".into()),
                name: Some("build".into()),
                cwd: "/now".into(),
                rows: 40,
                cols: 120,
            })
        );
        let plain = park(
            &json!({"address": "t7", "state": "live", "cwd": "/start"}),
            false,
        )
        .expect("a record with nothing optional");
        assert_eq!((plain.cwd, plain.parent), ("/start".into(), None));

        for (why, extra) in [
            ("finished", json!({"state": "finished"})),
            ("where Lawn runs", json!({"address": "t1"})),
            ("on another machine", json!({"address": "box/t7"})),
            (
                "another session's",
                json!({"labels": {"lawn": "core-work"}}),
            ),
            ("a pane being made", json!({"labels": {"lawn": "core"}})),
        ] {
            assert_eq!(park(&record(extra), false), None, "{why}");
        }
        // One of this session's own has lost its pane.
        let own = record(json!({"labels": {"lawn": "core"}}));
        assert!(park(&own, true).is_some());
        let other = record(json!({"labels": {"lawn": "core-work"}}));
        assert_eq!(park(&other, true), None);
    }

    #[cfg(unix)]
    #[test]
    fn pane_keeps_only_what_lawn_named_and_the_cores_own() {
        let keeps = |name: &str| pane_keeps("HOME=LAWN_KEEP", std::ffi::OsStr::new(name));
        for name in ["HOME", "LAWN_KEEP", "HC_TERMINAL_ID", "PWD"] {
            assert!(keeps(name), "{name}");
        }
        for name in ["SHELLOPTS", "HOM", "LAWN", "BASH_FUNC_x%%", ""] {
            assert!(!keeps(name), "{name}");
        }
    }

    #[cfg(unix)]
    fn attach(
        core: &Arc<Core>,
        argv: &[&str],
        events: mpsc::Sender<AppEvent>,
    ) -> (String, PaneRuntime) {
        let address = core
            .run(json!({"argv": argv, "no_parent": true}))
            .expect("run");
        let attached = core
            .attach(json!({"terminal": address, "cols": 80, "rows": 24}))
            .expect("attach");
        let runtime = PaneRuntime::attach_core(
            PaneId::from_raw(7),
            core.clone(),
            attached,
            24,
            80,
            0,
            crate::terminal_theme::TerminalTheme::default(),
            None,
            events,
            Arc::new(Notify::new()),
            Arc::new(RenderSignal::new()),
            None,
        )
        .expect("attach_core");
        (address, runtime)
    }

    #[cfg(unix)]
    async fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !ready() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    #[cfg(unix)]
    fn attach_started(core: &Arc<Core>, cmd: &CommandBuilder) -> PaneRuntime {
        let attached = start(core, PaneId::from_raw(7), cmd, 24, 80).expect("start");
        let (events, _event_rx) = mpsc::channel(8);
        PaneRuntime::attach_core(
            PaneId::from_raw(7),
            core.clone(),
            attached,
            24,
            80,
            0,
            crate::terminal_theme::TerminalTheme::default(),
            None,
            events,
            Arc::new(Notify::new()),
            Arc::new(RenderSignal::new()),
            None,
        )
        .expect("attach_core")
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn core_pane_shows_a_program_that_ended_before_the_attach() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        let address = test
            .core
            .run(json!({"argv": ["sh", "-c", "echo done-early"], "no_parent": true}))
            .expect("run");
        test.core
            .call("terminal.wait", json!({"terminal": address}))
            .expect("wait for the exit");
        let attached = attach_new(&test.core, &address, 24, 80).expect("attach");
        let (events, mut event_rx) = mpsc::channel(8);
        let runtime = PaneRuntime::attach_core(
            PaneId::from_raw(7),
            test.core.clone(),
            attached,
            24,
            80,
            0,
            crate::terminal_theme::TerminalTheme::default(),
            None,
            events,
            Arc::new(Notify::new()),
            Arc::new(RenderSignal::new()),
            None,
        )
        .expect("attach_core");
        let died = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Some(AppEvent::PaneDied { exit_reason, .. }) = event_rx.recv().await {
                    break exit_reason;
                }
            }
        })
        .await
        .expect("PaneDied");
        assert_eq!(died, crate::platform::ChildExitReason::Exited);
        assert!(runtime.visible_text().contains("done-early"));
        // Typing into the ended pane goes nowhere and must not upset the core.
        runtime.try_send_bytes(Bytes::from_static(b"hi\r")).unwrap();
        drop(runtime);
        test.core
            .call("terminal.read", json!({"terminal": address}))
            .expect("the core still answers");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn core_pane_starts_with_a_path_that_lacks_env() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "echo path-ok; read line"]);
        cmd.env("PATH", "/nonexistent");
        let runtime = attach_started(&test.core, &cmd);
        wait_for("the program", || runtime.visible_text().contains("path-ok")).await;
        runtime.shutdown();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn core_pane_gets_only_the_env_lawn_built() {
        // The core starts with these; Lawn's own environment never has them.
        std::env::set_var("LAWN_TEST_CORE_ONLY", "/wrong/repository/.git");
        std::env::set_var("CLAUDECODE", "1");
        // A shell that inherits this stops at the first failing command, and
        // bash won't let a script unset it.
        std::env::set_var("SHELLOPTS", "errexit");
        let test = crate::hc::test_support::TestCore::start();
        std::env::remove_var("LAWN_TEST_CORE_ONLY");
        std::env::remove_var("CLAUDECODE");
        std::env::remove_var("SHELLOPTS");
        let Some(test) = test else {
            return;
        };
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args([
            "-c",
            "false; echo \"core=${LAWN_TEST_CORE_ONLY-unset} agent=${CLAUDECODE-unset} \
             kept=$LAWN_KEEP spaced=$LAWN_SPACED id=${HC_TERMINAL_ID:+set} \
             pwd=${PWD:+set}\"; \
             read line",
        ]);
        cmd.env("LAWN_KEEP", "yes");
        // A value shaped like other variables arrives whole.
        cmd.env("LAWN_SPACED", "a LAWN_KEEP=no\nPWD=x");
        let runtime = attach_started(&test.core, &cmd);
        wait_for("the program", || runtime.visible_text().contains("pwd=")).await;
        let text = runtime.visible_text();
        for part in [
            "core=unset",
            "agent=unset",
            "kept=yes",
            "spaced=a LAWN_KEEP=no",
            "id=set",
            "pwd=set",
        ] {
            assert!(text.contains(part), "{part} missing from {text:?}");
        }
        runtime.shutdown();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn core_pane_follows_a_cd_the_shell_does_not_report() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        let start_dir = std::env::current_dir().unwrap();
        let mut cmd = CommandBuilder::new("/bin/sh");
        // `sh` sends no OSC 7; only the core sees it change directory.
        cmd.args(["-c", "sleep 1; cd /; exec sleep 30"]);
        cmd.cwd(&start_dir);
        let runtime = attach_started(&test.core, &cmd);
        wait_for("the cd", || {
            runtime.cwd().as_deref() == Some(std::path::Path::new("/"))
        })
        .await;
        assert_eq!(
            runtime.cwd_for_persistence().as_deref(),
            Some(std::path::Path::new("/"))
        );
        runtime.shutdown();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn core_pane_shows_output_takes_input_and_detaches_on_drop() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        let (events, _event_rx) = mpsc::channel(8);
        let (address, runtime) = attach(
            &test.core,
            &[
                "sh",
                "-c",
                "echo ready; read line; echo got-$line; sleep 30",
            ],
            events,
        );
        wait_for("the prompt", || runtime.visible_text().contains("ready")).await;
        runtime.try_send_bytes(Bytes::from_static(b"hi\r")).unwrap();
        wait_for("the echo", || runtime.visible_text().contains("got-hi")).await;

        drop(runtime);
        // The program outlives the pane, even after a kill would have landed.
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(
            test.core
                .call("terminal.get", json!({"terminal": address}))
                .expect("get")["state"],
            "live"
        );
        test.core
            .attach(json!({"terminal": address, "cols": 80, "rows": 24}))
            .expect("still running after the pane is gone");
        test.core
            .call("terminal.kill", json!({"terminal": address}))
            .expect("kill");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn core_pane_keeps_keys_out_of_a_pending_submission() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        let (events, _event_rx) = mpsc::channel(8);
        let (address, runtime) = attach(
            &test.core,
            &[
                "sh",
                "-c",
                "stty -echo; echo ready; read a; read b; echo got-$a-$b; sleep 30",
            ],
            events,
        );
        wait_for("the prompt", || runtime.visible_text().contains("ready")).await;
        let submitted = runtime
            .queue_user_input_submission(
                Bytes::from_static(b"one"),
                Bytes::from_static(b"\r"),
                std::time::Duration::from_millis(150),
                None,
            )
            .unwrap();
        runtime
            .try_send_bytes(Bytes::from_static(b"two\r"))
            .unwrap();
        wait_for("both lines", || {
            runtime.visible_text().contains("got-one-two")
        })
        .await;
        submitted.recv().unwrap().unwrap();
        test.core
            .call("terminal.kill", json!({"terminal": address}))
            .expect("kill");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn core_pane_shows_saved_history_before_the_program() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        let address = test
            .core
            .run(json!({"argv": ["sh", "-c", "echo fresh; sleep 30"], "no_parent": true}))
            .expect("run");
        let attached = test
            .core
            .attach(json!({"terminal": address, "cols": 80, "rows": 24}))
            .expect("attach");
        let (events, _event_rx) = mpsc::channel(8);
        let runtime = PaneRuntime::attach_core(
            PaneId::from_raw(7),
            test.core.clone(),
            attached,
            24,
            80,
            1 << 20,
            crate::terminal_theme::TerminalTheme::default(),
            None,
            events,
            Arc::new(Notify::new()),
            Arc::new(RenderSignal::new()),
            Some("saved-line\r\n"),
        )
        .expect("attach_core");
        wait_for("the program", || runtime.visible_text().contains("fresh")).await;
        assert!(runtime
            .recent_text_snapshot(100)
            .text
            .contains("saved-line"));
        runtime.shutdown();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reattached_core_pane_shows_the_cores_history() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        let address = test
            .core
            .run(json!({
                "argv": ["sh", "-c", "seq 1 100; echo done; sleep 30"],
                "no_parent": true,
            }))
            .expect("run");
        wait_for("the program", || {
            test.core
                .call("terminal.read", json!({"terminal": address}))
                .is_ok_and(|read| read.to_string().contains("done"))
        })
        .await;
        let attached = test
            .core
            .attach(reattach_params(&address, 24, 80))
            .expect("attach");
        let (events, _event_rx) = mpsc::channel(8);
        let runtime = PaneRuntime::attach_core(
            PaneId::from_raw(7),
            test.core.clone(),
            attached,
            24,
            80,
            1 << 20,
            crate::terminal_theme::TerminalTheme::default(),
            None,
            events,
            Arc::new(Notify::new()),
            Arc::new(RenderSignal::new()),
            None,
        )
        .expect("attach_core");
        wait_for("the screen", || runtime.visible_text().contains("done")).await;
        let history = runtime.recent_text_snapshot(200).text;
        assert!(history.lines().any(|line| line.trim() == "1"), "{history}");
        runtime.shutdown();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reattaching_a_finished_or_unknown_terminal_says_it_is_gone() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        let address = test
            .core
            .run(json!({"argv": ["true"], "no_parent": true, "keep": true}))
            .expect("run");
        wait_for("the program to finish", || {
            test.core
                .call("terminal.get", json!({"terminal": address}))
                .is_ok_and(|record| record["state"] == "finished")
        })
        .await;
        for address in [address.as_str(), "t99"] {
            let err = test
                .core
                .attach(reattach_params(address, 24, 80))
                .map(|_| ())
                .expect_err("attach");
            assert_eq!(
                reattach_error(err).kind(),
                std::io::ErrorKind::NotFound,
                "{address}"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn closing_a_core_pane_ends_its_program() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        let (events, _event_rx) = mpsc::channel(8);
        let (address, runtime) = attach(&test.core, &["sleep", "30"], events);
        runtime.shutdown();
        wait_for("the program to end", || {
            test.core
                .call("terminal.get", json!({"terminal": address}))
                .is_ok_and(|record| record["state"] == "finished")
        })
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn core_pane_dies_when_its_program_exits() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        let (events, mut event_rx) = mpsc::channel(8);
        let (_address, _runtime) = attach(&test.core, &["sh", "-c", "sleep 0.2"], events);
        let died = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Some(AppEvent::PaneDied { exit_reason, .. }) = event_rx.recv().await {
                    break exit_reason;
                }
            }
        })
        .await
        .expect("PaneDied");
        assert_eq!(died, crate::platform::ChildExitReason::Exited);
    }
}
