use super::live::{program, record, workspace};
use super::*;
use crate::hc::test_support::{Link, TestCore};
use crate::session_doc::Layout;

const SESSION: &str = "work";

fn linked() -> Option<(TestCore, TestCore, Link)> {
    let (here, there) = (TestCore::start()?, TestCore::start()?);
    let link = here.link(&there, "boxb");
    here.becomes(&link.name, "connected");
    Some((here, there, link))
}

fn far(here: &TestCore) -> Lifecycle {
    Lifecycle::on(here.core.clone(), SESSION, Some("boxb"))
}

fn layout(lifecycle: &Lifecycle) -> Layout {
    lifecycle.store.load().unwrap().layout
}

fn saved(core: &TestCore) -> Sessions {
    Store::new(core.core.clone(), SESSION, None)
        .load_all()
        .unwrap()
        .0
}

fn away(here: &TestCore, link: &Link) {
    link.cut();
    here.becomes(&link.name, "unreachable");
}

fn back(here: &TestCore, link: &Link) {
    link.mend();
    here.core
        .call("machine.connect", json!({"machine": link.name}))
        .unwrap();
    here.becomes(&link.name, "connected");
}

fn run_unacknowledged(core: &TestCore, reference: &str) -> String {
    core.core
        .run(json!({
            "argv": program(), "keep": true, "no_parent": true,
            "labels": {START_LABEL: reference}, "name": start_name(reference),
        }))
        .unwrap()
}

#[test]
fn a_start_on_a_linked_machine_is_saved_on_that_machine() {
    let Some((here, there, _link)) = linked() else {
        return;
    };

    let created = far(&here).start(workspace(), None, &program()).unwrap();

    assert_eq!(created.terminal, "t1");
    assert_eq!(record(&there.core, "t1")["keep"], true);
    let there = &saved(&there)[SESSION];
    assert!(there.shows("t1"));
    assert!(there.pending_starts.is_empty());
    assert!(saved(&here).is_empty());
}

#[test]
fn a_start_whose_answer_was_lost_is_recovered_from_its_machine() {
    let Some((here, there, _link)) = linked() else {
        return;
    };
    let reference = far(&here).store.reserve_start(&workspace()).unwrap();
    let terminal = run_unacknowledged(&there, &reference);

    far(&here).recover().unwrap();

    let layout = layout(&far(&here));
    assert!(layout.shows(&terminal), "{terminal}: {layout:?}");
    assert!(layout.pending_starts.is_empty());
}

#[test]
fn a_start_made_on_a_machine_itself_is_found_from_a_core_linked_to_it() {
    let Some((here, there, _link)) = linked() else {
        return;
    };
    let near = Lifecycle::new(there.core.clone(), SESSION);
    let terminal = near.start(workspace(), None, &program()).unwrap().terminal;
    let started = record(&there.core, &terminal);
    let reference = started["labels"][START_LABEL].as_str().unwrap();

    assert_eq!(far(&here).lookup(reference).unwrap(), Some(terminal));
}

#[test]
fn a_start_is_recovered_only_from_its_own_machine() {
    let Some((here, _there, _link)) = linked() else {
        return;
    };
    let reference = far(&here).store.reserve_start(&workspace()).unwrap();
    let terminal = run_unacknowledged(&here, &reference);

    far(&here).recover().unwrap();

    let layout = layout(&far(&here));
    assert!(!layout.owns(&terminal));
    assert!(layout.pending_starts.contains_key(&reference));
    assert!(saved(&here).is_empty());
}

#[test]
fn a_terminal_that_only_took_the_name_of_a_start_is_not_placed() {
    let Some((here, there, _link)) = linked() else {
        return;
    };
    let reference = far(&here).store.reserve_start(&workspace()).unwrap();
    let terminal = there
        .core
        .run(json!({"argv": program(), "name": start_name(&reference)}))
        .unwrap();

    far(&here).recover().unwrap();

    let layout = layout(&far(&here));
    assert!(!layout.owns(&terminal));
    assert!(layout.pending_starts.contains_key(&reference));
}

