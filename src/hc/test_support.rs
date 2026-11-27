//! A throwaway core for tests that need a real one.

use super::*;

/// A throwaway core under its own `HC_SESSION`, stopped when dropped.
pub(crate) struct TestCore {
    pub(crate) core: Arc<Core>,
    session: String,
    state: std::path::PathBuf,
}

impl TestCore {
    pub(crate) fn start() -> Option<Self> {
        let bin = hc_bin();
        if crate::noninteractive_process::command(&bin)
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping: herdr-core (`hc`) is not installed");
            return None;
        }
        let session = format!(
            "lawn-test-{}-{}",
            std::process::id(),
            NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
        );
        // macOS's temp dir is too deep for a Unix socket path.
        #[cfg(unix)]
        let state = std::path::Path::new("/tmp").join(&session);
        #[cfg(not(unix))]
        let state = std::env::temp_dir().join(&session);
        let core = Core::connect_with(isolated(&session, &state), CALL_TIMEOUT)
            .expect("connect to a fresh core");
        Some(TestCore {
            core,
            session,
            state,
        })
    }

    #[cfg(unix)]
    pub(crate) fn disconnect(&self) {
        crate::platform::signal_processes(&[self.core.child.id()], crate::platform::Signal::Kill);
    }

    pub(crate) fn share(&self) {
        for (name, value) in isolated(&self.session, &self.state).get_envs() {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
        *SHARED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(self.core.clone());
    }
}

fn isolated(session: &str, state: &std::path::Path) -> Command {
    let mut command = crate::noninteractive_process::command(hc_bin());
    #[cfg(unix)]
    command
        .env("HC_SOCKET", state.join("hc.sock"))
        .env("XDG_RUNTIME_DIR", state.join("run"));
    #[cfg(not(unix))]
    command.env_remove("HC_SOCKET");
    command
        .env("HC_SESSION", session)
        .env("XDG_STATE_HOME", state.join("state"))
        .env("XDG_CONFIG_HOME", state.join("config"));
    command
}

static NEXT_SESSION: AtomicU64 = AtomicU64::new(0);

impl Drop for TestCore {
    fn drop(&mut self) {
        let _ = isolated(&self.session, &self.state)
            .args(["server", "stop"])
            .output();
        let _ = std::fs::remove_dir_all(&self.state);
    }
}
