use super::*;
use crate::hc::test_support::TestCore;

struct PausedHolder(String);

impl PausedHolder {
    fn pause(core: &Core) -> Self {
        let status = core.call("server.status", json!({})).unwrap();
        let pid = status["holder"]["pid"].as_u64().unwrap().to_string();
        assert!(std::process::Command::new("kill")
            .args(["-STOP", &pid])
            .status()
            .unwrap()
            .success());
        Self(pid)
    }
}

impl Drop for PausedHolder {
    fn drop(&mut self) {
        let _ = std::process::Command::new("kill")
            .args(["-CONT", &self.0])
            .status();
    }
}

#[test]
fn a_live_kill_reply_keeps_the_close_reservation() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let created = lifecycle
        .start(
            Destination::Workspace { name: None },
            Some(Path::new("/tmp")),
            &["sh".to_owned(), "-c".to_owned(), "sleep 120".to_owned()],
        )
        .unwrap();
    let paused = PausedHolder::pause(&test.core);

    let error = lifecycle.close(&created.terminal).unwrap_err();

    assert!(error.contains("has not ended"), "{error}");
    assert!(lifecycle
        .store
        .load()
        .unwrap()
        .layout
        .closing
        .contains(&created.terminal));
    assert!(Lifecycle::new(test.core.clone(), "other")
        .unpark(&created.terminal, None)
        .is_err());
    drop(paused);
    lifecycle.recover().unwrap();
    assert!(lifecycle.store.load().unwrap().layout.closing.is_empty());
}
