use std::time::{Duration, Instant};

use std::path::PathBuf;

use super::session::{Operation, ScrollbackText};
use super::{run_with_bounded_shutdown, SHUTDOWN_GRACE};
use crate::session_doc::{Destination, Direction};

#[test]
fn leaving_the_loop_stops_waiting_for_a_stalled_core_call() {
    let (_never, stalled) = std::sync::mpsc::channel::<()>();
    let started = Instant::now();

    run_with_bounded_shutdown(async move {
        tokio::task::spawn_blocking(move || stalled.recv_timeout(Duration::from_secs(10)));
    })
    .expect("runtime");

    assert!(started.elapsed() < SHUTDOWN_GRACE + Duration::from_secs(2));
}

#[test]
fn operations_wait_on_one_blocking_task_until_the_split_before_them_finishes() {
    let bound = Duration::from_secs(10);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .build()
        .expect("runtime");
    let _entered = runtime.enter();
    let (report, reports) = std::sync::mpsc::channel();
    let (open, gate) = std::sync::mpsc::channel::<()>();
    let queue = super::unix::operations(move |operation, _cwd| match operation {
        Operation::Start(..) => {
            report.send(("began", tokio::task::try_id())).ok();
            gate.recv_timeout(bound).ok();
            report.send(("split", tokio::task::try_id())).ok();
        }
        Operation::Park(_) => {
            report.send(("parked", tokio::task::try_id())).ok();
        }
        Operation::Close(_) => {
            report.send(("closed", tokio::task::try_id())).ok();
        }
        Operation::Arrange(_) | Operation::Name { .. } | Operation::Overlay(_) => {}
    });
    let split = Destination::Split {
        beside: "t1".to_owned(),
        direction: Direction::Right,
        ratio: 0.5,
    };
    queue
        .send((Operation::Start(1, split), PathBuf::new()))
        .expect("queued");
    let began = reports.recv_timeout(bound).expect("the split began");
    for operation in [
        Operation::Park("t1".to_owned()),
        Operation::Close(vec!["t1".to_owned()]),
    ] {
        queue.send((operation, PathBuf::new())).expect("queued");
    }
    open.send(()).expect("opened");
    let rest = [(); 3].map(|()| reports.recv_timeout(bound).expect("the operation ran"));
    let reports = [began, rest[0], rest[1], rest[2]];

    assert_eq!(
        reports.map(|(said, _)| said),
        ["began", "split", "parked", "closed"]
    );
    assert!(began.1.is_some(), "{reports:?}");
    assert!(
        reports.iter().all(|(_, task)| *task == began.1),
        "{reports:?}"
    );
}