#[test]
fn a_start_refused_for_its_name_does_not_block_the_next_start() {
    let Some((here, there, _link)) = linked() else {
        return;
    };
    let reference = far(&here).store.reserve_start(&workspace()).unwrap();
    far(&here).drop_start(&reference).unwrap();
    let (document, version) = reference.rsplit_once(':').unwrap();
    let version = version.parse::<u64>().unwrap();
    let holders = (version + 1..version + 4).map(|next| {
        let name = start_name(&format!("{document}:{next}"));
        there
            .core
            .run(json!({"argv": program(), "name": name}))
            .unwrap()
    });
    let holders = holders.collect::<Vec<_>>();

    let error = far(&here).start(workspace(), None, &program()).unwrap_err();

    assert!(error.contains("already named"), "{error}");
    assert!(layout(&far(&here)).pending_starts.is_empty());
    for holder in holders {
        there
            .core
            .call("terminal.kill", json!({"terminal": holder}))
            .unwrap();
    }
    let created = far(&here).start(workspace(), None, &program()).unwrap();
    assert!(layout(&far(&here)).owns(&created.terminal));
}

#[test]
fn a_start_that_made_no_terminal_is_dropped_while_another_terminal_holds_its_name() {
    let Some((here, there, _link)) = linked() else {
        return;
    };
    let reference = far(&here).store.reserve_start(&workspace()).unwrap();
    there
        .core
        .run(json!({"argv": program(), "name": start_name(&reference)}))
        .unwrap();

    far(&here).drop_start(&reference).unwrap();

    assert!(layout(&far(&here)).pending_starts.is_empty());
}

