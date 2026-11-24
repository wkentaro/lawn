use std::io;

mod arrivals;
mod chrome;
mod copy_mode;
mod dialog;
mod follow;
mod gate;
mod keys;
mod notices;
mod select;
mod session;
mod surface;
mod terminals;
#[cfg(all(test, unix))]
mod tests;

pub(crate) fn maybe_run(args: &[String]) -> Option<io::Result<()>> {
    (args.len() == 1).then(run)
}

#[cfg(not(unix))]
fn run() -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "lawn needs herdr core, which this platform lacks",
    ))
}

#[cfg(unix)]
fn run() -> io::Result<()> {
    use std::io::IsTerminal as _;

    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other("lawn needs a terminal"));
    }
    crate::logging::init_file_logging_in(super::log_dir(), super::LOG_FILE);
    let core = super::core()?;
    let session = crate::session::active_name()
        .unwrap_or_else(|| crate::session::DEFAULT_SESSION_NAME.to_owned());
    let launched_in = std::env::current_dir()?;
    crate::core_lifecycle::Lifecycle::new(core.clone(), &session)
        .open(&launched_in)
        .map_err(io::Error::other)?;
    let host = super::host::host_geometry()?;
    run_with_bounded_shutdown(async {
        let stop = super::host::Stop::register()?;
        let (arriving, arrivals) = tokio::sync::mpsc::unbounded_channel();
        follow::follow(session.clone(), arriving.clone())?;
        terminals::follow(arriving)?;
        let mut guard = super::host::enter(true)?;
        unix::show(core, session, launched_in, arrivals, host, &mut guard, stop).await
    })?
}

#[cfg(unix)]
fn run_with_bounded_shutdown<T>(future: impl std::future::Future<Output = T>) -> io::Result<T> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let output = runtime.block_on(future);
    runtime.shutdown_timeout(SHUTDOWN_GRACE);
    Ok(output)
}

#[cfg(unix)]
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

#[cfg(unix)]
mod unix {
    use std::cell::Cell;
    use std::io;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    use crossterm::event::{KeyCode, KeyEventKind};
    use tokio::sync::mpsc;
    use tokio::time::Instant;
    use tracing::warn;

    use super::chrome::Chrome;
    use super::gate::Gate;
    use super::keys::{Keys, Mode, Routed};
    use super::notices::{Effect, Notifying};
    use super::session::Operation;
    use super::surface::{reloaded, Arrival, Pointed, Settings, Surface};
    use super::terminals::Report;
    use crate::client::terminal_setup::TerminalGuard;
    use crate::config::{Config, SoundConfig};
    use crate::core_client::commands::creation_lifecycle;
    use crate::core_client::host::{
        clipboard, deliver, follow_key_reporting, read_input, refresh_host_modes, relay, wait,
        HostKeys, HostSizes, Stop, FRAME,
    };
    use crate::core_client::screen::Screen;
    use crate::core_client::view::{Event, Events, Geometry, View, Wake};
    use crate::core_lifecycle::Lifecycle;
    use crate::hc::Core;
    use crate::input::{KeybindAction, KeybindMatch};
    use crate::layout::PaneId;
    use crate::raw_input::RawInputEvent;

    const RETRY: std::time::Duration = std::time::Duration::from_secs(1);

    fn release(bindings: &mut Keys, id: PaneId, view: &View) {
        for release in bindings.release(id) {
            view.key(release);
        }
    }

    pub(super) fn schedule_output(wake: &Wake, due: &mut Option<Instant>, last_draw: Instant) {
        if wake.presents_output() {
            due.get_or_insert_with(|| (last_draw + FRAME).max(Instant::now()));
        }
    }

    fn connected(core: Arc<Core>) -> io::Result<Arc<Core>> {
        if core.is_closed() {
            return crate::core_client::core();
        }
        Ok(core)
    }

    fn reload(running: &mut Config, surface: &mut Surface, bindings: &mut Keys) {
        let loaded = match crate::config::load_live_config() {
            Ok(loaded) => loaded,
            Err(diagnostics) => {
                warn!(?diagnostics, "the config did not reload");
                return;
            }
        };
        let (next, kept) = reloaded(running, loaded.config, &loaded.invalid_sections);
        if !loaded.diagnostics.is_empty() || !kept.is_empty() {
            warn!(diagnostics = ?loaded.diagnostics, ?kept, "the config reloaded with problems");
        }
        surface.configure(&next);
        bindings.rebind(surface.keybinds().clone());
        *running = next;
    }

