use std::process::Command;

#[cfg(unix)]
#[test]
fn navigator_seed_protects_both_production_sessions() {
    let root = std::env::temp_dir().join(format!("lawn-seed-guard-{}", std::process::id()));
    for app in ["lawn", "herdr"] {
        let output = Command::new("bash")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/scripts/seed_navigator_demo.sh"
            ))
            .env("XDG_CONFIG_HOME", &root)
            .env("HERDR_NAV_SOCKET_PATH", root.join(app).join("herdr.sock"))
            .output()
            .expect("run demo seed guard");
        assert_eq!(output.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("refusing to seed main session"),
            "{output:?}"
        );
    }
}

#[test]
fn lawn_uses_its_own_configuration_and_override() {
    let root = std::env::temp_dir().join(format!("lawn-identity-{}", std::process::id()));
    let legacy = root.join("herdr-config.toml");
    let override_path = root.join("lawn-config.toml");
    let app_dir = if cfg!(debug_assertions) {
        "lawn-dev"
    } else {
        "lawn"
    };

    for custom_path in [None, Some(&override_path)] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lawn"));
        command
            .arg("--help")
            .env("XDG_CONFIG_HOME", &root)
            .env("XDG_STATE_HOME", &root)
            .env("HERDR_CONFIG_PATH", &legacy)
            .env_remove("LAWN_CONFIG_PATH")
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_CLIENT_SOCKET_PATH")
            .env_remove("HERDR_SESSION")
            .env_remove("HERDR_ENV");
        if let Some(path) = custom_path {
            command.env("LAWN_CONFIG_PATH", path);
        }
        let output = command.output().expect("run Lawn help");
        assert!(output.status.success(), "{output:?}");
        let help = String::from_utf8(output.stdout).expect("UTF-8 help");
        let expected = custom_path
            .cloned()
            .unwrap_or_else(|| root.join(app_dir).join("config.toml"));
        assert!(help.contains("Usage: lawn [options]"), "{help}");
        assert!(
            help.contains(&format!("Config: {}", expected.display())),
            "{help}"
        );
        assert!(
            !help.contains(&legacy.to_string_lossy().to_string()),
            "{help}"
        );
        assert!(help.contains("LAWN_CONFIG_PATH overrides"), "{help}");
    }
}
