use std::fs::File;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::snapshot::{parse_snapshot, SessionSnapshot};

static VERSION: Mutex<Option<u64>> = Mutex::new(None);

fn version() -> std::sync::MutexGuard<'static, Option<u64>> {
    VERSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_path() -> std::path::PathBuf {
    crate::session::data_dir().join("layout.lock")
}

fn lock_file() -> std::io::Result<File> {
    super::io::open_lock_file(&lock_path())
}

pub(crate) fn own() -> std::io::Result<Option<File>> {
    if !crate::hc::enabled() {
        return Ok(None);
    }
    own_within(Duration::from_secs(5)).map(Some)
}

fn own_within(wait: Duration) -> std::io::Result<File> {
    let file = lock_file()?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                let path = lock_path();
                return Err(std::io::Error::other(format!(
                    "another Lawn process is using {}",
                    path.display()
                )));
            }
            Err(std::fs::TryLockError::Error(err)) => {
                tracing::warn!(%err, "cannot lock the layout; starting without it");
                return Ok(file);
            }
        }
    }
}

fn reachable_core() -> Option<std::sync::Arc<crate::hc::Core>> {
    crate::hc::backend()?
        .inspect_err(|err| tracing::warn!(%err, "cannot read the layout from herdr core"))
        .ok()
}

pub(crate) fn name() -> String {
    let session = crate::session::active_name()
        .unwrap_or_else(|| crate::session::DEFAULT_SESSION_NAME.to_owned());
    name_for(&session)
}

fn name_for(session: &str) -> String {
    use sha2::{Digest, Sha256};
    let app = crate::config::app_dir_name();
    let readable = format!("{app}-layout-{session}");
    if readable.len() <= 32 && !session.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return readable;
    }
    let digest = format!("{:x}", Sha256::digest(session.as_bytes()));
    format!("{app}-layout.{}", &digest[..16])
}

pub(super) fn load() -> Option<Option<SessionSnapshot>> {
    *version() = None;
    let core = reachable_core()?;
    let name = name();
    let doc = core
        .doc_get(&name, None)
        .inspect_err(|err| tracing::warn!(%err, name, "cannot read the layout document"))
        .ok()?;
    let (doc, snapshot) = read_document(&core, &name, doc)
        .inspect_err(
            |err| tracing::warn!(%err, name, "cannot read the layout document; leaving it alone"),
        )
        .ok()?;
    *version() = Some(doc.version);
    (doc.version != 0).then_some(snapshot)
}

fn read_document(
    core: &crate::hc::Core,
    name: &str,
    mut doc: crate::hc::Doc,
) -> Result<(crate::hc::Doc, Option<SessionSnapshot>), String> {
    for _ in 0..8 {
        if doc.version == 0 || doc.value.is_null() {
            return Ok((doc, None));
        }
        match read_layout(&doc.value) {
            Ok(snapshot) => return Ok((doc, Some(snapshot))),
            Err(err) => {
                let current = core.doc_get(name, None).map_err(|err| err.to_string())?;
                if current.version == doc.version {
                    return Err(err);
                }
                doc = current;
            }
        }
    }
    Err(format!("{name} kept changing; try again"))
}

