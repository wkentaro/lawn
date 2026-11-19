use std::io;

mod chrome;
mod copy_mode;
mod dialog;
mod follow;
mod keys;
mod select;
mod session;
mod starts;
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
        let layouts = follow::follow(session.clone())?;
        let mut guard = super::host::enter(true)?;
        unix::show(core, session, launched_in, layouts, host, &mut guard, stop).await
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
    use std::io;
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use crossterm::event::{KeyCode, KeyEventKind};
    use tokio::sync::mpsc;
    use tokio::time::Instant;
    use tracing::warn;

    use super::chrome::Chrome;
    use super::keys::{Keys, Mode, Routed};
    use super::select::Copying;
    use super::session::{Operation, Overlay, PaneChrome};
    use super::surface::{Pointed, Surface};
    use crate::client::terminal_setup::TerminalGuard;
    use crate::config::{Config, CustomCommandAction, CustomCommandKeybind, LiveKeybindConfig};
    use crate::core_client::commands::creation_lifecycle;
    use crate::core_client::host::{
        clipboard, deliver, follow_key_reporting, read_input, refresh_host_modes, relay, wait,
        HostKeys, HostSizes, Stop, FRAME,
    };
    use crate::core_client::screen::Screen;
    use crate::core_client::view::{Event, Events, Geometry, Scrollback, View, Wake};
    use crate::core_lifecycle::Lifecycle;
    use crate::events::AppEvent;
    use crate::hc::Core;
    use crate::input::{KeybindAction, KeybindMatch};
    use crate::raw_input::RawInputEvent;
    use crate::session_doc::Loaded;

    const RETRY: std::time::Duration = std::time::Duration::from_secs(1);

    fn connected(core: Arc<Core>) -> io::Result<Arc<Core>> {
        if core.is_closed() {
            return crate::core_client::core();
        }
        Ok(core)
    }

    fn live_keybinds(config: &Config) -> LiveKeybindConfig {
        config
            .live_keybinds_with_diagnostics()
            .map(|(keybinds, _diagnostics)| keybinds)
            .unwrap_or_else(|_diagnostics| LiveKeybindConfig {
                prefix: config.prefix_keys(),
                keybinds: config.keybinds(),
            })
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

    pub(super) fn perform(
        core: io::Result<Arc<Core>>,
        session: &str,
        operation: Operation,
        cwd: &std::path::Path,
        started: &mpsc::UnboundedSender<(u64, Option<(String, u64)>)>,
    ) {
        let core = match core {
            Ok(core) => core,
            Err(error) => {
                warn!(%error, ?operation, "the core is unreachable");
                if let Operation::Start(request, _) | Operation::Overlay(Overlay { request, .. }) =
                    operation
                {
                    started.send((request, None)).ok();
                }
                return;
            }
        };
        let lifecycle = || Lifecycle::new(core.clone(), session);
        let start = |request, destination, argv: &[String], env: &[(String, String)]| {
            let report = |error| warn!(%error, session, "recovery failed");
            let placed = match creation_lifecycle(&core, session, report)
                .and_then(|lifecycle| lifecycle.start_with_env(destination, cwd, argv, env))
            {
                Ok(created) => Some((created.terminal, created.version)),
                Err(error) => {
                    warn!(%error, "the program did not start");
                    None
                }
            };
            started.send((request, placed)).ok();
        };
        match operation {
            Operation::Start(request, destination) => start(request, destination, &[], &[]),
            Operation::Overlay(overlay) => start(
                overlay.request,
                overlay.destination,
                &overlay.argv,
                &overlay.env,
            ),
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
            Operation::Arrange(arrangement) => {
                if let Err(error) = lifecycle().arrange(&arrangement) {
                    warn!(%error, "the session was not rearranged");
                }
            }
        }
    }

    pub(super) async fn detached(
        command: String,
        env: Vec<(String, String)>,
        cwd: Option<PathBuf>,
    ) -> io::Result<tokio::process::Child> {
        tokio::task::spawn_blocking(move || {
            let mut process = crate::platform::detached_custom_command_process(&command);
            process
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .envs(env);
            if let Some(cwd) = cwd.filter(|cwd| cwd.is_dir()) {
                process.current_dir(cwd);
            }
            tokio::process::Command::from(process).spawn()
        })
        .await
        .unwrap_or_else(|error| Err(io::Error::other(error)))
    }

    fn command(
        surface: &mut Surface,
        binding: CustomCommandKeybind,
        start: impl Fn(&Surface, Operation),
    ) {
        let label = binding.label;
        match binding.action {
            CustomCommandAction::Shell => {
                let (env, cwd) = surface.command_env();
                tokio::spawn(async move {
                    if let Err(error) = detached(binding.command, env, cwd).await {
                        warn!(%error, binding = %label, "the custom command did not start");
                    }
                });
            }
            CustomCommandAction::Pane => match surface.overlay(&binding.command) {
                Some(operation) => start(surface, operation),
                None => {
                    warn!(binding = %label, "the custom command has no focused pane to open beside")
                }
            },
            CustomCommandAction::Popup | CustomCommandAction::PluginAction => warn!(
                binding = %label,
                action = ?binding.action,
                "the core client does not run popup or plugin_action custom commands"
            ),
        }
    }

    pub(super) async fn show(
        core: Arc<Core>,
        session: String,
        launched_in: PathBuf,
        mut layouts: mpsc::UnboundedReceiver<Loaded>,
        mut host: Geometry,
        guard: &mut TerminalGuard,
        mut stop: Stop,
    ) -> io::Result<()> {
        let config = Config::load().config;
        let keybinds = live_keybinds(&config);
        let (statuses_tx, mut statuses) = mpsc::channel(crate::config::MAX_TAB_BAR_RIGHT_ENTRIES);
        let mut surface = Surface::new(
            host,
            PaneChrome {
                borders: config.ui.pane_borders,
                gaps: config.ui.pane_gaps,
                outer_borders: config.ui.pane_outer_borders,
            },
            Chrome::new(&config, statuses_tx)
                .remembering(super::chrome::preferences_path(&session)),
            crate::ui::theme::client_palette_from_config(&config),
            config.ui.pane_scrollbars,
            Copying {
                on_select: config.ui.copy_on_select,
                toast: config
                    .ui
                    .toast
                    .clipboard
                    .enabled
                    .then_some(config.ui.toast.clipboard.position),
            },
            keybinds.clone(),
        );
        let mouse_capture = Arc::new(AtomicBool::new(true));
        let mut input = read_input(
            guard.take_buffered_host_input(),
            guard.host_escape_disambiguation_active(),
            mouse_capture.clone(),
        )?;
        let mut sizes = HostSizes::watch(host)?;
        let mut reports = super::terminals::follow()?;
        let (copy, mut copied) = clipboard()?;
        let wake = Wake::default();
        let mut events = Events::default();
        let (attached_tx, mut attached) = mpsc::unbounded_channel();
        let (started_tx, mut started) = mpsc::unbounded_channel();
        let mut screen = Screen::default();
        let mut keys = HostKeys::new(guard);
        let mut bindings = Keys::new(keybinds, keys.erase_is_ctrl_h, keys.kitty);
        let scrollback = Scrollback::from(&config);
        let mut out = io::stdout();
        let mut due = None;
        let mut last_draw = Instant::now();
        let operations = {
            let (core, session) = (core.clone(), session.clone());
            operations(move |operation, cwd| {
                perform(
                    connected(core.clone()),
                    &session,
                    operation,
                    &cwd,
                    &started_tx,
                );
            })
        };
        let start = |surface: &Surface, operation: Operation| {
            let cwd = surface
                .focused()
                .and_then(View::cwd)
                .unwrap_or_else(|| launched_in.clone());
            operations.send((operation, cwd)).ok();
        };
        let act = |surface: &mut Surface, action: KeybindAction| {
            if let Some(operation) = surface.act(action, config.ui.confirm_close) {
                start(surface, operation);
            }
        };
        loop {
            tokio::select! {
                () = wake.wait() => {
                    due.get_or_insert_with(|| (last_draw + FRAME).max(Instant::now()));
                }
                () = wait(due) => {
                    let dirty = wake.take();
                    let (rows, cursor) = surface.frame(&dirty, |terminal, id, geometry| {
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
                                View::attach(core, &terminal, geometry, id, &wake, &events, scrollback)
                            })
                            .await
                            .unwrap_or_else(|error| Err(io::Error::other(error)));
                            if view.is_err() {
                                tokio::time::sleep(RETRY).await;
                            }
                            attached.send((id, geometry, view)).ok();
                        });
                    }, |id, view| {
                        for release in bindings.release(id) {
                            view.key(release);
                        }
                    });
                    screen.present(&mut out, (host.cols, host.rows), rows, cursor)?;
                    let all_keys = surface.focused().is_some_and(View::reports_all_keys);
                    follow_key_reporting(all_keys, &mut keys.all, &mut out)?;
                    last_draw = Instant::now();
                    due = None;
                }
                () = wait(surface.deadline().map(Instant::from_std)) => {
                    surface.tick(std::time::Instant::now());
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
                    due.get_or_insert_with(Instant::now);
                }
                Some((request, placed)) = started.recv() => {
                    surface.started(request, placed);
                    if surface.needs_frame() {
                        due.get_or_insert_with(Instant::now);
                    }
                }
                Some(report) = statuses.recv() => {
                    if let AppEvent::TabBarCommandFinished { segment_index, result, .. } = report {
                        surface.finished(segment_index, result);
                    }
                    if surface.needs_frame() {
                        due.get_or_insert_with(Instant::now);
                    }
                }
                Some(report) = reports.recv() => {
                    surface.heard(report);
                    if surface.needs_frame() {
                        due.get_or_insert_with(Instant::now);
                    }
                }
                loaded = layouts.recv() => {
                    let Some(loaded) = loaded else {
                        return Err(io::Error::other("lawn stopped following the session"));
                    };
                    surface.update(loaded.layout, loaded.version);
                    if surface.needs_frame() {
                        due.get_or_insert_with(Instant::now);
                    }
                }
                batch = input.recv() => {
                    let Some(batch) = batch else {
                        return Ok(());
                    };
                    if crate::raw_input::events_require_host_mode_refresh(&batch) {
                        refresh_host_modes(guard, &mouse_capture, keys.all, false);
                    }
                    for event in batch {
                        let key = match event {
                            RawInputEvent::Key(key) => key,
                            RawInputEvent::Mouse(mouse) => {
                                match surface.mouse(mouse, std::time::Instant::now()) {
                                    Some(Pointed::Copied(text)) => {
                                        copy.send(text.into_bytes()).ok();
                                    }
                                    Some(Pointed::Act(action)) => act(&mut surface, action),
                                    Some(Pointed::Run(operation)) => start(&surface, operation),
                                    None => {}
                                }
                                continue;
                            }
                            RawInputEvent::OuterFocusGained => {
                                surface.host_focus(true);
                                continue;
                            }
                            RawInputEvent::OuterFocusLost => {
                                surface.host_focus(false);
                                continue;
                            }
                            event => {
                                if surface.dialog_input(&event)
                                    || surface.switching()
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
                        let focused = surface.focused_id();
                        let deselects = key.kind != KeyEventKind::Release
                            && !matches!(key.code, KeyCode::Modifier(_))
                            && !surface.dialog_open();
                        let routes = bindings.route_host(
                            key,
                            focused,
                            surface.copyable(),
                            match (
                                surface.dialog_open(),
                                surface.switching(),
                                surface.copy_mode(),
                                surface.prompting(),
                            ) {
                                (true, ..) => Mode::Dialog,
                                (false, true, ..) => Mode::Switcher,
                                (false, false, false, _) => Mode::Panes,
                                (false, false, true, false) => Mode::Copy,
                                (false, false, true, true) => Mode::Prompt,
                            },
                            |key| keys.committed_text(key),
                        );
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
                                    if let Some(operation) = surface.dialog_key(&key) {
                                        start(&surface, operation);
                                    }
                                    continue;
                                }
                                Routed::Dismiss => {
                                    surface.close_switcher();
                                    continue;
                                }
                                Routed::Binding(KeybindMatch::Action(KeybindAction::CopyMode)) => {
                                    surface.enter_copy_mode();
                                    continue;
                                }
                                Routed::Binding(KeybindMatch::Action(KeybindAction::Detach)) => {
                                    return Ok(());
                                }
                                Routed::Binding(KeybindMatch::Action(action)) => action,
                                Routed::Binding(KeybindMatch::Command(binding)) => {
                                    command(&mut surface, binding, start);
                                    continue;
                                }
                            };
                            act(&mut surface, action);
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
                        let Some(terminal) = surface.terminal(id).map(str::to_owned) else {
                            continue;
                        };
                        operations
                            .send((Operation::Close(vec![terminal]), launched_in.clone()))
                            .ok();
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
