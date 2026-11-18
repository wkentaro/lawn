use std::io;
use std::os::fd::AsRawFd as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
use tokio::signal::unix::{signal, Signal, SignalKind};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::warn;

use super::view::{Event, Geometry, View};
use crate::client::terminal_setup::{set_mouse_capture, TerminalGuard};
use crate::client::HostResize;
use crate::input::TerminalKey;
use crate::raw_input::RawInputEvent;

/// The shortest time between two draws, so a burst of output draws once.
pub(super) const FRAME: Duration = Duration::from_millis(8);

/// The signals that end the attach, registered before the host terminal
/// changes so none of them can leave it in raw mode.
pub(super) struct Stop {
    interrupted: Signal,
    terminated: Signal,
    hung_up: Signal,
}

impl Stop {
    pub(super) fn register() -> io::Result<Self> {
        Ok(Self {
            interrupted: signal(SignalKind::interrupt())?,
            terminated: signal(SignalKind::terminate())?,
            hung_up: signal(SignalKind::hangup())?,
        })
    }

    pub(super) async fn recv(&mut self) {
        tokio::select! {
            _ = self.interrupted.recv() => {}
            _ = self.terminated.recv() => {}
            _ = self.hung_up.recv() => {}
        }
    }
}

pub(super) fn host_geometry() -> io::Result<Geometry> {
    let (cols, rows, cell_width_px, cell_height_px) = crate::client::host_terminal_geometry()?;
    Ok(Geometry {
        cols,
        rows,
        cell_width_px,
        cell_height_px,
    })
}

/// The host's sizes after `host`, from a thread that polls them as the
/// old client does. An error means the host terminal is gone.
pub(super) struct HostSizes {
    sizes: mpsc::UnboundedReceiver<io::Result<Geometry>>,
    quit: Arc<AtomicBool>,
}

impl HostSizes {
    pub(super) fn watch(host: Geometry) -> io::Result<Self> {
        let (tx, sizes) = mpsc::unbounded_channel();
        let quit = Arc::new(AtomicBool::new(false));
        let watching = quit.clone();
        crate::thread_spawn::spawn_named("lawn-resize", move || {
            crate::platform::watch_terminal_resize_signal();
            follow_host_size(
                host,
                host_geometry,
                crate::platform::take_terminal_resize_signal,
                &watching,
                &tx,
            );
        })?;
        Ok(Self { sizes, quit })
    }

    pub(super) async fn recv(&mut self) -> io::Result<Geometry> {
        self.sizes
            .recv()
            .await
            .unwrap_or_else(|| Err(io::Error::other("the resize watcher stopped")))
    }
}

impl Drop for HostSizes {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Release);
    }
}

/// Sends `tx` the host's size on each resize signal or change. A
/// zero-sized sample, which a window drag can produce, keeps the last
/// size and the pending signal until a valid sample follows.
pub(super) fn follow_host_size(
    host: Geometry,
    mut read: impl FnMut() -> io::Result<Geometry>,
    take_signal: impl FnMut() -> bool,
    quit: &AtomicBool,
    tx: &mpsc::UnboundedSender<io::Result<Geometry>>,
) {
    let sample = |geometry: Geometry| {
        (
            geometry.cols,
            geometry.rows,
            geometry.cell_width_px,
            geometry.cell_height_px,
            false,
        )
    };
    crate::client::resize_poll_loop_with(
        sample(host),
        |_| read().map(sample),
        take_signal,
        quit,
        |report| {
            tx.send(match report {
                HostResize::Resized((cols, rows, cell_width_px, cell_height_px, _)) => {
                    Ok(Geometry {
                        cols,
                        rows,
                        cell_width_px,
                        cell_height_px,
                    })
                }
                HostResize::Unavailable(error) => Err(error),
            })
            .is_ok()
        },
    );
}

/// How the host reports keys, which tells committed text from typed keys.
#[derive(Debug, Clone, Copy)]
pub(super) struct HostKeys {
    /// The host reports key releases, so it speaks the Kitty keyboard
    /// protocol.
    pub(super) kitty: bool,
    /// The host reports every key as an escape code.
    pub(super) all: bool,
    /// The host tty's erase character is `^H`, so 0x08 is Backspace.
    pub(super) erase_is_ctrl_h: bool,
}

impl HostKeys {
    pub(super) fn new(guard: &TerminalGuard) -> Self {
        Self {
            kitty: guard.host_escape_disambiguation_active(),
            all: false,
            erase_is_ctrl_h: crate::platform::terminal_erase_byte() == Some(0x08),
        }
    }

    /// The text an input method committed, which must reach the program
    /// as text rather than through the key encoder.
    pub(super) fn committed_text<'k>(&self, key: &'k TerminalKey) -> Option<&'k str> {
        self.all.then(|| key.committed_text(self.kitty)).flatten()
    }
}

pub(super) async fn wait(due: Option<Instant>) {
    match due {
        Some(due) => tokio::time::sleep_until(due).await,
        None => std::future::pending().await,
    }
}

