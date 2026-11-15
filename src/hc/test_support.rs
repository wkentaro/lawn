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

    /// Makes this core the one `backend()` returns, for code that looks it up.
    pub(crate) fn share(&self) {
        *SHARED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(self.core.clone());
    }
}

/// An `hc` that can't reach the user's core: its own session, state,
/// config and socket.
fn isolated(session: &str, state: &std::path::Path) -> Command {
    let mut command = crate::noninteractive_process::command(hc_bin());
    #[cfg(unix)]
    command.env("HC_SOCKET", state.join("hc.sock"));
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
