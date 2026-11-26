//! One framed connection to herdr-core through `hc connect`: JSON-RPC on
//! channel 0, terminal byte streams and event streams on their own channels.
//!
//! `hc connect` finds (and on Linux starts) the user's core the way the
//! herdr apps do, and `ssh box hc connect` speaks the same protocol.

// The first callers are core-backed panes, which land next.
#![allow(dead_code)]

pub(crate) mod frame;

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc as tokio_mpsc;

/// The core API generation Lawn speaks.
pub(crate) const GENERATION: u64 = 1;
/// Methods Lawn calls; a core without one of them can't run it.
pub(crate) const USES: &[&str] = &[
    "terminal.run",
    "terminal.attach",
    "terminal.get",
    "terminal.label",
    "doc.get",
    "doc.set",
    "events.subscribe",
];
/// Overrides the `hc` binary, for tests and unusual installs.
pub(crate) const HC_BIN_ENV: &str = "LAWN_HC_BIN";
/// Keeps panes on Lawn's own PTYs. The test recipes set it until the PTY
/// backend goes, so tests never start terminals in a real core.
pub(crate) const PTY_PANES_ENV: &str = "LAWN_PTY_PANES";
const INSTALL_HINT: &str = "curl -fsSL https://releases.herdr.dev/core/install.sh | sh";
/// Credit granted to an event stream; events are small and must not stall.
const EVENT_CREDIT: u32 = 1024 * 1024;
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// How much of `hc connect`'s stderr to keep for error messages.
const STDERR_CAP: usize = 64 * 1024;

#[derive(Debug)]
pub(crate) enum HcError {
    /// The request could not be submitted or the core could not be reached.
    Unavailable(String),
    /// No reply was received. The request may have taken effect. A deadline
    /// does not cancel it; wait on `pending` again to receive a late reply.
    Unknown {
        pending: Box<PendingRequest>,
        reason: UnknownReason,
    },
    /// The core answered with an error, such as `conflict` or `not_found`.
    Rpc {
        method: String,
        kind: Option<String>,
        message: String,
        hint: Option<String>,
    },
    /// The core is too old or too new for this Lawn.
    Incompatible(String),
}

impl HcError {
    pub(crate) fn kind(&self) -> Option<&str> {
        match self {
            HcError::Rpc { kind, .. } => kind.as_deref(),
            _ => None,
        }
    }

    fn rpc(method: &str, error: &Value) -> Self {
        let text = |pointer: &str| {
            error
                .pointer(pointer)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        HcError::Rpc {
            method: method.to_owned(),
            kind: text("/data/kind"),
            message: text("/message").unwrap_or_else(|| "core call failed".to_owned()),
            hint: text("/data/hint"),
        }
    }
}

impl std::fmt::Display for HcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HcError::Unavailable(message) | HcError::Incompatible(message) => f.write_str(message),
            HcError::Unknown { pending, reason } => write!(
                f,
                "{}: outcome unknown ({reason}); the request may have taken effect",
                pending.method
            ),
            HcError::Rpc {
                method,
                message,
                hint,
                ..
            } => {
                write!(f, "{method}: {message}")?;
                if let Some(hint) = hint {
                    write!(f, " ({hint})")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for HcError {}

pub(crate) type HcResult<T> = Result<T, HcError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnknownReason {
    Deadline,
    Disconnected,
}

impl std::fmt::Display for UnknownReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Deadline => "no reply before the deadline",
            Self::Disconnected => "connection lost before the reply",
        })
    }
}

/// One submitted request. Waiting does not resubmit it. Dropping it stops
/// waiting and closes an unclaimed stream, but does not undo its effects.
pub(crate) struct PendingRequest {
    id: u64,
    method: String,
    reply: Mutex<mpsc::Receiver<Result<Value, Value>>>,
    routes: Arc<Mutex<Routes>>,
    writer: mpsc::Sender<Outgoing>,
    claimed: bool,
}

impl std::fmt::Debug for PendingRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingRequest")
            .field("id", &self.id)
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}