    fn notify(effects: Vec<Effect>, sound: &SoundConfig) {
        for effect in effects {
            match effect {
                Effect::Sound(played) => crate::sound::play(played, sound),
                Effect::Terminal { title, body } => {
                    if let Err(error) =
                        crate::terminal_notify::show_notification(&title, Some(&body))
                    {
                        warn!(%error, "the notification did not reach the host terminal");
                    }
                }
                Effect::System { title, body } => {
                    tokio::task::spawn_blocking(move || {
                        if let Err(error) =
                            crate::platform::show_desktop_notification(&title, Some(&body))
                        {
                            warn!(%error, "the system notification did not show");
                        }
                    });
                }
            }
        }
    }

    pub(super) fn save(
        edit: crate::config::ConfigEdit<'_>,
        running: &mut Config,
        surface: &mut Surface,
        bindings: &mut Keys,
    ) {
        let written = crate::config::write_edit(edit)
            .inspect_err(|error| warn!(%error, "the setting was not saved"))
            .is_ok();
        if written {
            reload(running, surface, bindings);
        }
        surface.saved(written);
    }

    fn tend(
        core: Arc<Core>,
        session: String,
        job: impl FnOnce(&Lifecycle) -> Result<(), String> + Send + 'static,
    ) {
        tokio::task::spawn_blocking(move || {
            let tended = connected(core)
                .map_err(|error| error.to_string())
                .and_then(|core| job(&Lifecycle::new(core, &session)));
            if let Err(error) = tended {
                warn!(%error, session, "the session's agents were not tended");
            }
        });
    }

    fn resumer(
        core: Arc<Core>,
        session: String,
        spacing: Duration,
    ) -> (
        std::sync::mpsc::Sender<Option<String>>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (resumes, queue) = std::sync::mpsc::channel::<Option<String>>();
        let (stop, stopped) = std::sync::mpsc::channel();
        tokio::task::spawn_blocking(move || {
            crate::core_lifecycle::drain(
                queue,
                crate::core_lifecycle::Pace::new(spacing, stopped),
                |only, pace| {
                    let resumed = connected(core.clone())
                        .map_err(|error| error.to_string())
                        .and_then(|core| {
                            Lifecycle::new(core, &session).resume_agents(only.as_deref(), pace)
                        });
                    if let Err(error) = resumed {
                        warn!(%error, session, "the session's agents were not resumed");
                    }
                },
            );
        });
        (resumes, stop)
    }

    pub(super) fn operations(
        mut perform: impl FnMut(Operation, PathBuf) + Send + 'static,
    ) -> std::sync::mpsc::Sender<(Operation, PathBuf)> {
        let (queue, queued) = std::sync::mpsc::channel();
        tokio::task::spawn_blocking(move || {
            for (operation, cwd) in queued {
                perform(operation, cwd);
            }
        });
        queue
    }

    fn perform(
        core: &Arc<Core>,
        session: &str,
        operation: Operation,
        cwd: &std::path::Path,
        created: &mpsc::UnboundedSender<String>,
    ) -> Option<u64> {
        let core = match connected(core.clone()) {
            Ok(core) => core,
            Err(error) => {
                warn!(%error, ?operation, "the core is unreachable");
                return None;
            }
        };
        let lifecycle = || Lifecycle::new(core.clone(), session);
        match operation {
            Operation::Start(destination) => {
                let report = |error| warn!(%error, session, "recovery failed");
                match creation_lifecycle(&core, session, report)
                    .and_then(|lifecycle| lifecycle.start(destination, cwd, &[]))
                {
                    Ok(started) => {
                        created.send(started.terminal).ok();
                    }
                    Err(error) => warn!(%error, "the program did not start"),
                }
            }
            Operation::Close(terminals) => {
                for terminal in terminals {
                    if let Err(error) = lifecycle().close(&terminal) {
                        warn!(%error, terminal, "the pane did not close");
                    }
                }
            }
            Operation::Park(terminal) => {
                if let Err(error) = lifecycle().park(&terminal) {
                    warn!(%error, terminal, "the pane did not park");
                }
            }
            Operation::Name { terminal, name } => {
                if let Err(error) = super::terminals::rename(&core, &terminal, &name) {
                    warn!(%error, terminal, "the pane was not renamed");
                }
            }
            Operation::Arrange(arrangement) => {
                return lifecycle().arrange(&arrangement).unwrap_or_else(|error| {
                    warn!(%error, "the session was not rearranged");
                    None
                });
            }
        }
        None
    }

