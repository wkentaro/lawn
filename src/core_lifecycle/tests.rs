use super::*;

#[test]
fn remote_matches_never_resolve_a_local_start() {
    assert_eq!(
        local_start(&json!({"terminals": [{"address": "box/t1"}]})).unwrap(),
        None
    );
    assert_eq!(
        local_start(&json!({"terminals": [
            {"address": "box/t1"}, {"address": "t2"}, {"address": "other/t3"}
        ]}))
        .unwrap(),
        Some("t2")
    );
    assert!(local_start(&json!({"terminals": [{"address": "t1"}, {"address": "t2"}]})).is_err());
}

#[cfg(unix)]
mod close;
#[cfg(unix)]
mod first;
#[cfg(unix)]
mod live;
