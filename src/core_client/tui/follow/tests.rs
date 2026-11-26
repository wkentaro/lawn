use std::path::Path;
use std::time::Duration;

use tokio::sync::mpsc;

use super::{follow, Follower, Stale};
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

fn core_id(here: &TestCore, machine: &str) -> String {
    let cores = here.core.cores().expect("the cores");
    let id = crate::hc::core_id(&cores, machine).expect("a core id");
    id.to_owned()
}

fn start_on(here: &TestCore, machine: &str) -> String {
    let workspace = crate::session_doc::Destination::Workspace { name: None };
    let program = ["sleep".to_owned(), "60".to_owned()];
    Lifecycle::on(here.core.clone(), SESSION, Some(machine))
        .start(workspace, None, &program)
        .expect("start")
        .terminal
}

#[test]
fn a_document_event_marks_only_the_machine_it_came_from() {
    let mut stale = Stale::default();
    let edited = |document: &str, machine: Option<&str>| {
        let params =
            serde_json::json!({"type": "doc", "data": {"name": document}, "machine": machine});
        format!(
            "{}\n",
            serde_json::json!({"method": "event", "params": params})
        )
    };
    let sessions = crate::session_doc::name();

    stale.heard(b"{\"method\":\"event\",\"params\":{\"type\":\"terminal\"}}\n");
    stale.heard(edited("another-document", Some("boxb")).as_bytes());
    assert!(!stale.changed);
    let far = edited(&sessions, Some("boxb"));
    let (start, end) = far.split_at(40);
    stale.heard(start.as_bytes());
    assert!(stale.machines.is_empty());
    stale.heard(end.as_bytes());
    stale.heard(edited(&sessions, None).as_bytes());
    assert_eq!(Vec::from_iter(&stale.machines), ["boxb"]);
    assert!(stale.changed && !stale.every_machine);

    stale.heard(b"{\"method\":\"missed\",\"params\":{}}\n");
    assert!(stale.every_machine);
}

#[tokio::test]
async fn a_linked_machines_workspaces_are_followed_under_its_core_id() {
    let (Some(here), Some(there)) = (shared(), TestCore::start()) else {
        return;
    };
    let _link = here.link(&there, "boxb");
    here.becomes("boxb", "connected");
    let id = core_id(&here, "boxb");
    assert_eq!(start_on(&here, "boxb"), "t1");
    let mut layouts = followed();
    until(&mut layouts, "the machine's pane", |loaded| {
        loaded.layout.shows(&format!("{id}/t1"))
    })
    .await;

    assert_eq!(start_on(&here, "boxb"), "t2");

    let loaded = until(&mut layouts, "the machine's next pane", |loaded| {
        loaded.layout.shows(&format!("{id}/t2"))
    })
    .await;
    assert_eq!(loaded.layout.workspaces.len(), 2);
    assert!(!loaded.layout.shows("t1"));
}

#[tokio::test]
async fn a_linked_machine_that_does_not_answer_holds_back_no_layout_of_this_machine() {
    let (Some(here), Some(there)) = (shared(), TestCore::start()) else {
        return;
    };
    let _link = here.link(&there, "boxb");
    here.becomes("boxb", "connected");
    let far = format!("{}/{}", core_id(&here, "boxb"), start_on(&here, "boxb"));
    let stalled = there.stall();
    let mut layouts = followed();
    until(&mut layouts, "the first layout", |_| true).await;

    let terminal = start(&here);

    let seen = until(&mut layouts, "this machine's pane", |loaded| {
        loaded.layout.shows(&terminal)
    });
    let loaded = tokio::time::timeout(Duration::from_secs(3), seen)
        .await
        .expect("this machine's pane waited for the linked machine");
    assert!(!loaded.layout.shows(&far));
    drop(stalled);
    until(&mut layouts, "the machine's pane", |loaded| {
        loaded.layout.shows(&far)
    })
    .await;
}

#[tokio::test]
async fn a_renamed_machine_keeps_its_workspaces_and_brings_no_new_shell() {
    let (Some(here), Some(there)) = (shared(), TestCore::start()) else {
        return;
    };
    let _link = here.link(&there, "boxb");
    here.becomes("boxb", "connected");
    let id = core_id(&here, "boxb");
    start_on(&here, "boxb");
    let mut layouts = followed();
    until(&mut layouts, "the machine's pane", |loaded| {
        loaded.layout.shows(&format!("{id}/t1"))
    })
    .await;

    let renamed = serde_json::json!({"machine": "boxb", "new_name": "boxc"});
    here.core
        .call("machine.rename", renamed)
        .expect("rename the machine");
    start_on(&here, "boxc");

    let loaded = until(
        &mut layouts,
        "the pane started after the rename",
        |loaded| loaded.layout.shows(&format!("{id}/t2")),
    )
    .await;
    assert!(loaded.layout.shows(&format!("{id}/t1")));
    let near = Lifecycle::new(here.core.clone(), SESSION).layout();
    assert_eq!(near.expect("this machine's layout").workspaces, []);
}

#[tokio::test]
async fn a_machine_whose_core_id_names_another_machine_is_followed_only_under_its_core_id() {
    let (Some(here), Some(there), Some(third)) = (shared(), TestCore::start(), TestCore::start())
    else {
        return;
    };
    let (_link, _other) = (here.link(&there, "boxb"), here.link(&third, "boxc"));
    here.becomes("boxb", "connected");
    here.becomes("boxc", "connected");
    let (id, other) = (core_id(&here, "boxb"), core_id(&here, "boxc"));
    start_on(&here, "boxb");
    let (layouts, _received) = mpsc::unbounded_channel::<Loaded>();
    let follower = Follower {
        session: SESSION.to_owned(),
        layouts,
        shown: std::sync::Mutex::default(),
    };
    let followed = || {
        let mut stale = Stale {
            every_machine: true,
            ..Stale::default()
        };
        assert!(follower.reload(&here.core, &mut stale));
        Vec::from_iter(follower.shown().far.keys().cloned())
    };
    let rename = |from: &str, to: &str| {
        let renamed = serde_json::json!({"machine": from, "new_name": to});
        here.core
            .call("machine.rename", renamed)
            .expect("rename the machine");
        here.becomes(to, "connected");
    };

    rename("boxc", &id);
    assert_eq!(followed(), [other]);

    rename(&id, "boxc");
    assert!(followed().contains(&id));
}