pub(super) fn save(snapshot: Option<&SessionSnapshot>) -> Result<(), String> {
    let Some(core) = crate::hc::backend() else {
        return Ok(());
    };
    let core = core.map_err(|err| err.to_string())?;
    let name = name();
    let mut held = version();
    let base = match *held {
        Some(base) => base,
        None => match core.doc_get(&name, None).map_err(|err| err.to_string())? {
            doc if doc.version == 0 => 0,
            _ => return Err(format!("{name} holds a layout this Lawn did not load")),
        },
    };
    let value = document_value(&core, &name, snapshot, base)?;
    let result = core.doc_set(&name, None, &value, base);
    if let Err(err) = prune_payloads(&core, &name, Some(&value)) {
        tracing::warn!(%err, "cannot prune layout payloads");
    }
    match result {
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

pub(crate) fn edit<T>(
    mut change: impl FnMut(&mut SessionSnapshot) -> (T, bool),
) -> Result<Option<T>, String> {
    if !crate::session::data_dir().exists() {
        return Ok(None);
    }
    let Some(core) = reachable_core() else {
        return Ok(None);
    };
    let name = name();
    let mut lock = None;
    for _ in 0..8 {
        let doc = core.doc_get(&name, None).map_err(|err| err.to_string())?;
        if doc.version == 0 || doc.value.is_null() {
            return Ok(None);
        }
        let mut snapshot = parse_snapshot(&doc.value.to_string())?;
        let loaded = super::snapshot::layout_fingerprint(&snapshot);
        let (out, changed) = change(&mut snapshot);
        if !changed {
            return Ok(Some(out));
        }
        if lock.is_none() {
            let file = lock_file().map_err(|err| err.to_string())?;
            match file.try_lock() {
                Ok(()) => {}
                Err(std::fs::TryLockError::Error(err))
                    if err.kind() != std::io::ErrorKind::Unsupported =>
                {
                    return Err(err.to_string())
                }
                Err(_) => return Ok(None),
            }
            lock = Some(file);
        }
        let value = serde_json::to_value(&snapshot).map_err(|err| err.to_string())?;
        match core.doc_set(&name, None, &value, doc.version) {
            Ok(_) => {
                if let Err(err) = super::io::carry_history(loaded, &snapshot) {
                    tracing::warn!(%err, "cannot keep pane history with the edited layout");
                }
                return Ok(Some(out));
            }
            Err(err) if err.kind() == Some("conflict") => continue,
            Err(err) => return Err(err.to_string()),
        }
    }
    Err(format!("{name} kept changing; try again"))
}

pub(crate) fn clear(session: &str) -> Result<(), String> {
    set_current(&name_for(session), None)
}

pub(super) fn hold_for_restore() -> Result<Option<File>, String> {
    let file = lock_file().map_err(|err| err.to_string())?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Err(format!(
            "a Lawn server is using {}; stop it before restoring",
            lock_path().display()
        )),
        Err(std::fs::TryLockError::Error(err)) if err.kind() == std::io::ErrorKind::Unsupported => {
            Ok(None)
        }
        Err(std::fs::TryLockError::Error(err)) => Err(err.to_string()),
    }
}

pub(super) fn replace(snapshot: &SessionSnapshot) -> Result<(), String> {
    set_current(&name(), Some(snapshot))
}

fn set_current(name: &str, snapshot: Option<&SessionSnapshot>) -> Result<(), String> {
    let Some(core) = crate::hc::backend() else {
        return Ok(());
    };
    let core = core.map_err(|err| err.to_string())?;
    let current = core
        .doc_get(name, None)
        .map_err(|err| err.to_string())?
        .version;
    let value = document_value(&core, name, snapshot, current)?;
    let result = core.doc_set(name, None, &value, current);
    if snapshot.is_some() {
        if let Err(err) = prune_payloads(&core, name, Some(&value)) {
            tracing::warn!(%err, "cannot prune layout payloads");
        }
    }
    result.map(drop).map_err(|err| err.to_string())
}

fn payload_path(fingerprint: &str) -> Result<std::path::PathBuf, String> {
    if fingerprint.len() != 64
        || !fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid layout payload fingerprint".into());
    }
    Ok(crate::session::data_dir()
        .join("session-layouts")
        .join(format!("{fingerprint}.json")))
}

fn read_payload(fingerprint: &str) -> Result<SessionSnapshot, String> {
    let path = payload_path(fingerprint)?;
    let content = std::fs::read_to_string(&path).map_err(|err| err.to_string())?;
    let snapshot = parse_snapshot(&content)?;
    if super::snapshot::layout_fingerprint(&snapshot).as_deref() != Some(fingerprint) {
        return Err(format!(
            "layout payload {} does not match its fingerprint",
            path.display()
        ));
    }
    Ok(snapshot)
}

fn read_layout(value: &serde_json::Value) -> Result<SessionSnapshot, String> {
    let Some(marker) = value.get("lawn_layout_file") else {
        return parse_snapshot(&value.to_string());
    };
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(u32::MAX.into())
        || marker.get("version").and_then(serde_json::Value::as_u64) != Some(1)
    {
        return Err("unsupported file-backed layout version".into());
    }
    let fingerprint = marker
        .get("fingerprint")
        .and_then(serde_json::Value::as_str)
        .ok_or("file-backed layout has no fingerprint")?;
    read_payload(fingerprint)
}