struct Configured {
    path: PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Configured {
    fn at(name: &str) -> Self {
        let lock = crate::config::test_config_env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("lawn-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);
        Self { path, _lock: lock }
    }
}

impl Drop for Configured {
    fn drop(&mut self) {
        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        if let Some(dir) = self.path.parent() {
            std::fs::remove_dir_all(dir).ok();
        }
    }
}

fn running(config: &crate::config::Config) -> (super::surface::Surface, super::keys::Keys) {
    let surface = super::surface::Surface::new(
        crate::core_client::view::Geometry {
            cols: 120,
            rows: 30,
            cell_width_px: 0,
            cell_height_px: 0,
        },
        super::chrome::Chrome::new(config, tokio::sync::mpsc::channel(1).0),
        super::surface::Settings::from(config),
    );
    let keys = super::keys::Keys::new(surface.keybinds().clone(), false, false);
    (surface, keys)
}

#[tokio::test]
async fn saving_a_setting_edits_only_its_key_and_applies_everything_the_file_now_says() {
    let file = Configured::at("save-applies");
    std::fs::write(&file.path, "[ui]\nstatus_indicators = \"dots\"\n").expect("write");
    let mut config = crate::config::load_live_config().expect("config").config;
    let (mut surface, mut keys) = running(&config);
    std::fs::write(
        &file.path,
        "[ui]\nstatus_indicators = \"symbols\"\nconfirm_close = true\n",
    )
    .expect("write");

    super::unix::save(
        crate::config::ConfigEdit::Theme("gruvbox"),
        &mut config,
        &mut surface,
        &mut keys,
    );

    let written = std::fs::read_to_string(&file.path).expect("read");
    assert!(
        written.contains("status_indicators = \"symbols\""),
        "{written}"
    );
    assert!(written.contains("confirm_close = true"), "{written}");
    assert!(written.contains("name = \"gruvbox\""), "{written}");
    assert_eq!(config.theme.name.as_deref(), Some("gruvbox"));
    assert_eq!(
        config.ui.status_indicators,
        crate::config::StatusIndicatorStyle::Symbols
    );
    assert!(surface.confirms_close());
}

#[tokio::test]
async fn a_save_the_file_refuses_leaves_the_dialog_open_and_the_running_config_as_it_was() {
    let file = Configured::at("save-refused");
    std::fs::create_dir_all(&file.path).expect("dir");
    let mut config = crate::config::Config::default();
    let (mut surface, mut keys) = running(&config);
    surface.act(crate::input::KeybindAction::Settings, false);
    for _ in 0..7 {
        surface.dialog_key(&crate::input::TerminalKey::new(
            crossterm::event::KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));
    }

    super::unix::save(
        crate::config::ConfigEdit::Theme("gruvbox"),
        &mut config,
        &mut surface,
        &mut keys,
    );

    assert!(surface.dialog_open());
    assert_eq!(config.theme.name, None);
}

fn drawn(surface: &mut super::surface::Surface) -> String {
    let (rows, _) = surface.frame(&std::collections::HashSet::new(), |_, _, _| {}, |_, _| {});
    rows.iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn a_saved_setting_the_reload_kept_out_leaves_the_dialog_on_the_value_in_effect() {
    let file = Configured::at("save-kept-out");
    std::fs::write(&file.path, "[ui]\nstatus_indicators = \"dots\"\n").expect("write");
    let mut config = crate::config::load_live_config().expect("config").config;
    let (mut surface, mut keys) = running(&config);
    surface.act(crate::input::KeybindAction::Settings, false);
    for code in [
        crossterm::event::KeyCode::Tab,
        crossterm::event::KeyCode::Down,
    ] {
        surface.dialog_key(&crate::input::TerminalKey::new(
            code,
            crossterm::event::KeyModifiers::NONE,
        ));
    }
    std::fs::write(
        &file.path,
        "[ui]\nstatus_indicators = \"dots\"\nsidebar_min_width = 40\nsidebar_max_width = 20\n",
    )
    .expect("write");

    super::unix::save(
        crate::config::ConfigEdit::StatusIndicators(crate::config::StatusIndicatorStyle::Symbols),
        &mut config,
        &mut surface,
        &mut keys,
    );

    assert_eq!(
        config.ui.status_indicators,
        crate::config::StatusIndicatorStyle::Dots
    );
    assert!(surface.dialog_open());
    let screen = drawn(&mut surface);
    assert!(screen.contains("▸ color dots"), "{screen}");
}

#[tokio::test]
async fn a_theme_apply_the_reload_kept_out_leaves_the_dialog_open_while_auto_switch_still_paints() {
    let file = Configured::at("save-auto-switch");
    let theme = "[theme]\nname = \"nord\"\nauto_switch = true\ndark_name = \"gruvbox\"\n";
    std::fs::write(&file.path, theme).expect("write");
    let mut config = crate::config::load_live_config().expect("config").config;
    let (mut surface, mut keys) = running(&config);
    surface.act(crate::input::KeybindAction::Settings, false);
    std::fs::write(&file.path, format!("{theme}light_name = 42\n")).expect("write");

    super::unix::save(
        crate::config::ConfigEdit::Theme("nord"),
        &mut config,
        &mut surface,
        &mut keys,
    );

    let written = std::fs::read_to_string(&file.path).expect("read");
    assert!(written.contains("auto_switch = false"), "{written}");
    assert!(config.theme.auto_switch);
    assert!(surface.dialog_open());
}

#[tokio::test]
async fn applying_the_first_theme_while_an_alias_the_reload_kept_stays_in_effect_leaves_the_dialog_open(
) {
    let file = Configured::at("save-alias");
    let theme = "[theme]\nname = \"latte\"\n";
    std::fs::write(&file.path, theme).expect("write");
    let mut config = crate::config::load_live_config().expect("config").config;
    let (mut surface, mut keys) = running(&config);
    surface.act(crate::input::KeybindAction::Settings, false);
    std::fs::write(&file.path, format!("{theme}light_name = 42\n")).expect("write");

    super::unix::save(
        crate::config::ConfigEdit::Theme(crate::config::THEME_NAMES[0]),
        &mut config,
        &mut surface,
        &mut keys,
    );

    assert_eq!(config.theme.name.as_deref(), Some("latte"));
    assert!(surface.dialog_open());
}

#[tokio::test]
async fn a_refused_write_of_the_running_theme_leaves_the_dialog_open() {
    let file = Configured::at("save-refused-running");
    std::fs::create_dir_all(&file.path).expect("dir");
    let mut config: crate::config::Config =
        toml::from_str("[theme]\nname = \"gruvbox\"").expect("config");
    let (mut surface, mut keys) = running(&config);
    surface.act(crate::input::KeybindAction::Settings, false);

    super::unix::save(
        crate::config::ConfigEdit::Theme("gruvbox"),
        &mut config,
        &mut surface,
        &mut keys,
    );

    assert!(surface.dialog_open());
}

fn beside(terminal: &str) -> Destination {
    Destination::Split {
        beside: terminal.to_owned(),
        direction: Direction::Right,
        ratio: 0.5,
    }
}

fn answers_to(
    core: std::io::Result<std::sync::Arc<crate::hc::Core>>,
    operation: Operation,
) -> Vec<super::unix::Answer> {
    let (started, mut answers) = tokio::sync::mpsc::unbounded_channel();
    super::unix::perform(
        core,
        "work",
        operation,
        std::path::Path::new("/tmp"),
        &started,
    );
    drop(started);
    std::iter::from_fn(|| answers.try_recv().ok()).collect()
}

fn refusing_core(
    asked: std::sync::mpsc::Sender<(String, serde_json::Value)>,
) -> std::sync::Arc<crate::hc::Core> {
    let mut document = (0, serde_json::Value::Null);
    crate::hc::test_support::answering(move |method, params| {
        asked.send((method.to_owned(), params.clone())).ok();
        match method {
            "doc.get" => Ok(serde_json::json!({"version": document.0, "value": document.1})),
            "doc.set" => {
                document = (document.0 + 1, params["value"].clone());
                Ok(serde_json::json!({"version": document.0}))
            }
            "terminal.list" => Ok(serde_json::json!({"terminals": []})),
            _ => Err(serde_json::json!({"code": -32000, "message": "refused"})),
        }
    })
}

#[test]
fn a_start_the_core_refuses_answers_its_request_once_without_a_terminal() {
    let (asked, methods) = std::sync::mpsc::channel();
    let core = refusing_core(asked);

    let answers = answers_to(
        Ok(core),
        Operation::Start(7, Destination::Workspace { name: None }),
    );

    assert!(methods
        .try_iter()
        .any(|(method, _)| method == "terminal.run"));
    assert_eq!(answers, [(7, None, None)]);
}

#[test]
fn a_command_pane_asks_the_core_to_start_its_program_at_the_size_of_its_pane() {
    let (asked, methods) = std::sync::mpsc::channel();
    let size = crate::core_lifecycle::Size { cols: 78, rows: 22 };
    let overlay = super::session::Overlay {
        request: 7,
        destination: Destination::Workspace { name: None },
        argv: vec!["stty".to_owned(), "size".to_owned()],
        env: Vec::new(),
        size: Some(size),
        scrollback: None,
    };

    answers_to(Ok(refusing_core(asked)), Operation::Overlay(overlay));

    let runs: Vec<_> = methods
        .try_iter()
        .filter(|(method, _)| method == "terminal.run")
        .map(|(_, params)| params["size"].clone())
        .collect();
    assert_eq!(runs, [serde_json::json!({"cols": 78, "rows": 22})]);
}

#[test]
fn a_start_with_the_core_unreachable_answers_its_request_once_without_a_terminal() {
    let answers = answers_to(
        Err(std::io::Error::other("unreachable")),
        Operation::Start(7, beside("t1")),
    );

    assert_eq!(answers, [(7, None, None)]);
}

pub(super) fn scratch_temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lawn-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch directory");
    std::env::set_var("TMPDIR", &dir);
    dir
}

pub(super) fn files_in(dir: &std::path::Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .expect("scratch directory")
        .map(|entry| entry.expect("entry").path())
        .collect()
}

#[tokio::test]
async fn an_edit_whose_start_recovery_placed_first_keeps_its_file_until_its_pane_closes() {
    let dir = scratch_temp_dir("edit-recovered");
    let (temp_file, _argv) =
        super::unix::editing(&ScrollbackText("text".to_owned())).expect("written");
    let (started, mut answers) = tokio::sync::mpsc::unbounded_channel();
    let recovered = crate::session_doc::Created {
        terminal: "t9".to_owned(),
        workspace: None,
        tab: None,
        version: 4,
    };

    super::unix::answer(&started, 3, Ok(recovered), Some(temp_file.clone()));
    let (request, placed, kept) = answers.try_recv().expect("an answer");
    let mut scrollbacks = std::collections::HashMap::new();
    super::unix::keep(&mut scrollbacks, &placed, kept);

    assert_eq!((request, placed), (3, Some(("t9".to_owned(), 4))));
    assert!(temp_file.exists());
    assert!(super::unix::forget(&mut scrollbacks, ["t8".to_owned()]).is_none());
    assert!(temp_file.exists());
    super::unix::forget(&mut scrollbacks, ["t9".to_owned()])
        .expect("the file is forgotten")
        .await
        .expect("removed");
    assert!(!temp_file.exists());
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn a_shell_command_runs_detached_in_the_pane_directory_with_the_command_env() {
    let dir = std::env::temp_dir().join(format!("lawn-shell-command-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch directory");
    let env = vec![
        ("HOME".to_owned(), dir.display().to_string()),
        ("LAWN_PROBE".to_owned(), "probe".to_owned()),
    ];

    let status = super::unix::detached(
        "printf '%s:%s' \"$LAWN_PROBE\" \"$(pwd -P)\" > out".to_owned(),
        env,
        Some(dir.clone()),
    )
    .await
    .expect("the command started")
    .wait()
    .await
    .expect("the command ended");

    let written = std::fs::read_to_string(dir.join("out"));
    let expected = format!("probe:{}", dir.canonicalize().expect("scratch").display());
    std::fs::remove_dir_all(&dir).expect("remove the scratch directory");
    assert!(status.success());
    assert_eq!(written.expect("the command wrote its output"), expected);
}

#[test]
fn an_edit_under_a_relative_temp_dir_opens_and_tracks_its_file_by_an_absolute_path() {
    let dir = scratch_temp_dir("edit-relative");
    std::env::set_current_dir(dir.parent().expect("temp root")).expect("current directory");
    std::env::set_var("TMPDIR", dir.file_name().expect("scratch name"));

    let (temp_file, argv) =
        super::unix::editing(&ScrollbackText("text".to_owned())).expect("written");

    assert!(temp_file.is_absolute());
    assert_eq!(
        std::fs::read_to_string(&temp_file).expect("the file"),
        "text"
    );
    let shown = temp_file.display().to_string();
    assert!(argv.iter().any(|arg| arg.contains(&shown)), "{argv:?}");
    std::fs::remove_dir_all(&dir).ok();
}
