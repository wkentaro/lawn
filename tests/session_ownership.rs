#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = PathBuf::from(format!(
            "/tmp/lo-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("config/lawn-dev")).unwrap();
        fs::write(
            root.join("config/lawn-dev/config.toml"),
            "onboarding = false\n",
        )
        .unwrap();
        Self { root }
    }

    fn command(&self, endpoint: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lawn"));
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("SHELL", "/bin/sh")
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_RUNTIME_DIR", self.root.join("runtime"))
            .env("HC_SOCKET", self.root.join("core.sock"))
            .env("HERDR_SESSION", "work")
            .env("LAWN_PTY_PANES", "1")
            .env(
                "HERDR_SOCKET_PATH",
                self.root.join(format!("{endpoint}.sock")),
            )
            .env(
                "HERDR_CLIENT_SOCKET_PATH",
                self.root.join(format!("{endpoint}-client.sock")),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    fn start(&self, endpoint: &str) -> Server {
        Server(self.command(endpoint).arg("server").spawn().unwrap())
    }

    fn wait_ready(&self, endpoint: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(mut stream) = UnixStream::connect(self.root.join(format!("{endpoint}.sock")))
            {
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                writeln!(
                    stream,
                    "{{\"id\":\"ready\",\"method\":\"ping\",\"params\":{{}}}}"
                )
                .unwrap();
                let mut line = String::new();
                if BufReader::new(stream).read_line(&mut line).is_ok() && line.contains("pong") {
                    return;
                }
            }
            assert!(Instant::now() < deadline, "server did not become ready");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_held_offline_guard_prevents_socket_publication() {
    let fixture = Fixture::new();
    let directory = fixture.root.join("config/lawn-dev/session-locks");
    fs::create_dir_all(&directory).unwrap();
    let owner = fs::File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("work.lock"))
        .unwrap();
    owner.try_lock().unwrap();
    let mut server = fixture.start("blocked");
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = server.0.try_wait().unwrap() {
            assert!(!status.success());
            assert!(!fixture.root.join("blocked.sock").exists());
            assert!(!fixture
                .root
                .join("config/lawn-dev/sessions/work/session.json")
                .exists());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "server did not refuse offline ownership"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(owner);
    let _server = fixture.start("available");
    fixture.wait_ready("available");
}

#[test]
fn deletion_and_recreation_keep_the_same_lock_inode() {
    use std::os::unix::fs::MetadataExt;

    let fixture = Fixture::new();
    let lock_directory = fixture.root.join("config/lawn-dev/session-locks");
    let session_directory = fixture.root.join("config/lawn-dev/sessions/work");
    fs::create_dir_all(&lock_directory).unwrap();
    fs::create_dir_all(&session_directory).unwrap();
    let path = lock_directory.join("work.lock");
    let owner = fs::File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let inode = owner.metadata().unwrap().ino();
    assert!(fixture
        .command("unused")
        .args(["session", "delete", "work"])
        .status()
        .unwrap()
        .success());
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    fs::create_dir_all(&session_directory).unwrap();
    owner.try_lock().unwrap();
    assert!(!fixture
        .command("unused")
        .args(["session", "delete", "work"])
        .status()
        .unwrap()
        .success());
    assert!(session_directory.is_dir());
}

#[test]
fn different_sockets_cannot_open_the_same_session() {
    let fixture = Fixture::new();
    let _first = fixture.start("first");
    fixture.wait_ready("first");
    let mut second = fixture.start("second");
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = second.0.try_wait().unwrap() {
            assert!(!status.success());
            assert!(!fixture.root.join("second.sock").exists());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "a second server opened the same session"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    fixture.wait_ready("first");
}

#[test]
fn restoring_a_session_refuses_its_custom_socket_server() {
    let fixture = Fixture::new();
    let _server = fixture.start("custom");
    fixture.wait_ready("custom");
    let path = fixture
        .root
        .join("config/lawn-dev/sessions/work/session.json");
    let before = fs::read(&path).ok();
    let source = fixture.root.join("restore.json");
    fs::write(
        &source,
        r#"{"version":3,"workspaces":[],"active":null,"selected":0,"sidebar_width":42}"#,
    )
    .unwrap();
    let result = fixture
        .command("unused")
        .args(["session", "restore", "work"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(!result.status.success(), "restored a running session");
    assert_eq!(fs::read(&path).ok(), before);
    fixture.wait_ready("custom");
}

#[test]
fn deleting_a_session_refuses_its_custom_socket_server() {
    let fixture = Fixture::new();
    let _server = fixture.start("custom");
    fixture.wait_ready("custom");
    let directory = fixture.root.join("config/lawn-dev/sessions/work");
    assert!(directory.is_dir());
    let result = fixture
        .command("unused")
        .args(["session", "delete", "work"])
        .output()
        .unwrap();
    assert!(!result.status.success(), "deleted a running session");
    assert!(directory.is_dir());
    fixture.wait_ready("custom");
}