fn payload_fingerprint(value: &serde_json::Value) -> Option<&str> {
    let marker = value.get("lawn_layout_file")?;
    if value.get("version")?.as_u64()? != u64::from(u32::MAX)
        || marker.get("version")?.as_u64()? != 1
    {
        return None;
    }
    let fingerprint = marker.get("fingerprint")?.as_str()?;
    payload_path(fingerprint).ok()?;
    Some(fingerprint)
}

fn prune_payloads(
    core: &crate::hc::Core,
    name: &str,
    candidate: Option<&serde_json::Value>,
) -> Result<(), String> {
    let directory = crate::session::data_dir().join("session-layouts");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.to_string()),
    };
    let current = core.doc_get(name, None).map_err(|err| err.to_string())?;
    if current
        .value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        == Some(u64::from(u32::MAX))
        && payload_fingerprint(&current.value).is_none()
    {
        return Ok(());
    }
    let mut keep = std::collections::HashSet::new();
    if let Some(fingerprint) = payload_fingerprint(&current.value) {
        keep.insert(fingerprint.to_owned());
    }
    if let Some(fingerprint) = candidate.and_then(payload_fingerprint) {
        keep.insert(fingerprint.to_owned());
    }
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| err.to_string())?;
        if !entry.file_type().map_err(|err| err.to_string())?.is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let Some(fingerprint) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if payload_path(fingerprint).is_err() {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .map_err(|err| err.to_string())?;
        files.push((modified, path.clone(), fingerprint.to_owned()));
    }
    files.sort();
    if candidate.is_none() {
        if let Some((_, _, fingerprint)) = files
            .iter()
            .rev()
            .find(|(_, _, fingerprint)| !keep.contains(fingerprint))
        {
            keep.insert(fingerprint.clone());
        }
    }
    for (_, path, fingerprint) in files {
        if !keep.contains(&fingerprint) {
            std::fs::remove_file(path).map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

fn document_value(
    core: &crate::hc::Core,
    name: &str,
    snapshot: Option<&SessionSnapshot>,
    base: u64,
) -> Result<serde_json::Value, String> {
    let value = serde_json::to_value(snapshot).map_err(|err| err.to_string())?;
    if serde_json::to_vec(&value)
        .map_err(|err| err.to_string())?
        .len()
        <= 1024 * 1024
    {
        return Ok(value);
    }
    let snapshot = snapshot.ok_or("an oversized layout is missing")?;
    let fingerprint =
        super::snapshot::layout_fingerprint_value(value).ok_or("cannot fingerprint the layout")?;
    if core
        .doc_get(name, None)
        .map_err(|err| err.to_string())?
        .version
        != base
    {
        return Err(format!("{name} changed elsewhere; not replacing it"));
    }
    prune_payloads(core, name, None)?;
    let path = payload_path(&fingerprint)?;
    if path.try_exists().map_err(|err| err.to_string())? {
        read_payload(&fingerprint)?;
    } else {
        super::io::save_to_path(&path, snapshot).map_err(|err| err.to_string())?;
    }
    Ok(serde_json::json!({
        "version": u32::MAX,
        "lawn_layout_file": {"version": 1, "fingerprint": fingerprint}
    }))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn test_core() -> Option<crate::hc::test_support::TestCore> {
        let test = crate::hc::test_support::TestCore::start()?;
        std::env::set_var("XDG_CONFIG_HOME", test.config_home());
        test.share();
        crate::hc::use_core_for_test();
        Some(test)
    }

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
        let app = crate::config::app_dir_name();
        assert_eq!(names[0], format!("{app}-layout-default"));
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "{names:?}");
    }

    #[test]
    fn a_hashed_name_round_trips_and_clears() {
        let Some(_test) = test_core() else {
            return;
        };
        let session = "Long-Session-Name-That-Core-Cannot-Take";
        std::env::set_var(crate::session::SESSION_ENV_VAR, session);

        save(Some(&snapshot(31))).expect("save");
        let loaded = load().expect("written").expect("not cleared");
        assert_eq!(loaded.sidebar_width, Some(31));
        clear(session).expect("delete the session");
        assert!(matches!(load(), Some(None)));
    }

    #[test]
    fn layout_round_trips_through_the_document() {
        let Some(_test) = test_core() else {
            return;
        };
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
    fn a_restore_replaces_whatever_layout_the_document_holds() {
        let Some(test) = test_core() else {
            return;
        };
        std::env::remove_var(crate::session::SESSION_ENV_VAR);
        let newer = serde_json::json!({"version": u32::MAX, "workspaces": [], "selected": 0});
        test.core
            .doc_set(&name(), None, &newer, 0)
            .expect("doc.set");
        std::fs::create_dir_all(crate::session::data_dir()).expect("the session ran");
        let source = crate::session::data_dir().join("saved.json");
        let saved = serde_json::to_string(&snapshot(31)).expect("json");
        std::fs::write(&source, saved).expect("write");

        super::super::io::restore_from(&source).expect("restore");

        let loaded = load().expect("written").expect("not cleared");
        assert_eq!(loaded.sidebar_width, Some(31), "the document wins on load");
    }

    #[test]
    fn an_edit_keeps_the_pane_history_that_matched_its_layout() {
        use super::super::snapshot::{layout_fingerprint, SessionHistorySnapshot};
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        test.share();
        crate::hc::use_core_for_test();
        std::fs::create_dir_all(crate::session::data_dir()).expect("the session ran");
        let history_path = crate::session::data_dir().join("session-history.json");
        let write_history = |fingerprint: Option<String>| {
            let history = SessionHistorySnapshot {
                version: super::super::snapshot::SNAPSHOT_VERSION,
                layout_fingerprint: fingerprint,
                workspaces: vec![],
            };
            super::super::io::save_history_to_path(&history_path, Some(&history)).expect("history");
        };
        let widen = |width| {
            edit(|snapshot| {
                snapshot.sidebar_width = Some(width);
                ((), true)
            })
        };
        test.core
            .doc_set(
                &name(),
                None,
                &serde_json::to_value(snapshot(31)).expect("json"),
                0,
            )
            .expect("doc.set");
        write_history(layout_fingerprint(&snapshot(31)));

        assert_eq!(widen(40), Ok(Some(())));

        let carried = super::super::io::load_history().expect("history");
        assert_eq!(
            carried.layout_fingerprint,
            layout_fingerprint(&snapshot(40))
        );

        write_history(Some("another layout".into()));
        assert_eq!(widen(50), Ok(Some(())));
        let kept = super::super::io::load_history().expect("history");
        assert_eq!(kept.layout_fingerprint.as_deref(), Some("another layout"));
    }

    #[test]
    fn a_restore_never_lands_under_a_running_server() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        test.share();
        crate::hc::use_core_for_test();
        std::fs::create_dir_all(crate::session::data_dir()).expect("the session ran");
        let held = serde_json::to_value(snapshot(31)).expect("json");
        test.core.doc_set(&name(), None, &held, 0).expect("doc.set");
        let source = crate::session::data_dir().join("saved.json");
        let saved = serde_json::to_string(&snapshot(40)).expect("json");
        std::fs::write(&source, saved).expect("write");
        let _server = own().expect("the server starts").expect("core panes");

        let refused = super::super::io::restore_from(&source).expect_err("a server holds it");

        assert!(refused.contains("stop it before restoring"), "{refused}");
        let stored = test.core.doc_get(&name(), None).expect("doc.get").value;
        assert_eq!(stored, held);
        assert!(!super::super::io::session_path().exists());
    }

    #[test]
    fn a_save_never_replaces_a_layout_it_did_not_load() {
        let Some(test) = test_core() else {
            return;
        };
        let stored = |test: &crate::hc::test_support::TestCore| {
            test.core.doc_get(&name(), None).expect("doc.get").value
        };

        let newer = serde_json::json!({"version": u32::MAX, "workspaces": [], "selected": 0});
        test.core
            .doc_set(&name(), None, &newer, 0)
            .expect("doc.set");
        assert!(load().is_none());
        assert!(save(Some(&snapshot(31))).is_err());
        assert_eq!(stored(&test), newer);

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

    fn oversized_snapshot(sidebar_width: u16) -> SessionSnapshot {
        let mut saved = snapshot(sidebar_width);
        saved.collapsed_space_keys.insert("x".repeat(1024 * 1024));
        saved
    }

    fn stored_payload(test: &crate::hc::test_support::TestCore) -> std::path::PathBuf {
        let doc = test.core.doc_get(&name(), None).expect("doc.get");
        assert_eq!(doc.value["lawn_layout_file"]["version"], 1);
        assert!(parse_snapshot(&doc.value.to_string()).is_err());
        payload_path(
            doc.value["lawn_layout_file"]["fingerprint"]
                .as_str()
                .expect("fingerprint"),
        )
        .expect("payload path")
    }

    #[test]
    fn oversized_layouts_survive_repeated_saves_and_restore() {
        let Some(test) = test_core() else {
            return;
        };
        let mut writer = super::super::SessionWriter::new(false);
        writer.save(&snapshot(31), None);
        let mut previous: Option<(std::path::PathBuf, Vec<u8>, std::time::SystemTime)> = None;
        for width in [40, 41, 41] {
            writer.save(&oversized_snapshot(width), None);
            let path = stored_payload(&test);
            let bytes = std::fs::read(&path).expect("payload");
            if let Some((old_path, old_bytes, modified)) = previous {
                if old_path == path {
                    assert_eq!(bytes, old_bytes);
                    assert_eq!(
                        std::fs::metadata(&path).unwrap().modified().unwrap(),
                        modified
                    );
                } else {
                    assert!(!old_path.exists());
                }
            }
            previous = Some((
                path.clone(),
                bytes,
                std::fs::metadata(&path).unwrap().modified().unwrap(),
            ));
            *version() = None;
            assert_eq!(
                super::super::io::load()
                    .expect("saved layout")
                    .sidebar_width,
                Some(width)
            );
        }
        let source = crate::session::data_dir().join("saved.json");
        super::super::io::save_to_path(&source, &oversized_snapshot(42)).expect("source");
        super::super::io::restore_from(&source).expect("restore oversized layout");
        assert_eq!(
            super::super::io::load()
                .expect("restored layout")
                .sidebar_width,
            Some(42)
        );
        writer.save(&snapshot(43), None);
        assert!(test
            .core
            .doc_get(&name(), None)
            .expect("doc.get")
            .value
            .get("lawn_layout_file")
            .is_none());
        assert_eq!(
            super::super::io::load()
                .expect("inline layout")
                .sidebar_width,
            Some(43)
        );
    }

    #[test]
    fn file_backing_starts_only_above_the_core_document_limit() {
        let Some(test) = test_core() else {
            return;
        };
        let mut saved = snapshot(31);
        saved.collapsed_space_keys.insert(String::new());
        let overhead = serde_json::to_vec(&saved).unwrap().len();
        saved.collapsed_space_keys = ["x".repeat(1024 * 1024 - overhead)].into();
        let value = document_value(&test.core, &name(), Some(&saved), 0).unwrap();
        assert_eq!(serde_json::to_vec(&value).unwrap().len(), 1024 * 1024);
        assert!(value.get("lawn_layout_file").is_none());
        let revision = test.core.doc_set(&name(), None, &value, 0).unwrap();
        saved.collapsed_space_keys = ["x".repeat(1024 * 1024 - overhead + 1)].into();
        let too_large = serde_json::to_value(&saved).unwrap();
        assert!(test
            .core
            .doc_set(&name(), None, &too_large, revision)
            .is_err());
        let marker = document_value(&test.core, &name(), Some(&saved), revision).unwrap();
        assert!(parse_snapshot(&marker.to_string()).is_err());
        assert_eq!(read_layout(&marker).unwrap().sidebar_width, Some(31));
        test.core.doc_set(&name(), None, &marker, revision).unwrap();
    }

    #[test]
    fn payload_retention_is_bounded_across_interrupted_publications() {
        let Some(test) = test_core() else {
            return;
        };
        let mut writer = super::super::SessionWriter::new(false);
        writer.save(&oversized_snapshot(31), None);
        let original = stored_payload(&test);
        let original_bytes = std::fs::read(&original).unwrap();
        let base = test.core.doc_get(&name(), None).unwrap().version;
        let directory = original.parent().unwrap();
        for width in 40..50 {
            document_value(&test.core, &name(), Some(&oversized_snapshot(width)), base).unwrap();
            assert!(std::fs::read_dir(directory).unwrap().count() <= 3);
            assert_eq!(std::fs::read(&original).unwrap(), original_bytes);
            assert_eq!(load().unwrap().unwrap().sidebar_width, Some(31));
        }
        writer.save(&oversized_snapshot(51), None);
        assert_eq!(std::fs::read_dir(directory).unwrap().count(), 1);
        assert_eq!(load().unwrap().unwrap().sidebar_width, Some(51));
    }

    #[test]
    fn a_reader_retries_when_its_old_payload_has_been_pruned() {
        let Some(test) = test_core() else {
            return;
        };
        let mut writer = super::super::SessionWriter::new(false);
        writer.save(&oversized_snapshot(31), None);
        let old = test.core.doc_get(&name(), None).unwrap();
        let old_path = stored_payload(&test);
        writer.save(&oversized_snapshot(42), None);
        assert!(!old_path.exists());
        let (current, snapshot) = read_document(&test.core, &name(), old).unwrap();
        assert_eq!(current.version, 2);
        assert_eq!(snapshot.unwrap().sidebar_width, Some(42));
    }

    #[test]
    fn a_prepared_payload_cannot_override_a_conflicting_document() {
        let Some(test) = test_core() else {
            return;
        };
        save(Some(&oversized_snapshot(31))).unwrap();
        let base = test.core.doc_get(&name(), None).unwrap().version;
        let candidate =
            document_value(&test.core, &name(), Some(&oversized_snapshot(40)), base).unwrap();
        let theirs = serde_json::to_value(snapshot(99)).unwrap();
        test.core.doc_set(&name(), None, &theirs, base).unwrap();
        assert!(test.core.doc_set(&name(), None, &candidate, base).is_err());
        prune_payloads(&test.core, &name(), Some(&candidate)).unwrap();
        assert_eq!(test.core.doc_get(&name(), None).unwrap().value, theirs);
        assert_eq!(read_layout(&candidate).unwrap().sidebar_width, Some(40));
        assert_eq!(load().unwrap().unwrap().sidebar_width, Some(99));
    }

    #[test]
    fn a_missing_or_mismatched_payload_does_not_unlock_its_document() {
        let Some(test) = test_core() else {
            return;
        };
        let mut writer = super::super::SessionWriter::new(false);
        writer.save(&oversized_snapshot(31), None);
        let path = stored_payload(&test);
        let held = test.core.doc_get(&name(), None).expect("doc.get");
        std::fs::remove_file(&path).expect("missing payload");
        assert!(load().is_none());
        assert!(save(Some(&snapshot(32))).is_err());
        assert_eq!(
            super::super::io::load().expect("backup file").sidebar_width,
            Some(31)
        );
        super::super::io::save_to_path(&path, &snapshot(33)).expect("mismatched payload");
        assert!(load().is_none());
        assert!(save(Some(&snapshot(34))).is_err());
        let current = test.core.doc_get(&name(), None).expect("doc.get");
        assert_eq!(current.value, held.value);
        assert_eq!(current.version, held.version);
    }

    #[test]
    fn unknown_file_layouts_are_not_empty_legacy_snapshots() {
        let Some(test) = test_core() else {
            return;
        };
        for marker in [
            serde_json::json!({"version": u32::MAX, "lawn_layout_file": {"version": 2, "fingerprint": "a".repeat(64)}}),
            serde_json::json!({"version": u32::MAX, "lawn_layout_file": {"version": 1, "fingerprint": "../session"}}),
            serde_json::json!({"version": u32::MAX, "future_layout_file": {"version": 1}}),
        ] {
            let current = test.core.doc_get(&name(), None).expect("doc.get").version;
            test.core
                .doc_set(&name(), None, &marker, current)
                .expect("future marker");
            assert!(load().is_none());
            assert!(save(Some(&snapshot(31))).is_err());
            assert_eq!(
                test.core.doc_get(&name(), None).expect("doc.get").value,
                marker
            );
        }
    }

    #[test]
    fn null_still_clears_a_file_backed_layout() {
        let Some(_test) = test_core() else {
            return;
        };
        let mut writer = super::super::SessionWriter::new(false);
        writer.save(&oversized_snapshot(31), None);
        save(None).expect("clear document");
        assert!(super::super::io::session_path().exists());
        assert!(matches!(load(), Some(None)));
        assert!(super::super::io::load().is_none());
        writer.save(&oversized_snapshot(32), None);
        writer.clear();
        assert!(!super::super::io::session_path().exists());
        assert!(matches!(load(), Some(None)));
    }

    #[test]
    fn unpublished_payloads_do_not_replace_a_committed_layout() {
        let Some(test) = test_core() else {
            return;
        };
        let mut writer = super::super::SessionWriter::new(false);
        writer.save(&oversized_snapshot(31), None);
        let original = stored_payload(&test);
        let bytes = std::fs::read(&original).expect("original payload");
        let base = test.core.doc_get(&name(), None).unwrap().version;
        document_value(&test.core, &name(), Some(&oversized_snapshot(40)), base)
            .expect("prepare without publishing");
        assert_eq!(
            super::super::io::load()
                .expect("committed layout")
                .sidebar_width,
            Some(31)
        );
        let before = test.core.doc_get(&name(), None).expect("doc.get");
        let theirs = serde_json::to_value(snapshot(99)).expect("json");
        test.core
            .doc_set(&name(), None, &theirs, before.version)
            .expect("another writer");
        let latest = oversized_snapshot(42);
        writer.save(&latest, None);
        assert_eq!(
            test.core.doc_get(&name(), None).expect("doc.get").value,
            theirs
        );
        assert_eq!(std::fs::read(original).expect("original payload"), bytes);
        let pending = super::super::snapshot::layout_fingerprint(&latest).expect("fingerprint");
        assert!(!payload_path(&pending).unwrap().exists());
        let file = std::fs::read_to_string(super::super::io::session_path()).expect("latest file");
        assert_eq!(
            parse_snapshot(&file).expect("snapshot").sidebar_width,
            Some(42)
        );
        assert_eq!(
            super::super::io::load()
                .expect("newer document")
                .sidebar_width,
            Some(99)
        );
    }

    #[test]
    fn an_edit_never_lands_under_a_running_server() {
        let Some(test) = crate::hc::test_support::TestCore::start() else {
            return;
        };
        test.share();
        crate::hc::use_core_for_test();
        std::fs::create_dir_all(crate::session::data_dir()).expect("the session ran");
        let value = serde_json::to_value(snapshot(31)).expect("json");
        test.core
            .doc_set(&name(), None, &value, 0)
            .expect("doc.set");
        let widen = || {
            edit(|snapshot| {
                snapshot.sidebar_width = Some(40);
                ((), true)
            })
        };

        assert_eq!(widen(), Ok(Some(())), "no server yet");
        let _server = own().expect("the server starts").expect("core panes");
        let loaded = load().expect("written").expect("not cleared");
        assert_eq!(loaded.sidebar_width, Some(40), "the server starts with it");
        assert_eq!(widen(), Ok(None), "the server owns the layout now");
        let width = edit(|snapshot| (snapshot.sidebar_width, false));
        assert_eq!(width, Ok(Some(Some(40))), "reading needs no lock");
        save(Some(&snapshot(32))).expect("the server keeps saving");
    }

    #[test]
    fn a_second_server_gives_up_instead_of_waiting() {
        let dir = std::env::temp_dir().join(format!("lawn-lock-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let running = own_within(Duration::ZERO).expect("the first server");

        let err = own_within(Duration::from_millis(100)).expect_err("taken");
        assert!(err.to_string().contains("another Lawn process"), "{err}");

        drop(running);
        own_within(Duration::ZERO).expect("free once it exits");
        std::fs::remove_dir_all(dir).expect("clean up");
    }
}
