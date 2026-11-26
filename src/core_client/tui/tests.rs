use std::time::{Duration, Instant};

use std::path::PathBuf;

use super::session::Operation;
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
        Operation::Start(_) => {
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
        Operation::Arrange(_) | Operation::Name { .. } => {}
    });
    let split = Destination::Split {
        beside: "t1".to_owned(),
        direction: Direction::Right,
        ratio: 0.5,
    };
    queue.send((Operation::Start(split), None)).expect("queued");
    let began = reports.recv_timeout(bound).expect("the split began");
    for operation in [
        Operation::Park("t1".to_owned()),
        Operation::Close(vec!["t1".to_owned()]),
    ] {
        queue.send((operation, None)).expect("queued");
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
        super::chrome::Chrome::new(config),
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

#[test]
fn an_operation_on_a_linked_machines_pane_runs_on_that_machine() {
    use crate::core_lifecycle::Lifecycle;
    use crate::hc::test_support::TestCore;
    use crate::session_doc::Arrangement;

    let (Some(here), Some(there)) = (TestCore::start(), TestCore::start()) else {
        return;
    };
    let _link = here.link(&there, "boxb");
    here.becomes("boxb", "connected");
    let core = &here.core;
    let cores = core.cores().expect("the cores");
    let id = crate::hc::core_id(&cores, "boxb").expect("a core id");
    let workspace = || Destination::Workspace { name: None };
    let near = Lifecycle::new(core.clone(), "live");
    let far = Lifecycle::on(core.clone(), "live", Some("boxb"));
    for lifecycle in [&near, &far] {
        lifecycle.start(workspace(), None, &[]).expect("start");
    }
    let named = |workspace: String| {
        Operation::Arrange(Arrangement::NameWorkspace {
            workspace,
            name: "moved".to_owned(),
        })
    };
    let first = |lifecycle: &Lifecycle| lifecycle.layout().expect("layout").workspaces.remove(0);
    let (created, mut creations) = tokio::sync::mpsc::unbounded_channel();
    let perform = |operation| super::unix::perform(core, "live", operation, None, &created);

    perform(Operation::Start(Destination::Split {
        beside: format!("{id}/t1"),
        direction: Direction::Right,
        ratio: 0.5,
    }));
    let far_written = perform(named(format!("{id}/{}", first(&far).id)));
    let near_written = perform(named(first(&near).id));
    perform(Operation::Park(format!("{id}/t1")));
    perform(Operation::Close(vec![format!("{id}/t2")]));

    assert_eq!(creations.try_recv(), Ok(format!("{id}/t2")));
    assert_eq!((far_written, near_written.is_some()), (None, true));
    let there = Lifecycle::new(there.core.clone(), "live")
        .layout()
        .expect("layout");
    assert_eq!(there.workspaces[0].name.as_deref(), Some("moved"));
    assert_eq!(there.workspaces[0].id, first(&far).id);
    assert!(
        there.parked.contains("t1") && !there.owns("t2"),
        "{there:?}"
    );
    assert!(near.layout().expect("layout").shows("t1"));
}

#[test]
fn a_close_drawn_before_two_machines_swapped_names_reaches_the_machine_it_was_drawn_from() {
    use crate::core_lifecycle::Lifecycle;
    use crate::hc::test_support::TestCore;

    let (Some(here), Some(first), Some(second)) =
        (TestCore::start(), TestCore::start(), TestCore::start())
    else {
        return;
    };
    let core = &here.core;
    let workspace = || Destination::Workspace { name: None };
    let _first = here.link(&first, "boxb");
    here.becomes("boxb", "connected");
    let cores = core.cores().expect("the cores");
    let drawn = crate::hc::core_id(&cores, "boxb").expect("a core id");
    let renamed = serde_json::json!({"machine": "boxb", "new_name": "boxz"});
    core.call("machine.rename", renamed).expect("rename");
    let _second = here.link(&second, "boxb");
    here.becomes("boxb", "connected");
    for machine in ["boxz", "boxb"] {
        Lifecycle::on(core.clone(), "live", Some(machine))
            .start(workspace(), None, &[])
            .expect("start");
    }
    let (created, _creations) = tokio::sync::mpsc::unbounded_channel();

    let close = Operation::Close(vec![format!("{drawn}/t1")]);
    super::unix::perform(core, "live", close, None, &created);

    let owns = |test: &TestCore| {
        let layout = Lifecycle::new(test.core.clone(), "live").layout();
        layout.expect("layout").owns("t1")
    };
    assert_eq!((owns(&first), owns(&second)), (false, true));
}

#[test]
fn a_close_drawn_for_a_machine_since_unlinked_spares_the_machine_now_named_with_its_core_id() {
    use crate::core_lifecycle::Lifecycle;
    use crate::hc::test_support::TestCore;

    let (Some(here), Some(first), Some(second)) =
        (TestCore::start(), TestCore::start(), TestCore::start())
    else {
        return;
    };
    let core = &here.core;
    let (_first, _second) = (here.link(&first, "boxb"), here.link(&second, "boxc"));
    for machine in ["boxb", "boxc"] {
        here.becomes(machine, "connected");
        Lifecycle::on(core.clone(), "live", Some(machine))
            .start(Destination::Workspace { name: None }, None, &[])
            .expect("start");
    }
    let cores = core.cores().expect("the cores");
    let drawn = crate::hc::core_id(&cores, "boxb").expect("a core id");
    let unlinked = serde_json::json!({"machine": "boxb"});
    core.call("machine.rm", unlinked).expect("unlink");
    let renamed = serde_json::json!({"machine": "boxc", "new_name": drawn});
    core.call("machine.rename", renamed).expect("rename");
    here.becomes(drawn, "connected");
    let (created, _creations) = tokio::sync::mpsc::unbounded_channel();

    let close = Operation::Close(vec![format!("{drawn}/t1")]);
    super::unix::perform(core, "live", close, None, &created);

    let layout = Lifecycle::new(second.core.clone(), "live").layout();
    assert!(layout.expect("layout").owns("t1"));
}
