//! The layout kept in a herdr core document, where it outlives Lawn and any
//! process on this machine can read it.

use std::sync::Mutex;

use super::snapshot::{parse_snapshot, SessionSnapshot};

/// The document version this Lawn's layout came from or last wrote. A save
/// replaces only that version, so it never overwrites a layout it did not
/// load: another writer's edit, one a failed read missed, or one from a
/// newer Lawn.
static VERSION: Mutex<Option<u64>> = Mutex::new(None);

fn version() -> std::sync::MutexGuard<'static, Option<u64>> {
    VERSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(super) fn name() -> String {
    let session = crate::session::active_name()
        .unwrap_or_else(|| crate::session::DEFAULT_SESSION_NAME.to_owned());
    name_for(&session)
}

/// Core names are 1-32 of `a-z 0-9 - _ .`, and session names may be longer
/// or uppercase. Those get a hash after a `.`, which a readable name, with
/// its `-`, never has.
fn name_for(session: &str) -> String {
    use sha2::{Digest, Sha256};
    let readable = format!("lawn-layout-{session}");
    if readable.len() <= 32 && !session.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return readable;
    }
    let digest = format!("{:x}", Sha256::digest(session.as_bytes()));
    format!("lawn-layout.{}", &digest[..16])
}

/// The layout the document holds, `Some(None)` once it was cleared. `None`
/// when panes don't run in core, the core can't answer, or nothing was
/// written yet: the caller then reads the session file, once.
pub(super) fn load() -> Option<Option<SessionSnapshot>> {
    let core = crate::hc::backend()?
        .inspect_err(|err| tracing::warn!(%err, "cannot read the layout from herdr core"))
        .ok()?;
    let name = name();
    let doc = core
        .doc_get(&name, None)
        .inspect_err(|err| tracing::warn!(%err, name, "cannot read the layout document"))
        .ok()?;
    if doc.version == 0 {
        *version() = Some(0);
        return None;
    }
    if doc.value.is_null() {
        *version() = Some(doc.version);
        return Some(None);
    }
    match parse_snapshot(&doc.value.to_string()) {
        Ok(snapshot) => {
            *version() = Some(doc.version);
            Some(Some(snapshot))
        }
        Err(err) => {
            tracing::warn!(%err, name, "cannot parse the layout document; leaving it alone");
            None
        }
    }
}

/// Writes the layout, or clears it with `None`, over the version this Lawn
/// holds. Does nothing when panes don't run in core.
pub(super) fn save(snapshot: Option<&SessionSnapshot>) -> Result<(), String> {
    let Some(core) = crate::hc::backend() else {
        return Ok(());
    };
    let core = core.map_err(|err| err.to_string())?;
    let name = name();
    let mut held = version();
    let base = match *held {
        Some(base) => base,
        // Nothing loaded: only a document nobody wrote is free to take.
        None => match core.doc_get(&name, None).map_err(|err| err.to_string())? {
            doc if doc.version == 0 => 0,
            _ => return Err(format!("{name} holds a layout this Lawn did not load")),
        },
    };
    let value = serde_json::to_value(snapshot).map_err(|err| err.to_string())?;
    match core.doc_set(&name, None, &value, base) {
        Ok(next) => {
            *held = Some(next);
            Ok(())
        }
        Err(err) if err.kind() == Some("conflict") => {
            Err(format!("{name} changed elsewhere; not replacing it"))
        }
        Err(err) => Err(err.to_string()),
    }
}

/// Clears a deleted session's layout, so reusing its name starts fresh.
pub(crate) fn clear(session: &str) -> Result<(), String> {
    let Some(core) = crate::hc::backend() else {
        return Ok(());
    };
    let core = core.map_err(|err| err.to_string())?;
    let name = name_for(session);
    let current = core
        .doc_get(&name, None)
        .map_err(|err| err.to_string())?
        .version;
    core.doc_set(&name, None, &serde_json::Value::Null, current)
        .map(drop)
        .map_err(|err| err.to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn snapshot(sidebar_width: u16) -> SessionSnapshot {
        SessionSnapshot {
            version: super::super::snapshot::SNAPSHOT_VERSION,
            workspaces: vec![],
            active: None,
            selected: 0,
            sidebar_width: Some(sidebar_width),
            sidebar_section_split: None,
            collapsed_space_keys: Default::default(),
        }
    }

    #[test]
    fn every_session_name_gets_its_own_valid_core_name() {
        let names = [
            "default",
            "Work",
            "work",
            "a-lowercase-name-of-21",
            &"x".repeat(64),
        ]
        .map(name_for);
        for name in &names {
            assert!(name.len() <= 32, "{name}");
            assert!(
                name.bytes().all(|b| b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || matches!(b, b'-' | b'_' | b'.')),
                "{name}"
            );
        }
        assert_eq!(names[0], "lawn-layout-default");
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "{names:?}");
    }

    #[test]
    fn a_hashed_name_round_trips_and_clears() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        test.share();
        crate::hc::use_core_for_test();
        let session = "Long-Session-Name-That-Core-Cannot-Take";
        // Each nextest test runs in its own process.
        std::env::set_var(crate::session::SESSION_ENV_VAR, session);

        save(Some(&snapshot(31))).expect("save");
        let loaded = load().expect("written").expect("not cleared");
        assert_eq!(loaded.sidebar_width, Some(31));
        clear(session).expect("delete the session");
        assert!(matches!(load(), Some(None)));
    }

    #[test]
    fn layout_round_trips_through_the_document() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        test.share();
        crate::hc::use_core_for_test();
        assert!(
            load().is_none(),
            "an unwritten document leaves it to the file"
        );

        save(Some(&snapshot(31))).expect("save");
        save(Some(&snapshot(32))).expect("save over it");
        let loaded = load().expect("written").expect("not cleared");
        assert_eq!(loaded.sidebar_width, Some(32));

        save(None).expect("clear");
        assert!(
            matches!(load(), Some(None)),
            "a cleared document is no layout"
        );

        save(Some(&snapshot(33))).expect("save");
        clear(crate::session::DEFAULT_SESSION_NAME).expect("delete the session");
        assert!(
            matches!(load(), Some(None)),
            "a deleted session starts fresh"
        );
    }

    #[test]
    fn a_save_never_replaces_a_layout_it_did_not_load() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        test.share();
        crate::hc::use_core_for_test();
        let stored = |test: &crate::hc::test_support::TestCore| {
            test.core.doc_get(&name(), None).expect("doc.get").value
        };

        // A newer Lawn's layout: the file is read, and the document kept.
        let newer = serde_json::json!({"version": u32::MAX, "workspaces": [], "selected": 0});
        test.core
            .doc_set(&name(), None, &newer, 0)
            .expect("doc.set");
        assert!(load().is_none());
        assert!(save(Some(&snapshot(31))).is_err());
        assert_eq!(stored(&test), newer);

        // Another writer's edit after this Lawn loaded.
        clear(crate::session::DEFAULT_SESSION_NAME).expect("clear");
        load();
        save(Some(&snapshot(31))).expect("save");
        let theirs = serde_json::to_value(snapshot(40)).expect("json");
        let current = test.core.doc_get(&name(), None).expect("doc.get").version;
        test.core
            .doc_set(&name(), None, &theirs, current)
            .expect("doc.set");
        assert!(save(Some(&snapshot(32))).is_err());
        assert_eq!(stored(&test), theirs);
    }
}