    pub(super) async fn show(
        core: Arc<Core>,
        session: String,
        launched_in: PathBuf,
        mut arrivals: mpsc::UnboundedReceiver<Arrival>,
        mut host: Geometry,
        guard: &mut TerminalGuard,
        mut stop: Stop,
    ) -> io::Result<()> {
        let mut config = Config::load().config;
        let mut surface = Surface::new(
            host,
            Chrome::new(&config).remembering(super::chrome::preferences_path(&session)),
            Settings::from(&config),
        )
        .notifying(Notifying::from_config(&config))
        .on_a_kitty_host(guard.host_escape_disambiguation_active());
        let mouse_capture = Arc::new(AtomicBool::new(true));
        let mut input = read_input(
            guard.take_buffered_host_input(),
            guard.host_escape_disambiguation_active(),
            mouse_capture.clone(),
        )?;
        let mut sizes = HostSizes::watch(host)?;
        let (copy, mut copied) = clipboard()?;
        let wake = Wake::default();
        let mut events = Events::default();
        let (attached_tx, mut attached) = mpsc::unbounded_channel();
        let (created_tx, mut created) = mpsc::unbounded_channel();
        let (finished_tx, mut finished) = mpsc::unbounded_channel();
        let mut gate = Gate::default();
        let edits = Cell::new(0);
        let mut screen = Screen::default();
        let mut keys = HostKeys::new(guard);
        let mut bindings = Keys::new(surface.keybinds().clone(), keys.erase_is_ctrl_h, keys.kitty);
        let resumes = config.session.resume_agents_on_restore.then(|| {
            resumer(
                core.clone(),
                session.clone(),
                Duration::from_millis(config.session.startup_per_agent_delay_ms.into()),
            )
        });
        let mut out = io::stdout();
        let mut due = None;
        let mut last_draw = Instant::now();
        let operations = {
            let (core, session) = (core.clone(), session.clone());
            operations(move |operation, cwd| {
                let arranging = matches!(operation, Operation::Arrange(_));
                let written = perform(&core, &session, operation, &cwd, &created_tx);
                if arranging {
                    finished_tx.send(written).ok();
                }
            })
        };
        let run = |surface: &Surface, operation: Operation| {
            let cwd = surface
                .folder(&operation)
                .unwrap_or_else(|| launched_in.clone());
            if matches!(operation, Operation::Arrange(_)) {
                edits.set(edits.get() + 1);
            }
            operations.send((operation, cwd)).ok();
        };
        let settle = |surface: &mut Surface,
                      (operation, effects): (Option<Operation>, Vec<Effect>),
                      sound: &SoundConfig| {
            if let Some(operation) = operation {
                run(surface, operation);
            }
            for report in surface.tending() {
                match report {
                    Report::Resume(only) => {
                        if let Some((resumes, _)) = &resumes {
                            resumes.send(only).ok();
                        }
                    }
                    Report::Forget(only) => {
                        tend(core.clone(), session.clone(), move |lifecycle| {
                            lifecycle.forget_agents(only.as_deref())
                        });
                    }
                    _ => {}
                }
            }
            notify(effects, sound);
        };
        let act = |surface: &mut Surface, action: KeybindAction| {
            if let Some(operation) = surface.act(action, surface.confirms_close()) {
                run(surface, operation);
            }
        };
        loop {
            gate.queued(edits.take(), std::time::Instant::now());
            tokio::select! {
                () = wake.wait() => schedule_output(&wake, &mut due, last_draw),
                () = wait(due) => {
                    let dirty = wake.take();
                    let scrollback = surface.scrollback();
                    let (rows, cursor) = surface.frame(&dirty, |terminal, id, placing| {
                        let (core, terminal, wake, events, attached) = (
                            core.clone(),
                            terminal.to_owned(),
                            wake.clone(),
                            events.sender(),
                            attached_tx.clone(),
                        );
                        tokio::spawn(async move {
                            let view = tokio::task::spawn_blocking(move || {
                                let core = connected(core)?;
                                View::attach(core, &terminal, placing, id, &wake, &events, scrollback)
                            })
                            .await
                            .unwrap_or_else(|error| Err(io::Error::other(error)));
                            if view.is_err() {
                                tokio::time::sleep(RETRY).await;
                            }
                            attached.send((id, placing.geometry, view)).ok();
                        });
                    }, |id, view| release(&mut bindings, id, view));
                    follow_key_reporting(surface.wants_all_keys(), &mut keys.all, &mut out)?;
                    screen.present(&mut out, (host.cols, host.rows), rows, cursor)?;
                    gate.drawn();
                    last_draw = Instant::now();
                    due = None;
                    if let Some(panes) = surface.presented() {
                        wake.present(panes);
                        schedule_output(&wake, &mut due, last_draw);
                    }
                }
                () = wait(surface.deadline().map(Instant::from_std)) => {
                    let caught = surface.caught_up(&mut gate, &mut arrivals, std::time::Instant::now());
                    settle(&mut surface, caught, &config.ui.sound);
                    if surface.needs_frame() {
                        due.get_or_insert_with(Instant::now);
                    }
                }
                Some(()) = copied.recv() => {
                    surface.copied(std::time::Instant::now());
                    due.get_or_insert_with(Instant::now);
                }
                Some((id, geometry, view)) = attached.recv() => {
                    surface.attached(id, geometry, view);
                    if surface.needs_frame() {
                        due.get_or_insert_with(Instant::now);
                    }
                }
                Some(terminal) = created.recv() => {
                    surface.created(terminal);
                    if surface.needs_frame() {
                        due.get_or_insert_with(Instant::now);
                    }
                }
                arrival = arrivals.recv() => {
                    let Some(arrival) = arrival else {
                        return Err(io::Error::other("lawn stopped following the session"));
                    };
                    surface.received(arrival, &mut gate);
                    let caught = surface.caught_up(&mut gate, &mut arrivals, std::time::Instant::now());
                    settle(&mut surface, caught, &config.ui.sound);
                    if surface.needs_frame() {
                        due.get_or_insert_with(Instant::now);
                    }
                }
                Some(written) = finished.recv() => gate.finished(written),
                () = wait(gate.deadline().map(Instant::from_std)) => {}
                ready = gate.ready(&mut input) => {
                    if ready.is_none() {
                        return Ok(());
                    }
                    loop {
                        surface.end_input(|id, view| release(&mut bindings, id, view));
                        gate.queued(edits.take(), std::time::Instant::now());
                        let Some(event) = gate.take(std::time::Instant::now()) else {
                            break;
                        };
                        let key = match event {
                            RawInputEvent::Key(key) => key,
                            RawInputEvent::Mouse(mouse) => {
                                match surface.mouse(mouse, std::time::Instant::now()) {
                                    Some(Pointed::Copied(text)) => {
                                        copy.send(text.into_bytes()).ok();
                                    }
                                    Some(Pointed::Act(KeybindAction::Detach)) => return Ok(()),
                                    Some(Pointed::Act(KeybindAction::ReloadConfig)) => {
                                        reload(&mut config, &mut surface, &mut bindings);
                                    }
                                    Some(Pointed::Act(action)) => act(&mut surface, action),
                                    Some(Pointed::Run(operation)) => run(&surface, operation),
                                    Some(Pointed::Save(edit)) => {
                                        save(edit, &mut config, &mut surface, &mut bindings);
                                    }
                                    None => {}
                                }
                                continue;
                            }
                            RawInputEvent::OuterFocusGained => {
                                refresh_host_modes(guard, &mouse_capture, keys.all, false);
                                surface.host_focus(true);
                                continue;
                            }
                            RawInputEvent::OuterFocusLost => {
                                surface.host_focus(false);
                                continue;
                            }
                            event => {
                                if surface.dialog_input(&event)
                                    || surface.navigating()
                                    || surface.resizing()
                                    || surface.copy_mode_input(&event)
                                {
                                    continue;
                                }
                                if matches!(event, RawInputEvent::Text(_) | RawInputEvent::Paste(_)) {
                                    surface.deselect();
                                }
                                if let Some(view) = surface.focused() {
                                    deliver(view, keys, event);
                                }
                                continue;
                            }
                        };
                        let focused = surface.input_focus();
                        let deselects = key.kind != KeyEventKind::Release
                            && !matches!(key.code, KeyCode::Modifier(_))
                            && !surface.dialog_open();
                        let routes = bindings.route_host(
                            key,
                            focused,
                            surface.copyable(),
                            match (
                                surface.dialog_open(),
                                surface.navigate_session(),
                                surface.resizing(),
                                surface.copy_mode(),
                                surface.prompting(),
                            ) {
                                (true, ..) => Mode::Dialog,
                                (false, Some(session), ..) if surface.selection_vanished() => {
                                    Mode::NavigateVanished(session)
                                }
                                (false, Some(session), ..) => Mode::Navigate(session),
                                (false, None, true, ..) => Mode::Resize,
                                (false, None, false, false, _) => Mode::Panes,
                                (false, None, false, true, false) => Mode::Copy,
                                (false, None, false, true, true) => Mode::Prompt,
                            },
                            |key| keys.committed_text(key),
                        );
                        surface.hold_prefix(bindings.prefixed());
                        for routed in routes {
                            let action = match routed {
                                Routed::Copy => {
                                    if let Some(text) = surface.copy() {
                                        copy.send(text.into_bytes()).ok();
                                    }
                                    continue;
                                }
                                Routed::Text(pane, text) => {
                                    if let Some(view) = surface.view(pane) {
                                        view.text(&text);
                                    }
                                    continue;
                                }
                                Routed::Key(pane, key) => {
                                    if let Some(view) = surface.view(pane) {
                                        view.key(key);
                                    }
                                    continue;
                                }
                                Routed::CopyMode(key) => {
                                    if surface
                                        .paste_shortcut(&key, crate::platform::read_clipboard_text)
                                    {
                                        continue;
                                    }
                                    if let Some(text) = surface.copy_mode_key(&key) {
                                        copy.send(text.into_bytes()).ok();
                                    }
                                    continue;
                                }
                                Routed::Dialog(key) => {
                                    if surface
                                        .paste_shortcut(&key, crate::platform::read_clipboard_text)
                                    {
                                        continue;
                                    }
                                    match surface.dialog_key(&key) {
                                        Some(Pointed::Act(action)) => action,
                                        Some(Pointed::Run(operation)) => {
                                            run(&surface, operation);
                                            continue;
                                        }
                                        Some(Pointed::Save(edit)) => {
                                            save(edit, &mut config, &mut surface, &mut bindings);
                                            continue;
                                        }
                                        Some(Pointed::Copied(_)) | None => continue,
                                    }
                                }
                                Routed::Resize(key) => {
                                    if let Some(operation) = surface.resize_key(&key) {
                                        run(&surface, operation);
                                    }
                                    continue;
                                }
                                Routed::Dismiss => {
                                    surface.leave_navigate();
                                    continue;
                                }
                                Routed::Navigate(step) => match surface.navigate(step) {
                                    Some(action) => action,
                                    None => continue,
                                },
                                Routed::Binding(KeybindMatch::Action(KeybindAction::CopyMode)) => {
                                    surface.enter_copy_mode();
                                    continue;
                                }
                                Routed::Binding(KeybindMatch::Action(
                                    KeybindAction::OpenNotificationTarget,
                                )) => {
                                    surface.open_notice(std::time::Instant::now());
                                    continue;
                                }
                                Routed::Binding(KeybindMatch::Action(action)) => action,
                                Routed::Binding(KeybindMatch::Command(_)) => continue,
                            };
                            match action {
                                KeybindAction::Detach => return Ok(()),
                                KeybindAction::ReloadConfig => {
                                    reload(&mut config, &mut surface, &mut bindings);
                                }
                                action => act(&mut surface, action),
                            }
                        }
                        if deselects {
                            surface.deselect();
                        }
                    }
                    if surface.needs_frame() {
                        due.get_or_insert_with(Instant::now);
                    }
                }
                size = sizes.recv() => {
                    let Ok(size) = size else {
                        return Ok(());
                    };
                    host = size;
                    surface.resize(host);
                    refresh_host_modes(guard, &mouse_capture, keys.all, true);
                    due = Some(Instant::now());
                }
                event = events.recv() => match event {
                    Event::Ended(id) => {
                        if let Some(operation) = surface.ended(id) {
                            operations.send((operation, launched_in.clone())).ok();
                        }
                    }
                    Event::Detached(id) => {
                        surface.forget(id);
                        due.get_or_insert_with(Instant::now);
                    }
                    Event::Copy(bytes) => {
                        copy.send(bytes).ok();
                    }
                    event => relay(event, &mut out),
                },
                () = stop.recv() => return Ok(()),
            }
        }
    }
}