impl PendingRequest {
    /// A timeout returns this handle in `HcError::Unknown`; it can be waited
    /// on again. A disconnect cannot be resolved through this connection.
    pub(crate) fn wait(mut self, timeout: Duration) -> HcResult<Value> {
        let received = lock(&self.reply).recv_timeout(timeout);
        match received {
            Ok(reply) => {
                self.claimed = true;
                reply.map_err(|error| HcError::rpc(&self.method, &error))
            }
            Err(error) => Err(HcError::Unknown {
                pending: Box::new(self),
                reason: match error {
                    mpsc::RecvTimeoutError::Timeout => UnknownReason::Deadline,
                    mpsc::RecvTimeoutError::Disconnected => UnknownReason::Disconnected,
                },
            }),
        }
    }
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        if self.claimed {
            return;
        }
        let mut routes = lock(&self.routes);
        routes.replies.remove(&self.id);
        if let Ok(Ok(result)) = lock(&self.reply).try_recv() {
            abandon_reply(&mut routes, &self.writer, &result);
        }
    }
}

/// What arrives on an attach or event channel.
#[derive(Debug, PartialEq)]
pub(crate) enum Stream {
    Data(Vec<u8>),
    /// The core resized a terminal this observer watches.
    Resize {
        cols: u16,
        rows: u16,
    },
    /// The core ended the stream; the payload says why (`{"reason":"exited",…}`).
    Closed(Value),
    /// The connection to the core dropped. Programs keep running; reconnect.
    Lost,
}

struct Route {
    tx: tokio_mpsc::UnboundedSender<Stream>,
    /// Both ends support AUTOMATIC_INPUT on this channel.
    automatic_input: bool,
}

#[derive(Default)]
struct Routes {
    closed: bool,
    replies: HashMap<u64, mpsc::Sender<Result<Value, Value>>>,
    streams: HashMap<u32, Route>,
    /// Frames for a channel whose attach or subscribe reply isn't handled yet.
    early: HashMap<u32, Vec<Stream>>,
    /// Channels we closed whose frames still drain until the core's CLOSE.
    detached: HashSet<u32>,
}

/// The channel of a stream that had already ended when it was opened. The
/// core never knew it, so nothing is sent on it.
const ENDED_CHANNEL: u32 = u32::MAX;

/// History shown above the last screen of a program that already ended.
const ENDED_HISTORY_LINES: u32 = 1000;

pub(crate) struct Attached {
    pub(crate) channel: u32,
    pub(crate) stream: tokio_mpsc::UnboundedReceiver<Stream>,
    /// The terminal record as the core resolved it.
    pub(crate) terminal: Value,
}

pub(crate) struct Subscription {
    pub(crate) channel: u32,
    pub(crate) stream: tokio_mpsc::UnboundedReceiver<Stream>,
    /// `seq` and `terminals` to apply the events to.
    pub(crate) snapshot: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Doc {
    pub(crate) value: Value,
    pub(crate) version: u64,
}

/// A frame for the core, and who to tell once it's written.
type Outgoing = (Vec<u8>, Option<mpsc::Sender<()>>);

pub(crate) struct Core {
    /// The `hello` result: generation, version, methods, features.
    pub(crate) hello: Value,
    writer: mpsc::Sender<Outgoing>,
    routes: Arc<Mutex<Routes>>,
    next_id: AtomicU64,
    child: Child,
}

fn hc_bin() -> std::ffi::OsString {
    std::env::var_os(HC_BIN_ENV)
        .filter(|bin| !bin.is_empty())
        .unwrap_or_else(|| "hc".into())
}

static SHARED: Mutex<Option<Arc<Core>>> = Mutex::new(None);

/// Whether this Lawn runs its panes in herdr core, which it does wherever
/// the platform can.
#[cfg(not(test))]
pub(crate) fn enabled() -> bool {
    crate::platform::capabilities().core_panes && std::env::var_os(PTY_PANES_ENV).is_none()
}

/// Unit tests stay on PTYs unless they opt in with `use_core_for_test`.
#[cfg(test)]
pub(crate) fn enabled() -> bool {
    TEST_CORE_PANES.load(Ordering::Relaxed)
}

#[cfg(test)]
static TEST_CORE_PANES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Runs this test process's panes in a core. Nextest gives each test its
/// own process.
#[cfg(test)]
pub(crate) fn use_core_for_test() {
    TEST_CORE_PANES.store(true, Ordering::Relaxed);
}

/// Whether `hc` starts.
pub(crate) fn installed() -> bool {
    crate::noninteractive_process::command(hc_bin())
        .arg("--version")
        .output()
        .is_ok()
}

/// Fails with install help when panes need a core and `hc` won't start.
pub(crate) fn require_installed() -> io::Result<()> {
    if !enabled() || installed() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "Lawn runs its panes in herdr core, but `{}` was not found.\nInstall it with: {INSTALL_HINT}",
            hc_bin().to_string_lossy()
        ),
    ))
}

