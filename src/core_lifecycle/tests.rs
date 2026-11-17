use super::*;
use crate::session_doc::Layout;

#[test]
fn a_schema_one_layout_defaults_to_no_pending_operations() {
    let layout: Layout =
        serde_json::from_value(json!({"workspaces": [], "parked": {}, "next_id": 0})).unwrap();
    assert!(layout.pending_starts.is_empty());
    assert!(layout.closing.is_empty());
}

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
mod live;
