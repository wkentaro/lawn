// Modified for Lawn.
use super::harness::*;

#[test]
fn workspace_and_pane_management_commands_work() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");

    let herdr = spawn_herdr(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let reloaded = run_cli(&socket_path, &["server", "reload-config"]);
    assert!(
        reloaded.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&reloaded.stderr)
    );
    let reload_json: serde_json::Value = serde_json::from_slice(&reloaded.stdout).unwrap();
    assert_eq!(reload_json["result"]["type"], "config_reload");
    assert_eq!(reload_json["result"]["status"], "applied");

    let listed = run_cli(&socket_path, &["workspace", "list"]);
    assert!(listed.status.success());
    let listed_json: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed_json["result"]["type"], "workspace_list");
    assert_eq!(
        listed_json["result"]["workspaces"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    let created = run_cli(
        &socket_path,
        &["workspace", "create", "--cwd", base.to_str().unwrap()],
    );
    assert!(created.status.success());
    let created_json: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let workspace_id = created_json["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();

    let panes = run_cli(&socket_path, &["pane", "list", "--workspace", "1"]);
    assert!(panes.status.success());
    let panes_json: serde_json::Value = serde_json::from_slice(&panes.stdout).unwrap();
    assert_eq!(panes_json["result"]["panes"].as_array().unwrap().len(), 1);

    let split = run_cli(
        &socket_path,
        &["pane", "split", "1-1", "--direction", "right"],
    );
    assert!(
        split.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&split.stderr)
    );
    let split_json: serde_json::Value = serde_json::from_slice(&split.stdout).unwrap();
    let split_pane_id = split_json["result"]["pane"]["pane_id"].as_str().unwrap();

    let fetched = run_cli(&socket_path, &["pane", "get", split_pane_id]);
    assert!(fetched.status.success());
    let fetched_json: serde_json::Value = serde_json::from_slice(&fetched.stdout).unwrap();
    assert_eq!(fetched_json["result"]["pane"]["pane_id"], split_pane_id);

    let closed = run_cli(&socket_path, &["pane", "close", split_pane_id]);
    assert!(closed.status.success());
    let closed_json: serde_json::Value = serde_json::from_slice(&closed.stdout).unwrap();
    assert_eq!(closed_json["result"]["type"], "ok");

    let renamed = run_cli(
        &socket_path,
        &["workspace", "rename", &workspace_id, "demo"],
    );
    assert!(renamed.status.success());
    let renamed_json: serde_json::Value = serde_json::from_slice(&renamed.stdout).unwrap();
    assert_eq!(renamed_json["result"]["workspace"]["label"], "demo");

    let focused = run_cli(&socket_path, &["workspace", "focus", &workspace_id]);
    assert!(focused.status.success());

    let closed_workspace = run_cli(&socket_path, &["workspace", "close", &workspace_id]);
    assert!(closed_workspace.status.success());
    let closed_workspace_json: serde_json::Value =
        serde_json::from_slice(&closed_workspace.stdout).unwrap();
    assert_eq!(closed_workspace_json["result"]["type"], "ok");

    cleanup_spawned_herdr(herdr, base);
}

#[test]
fn config_check_reports_invalid_config_without_server() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let config_dir = config_home.join(app_dir_name());
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        "[keys\nnew_workspace = \"g\"\n",
    )
    .unwrap();

    let checked = run_named_cli(&config_home, &runtime_dir, &["config", "check"]);

    assert_eq!(checked.status.code(), Some(1));
    assert!(
        checked.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let stdout = String::from_utf8_lossy(&checked.stdout);
    assert!(stdout.contains("config: issues found"), "{stdout}");
    assert!(stdout.contains("TOML parse error"), "{stdout}");
    assert!(stdout.contains("line 1"), "{stdout}");

    fs::write(
        config_dir.join("config.toml"),
        "[ui]\nsidebar_min_width = 50\nsidebar_max_width = 30\n",
    )
    .unwrap();
    let checked = run_named_cli(&config_home, &runtime_dir, &["config", "check"]);
    let stdout = String::from_utf8_lossy(&checked.stdout);
    assert_eq!(checked.status.code(), Some(1));
    assert!(stdout.contains("sidebar_min_width (50)"), "{stdout}");
    assert!(stdout.contains("sidebar_max_width (30)"), "{stdout}");

    cleanup_test_base(&base);
}

#[test]
fn config_check_reports_unknown_keys_without_treating_comments_as_keys() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let config_dir = config_home.join(app_dir_name());
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        r#"made_up_toplevel = 1
"made.up" = 2
# commented_out_toplevel = true

[ui]
mouse_capture = true
mouse_captur = false
# commented_out_ui_key = true