/// The core panes run in, when they run in one. Reconnects after the
/// connection drops.
pub(crate) fn backend() -> Option<HcResult<Arc<Core>>> {
    if !enabled() {
        return None;
    }
    let mut shared = SHARED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(core) = shared.as_ref().filter(|core| !core.is_closed()) {
        return Some(Ok(core.clone()));
    }
    let core = Core::connect();
    *shared = core.as_ref().ok().cloned();
    Some(core)
}

fn lock<T>(routes: &Mutex<T>) -> MutexGuard<'_, T> {
    routes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One field of a reply, or an error naming what the core left out.
fn field<T>(
    method: &str,
    result: &Value,
    key: &str,
    convert: impl FnOnce(&Value) -> Option<T>,
) -> HcResult<T> {
    result
        .get(key)
        .and_then(convert)
        .ok_or_else(|| HcError::Unavailable(format!("{method} returned no {key}")))
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(crate) struct Machine {
    pub(crate) name: String,
    pub(crate) id: String,
    pub(crate) state: String,
}

pub(crate) fn core_id<'a>(cores: &'a [Machine], prefix: &'a str) -> Result<&'a str, String> {
    let named = |prefix: &'a str| {
        let named = move |core: &&Machine| core.name == prefix || core.id == prefix;
        cores.iter().filter(named)
    };
    let mut reached = named(prefix);
    let id = reached.next().map_or(prefix, |core| &core.id);
    if reached.next().is_some() {
        return Err(format!("{prefix} is the name of one core and the core id of another; rename one with `hc machine rename`"));
    }
    Ok(if named(id).nth(1).is_some() {
        prefix
    } else {
        id
    })
}

pub(crate) fn exact_core_id(cores: &[Machine], id: &str) -> Result<(), String> {
    if cores.iter().any(|core| core.name == id && core.id != id) {
        return Err(format!(
            "{id} is the name of a machine, so it reaches that machine and not the one with this core id"
        ));
    }
    if cores.iter().any(|core| core.id == id) {
        return Ok(());
    }
    Err(format!("no linked machine has the core id {id}"))
}

pub(crate) fn located(address: &str) -> Result<(Option<&str>, &str), String> {
    match address.split_once('/') {
        Some((_, local)) if local.contains('/') => {
            Err(format!("{address} is more than one machine away"))
        }
        Some((machine, local)) => Ok((Some(machine), local)),
        None => Ok((None, address)),
    }
}

pub(crate) fn qualified(machine: Option<&str>, local: &str) -> String {
    match machine {
        Some(machine) => format!("{machine}/{local}"),
        None => local.to_owned(),
    }
}

/// Whether a core's `hello` says it can serve Lawn.
pub(crate) fn check_hello(hello: &Value) -> HcResult<()> {
    let generation = hello.get("generation").and_then(Value::as_u64).unwrap_or(0);
    if generation != GENERATION {
        return Err(HcError::Incompatible(format!(
            "herdr-core speaks API generation {generation}; this Lawn speaks {GENERATION}"
        )));
    }
    let methods: Vec<&str> = hello
        .get("methods")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let missing: Vec<&str> = USES
        .iter()
        .copied()
        .filter(|m| !methods.contains(m))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(HcError::Incompatible(format!(
            "herdr-core lacks {}; run `hc update`",
            missing.join(", ")
        )))
    }
}

/// The core's own error, as `hc connect` prints it on stderr after any
/// warnings.
fn connect_error(stderr: &str) -> Option<String> {
    let reply: Value = stderr
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line.trim()).ok())?;
    let error = reply.get("error")?;
    let message = error.get("message")?.as_str()?;
    Some(
        match error
            .get("hint")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            Some(hint) => format!("{message} ({hint})"),
            None => message.to_owned(),
        },
    )
}