/// Re-sends the input modes a host forgets when it resets, as after a
/// reconnect or a window reload. A focus report shows the host still
/// reports focus, so only a resize re-enables it.
pub(super) fn refresh_host_modes(
    guard: &TerminalGuard,
    mouse_capture: &AtomicBool,
    all_keys: bool,
    focus_reporting: bool,
) {
    if let Err(error) = set_mouse_capture(mouse_capture.load(Ordering::Acquire), false) {
        warn!(%error, "the host mouse mode was not refreshed");
    }
    if let Err(error) =
        guard.refresh_host_keyboard_protocol(&mut io::stdout(), all_keys, focus_reporting)
    {
        warn!(%error, "the host keyboard mode was not refreshed");
    }
}

/// Switches the host to reporting every key while the program wants
/// that, replacing the keyboard flags the guard pushed so its pop on
/// exit still restores the host.
pub(super) fn follow_key_reporting(
    wanted: bool,
    active: &mut bool,
    out: &mut impl io::Write,
) -> io::Result<()> {
    if *active != wanted {
        crate::terminal_modes::set_host_kitty_keyboard_report_all(out, wanted)?;
        *active = wanted;
    }
    Ok(())
}

pub(super) fn deliver(view: &View, keys: HostKeys, event: RawInputEvent) {
    match event {
        RawInputEvent::Key(key) => match keys.committed_text(&key) {
            Some(text) => view.text(text),
            None => view.key(key.with_host_erase(keys.erase_is_ctrl_h)),
        },
        RawInputEvent::Text(text) => view.text(text.as_str()),
        RawInputEvent::Paste(text) => view.paste(text),
        RawInputEvent::Mouse(mouse) => {
            view.mouse(mouse);
        }
        RawInputEvent::OuterFocusGained => {
            view.focus(true);
        }
        RawInputEvent::OuterFocusLost => {
            view.focus(false);
        }
        RawInputEvent::HostDefaultColor { .. }
        | RawInputEvent::HostPaletteColors { .. }
        | RawInputEvent::HostColorSchemeChanged(_)
        | RawInputEvent::HostCellSizeReport { .. }
        | RawInputEvent::Unsupported => {}
    }
}

/// Ctrl+\, the key `hc attach` detaches on.
pub(super) fn detaches(key: &TerminalKey) -> bool {
    let key = key.as_key_event();
    key.kind != KeyEventKind::Release
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char('\\')
}

pub(super) fn read_input(
    initial: Vec<u8>,
    escape_disambiguation: bool,
    mouse_capture: Arc<AtomicBool>,
) -> io::Result<mpsc::UnboundedReceiver<Vec<RawInputEvent>>> {
    let (tx, rx) = mpsc::unbounded_channel();
    crate::thread_spawn::spawn_named("lawn-input", move || {
        let send = |chunks: Vec<Vec<u8>>| {
            let events: Vec<RawInputEvent> = chunks
                .iter()
                .flat_map(|chunk| crate::raw_input::parse_framed_input(chunk))
                .collect();
            events.is_empty() || tx.send(events).is_ok()
        };
        let fd = io::stdin().as_raw_fd();
        let mut framer = crate::raw_input::RawInputByteFramer::for_host_input();
        framer.set_host_escape_disambiguation_active(escape_disambiguation);
        if !send(framer.push(&initial)) {
            return;
        }
        let mut scratch = [0u8; 4096];
        let mut held = false;
        loop {
            if framer.has_pending_input() {
                let timeout = if held {
                    framer.held_input_flush_timeout_ms()
                } else {
                    framer.idle_flush_timeout_ms(mouse_capture.load(Ordering::Acquire))
                };
                match crate::platform::poll_fd_readable(fd, timeout) {
                    Ok(true) => {}
                    Ok(false) => {
                        let chunks = framer.flush_timeout();
                        held = chunks.is_empty() && framer.has_pending_input();
                        if !send(chunks) {
                            return;
                        }
                        continue;
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => return,
                }
            }
            match crate::platform::read_fd(fd, &mut scratch) {
                Ok(0) => return,
                Ok(read) => {
                    held = false;
                    if !send(framer.push(&scratch[..read])) {
                        return;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return,
            }
        }
    })?;
    Ok(rx)
}

pub(super) fn enter(mouse_capture: bool) -> io::Result<TerminalGuard> {
    let guard = crate::client::terminal_setup::setup_terminal(mouse_capture)?;
    let restore = guard.panic_restore();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        hook(info);
    }));
    Ok(guard)
}

pub(super) fn clipboard(
) -> io::Result<(mpsc::UnboundedSender<Vec<u8>>, mpsc::UnboundedReceiver<()>)> {
    let (copy, mut copies) = mpsc::unbounded_channel::<Vec<u8>>();
    let (done, copied) = mpsc::unbounded_channel();
    crate::thread_spawn::spawn_named("lawn-clipboard", move || {
        while let Some(bytes) = copies.blocking_recv() {
            if crate::selection::write_osc52_bytes(&bytes) && done.send(()).is_err() {
                return;
            }
        }
    })?;
    Ok((copy, copied))
}

pub(super) fn relay(event: Event, out: &mut impl io::Write) {
    match event {
        Event::Copy(content) => {
            crate::selection::write_osc52_bytes(&content);
        }
        Event::Bell(count) => {
            if let Err(error) = crate::terminal_effects::write_terminal_bells(out, count) {
                warn!(%error, "the bell did not reach the host");
            }
        }
        Event::Ended(_) | Event::Detached(_) => {}
    }
}

#[cfg(test)]
mod tests;