#[test]
fn a_start_that_ended_is_not_dropped_while_another_terminal_holds_its_name() {
    let Some((here, there, _link)) = linked() else {
        return;
    };
    let reference = far(&here).store.reserve_start(&workspace()).unwrap();
    let ended = there
        .core
        .run(json!({
            "argv": ["true"], "keep": true, "no_parent": true,
            "labels": {START_LABEL: reference}, "name": start_name(&reference),
        }))
        .unwrap();
    while record(&there.core, &ended)["state"] == "live" {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    there
        .core
        .run(json!({"argv": program(), "name": start_name(&reference)}))
        .unwrap();

    let error = far(&here).drop_start(&reference).unwrap_err();

    assert!(error.contains("is not pending"), "{error}");
    let layout = layout(&far(&here));
    assert!(layout.owns(&ended), "{layout:?}");
    assert!(layout.pending_starts.is_empty());
}

#[test]
fn a_start_on_a_machine_that_is_away_starts_nothing() {
    let Some((here, _there, link)) = linked() else {
        return;
    };
    away(&here, &link);

    let error = far(&here).start(workspace(), None, &program()).unwrap_err();

    assert!(error.contains("unreachable"), "{error}");
    back(&here, &link);
    assert_eq!(layout(&far(&here)), Layout::default());
    let listed = here.core.call("terminal.list", json!({"all": true}));
    assert_eq!(listed.unwrap()["terminals"], json!([]));
}

#[test]
fn a_start_stays_pending_while_its_machine_is_away() {
    let Some((here, there, link)) = linked() else {
        return;
    };
    let reference = far(&here).store.reserve_start(&workspace()).unwrap();
    let terminal = run_unacknowledged(&there, &reference);
    away(&here, &link);

    let error = far(&here).recover().unwrap_err();

    assert!(error.contains("unreachable"), "{error}");
    back(&here, &link);
    assert!(layout(&far(&here)).pending_starts.contains_key(&reference));
    far(&here).recover().unwrap();
    assert!(layout(&far(&here)).shows(&terminal));
}

#[test]
fn a_start_its_machine_refused_does_not_block_the_next() {
    let Some((here, _there, _link)) = linked() else {
        return;
    };
    let missing = Path::new("/no/such/folder/on/boxb");

    let error = far(&here)
        .start(workspace(), Some(missing), &program())
        .unwrap_err();

    assert!(!error.contains("unconfirmed"), "{error}");
    assert!(layout(&far(&here)).pending_starts.is_empty());
    far(&here).start(workspace(), None, &program()).unwrap();
}

#[test]
fn a_close_waits_for_a_machine_that_is_away() {
    let Some((here, _there, link)) = linked() else {
        return;
    };
    let terminal = far(&here)
        .start(workspace(), None, &program())
        .unwrap()
        .terminal;
    away(&here, &link);

    let error = far(&here).close(&terminal).unwrap_err();

    assert!(error.contains("unreachable"), "{error}");
    back(&here, &link);
    let seen = format!("boxb/{terminal}");
    assert!(layout(&far(&here)).shows(&terminal));
    assert_eq!(record(&here.core, &seen)["state"], "live");
    far(&here).close(&terminal).unwrap();
    assert!(!layout(&far(&here)).owns(&terminal));
    assert_ne!(record(&here.core, &seen)["state"], "live");
}

#[test]
fn a_pane_on_a_linked_machine_parks_and_returns() {
    let Some((here, there, _link)) = linked() else {
        return;
    };
    let lifecycle = far(&here);
    let terminal = lifecycle
        .start(workspace(), None, &program())
        .unwrap()
        .terminal;

    lifecycle.park(&terminal).unwrap();

    assert!(layout(&lifecycle).parked.contains(&terminal));
    assert_eq!(record(&there.core, &terminal)["keep"], false);
    assert_eq!(lifecycle.unpark(&terminal, None).unwrap(), terminal);
    assert!(layout(&lifecycle).shows(&terminal));
    assert_eq!(record(&there.core, &terminal)["keep"], true);
}

#[test]
fn a_terminal_started_outside_lawn_is_shown_on_its_own_machine() {
    let Some((here, there, _link)) = linked() else {
        return;
    };
    let lifecycle = far(&here);
    lifecycle.start(workspace(), None, &program()).unwrap();
    let outside = there.core.run(json!({"argv": program()})).unwrap();
    here.core.run(json!({"argv": program()})).unwrap();
    let same = here.core.run(json!({"argv": program()})).unwrap();

    assert_eq!(lifecycle.unpark(&outside, None).unwrap(), outside);

    assert!(layout(&lifecycle).shows(&outside));
    assert_eq!(record(&there.core, &outside)["keep"], true);
    assert_eq!(outside, same);
    assert_eq!(record(&here.core, &same)["keep"], false);
    assert!(saved(&here).is_empty());
}

#[test]
fn a_pane_is_closed_on_its_own_machine() {
    let Some((here, there, _link)) = linked() else {
        return;
    };
    let near = Lifecycle::new(here.core.clone(), SESSION);
    let kept = near
        .start(workspace(), Some(Path::new("/tmp")), &program())
        .unwrap();
    let gone = far(&here).start(workspace(), None, &program()).unwrap();
    assert_eq!(kept.terminal, gone.terminal);

    far(&here).close(&gone.terminal).unwrap();

    assert_ne!(record(&there.core, &gone.terminal)["state"], "live");
    assert_eq!(record(&here.core, &kept.terminal)["state"], "live");
    assert!(layout(&near).shows(&kept.terminal));
}

#[test]
fn a_renamed_machine_keeps_its_panes() {
    let Some((here, there, _link)) = linked() else {
        return;
    };
    let terminal = far(&here)
        .start(workspace(), None, &program())
        .unwrap()
        .terminal;
    here.core
        .call(
            "machine.rename",
            json!({"machine": "boxb", "new_name": "boxz"}),
        )
        .unwrap();
    here.becomes("boxz", "connected");
    let renamed = Lifecycle::on(here.core.clone(), SESSION, Some("boxz"));

    assert!(layout(&renamed).shows(&terminal));
    renamed.close(&terminal).unwrap();

    assert_ne!(record(&there.core, &terminal)["state"], "live");
}

#[test]
fn a_machine_renamed_in_the_middle_of_a_command_is_still_the_one_reached() {
    let Some((here, there, _link)) = linked() else {
        return;
    };
    let id = here.core.machines().unwrap().remove(0).id;
    let by_id = Lifecycle::on(here.core.clone(), SESSION, Some(&id));
    let terminal = by_id.start(workspace(), None, &program()).unwrap().terminal;
    here.core
        .call(
            "machine.rename",
            json!({"machine": "boxb", "new_name": "boxz"}),
        )
        .unwrap();

    by_id.close(&terminal).unwrap();

    assert_ne!(record(&there.core, &terminal)["state"], "live");
    assert!(layout(&by_id).closing.is_empty());
}

#[test]
fn a_removed_machine_leaves_nothing_behind_here() {
    let Some((here, there, _link)) = linked() else {
        return;
    };
    let terminal = far(&here)
        .start(workspace(), None, &program())
        .unwrap()
        .terminal;
    here.core
        .call("machine.rm", json!({"machine": "boxb"}))
        .unwrap();

    let error = far(&here).close(&terminal).unwrap_err();

    assert!(error.contains("boxb"), "{error}");
    assert!(saved(&here).is_empty());
    assert!(saved(&there)[SESSION].shows(&terminal));
    assert_eq!(record(&there.core, &terminal)["state"], "live");
}
