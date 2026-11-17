mod attach;
mod cli;
mod commands;
mod resume;
#[cfg_attr(not(unix), allow(dead_code))]
mod screen;
#[cfg_attr(not(unix), allow(dead_code))]
mod view;

pub(crate) const REWRITE_ENV: &str = "LAWN_REWRITE";

pub(crate) fn enabled() -> bool {
    std::env::var_os(REWRITE_ENV).is_some_and(|value| value == "1")
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
}

#[cfg(test)]
mod tests;
