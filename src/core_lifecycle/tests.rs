use super::*;

#[test]
fn a_start_resolves_only_to_a_terminal_on_its_own_machine() {
    let labels = json!({START_LABEL: "doc:1"});
    let listed = json!({"terminals": [
        {"address": "box/t1", "labels": labels},
        {"address": "t2", "labels": labels},
        {"address": "other/t3", "labels": labels},
    ]});
    assert_eq!(started(&listed, None, "doc:1").unwrap(), Some("t2"));
    assert_eq!(started(&listed, Some("box"), "doc:1").unwrap(), Some("t1"));
    assert_eq!(started(&listed, Some("away"), "doc:1").unwrap(), None);
    let twice = json!({"terminals": [
        {"address": "t1", "labels": labels}, {"address": "t2", "labels": labels},
    ]});
    assert!(started(&twice, None, "doc:1").is_err());
}

#[test]
fn a_start_resolves_only_to_a_terminal_with_its_label() {
    let listed = json!({"terminals": [
        {"address": "box/t1", "labels": {}},
        {"address": "box/t2", "labels": {START_LABEL: "doc:2"}},
    ]});
    assert_eq!(started(&listed, Some("box"), "doc:1").unwrap(), None);
    assert_eq!(started(&listed, Some("box"), "doc:2").unwrap(), Some("t2"));
}

#[test]
fn a_list_confirms_a_machine_only_when_it_holds_the_terminal_seen_there() {
    let listed = json!({
        "terminals": [{"address": "t1"}, {"address": "box/t2"}],
        "machines": [{"name": "box", "id": "idb"}],
    });
    assert!(confirms(&listed, Some("idb"), Some("box"), "box/t2"));
    assert!(confirms(&listed, Some("box"), Some("box"), "box/t2"));
    assert!(confirms(&listed, Some("here"), None, "t1"));
    assert!(!confirms(&listed, Some("idb"), Some("box"), "box/t1"));
    assert!(!confirms(&json!({}), Some("idb"), Some("box"), "box/t2"));
}

#[test]
fn a_list_taken_after_two_machines_swapped_names_confirms_only_the_core_that_held_the_terminal() {
    let listed = json!({
        "terminals": [{"address": "boxa/t2"}, {"address": "boxb/t2"}],
        "machines": [{"name": "boxb", "id": "ida"}, {"name": "boxa", "id": "idb"}],
    });
    assert!(!confirms(&listed, Some("idb"), Some("boxb"), "boxb/t2"));
    assert!(confirms(&listed, Some("idb"), Some("boxa"), "boxa/t2"));
}

#[test]
fn only_a_lost_line_to_a_machine_leaves_a_start_without_a_final_reply() {
    assert!(is_final(None, Some("unavailable")));
    assert!(is_final(Some("boxb"), Some("spawn_failed")));
    assert!(is_final(Some("boxb"), None));
    assert!(!is_final(Some("boxb"), Some("unavailable")));
}

#[test]
fn a_start_on_a_machine_is_named_after_its_whole_reference() {
    assert_eq!(start_name("lawn-sessions:12"), "lawn-sessions-12");
    assert_ne!(
        start_name("lawn-sessions:12"),
        start_name("lawn-dev-sessions:12")
    );
}

#[cfg(unix)]
mod close;
#[cfg(unix)]
mod first;
#[cfg(unix)]
mod linked;
#[cfg(unix)]
mod live;
