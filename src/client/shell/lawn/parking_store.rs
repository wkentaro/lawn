//! Reads the client-side `parking.json` that predates server parking.

use std::{collections::BTreeSet, io::Read, path::Path};

use serde::Deserialize;

const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedParking {
    version: u32,
    terminals: BTreeSet<String>,
}

/// Returns the saved terminal ids. A missing file is empty.
pub(super) fn read(path: &Path) -> Result<BTreeSet<String>, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(format!("Cannot read parked panes: {error}")),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Cannot read parked panes: {error}"))?;
    let saved: SavedParking = serde_json::from_slice(&bytes)
        .ok()
        .filter(|saved: &SavedParking| saved.version == 1 && bytes.len() as u64 <= MAX_BYTES)
        .ok_or("Saved parking state has an unsupported format")?;
    Ok(saved.terminals)
}

/// Removes the file once the server holds its parked panes.
pub(super) fn remove(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|error| format!("Cannot migrate parked panes: {error}"))?;
    let _ = std::fs::remove_file(path.with_extension("lock"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lawn-parking-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn saved_terminals_are_read_then_removed_and_invalid_state_is_kept() {
        let dir = dir();
        let path = dir.join("parking.json");
        assert!(read(&path).unwrap().is_empty());
        std::fs::write(&path, r#"{"version":1,"terminals":["a","b"]}"#).unwrap();
        std::fs::write(path.with_extension("lock"), "").unwrap();
        assert_eq!(
            read(&path).unwrap(),
            BTreeSet::from(["a".into(), "b".into()])
        );
        assert!(path.exists());
        remove(&path).unwrap();
        assert!(!path.exists() && !path.with_extension("lock").exists());
        for bytes in ["broken", r#"{"version":2,"terminals":["keep"]}"#] {
            std::fs::write(&path, bytes).unwrap();
            assert!(read(&path).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