/// Sends `hello` in plain mode and returns its checked result.
fn handshake(stdin: &mut impl Write, reader: &mut impl BufRead) -> HcResult<Value> {
    let mut request = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "hello",
        "params": {"framed": true, "generation": GENERATION},
    })
    .to_string();
    request.push('\n');
    let mut line = String::new();
    stdin
        .write_all(request.as_bytes())
        .and_then(|()| stdin.flush())
        .and_then(|()| reader.read_line(&mut line))
        .map_err(|error| HcError::Unavailable(format!("herdr-core did not answer: {error}")))?;
    let mut reply: Value = serde_json::from_str(&line)
        .map_err(|error| HcError::Unavailable(format!("herdr-core did not answer: {error}")))?;
    if let Some(error) = reply.get("error") {
        return Err(HcError::rpc("hello", error));
    }
    let hello = reply.get_mut("result").map(Value::take).unwrap_or_default();
    check_hello(&hello)?;
    Ok(hello)
}

impl Core {
    /// Connects to the user's core and switches to framed mode.
    pub(crate) fn connect() -> HcResult<Arc<Core>> {
        Self::connect_with(
            crate::noninteractive_process::command(hc_bin()),
            CALL_TIMEOUT,
        )
    }

    fn connect_with(mut command: Command, hello_timeout: Duration) -> HcResult<Arc<Core>> {
        let unavailable =
            |what: &str, error: io::Error| HcError::Unavailable(format!("{what}: {error}"));
        let mut child = command
            .arg("connect")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                unavailable("cannot run `hc connect`; is herdr-core installed?", error)
            })?;
        let (Some(stdin), Some(stdout), Some(mut stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(HcError::Unavailable("`hc connect` has no pipes".to_owned()));
        };
        let (stderr_tx, stderr_rx) = mpsc::channel();
        let stderr_thread = crate::thread_spawn::spawn_named("hc-stderr", move || {
            // Keep draining so a chatty core never blocks on a full pipe.
            let mut captured = Vec::new();
            let mut buffer = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                let room = STDERR_CAP.saturating_sub(captured.len());
                captured.extend_from_slice(&buffer[..n.min(room)]);
            }
            let _ = stderr_tx.send(String::from_utf8_lossy(&captured).into_owned());
        });

        // A blocked read can't be cancelled, so the handshake runs on a thread
        // and a core that never answers is killed, which ends the read.
        let (handshake_tx, handshake_rx) = mpsc::channel();
        let handshake_thread = stderr_thread.and_then(|_| {
            crate::thread_spawn::spawn_named("hc-hello", move || {
                let mut stdin = stdin;
                let mut reader = BufReader::new(stdout);
                let hello = handshake(&mut stdin, &mut reader);
                let _ = handshake_tx.send((hello, stdin, reader));
            })
        });
        let shaken = match handshake_thread {
            Err(error) => Err(unavailable("cannot start a thread", error)),
            Ok(_) => handshake_rx
                .recv_timeout(hello_timeout)
                .map_err(|_| HcError::Unavailable("herdr-core did not answer `hello`".to_owned())),
        };
        let (hello, stdin, reader) = match shaken {
            Ok((Ok(hello), stdin, reader)) => (hello, stdin, reader),
            Ok((Err(error), ..)) | Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                // An I/O failure usually means `hc connect` explained itself on stderr.
                if let HcError::Unavailable(_) = error {
                    let stderr = stderr_rx
                        .recv_timeout(Duration::from_secs(1))
                        .unwrap_or_default();
                    if let Some(message) = connect_error(&stderr) {
                        return Err(HcError::Unavailable(message));
                    }
                }
                return Err(error);
            }
        };

        // A core that stops reading never blocks the caller.
        let (writer, frames) = mpsc::channel();
        let routes = Arc::new(Mutex::new(Routes::default()));
        let reader_routes = routes.clone();
        let reader_writer = writer.clone();
        let threads =
            crate::thread_spawn::spawn_named("hc-writer", move || write_loop(stdin, &frames))
                .and_then(|_| {
                    crate::thread_spawn::spawn_named("hc-reader", move || {
                        read_loop(reader, &reader_routes, &reader_writer)
                    })
                });
        if let Err(error) = threads {
            let _ = child.kill();
            let _ = child.wait();
            return Err(unavailable("cannot start a thread", error));
        }
        Ok(Arc::new(Core {
            hello,
            writer,
            routes,
            next_id: AtomicU64::new(1),
            child,
        }))
    }

    pub(crate) fn is_closed(&self) -> bool {
        lock(&self.routes).closed
    }

    pub(crate) fn has_feature(&self, feature: &str) -> bool {
        self.hello
            .get("features")
            .and_then(Value::as_array)
            .is_some_and(|features| features.iter().any(|f| f == feature))
    }

    /// Every frame leaves through here. The core closes the whole connection
    /// on a frame over its limit, so stream bytes go out in pieces; `call`
    /// rejects an oversized request before it gets here.
    fn send(&self, channel: u32, kind: u8, payload: &[u8]) {
        self.queue(channel, kind, payload, None);
    }

    /// Queues `payload` in pieces; `done` hears back once the last is
    /// written. False when the writer is gone.
    fn queue(
        &self,
        channel: u32,
        kind: u8,
        payload: &[u8],
        mut done: Option<mpsc::Sender<()>>,
    ) -> bool {
        if channel == ENDED_CHANNEL {
            return false;
        }
        let mut pieces = frame::pieces(payload).peekable();
        while let Some(piece) = pieces.next() {
            let last = pieces.peek().is_none();
            let ack = if last { done.take() } else { None };
            if self
                .writer
                .send((frame::encode(channel, kind, piece), ack))
                .is_err()
            {
                return false;
            }
        }
        true
    }

    /// Submits once and waits for the usual deadline. An unknown outcome
    /// retains the request handle; it is not evidence the operation failed.
    pub(crate) fn call(&self, method: &str, params: Value) -> HcResult<Value> {
        self.request(method, params)?.wait(CALL_TIMEOUT)
    }

    /// Submits a request without waiting. Validation and a closed connection
    /// fail before submission. Once queued, a lost reply is an unknown outcome.
    pub(crate) fn request(&self, method: &str, params: Value) -> HcResult<PendingRequest> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = request_payload(id, method, params)?;
        let frame = frame::encode(0, frame::DATA, request.as_bytes());
        let (tx, rx) = mpsc::channel();
        let mut routes = lock(&self.routes);
        if routes.closed {
            return Err(HcError::Unavailable(format!("{method}: lost the core")));
        }
        routes.replies.insert(id, tx);
        if self.writer.send((frame, None)).is_err() {
            routes.replies.remove(&id);
            return Err(HcError::Unavailable(format!(
                "{method}: could not submit the request"
            )));
        }
        drop(routes);
        Ok(PendingRequest {
            id,
            method: method.to_owned(),
            reply: Mutex::new(rx),
            routes: self.routes.clone(),
            writer: self.writer.clone(),
            claimed: false,
        })
    }

    /// Registers a stream channel, delivering frames that beat its reply.
    fn route(&self, channel: u32, automatic_input: bool) -> tokio_mpsc::UnboundedReceiver<Stream> {
        let (tx, rx) = tokio_mpsc::unbounded_channel();
        let mut routes = lock(&self.routes);
        let mut ended = routes.closed;
        for item in routes.early.remove(&channel).unwrap_or_default() {
            ended |= matches!(item, Stream::Closed(_));
            let _ = tx.send(item);
        }
        if routes.closed {
            let _ = tx.send(Stream::Lost);
        } else if !ended {
            routes.streams.insert(
                channel,
                Route {
                    tx,
                    automatic_input,
                },
            );
        }
        rx
    }

    fn channel_of(method: &str, result: &Value) -> HcResult<u32> {
        field(method, result, "channel", |channel| {
            channel.as_u64().and_then(|c| u32::try_from(c).ok())
        })
    }

    /// Starts a program in a new terminal and returns its address (`t7`).
    /// `params` follows `terminal.run` in the core schema.
    pub(crate) fn run(&self, params: Value) -> HcResult<String> {
        let result = self.call("terminal.run", params)?;
        field("terminal.run", &result, "address", |a| {
            a.as_str().map(str::to_owned)
        })
    }

    /// Opens a stream to a live terminal. `params` follows `terminal.attach`.
    pub(crate) fn attach(&self, params: Value) -> HcResult<Attached> {
        let mut result = self.call("terminal.attach", params)?;
        let channel = Self::channel_of("terminal.attach", &result)?;
        let automatic_input = self.has_feature("automatic_input")
            && result.get("automatic_input") == Some(&Value::Bool(true));
        let stream = self.route(channel, automatic_input);
        Ok(Attached {
            channel,
            stream,
            terminal: result
                .get_mut("terminal")
                .map(Value::take)
                .unwrap_or_default(),
        })
    }

    /// A terminal whose program already ended, as the stream an attach would
    /// have given: what it left on screen, then the exit. `attach` refuses
    /// these with `finished`.
    pub(crate) fn finished(&self, address: &str) -> HcResult<Attached> {
        let read = self.call(
            "terminal.read",
            json!({"terminal": address, "lines": ENDED_HISTORY_LINES}),
        )?;
        let text = field("terminal.read", &read, "text", |text| {
            text.as_str().map(str::to_owned)
        })?
        .replace('\n', "\r\n");
        let (tx, stream) = tokio_mpsc::unbounded_channel();
        let _ = tx.send(Stream::Data(text.into_bytes()));
        let _ = tx.send(Stream::Closed(json!({"reason": "exited"})));
        Ok(Attached {
            channel: ENDED_CHANNEL,
            stream,
            terminal: json!({"address": address}),
        })
    }

    /// Follows terminal and document changes. `params` follows `events.subscribe`.
    pub(crate) fn subscribe(&self, params: Value) -> HcResult<Subscription> {
        let snapshot = self.call("events.subscribe", params)?;
        let channel = Self::channel_of("events.subscribe", &snapshot)?;
        let stream = self.route(channel, false);
        self.credit(channel, EVENT_CREDIT);
        Ok(Subscription {
            channel,
            stream,
            snapshot,
        })
    }

    pub(crate) fn machines(&self) -> HcResult<Vec<Machine>> {
        let result = self.call("machine.list", json!({}))?;
        field("machine.list", &result, "machines", |machines| {
            serde_json::from_value(machines.clone()).ok()
        })
    }

    pub(crate) fn cores(&self) -> HcResult<Vec<Machine>> {
        let text = |key: &str| self.hello["core"][key].as_str().unwrap_or_default();
        let mut cores = self.machines()?;
        cores.push(Machine {
            name: text("name").to_owned(),
            id: text("id").to_owned(),
            state: "connected".to_owned(),
        });
        Ok(cores)
    }

    pub(crate) fn holds(&self, machine: Option<&str>) -> HcResult<()> {
        let Some(id) = machine else {
            return Ok(());
        };
        exact_core_id(&self.cores()?, id).map_err(HcError::Unavailable)
    }

    pub(crate) fn doc_get(&self, name: &str, on: Option<&str>) -> HcResult<Doc> {
        let mut result = self.call("doc.get", json!({"name": name, "on": on}))?;
        let version = field("doc.get", &result, "version", Value::as_u64)?;
        Ok(Doc {
            value: result.get_mut("value").map(Value::take).unwrap_or_default(),
            version,
        })
    }

    /// Writes a document if `version` is still current and returns the new
    /// version. A stale version fails with kind `conflict`.
    pub(crate) fn doc_set(
        &self,
        name: &str,
        on: Option<&str>,
        value: &Value,
        version: u64,
    ) -> HcResult<u64> {
        let result = self.call(
            "doc.set",
            json!({"name": name, "on": on, "value": value, "version": version}),
        )?;
        field("doc.set", &result, "version", Value::as_u64)
    }

    /// Keys and pastes from a person.
    pub(crate) fn input(&self, channel: u32, bytes: &[u8]) {
        self.send(channel, frame::DATA, bytes);
    }

    /// Like `input`, but returns once the bytes are written to the core.
    pub(crate) fn write_input(&self, channel: u32, bytes: &[u8]) -> HcResult<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let lost = || HcError::Unavailable("input: lost the core".to_owned());
        let (done, written) = mpsc::channel();
        if !self.queue(channel, frame::DATA, bytes, Some(done)) {
            return Err(lost());
        }
        written.recv().map_err(|_| lost())
    }

    /// Replies the terminal engine generated. Paths without automatic-input
    /// support get them as plain input.
    pub(crate) fn automatic_input(&self, channel: u32, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let automatic = lock(&self.routes)
            .streams
            .get(&channel)
            .is_some_and(|route| route.automatic_input);
        let kind = if automatic {
            frame::AUTOMATIC_INPUT
        } else {
            frame::DATA
        };
        self.send(channel, kind, bytes);
    }

    pub(crate) fn credit(&self, channel: u32, bytes: u32) {
        self.send(channel, frame::CREDIT, &bytes.to_le_bytes());
    }

    pub(crate) fn resize(&self, channel: u32, cols: u16, rows: u16) {
        self.send(channel, frame::RESIZE, &frame::resize_payload(cols, rows));
    }

    pub(crate) fn focus(&self, channel: u32, focused: bool) {
        self.send(channel, frame::FOCUS, &[u8::from(focused)]);
    }

    /// Stops watching a terminal. The program keeps running.
    pub(crate) fn detach(&self, channel: u32) {
        {
            let mut routes = lock(&self.routes);
            if routes.streams.remove(&channel).is_some() {
                routes.detached.insert(channel);
            }
        }
        self.send(channel, frame::CLOSE, &[]);
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A request as one frame's payload, which can't be split.
fn request_payload(id: u64, method: &str, params: Value) -> HcResult<String> {
    let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    let request = request.to_string();
    if request.len() > frame::MAX_PAYLOAD {
        return Err(HcError::Unavailable(format!(
            "{method}: request is larger than the core accepts"
        )));
    }
    Ok(request)
}

fn deliver_reply(routes: &Mutex<Routes>, writer: &mpsc::Sender<Outgoing>, payload: &[u8]) {
    let Ok(mut message) = serde_json::from_slice::<Value>(payload) else {
        return;
    };
    let Some(id) = message.get("id").and_then(Value::as_u64) else {
        return;
    };
    let reply = match message.get_mut("error").map(Value::take) {
        Some(error) => Err(error),
        None => Ok(message
            .get_mut("result")
            .map(Value::take)
            .unwrap_or_default()),
    };
    let mut routes = lock(routes);
    let undelivered = match routes.replies.remove(&id) {
        Some(tx) => tx.send(reply).err().map(|err| err.0),
        None => Some(reply),
    };
    if let Some(Ok(result)) = undelivered {
        abandon_reply(&mut routes, writer, &result);
    }
}

fn abandon_reply(routes: &mut Routes, writer: &mpsc::Sender<Outgoing>, result: &Value) {
    let Some(channel) = result["channel"]
        .as_u64()
        .and_then(|channel| u32::try_from(channel).ok())
    else {
        return;
    };
    let early = routes.early.remove(&channel).unwrap_or_default();
    if !routes.closed && !early.iter().any(|item| matches!(item, Stream::Closed(_))) {
        routes.detached.insert(channel);
        let _ = writer.send((frame::encode(channel, frame::CLOSE, &[]), None));
    }
}

/// Writes frames in order; a frame's waiter hears back only once it's out,
/// and a failed write drops every waiter still queued.
fn write_loop(mut out: impl Write, frames: &mpsc::Receiver<Outgoing>) {
    while let Ok((frame, done)) = frames.recv() {
        if out.write_all(&frame).and_then(|()| out.flush()).is_err() {
            break;
        }
        if let Some(done) = done {
            let _ = done.send(());
        }
    }
}

fn read_loop(mut reader: impl Read, routes: &Mutex<Routes>, writer: &mpsc::Sender<Outgoing>) {
    loop {
        let frame = match frame::read(&mut reader) {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(error) => {
                tracing::warn!(%error, "herdr-core connection failed");
                break;
            }
        };
        if frame.channel == 0 {
            deliver_reply(routes, writer, &frame.payload);
            continue;
        }
        let item = match frame.kind {
            frame::DATA => Stream::Data(frame.payload),
            frame::RESIZE => match frame::parse_resize(&frame.payload) {
                Some((cols, rows)) => Stream::Resize { cols, rows },
                None => continue,
            },
            frame::CLOSE => {
                Stream::Closed(serde_json::from_slice(&frame.payload).unwrap_or(Value::Null))
            }
            _ => continue,
        };
        let closing = matches!(item, Stream::Closed(_));
        let mut r = lock(routes);
        if r.detached.contains(&frame.channel) {
            if closing {
                r.detached.remove(&frame.channel);
            }
            continue;
        }
        match r.streams.get(&frame.channel) {
            Some(route) => {
                let _ = route.tx.send(item);
            }
            None => r.early.entry(frame.channel).or_default().push(item),
        }
        if closing {
            r.streams.remove(&frame.channel);
        }
    }
    let mut r = lock(routes);
    r.closed = true;
    for (_, route) in r.streams.drain() {
        let _ = route.tx.send(Stream::Lost);
    }
    r.replies.clear();
}

// The live tests that use it run Unix shell lines.
#[cfg(all(test, unix))]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
