mod cli;
mod commands;

pub(crate) use cli::maybe_run;

pub(crate) const REWRITE_ENV: &str = "LAWN_REWRITE";

pub(crate) fn enabled() -> bool {
    std::env::var_os(REWRITE_ENV).is_some_and(|value| value == "1")
}

#[cfg(test)]
mod tests;
