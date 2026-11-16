use std::io;

/// Runs `lawn attach <TERMINAL>`, or returns `None` so the old CLI handles
/// the arguments.
pub(crate) fn maybe_run(args: &[String]) -> Option<io::Result<()>> {
    let [command, terminal] = args.get(1..)? else {
        return None;
    };
    if command != "attach" || terminal.starts_with('-') {
        return None;
    }
    Some(run(terminal))
}

#[cfg(not(unix))]
fn run(_terminal: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "lawn attach needs herdr core, which this platform lacks",
    ))
}

#[cfg(unix)]
fn run(terminal: &str) -> io::Result<()> {
    use std::io::IsTerminal as _;

    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other("lawn attach needs a terminal"));
    }
    let core = crate::hc::backend()
        .unwrap_or_else(crate::hc::Core::connect)
        .map_err(io::Error::other)?;
    let geometry = unix::host_geometry()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let stop = unix::Stop::register()?;
        let view = super::view::View::attach(core, terminal, geometry)?;
        let mut guard = crate::client::terminal_setup::setup_terminal(false)?;
        let restore = guard.panic_restore();
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            hook(info);
        }));
        unix::show(view, geometry, &mut guard, stop).await
    })
}

#[cfg(unix)]
mod unix {
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

    use super::super::screen::Screen;
    use super::super::view::{Event, Geometry, View};
    use crate::client::terminal_setup::{set_mouse_capture, TerminalGuard};
    use crate::client::HostResize;
    use crate::input::TerminalKey;
    use crate::raw_input::RawInputEvent;

    /// The shortest time between two draws, so a burst of output draws once.
    const FRAME: Duration = Duration::from_millis(8);

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

