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
    let core = super::core()?;
    let geometry = super::host::host_geometry()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let stop = super::host::Stop::register()?;
        let wake = super::view::Wake::default();
        let events = super::view::Events::default();
        let view = super::view::View::attach(
            core,
            terminal,
            geometry,
            crate::layout::PaneId::from_raw(0),
            &wake,
            &events.sender(),
        )?;
        let mut guard = super::host::enter(false)?;
        unix::show(view, &wake, events, geometry, &mut guard, stop).await
    })
}

#[cfg(unix)]
mod unix {
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use tokio::time::Instant;

    use crate::client::terminal_setup::{set_mouse_capture, TerminalGuard};
    use crate::core_client::host::{
        deliver, detaches, follow_key_reporting, read_input, refresh_host_modes, relay, wait,
        HostKeys, HostSizes, Stop, FRAME,
    };
    use crate::core_client::screen::Screen;
    use crate::core_client::view::{Event, Events, Geometry, View, Wake};
    use crate::raw_input::RawInputEvent;

    pub(super) async fn show(
        mut view: View,
        wake: &Wake,
        mut events: Events,
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
        let mut keys = HostKeys::new(guard);
        let mut out = io::stdout();
        let mut due = Some(Instant::now());
        let mut last_draw = Instant::now();
        loop {
            tokio::select! {
                () = wake.wait() => {
                    due.get_or_insert_with(|| (last_draw + FRAME).max(Instant::now()));
                }
                () = wait(due) => {
                    wake.take();
                    screen.present(
                        &mut out,
                        (host.cols, host.rows),
                        view.draw(),
                        view.cursor(),
                    )?;
                    follow_mouse_reporting(&view, &mouse_capture)?;
                    follow_key_reporting(view.reports_all_keys(), &mut keys.all, &mut out)?;
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
                event = events.recv() => match event {
                    Event::Ended(_) | Event::Detached(_) => return Ok(()),
                    event => relay(event, &mut out),
                },
                () = stop.recv() => return Ok(()),
            }
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

    /// Hands one input event to the view. False means detach.
    fn forward(view: &View, keys: HostKeys, event: RawInputEvent) -> bool {
        if matches!(&event, RawInputEvent::Key(key) if detaches(key)) {
            return false;
        }
        deliver(view, keys, event);
        true
    }
}

#[cfg(test)]
mod tests;
