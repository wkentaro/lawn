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
        let number = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        let session = format!("lawn-test-{}-{number}", std::process::id());
        // macOS's temp dir is too deep for a Unix socket path.
        #[cfg(unix)]
        let state = std::path::Path::new("/tmp").join(&session);
        #[cfg(not(unix))]
        let state = std::env::temp_dir().join(&session);
        let identity = state.join("state").join("herdr-core").join(&session);
        let mut private = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut private, 0o700);
        private.create(&state).expect("reserve the test directory");
        private
            .recursive(true)
            .create(&identity)
            .expect("make the core's state");
        let id = format!("{:016x}{number:016x}", std::process::id());
        std::fs::write(
            identity.join("core.json"),
            json!({"id": id, "name": "here"}).to_string(),
        )
        .expect("name the core");
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

    #[cfg(unix)]
    pub(crate) fn link(&self, other: &TestCore, name: &str) -> Link {
        use std::os::unix::fs::PermissionsExt;

        let link = Link {
            name: name.to_owned(),
            down: self.state.join(format!("{name}.down")),
            there: (other.session.clone(), other.state.clone()),
        };
        let via = self.state.join(format!("{name}.sh"));
        let script = [
            b"#!/bin/sh\n[ -e ".as_slice(),
            &quoted(&link.down),
            b" ] && exit 1\nexport HC_SOCKET=",
            &quoted(&other.state.join("hc.sock")),
            b" HC_SESSION=",
            &quoted(other.session.as_ref()),
            b" XDG_STATE_HOME=",
            &quoted(&other.state.join("state")),
            b" XDG_CONFIG_HOME=",
            &quoted(&other.state.join("config")),
            b"\nexec ",
            &quoted(hc_bin().as_ref()),
            b" connect\n",
        ]
        .concat();
        std::fs::write(&via, script).expect("write the link script");
        std::fs::set_permissions(&via, std::fs::Permissions::from_mode(0o755))
            .expect("make the link script runnable");
        self.core
            .call("machine.add", json!({"via": via, "name": name}))
            .expect("link the cores");
        link
    }

    #[cfg(unix)]
    pub(crate) fn becomes(&self, machine: &str, state: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let listed = self.core.call("machine.list", json!({})).expect("machines");
            let machines = listed["machines"].as_array().into_iter().flatten();
            let found = machines
                .filter(|listed| listed["name"] == machine)
                .map(|listed| listed["state"].clone())
                .next();
            if found.as_ref().is_some_and(|found| found == state) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{machine} is {found:?}, not {state}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub(crate) fn share(&self) {
        #[cfg(unix)]
        std::env::set_var("HC_SOCKET", self.state.join("hc.sock"));
        #[cfg(not(unix))]
        std::env::remove_var("HC_SOCKET");
        std::env::set_var("HC_SESSION", &self.session);
        std::env::set_var("XDG_STATE_HOME", self.state.join("state"));
        std::env::set_var("XDG_CONFIG_HOME", self.state.join("config"));
        *SHARED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(self.core.clone());
    }
}

#[cfg(unix)]
pub(crate) struct Link {
    pub(crate) name: String,
    down: std::path::PathBuf,
    there: (String, std::path::PathBuf),
}

#[cfg(unix)]
impl Link {
    pub(crate) fn cut(&self) {
        std::fs::write(&self.down, "").expect("mark the link down");
        let (session, state) = &self.there;
        let restarted = isolated(session, state)
            .args(["server", "restart"])
            .output()
            .expect("restart the linked core");
        assert!(restarted.status.success(), "{restarted:?}");
    }

    pub(crate) fn mend(&self) {
        std::fs::remove_file(&self.down).expect("mark the link up");
    }
}

#[cfg(unix)]
impl Drop for Link {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.down, "");
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

#[cfg(unix)]
pub(super) fn quoted(path: &std::path::Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;

    let mut quoted = vec![b'\''];
    for &byte in path.as_os_str().as_bytes() {
        match byte {
            b'\'' => quoted.extend_from_slice(b"'\\''"),
            byte => quoted.push(byte),
        }
    }
    quoted.push(b'\'');
    quoted
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
