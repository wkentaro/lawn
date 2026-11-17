use super::*;
use crate::hc::test_support::TestCore;

fn program() -> Vec<String> {
    ["sh", "-c", "sleep 120"].map(str::to_owned).to_vec()
}

#[test]
fn only_one_of_two_racing_clients_starts_the_first_shell() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let started: Vec<Option<Created>> = std::thread::scope(|scope| {
        let racers: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    Lifecycle::new(test.core.clone(), "work")
                        .start_first(Path::new("/tmp"), &program())
                        .unwrap()
                })
            })
            .collect();
        racers
            .into_iter()
            .map(|racer| racer.join().unwrap())
            .collect()
    });

    let created: Vec<_> = started.into_iter().flatten().collect();
    assert_eq!(created.len(), 1);
    let layout = Store::new(test.core.clone(), "work", None)
        .load()
        .unwrap()
        .layout;
    assert_eq!(layout.workspaces.len(), 1);
    assert!(layout.shows(&created[0].terminal));
    assert!(layout.pending_starts.is_empty());
}

#[test]
fn a_session_with_a_workspace_gets_no_first_shell() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let created = lifecycle
        .start_first(Path::new("/tmp"), &program())
        .unwrap()
        .expect("the first shell");

    assert_eq!(
        lifecycle.start_first(Path::new("/tmp"), &program()),
        Ok(None)
    );

    lifecycle.park(&created.terminal).unwrap();
    assert_eq!(
        lifecycle.start_first(Path::new("/tmp"), &program()),
        Ok(None)
    );
    let layout = lifecycle.store.load().unwrap().layout;
    assert_eq!(layout.workspaces.len(), 1);
    assert!(layout.owns(&created.terminal));
}

#[test]
fn a_start_under_way_blocks_the_first_shell() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let reference = lifecycle
        .store
        .reserve_start(&Destination::Workspace { name: None })
        .unwrap();

    assert_eq!(
        lifecycle.start_first(Path::new("/tmp"), &program()),
        Ok(None)
    );
    let layout = lifecycle.store.load().unwrap().layout;
    assert_eq!(
        layout.pending_starts.keys().collect::<Vec<_>>(),
        [&reference]
    );
    assert!(layout.workspaces.is_empty());
}

#[test]
fn a_closed_out_session_gets_a_first_shell_again() {
    let Some(test) = TestCore::start() else {
        return;
    };
    let lifecycle = Lifecycle::new(test.core.clone(), "work");
    let first = lifecycle
        .start_first(Path::new("/tmp"), &program())
        .unwrap()
        .expect("the first shell");
    lifecycle.close(&first.terminal).unwrap();

    let again = lifecycle
        .start_first(Path::new("/tmp"), &program())
        .unwrap()
        .expect("a replacement shell");

    assert_ne!(again.terminal, first.terminal);
    assert!(lifecycle
        .store
        .load()
        .unwrap()
        .layout
        .shows(&again.terminal));
}
