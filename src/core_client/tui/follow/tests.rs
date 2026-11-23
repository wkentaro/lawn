use std::path::Path;
use std::time::Duration;

use tokio::sync::mpsc;

use super::follow;
use crate::core_lifecycle::Lifecycle;
use crate::hc::test_support::TestCore;
use crate::session_doc::Loaded;

const SESSION: &str = "followed";

fn followed() -> mpsc::UnboundedReceiver<Loaded> {
    let (layouts, received) = mpsc::unbounded_channel();
    follow(SESSION.to_owned(), layouts).expect("follow");
    received
}

fn shared() -> Option<TestCore> {
    let test = TestCore::start()?;
    crate::hc::use_core_for_test();
    test.share();
    Some(test)
}

async fn until(
    layouts: &mut mpsc::UnboundedReceiver<Loaded>,
    what: &str,
    wanted: impl Fn(&Loaded) -> bool,
) -> Loaded {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let loaded = layouts.recv().await.expect("the follower stopped");
            if wanted(&loaded) {
                return loaded;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("never saw {what}"))
}

fn start(test: &TestCore) -> String {
    Lifecycle::new(test.core.clone(), SESSION)
        .start_first(Path::new("/"), &["sleep".to_owned(), "60".to_owned()])
        .expect("start")
        .expect("a first shell")
        .terminal
}

#[tokio::test]
async fn the_follower_sees_each_edit_and_follows_again_after_the_core_connection_drops() {
    let Some(test) = shared() else {
        return;
    };
    let terminal = start(&test);
    let mut layouts = followed();
    until(&mut layouts, "the started pane", |loaded| {
        loaded.layout.shows(&terminal)
    })
    .await;

    test.disconnect();
    let core = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match crate::hc::backend() {
                Some(Ok(core)) if !std::sync::Arc::ptr_eq(&core, &test.core) => return core,
                _ => {}
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("a new connection");
    Lifecycle::new(core, SESSION).park(&terminal).expect("park");

    until(&mut layouts, "the parked pane", |loaded| {
        loaded.layout.parked.contains(&terminal)
    })
    .await;
}

#[tokio::test]
async fn a_session_closed_out_while_followed_gets_one_new_shell() {
    let Some(test) = shared() else {
        return;
    };
    let terminal = start(&test);
    let mut layouts = followed();
    until(&mut layouts, "the started pane", |loaded| {
        loaded.layout.shows(&terminal)
    })
    .await;

    Lifecycle::new(test.core.clone(), SESSION)
        .close(&terminal)
        .expect("close");

    let layout = until(&mut layouts, "a new shell", |loaded| {
        !loaded.layout.workspaces.is_empty()
    })
    .await
    .layout;
    let terminals: Vec<_> = layout.terminals().collect();
    assert_eq!(terminals.len(), 1);
    assert_ne!(terminals[0], terminal);
}

#[tokio::test]
async fn the_first_load_after_the_core_connection_drops_is_passed_on_even_at_the_version_already_seen(
) {
    let Some(test) = shared() else {
        return;
    };
    let terminal = start(&test);
    let mut layouts = followed();
    let seen = until(&mut layouts, "the started pane", |loaded| {
        loaded.layout.shows(&terminal)
    })
    .await;

    test.disconnect();

    until(&mut layouts, "the same version again", |loaded| {
        loaded.version == seen.version
    })
    .await;
}
