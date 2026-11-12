use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

const MAX_BYTES: u64 = 1024 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedParking {
    version: u32,
    terminals: BTreeSet<String>,
}

fn read(path: &Path) -> Result<BTreeSet<String>, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(format!("Cannot read Later: {error}")),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Cannot read Later: {error}"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Later's saved state is too large".into());
    }
    let saved: SavedParking =
        serde_json::from_slice(&bytes).map_err(|error| format!("Cannot read Later: {error}"))?;
    if saved.version != 1
        || saved
            .terminals
            .iter()
            .any(|id| id.is_empty() || id.len() > 128)
    {
        return Err("Later's saved state has an unsupported format".into());
    }
    Ok(saved.terminals)
}

fn write(path: &Path, terminals: &BTreeSet<String>) -> Result<(), String> {
    let bytes = serde_json::to_vec(&SavedParking {
        version: 1,
        terminals: terminals.clone(),
    })
    .map_err(|error| format!("Cannot save Later: {error}"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Later's saved state is too large".into());
    }
    let temp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| format!("Cannot save Later: {error}"))?;
    let result = (|| -> std::io::Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(|error| format!("Cannot save Later: {error}"))
}

#[derive(Clone, Debug, Default)]
pub(super) struct ParkingStore {
    pub(super) path: Option<PathBuf>,
    pub(super) terminals: BTreeSet<String>,
}

impl ParkingStore {
    pub(super) fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            terminals: BTreeSet::new(),
        }
    }

    pub(super) fn reload(&mut self) -> Result<bool, String> {
        let Some(path) = &self.path else {
            return Ok(false);
        };
        let next = read(path)?;
        let changed = next != self.terminals;
        self.terminals = next;
        Ok(changed)
    }

    fn edit(&mut self, change: impl FnOnce(&mut BTreeSet<String>)) -> Result<bool, String> {
        let Some(path) = &self.path else {
            let before = self.terminals.clone();
            change(&mut self.terminals);
            return Ok(before != self.terminals);
        };
        let parent = path.parent().ok_or("Invalid Later state path")?;
        std::fs::create_dir_all(parent).map_err(|error| format!("Cannot save Later: {error}"))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))
            .map_err(|error| format!("Cannot lock Later: {error}"))?;
        lock.lock()
            .map_err(|error| format!("Cannot lock Later: {error}"))?;
        let mut next = read(path)?;
        let before = next.clone();
        change(&mut next);
        if next != before {
            write(path, &next)?;
        }
        let changed = self.terminals != next;
        self.terminals = next;
        Ok(changed)
    }

    pub(super) fn park(&mut self, terminal: &str) -> Result<bool, String> {
        if terminal.is_empty() || terminal.len() > 128 {
            return Err("Invalid terminal identity".into());
        }
        self.edit(|terminals| {
            terminals.insert(terminal.into());
        })
    }

    pub(super) fn restore(&mut self, terminal: &str) -> Result<bool, String> {
        self.edit(|terminals| {
            terminals.remove(terminal);
        })
    }

    pub(super) fn prune(
        &mut self,
        observed: &BTreeSet<String>,
        live: &BTreeSet<String>,
    ) -> Result<bool, String> {
        self.edit(|terminals| terminals.retain(|id| !observed.contains(id) || live.contains(id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lawn-parking-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("parking.json")
    }

    #[test]
    fn concurrent_windows_preserve_each_others_changes() {
        let path = path();
        let writers = (0..16)
            .map(|number| {
                let path = path.clone();
                std::thread::spawn(move || {
                    ParkingStore::new(Some(path)).park(&format!("terminal-{number}"))
                })
            })
            .collect::<Vec<_>>();
        for writer in writers {
            writer.join().unwrap().unwrap();
        }
        let mut first = ParkingStore::new(Some(path.clone()));
        first.reload().unwrap();
        assert_eq!(first.terminals.len(), 16);
        let mut second = ParkingStore::new(Some(path.clone()));
        first.restore("terminal-1").unwrap();
        second.park("terminal-new").unwrap();
        first.reload().unwrap();
        assert_eq!(first.terminals.len(), 16);
        assert!(!first.terminals.contains("terminal-1"));
        assert!(first.terminals.contains("terminal-new"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn authoritative_prune_keeps_additions_made_after_its_request() {
        let path = path();
        let mut first = ParkingStore::new(Some(path.clone()));
        first.park("closed").unwrap();
        let candidates = first.terminals.clone();
        let mut second = ParkingStore::new(Some(path.clone()));
        second.park("created-after-snapshot").unwrap();
        first.prune(&candidates, &BTreeSet::new()).unwrap();
        assert_eq!(
            first.terminals,
            BTreeSet::from(["created-after-snapshot".into()])
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn invalid_state_is_reported_and_never_overwritten() {
        let path = path();
        for bytes in ["broken", r#"{"version":2,"terminals":["keep"]}"#] {
            std::fs::write(&path, bytes).unwrap();
            let mut store = ParkingStore::new(Some(path.clone()));
            assert!(store.reload().is_err());
            assert!(store.park("new").is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
        }
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