        async fn recv(&mut self) {
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
    struct HostSizes {
        sizes: mpsc::UnboundedReceiver<io::Result<Geometry>>,
        quit: Arc<AtomicBool>,
    }

    impl HostSizes {
        fn watch(host: Geometry) -> io::Result<Self> {
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

        async fn recv(&mut self) -> io::Result<Geometry> {
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
    pub(in crate::core_client) fn follow_host_size(
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
    pub(in crate::core_client) struct HostKeys {
        /// The host reports key releases, so it speaks the Kitty keyboard
        /// protocol.
        pub(in crate::core_client) kitty: bool,
        /// The host reports every key as an escape code.
        pub(in crate::core_client) all: bool,
    }

    impl HostKeys {
        /// The text an input method committed, which must reach the program
        /// as text rather than through the key encoder.
        pub(in crate::core_client) fn committed_text<'k>(
            &self,
            key: &'k TerminalKey,
        ) -> Option<&'k str> {
            self.all.then(|| key.committed_text(self.kitty)).flatten()
        }
    }

    pub(super) async fn show(
        mut view: View,
        mut host: Geometry,
        guard: &mut TerminalGuard,
        mut stop: Stop,
    ) -> io::Result<()> {
        let mouse_capture = Arc::new(AtomicBool::new(false));
        let mut input = read_input(
            guard.take_buffered_host_input(),
            guard.host_escape_disambiguation_active(),
            mouse_capture.clone(),
        )?;
        let mut sizes = HostSizes::watch(host)?;
        let mut screen = Screen::default();
        let mut keys = HostKeys {
            kitty: guard.host_escape_disambiguation_active(),
            all: false,
        };
        let mut out = io::stdout();
        let mut due = Some(Instant::now());
        let mut last_draw = Instant::now();
        loop {
            let output = view.output();
            tokio::select! {
                () = output => {
                    due.get_or_insert_with(|| (last_draw + FRAME).max(Instant::now()));
                }
                () = wait(due) => {
                    screen.present(
                        &mut out,
                        (host.cols, host.rows),
                        view.draw(),
                        view.cursor(),
                    )?;
                    follow_mouse_reporting(&view, &mouse_capture)?;
                    follow_key_reporting(&view, &mut keys.all, &mut out)?;
                    last_draw = Instant::now();
                    due = None;
                }
                events = input.recv() => {
                    let Some(events) = events else {
                        return Ok(());
                    };
                    if crate::raw_input::events_require_host_mode_refresh(&events) {
                        refresh_host_modes(guard, &mouse_capture, keys.all, false);
                    }
                    for event in events {
                        if !forward(&view, keys, event) {
                            return Ok(());
                        }
                    }
                }
                size = sizes.recv() => {
                    let Ok(size) = size else {
                        return Ok(());
                    };
                    host = size;
                    view.resize(host);
                    refresh_host_modes(guard, &mouse_capture, keys.all, true);
                    due = Some(Instant::now());
                }
                event = view.event() => match event {
                    Event::Copy(content) => {
                        crate::selection::write_osc52_bytes(&content);
                    }
                    Event::Bell(count) => {
                        if let Err(error) =
                            crate::terminal_effects::write_terminal_bells(&mut out, count)
                        {
                            warn!(%error, "the bell did not reach the host");
                        }
                    }
                    Event::Ended => return Ok(()),
                },
                () = stop.recv() => return Ok(()),
            }
        }
    }

    async fn wait(due: Option<Instant>) {
        match due {
            Some(due) => tokio::time::sleep_until(due).await,
            None => std::future::pending().await,
        }
    }

    /// Re-sends the input modes a host forgets when it resets, as after a
    /// reconnect or a window reload. A focus report shows the host still
    /// reports focus, so only a resize re-enables it.
    fn refresh_host_modes(
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

    /// Captures the mouse only while the program wants it, so the host keeps
    /// its own selection otherwise.
    fn follow_mouse_reporting(view: &View, capture: &AtomicBool) -> io::Result<()> {
        let wanted = view.mouse_reporting();
        if capture.swap(wanted, Ordering::AcqRel) != wanted {
            set_mouse_capture(wanted, false)?;
        }
        Ok(())
    }

    /// Switches the host to reporting every key while the program wants
    /// that, replacing the keyboard flags the guard pushed so its pop on
    /// exit still restores the host.
    fn follow_key_reporting(
        view: &View,
        active: &mut bool,
        out: &mut impl io::Write,
    ) -> io::Result<()> {
        let wanted = view.reports_all_keys();
        if *active != wanted {
            crate::terminal_modes::set_host_kitty_keyboard_report_all(out, wanted)?;
            *active = wanted;
        }
        Ok(())
    }

    /// Hands one input event to the view. False means detach.
    fn forward(view: &View, keys: HostKeys, event: RawInputEvent) -> bool {
        match event {
            RawInputEvent::Key(key) if detaches(&key) => return false,
            RawInputEvent::Key(key) => match keys.committed_text(&key) {
                Some(text) => view.text(text),
                None => view.key(key),
            },
            RawInputEvent::Text(text) => view.text(text.as_str()),
            RawInputEvent::Paste(text) => view.paste(text),
            RawInputEvent::Mouse(mouse) => view.mouse(mouse),
            RawInputEvent::OuterFocusGained => view.focus(true),
            RawInputEvent::OuterFocusLost => view.focus(false),
            RawInputEvent::HostDefaultColor { .. }
            | RawInputEvent::HostPaletteColors { .. }
            | RawInputEvent::HostColorSchemeChanged(_)
            | RawInputEvent::HostCellSizeReport { .. }
            | RawInputEvent::Unsupported => {}
        }
        true
    }

    /// Ctrl+\, the key `hc attach` detaches on.
    pub(in crate::core_client) fn detaches(key: &TerminalKey) -> bool {
        let key = key.as_key_event();
        key.kind != KeyEventKind::Release
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('\\')
    }

    fn read_input(
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
}

#[cfg(all(test, unix))]
pub(super) use unix::{detaches, follow_host_size, HostKeys};

#[cfg(test)]
mod tests;
