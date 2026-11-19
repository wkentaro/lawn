use super::{Ending, Endings};

fn owning(terminals: &[&str]) -> Endings {
    let mut endings = Endings::default();
    endings.own(terminals.iter().copied());
    endings
}

fn finished(terminal: &str) -> Ending {
    Ending::Finished(terminal.to_owned())
}

fn snapshot(finished: &[&str], live: &[&str]) -> Ending {
    let set = |terminals: &[&str]| terminals.iter().map(|t| (*t).to_owned()).collect();
    Ending::Snapshot {
        finished: set(finished),
        live: set(live),
    }
}

fn closing(endings: &mut Endings) -> Vec<String> {
    endings.closing(|_| false)
}

#[test]
fn an_exit_seen_for_a_held_pane_survives_its_removal_and_a_snapshot_without_it() {
    let mut endings = owning(&["a"]);
    endings.hear(finished("a"));
    assert!(endings.closing(|_| true).is_empty());

    endings.hear(Ending::Removed("a".to_owned()));
    endings.hear(snapshot(&[], &[]));

    assert_eq!(closing(&mut endings), ["a"]);
}

#[test]
fn a_snapshot_listing_a_due_terminal_as_live_keeps_its_pane_open() {
    let mut endings = owning(&["a"]);
    endings.hear(finished("a"));

    endings.hear(snapshot(&[], &["a"]));

    assert!(closing(&mut endings).is_empty());
}

#[test]
fn a_created_or_restarted_terminal_is_no_longer_due() {
    let mut endings = owning(&["a"]);
    endings.hear(finished("a"));

    endings.hear(Ending::Live("a".to_owned()));

    assert!(closing(&mut endings).is_empty());
}

#[test]
fn an_exit_outside_the_session_closes_nothing() {
    let mut endings = owning(&["a"]);

    endings.hear(finished("z"));
    endings.hear(snapshot(&["y"], &[]));

    assert!(closing(&mut endings).is_empty());
}

#[test]
fn a_layout_that_drops_a_due_terminal_forgets_it_and_owning_it_again_makes_it_due_again() {
    let mut endings = owning(&["a"]);
    endings.hear(finished("a"));
    assert_eq!(closing(&mut endings), ["a"]);

    endings.own([]);
    assert!(closing(&mut endings).is_empty());
    endings.own(["a"]);

    assert_eq!(closing(&mut endings), ["a"]);
}

#[test]
fn a_close_the_view_asked_for_first_is_not_asked_again_by_the_follower() {
    let mut endings = owning(&["a"]);
    assert!(endings.ask("a"));

    endings.hear(finished("a"));

    assert!(closing(&mut endings).is_empty());
    assert!(!endings.ask("a"));
}

#[test]
fn a_close_the_follower_asked_for_first_is_not_asked_again_by_the_view() {
    let mut endings = owning(&["a"]);
    endings.hear(finished("a"));
    assert_eq!(closing(&mut endings), ["a"]);

    assert!(!endings.ask("a"));
}

#[test]
fn asking_for_a_terminal_the_layout_does_not_hold_records_nothing() {
    let mut endings = owning(&[]);
    assert!(endings.ask("z"));
    assert!(endings.ask("z"));

    endings.own(["z"]);
    endings.hear(finished("z"));

    assert_eq!(closing(&mut endings), ["z"]);
}
