mod attach;
mod cli;
mod commands;
#[cfg_attr(not(unix), allow(dead_code))]
mod screen;
#[cfg_attr(not(unix), allow(dead_code))]
mod view;

pub(crate) const REWRITE_ENV: &str = "LAWN_REWRITE";

pub(crate) fn enabled() -> bool {
    std::env::var_os(REWRITE_ENV).is_some_and(|value| value == "1")
}

pub(crate) fn maybe_run(args: &[String]) -> Option<std::io::Result<()>> {
    cli::maybe_run(args).or_else(|| attach::maybe_run(args))
}

#[cfg(test)]
mod tests;
