mod attach;
mod cli;
mod commands;
#[cfg(unix)]
mod host;
mod resume;
#[cfg_attr(not(unix), allow(dead_code))]
mod screen;
#[cfg_attr(not(unix), allow(dead_code))]
mod tui;
#[cfg_attr(not(unix), allow(dead_code))]
mod view;

pub(crate) const REWRITE_ENV: &str = "LAWN_REWRITE";

pub(crate) fn enabled() -> bool {
    std::env::var_os(REWRITE_ENV).is_some_and(|value| value == "1")
}

pub(crate) const LOG_FILE: &str = "herdr-client.log";

pub(crate) fn log_dir() -> std::path::PathBuf {
    log_dir_in(
        &crate::config::state_dir(),
        crate::session::active_name().as_deref(),
    )
}

fn log_dir_in(state: &std::path::Path, session: Option<&str>) -> std::path::PathBuf {
    let root = state.join("core-client");
    match session {
        Some(name) => root.join("sessions").join(name),
        None => root,
    }
}

fn core() -> std::io::Result<std::sync::Arc<crate::hc::Core>> {
    crate::hc::backend()
        .unwrap_or_else(crate::hc::Core::connect)
        .map_err(std::io::Error::other)
}

pub(crate) fn maybe_run(args: &[String]) -> Option<std::io::Result<()>> {
    if args.len() == 2 && args[1] == "--skill" {
        crate::platform::begin_cli_output();
        print!("{}", include_str!("core_client/SKILL.md"));
        return Some(Ok(()));
    }
    cli::maybe_run(args)
        .or_else(|| resume::maybe_run(args))
        .or_else(|| attach::maybe_run(args))
        .or_else(|| tui::maybe_run(args))
}

#[cfg(test)]
mod tests;