[keys]
new_tabb = "ctrl+t"
"#,
    )
    .unwrap();

    let checked = run_named_cli(&config_home, &runtime_dir, &["config", "check"]);

    assert_eq!(checked.status.code(), Some(1));
    assert!(
        checked.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let stdout = String::from_utf8_lossy(&checked.stdout);
    assert!(stdout.contains("config: issues found"), "{stdout}");
    assert!(
        stdout.contains("unknown config key made_up_toplevel; ignoring key"),
        "{stdout}"
    );
    assert!(
        stdout.contains("unknown config key \"made.up\"; ignoring key"),
        "{stdout}"
    );
    assert!(
        stdout.contains("unknown config key ui.mouse_captur; ignoring key"),
        "{stdout}"
    );
    assert!(
        stdout.contains("unknown config key keys.new_tabb; ignoring key"),
        "{stdout}"
    );
    assert!(!stdout.contains("commented_out"), "{stdout}");

    cleanup_test_base(&base);
}

#[test]
fn config_check_reports_unknown_theme_names() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let config_dir = config_home.join(app_dir_name());
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        "[theme]\nname = \"catppucin\"\n",
    )
    .unwrap();

    let checked = run_named_cli(&config_home, &runtime_dir, &["config", "check"]);

    assert_eq!(checked.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&checked.stdout);
    assert!(stdout.contains("config: issues found"), "{stdout}");
    assert!(
        stdout.contains("unknown theme name theme.name = \"catppucin\"; using \"catppuccin\""),
        "{stdout}"
    );
    assert!(stdout.contains("valid themes: catppuccin"), "{stdout}");

    cleanup_test_base(&base);
}

#[test]
fn config_check_reports_unreadable_config_path() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let config_path = config_home.join(app_dir_name()).join("config.toml");
    fs::create_dir_all(&config_path).unwrap();

    let checked = run_named_cli(&config_home, &runtime_dir, &["config", "check"]);

    assert_eq!(checked.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&checked.stdout);
    assert!(stdout.contains("config: issues found"), "{stdout}");
    assert!(stdout.contains("config read error"), "{stdout}");
    assert!(stdout.contains("using defaults"), "{stdout}");

    cleanup_test_base(&base);
}

#[test]
fn config_check_reports_ok_when_config_is_missing() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");

    let checked = run_named_cli(&config_home, &runtime_dir, &["config", "check"]);

    assert!(checked.status.success());
    let stdout = String::from_utf8_lossy(&checked.stdout);
    assert!(stdout.contains("config: ok"), "{stdout}");

    cleanup_test_base(&base);
}

#[test]
fn config_check_rejects_json_output() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");

    let checked = run_named_cli(&config_home, &runtime_dir, &["config", "check", "--json"]);

    assert_eq!(checked.status.code(), Some(2));
    assert!(checked.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&checked.stderr),
        "usage: lawn config check\n"
    );

    cleanup_test_base(&base);
}

#[test]
fn tab_management_commands_work() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");

    let herdr = spawn_herdr(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let created = run_cli(
        &socket_path,
        &["workspace", "create", "--cwd", base.to_str().unwrap()],
    );
    assert!(created.status.success());
    let created_json: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let workspace_id = created_json["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let first_tab_id = created_json["result"]["workspace"]["active_tab_id"]
        .as_str()
        .unwrap()
        .to_string();

    let created_tab = run_cli(
        &socket_path,
        &["tab", "create", "--workspace", &workspace_id],
    );
    assert!(created_tab.status.success());
    let created_tab_json: serde_json::Value = serde_json::from_slice(&created_tab.stdout).unwrap();
    let second_tab_id = created_tab_json["result"]["tab"]["tab_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(second_tab_id, format!("{workspace_id}:t2"));

    let listed_tabs = run_cli(&socket_path, &["tab", "list", "--workspace", &workspace_id]);
    assert!(listed_tabs.status.success());
    let listed_tabs_json: serde_json::Value = serde_json::from_slice(&listed_tabs.stdout).unwrap();
    assert_eq!(
        listed_tabs_json["result"]["tabs"].as_array().unwrap().len(),
        2
    );

    let renamed_tab = run_cli(&socket_path, &["tab", "rename", &second_tab_id, "logs"]);
    assert!(renamed_tab.status.success());
    let renamed_tab_json: serde_json::Value = serde_json::from_slice(&renamed_tab.stdout).unwrap();
    assert_eq!(renamed_tab_json["result"]["tab"]["label"], "logs");

    let focused_tab = run_cli(&socket_path, &["tab", "focus", &first_tab_id]);
    assert!(focused_tab.status.success());
    let focused_tab_json: serde_json::Value = serde_json::from_slice(&focused_tab.stdout).unwrap();
    assert_eq!(focused_tab_json["result"]["tab"]["tab_id"], first_tab_id);

    let tab_get = run_cli(&socket_path, &["tab", "get", &second_tab_id]);
    assert!(tab_get.status.success());
    let tab_get_json: serde_json::Value = serde_json::from_slice(&tab_get.stdout).unwrap();
    assert_eq!(tab_get_json["result"]["tab"]["tab_id"], second_tab_id);

    let closed_tab = run_cli(&socket_path, &["tab", "close", &second_tab_id]);
    assert!(closed_tab.status.success());
    let closed_tab_json: serde_json::Value = serde_json::from_slice(&closed_tab.stdout).unwrap();
    assert_eq!(closed_tab_json["result"]["type"], "ok");

    cleanup_spawned_herdr(herdr, base);
}
