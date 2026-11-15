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
    /// The core terminal's address (`t7`).
    address: String,
    /// Keys and submissions go through one queue so a submission's pause
    /// never lets other input in between its text and its Enter.
    input: std::sync::mpsc::Sender<Input>,
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
    }
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

/// `terminal.run` for a command Lawn built for a PTY: same program, folder
/// and environment, owned by no terminal so it outlives Lawn.
fn run_params(cmd: &CommandBuilder, rows: u16, cols: u16) -> serde_json::Value {
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
    // The core names the terminal itself.
    let keep = |key: &str| !key.starts_with("HC_");
    let env: serde_json::Map<String, serde_json::Value> = cmd
        .iter_full_env_as_str()
        .filter(|(key, _)| keep(key))
        .map(|(key, value)| (key.to_owned(), value.into()))
        .collect();
    // The core adds `env` on top of its own environment, so anything the
    // pane dropped has to be unset there too.
    // shortcut: drops what Lawn's own environment has plus the names every
    // pane drops, and nothing on Windows; send a clean environment once
    // `terminal.run` takes one.
    #[cfg(unix)]
    let argv = if argv.is_empty() {
        argv
    } else {
        // Names only: a value that isn't UTF-8 must not stop the pane.
        let mut removed: Vec<String> = std::env::vars_os()
            .filter_map(|(key, _)| key.into_string().ok())
            .filter(|key| keep(key) && !env.contains_key(key))
            .collect();
        // The core's own environment may hold these even when Lawn's doesn't.
        removed.extend(
            HOST_HANDLE_ENV
                .iter()
                .chain(&CHILD_AGENT_ENV)
                .filter(|key| !env.contains_key(**key))
                .map(|key| (*key).to_owned()),
        );
        removed.sort();
        removed.dedup();
        let mut wrapped = vec!["env".to_owned()];
        for key in removed {
            wrapped.extend(["-u".to_owned(), key]);
        }
        wrapped.extend(argv);
        wrapped
    };
    serde_json::json!({
        "argv": argv,
        "cwd": cmd.get_cwd().map(|cwd| cwd.to_string_lossy()),
        "env": env,
        "size": {"cols": cols, "rows": rows},
        "labels": {"lawn": "1"},
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
        .run(run_params(cmd, rows, cols))
        .map_err(std::io::Error::other)?;
    info!(pane = pane_id.raw(), %address, "pane runs in herdr-core");
    core.attach(serde_json::json!({"terminal": address, "cols": cols, "rows": rows}))
        .map_err(std::io::Error::other)
}

impl PaneRuntime {
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
        let params = run_params(&cmd, 24, 80);
        let argv: Vec<String> = serde_json::from_value(params["argv"].clone()).unwrap();
        assert_eq!(argv[argv.len() - 2..], ["claude", "--resume"]);
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
    fn run_params_start_lawns_login_shell_without_dropped_env() {
        let mut cmd = CommandBuilder::new_default_prog();
        cmd.env("SHELL", "/bin/fish");
        // PATH is in Lawn's environment, so dropping it must reach the core.
        cmd.env_remove("PATH");
        cmd.env_remove("TMUX");
        cmd.env_remove("CLAUDECODE");
        let argv: Vec<String> =
            serde_json::from_value(run_params(&cmd, 24, 80)["argv"].clone()).unwrap();
        assert_eq!(argv[0], "env");
        assert_eq!(argv[argv.len() - 2..], ["/bin/fish", "-l"]);
        assert!(argv.windows(2).any(|pair| pair == ["-u", "PATH"]));
        // Dropped by every pane, so unset even when Lawn's environment lacks it.
        assert!(argv.windows(2).any(|pair| pair == ["-u", "TMUX"]));
        assert!(argv.windows(2).any(|pair| pair == ["-u", "CLAUDECODE"]));
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
