use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use super::persistence;

/// Lightweight history entry for persistence (no full HTML/markdown).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub url: String,
    pub title: Option<String>,
    pub status: u16,
}

/// Serializable snapshot of session state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub version: u32,
    pub cookies_json: String,
    pub storage: HashMap<String, HashMap<String, String>>,
    pub history: Vec<HistoryEntry>,
    pub history_index: usize,
}

impl SessionSnapshot {
    pub const CURRENT_VERSION: u32 = 1;
}

/// Trait for session persistence backends.
pub trait SessionStore: Send {
    fn save(&self, snapshot: &SessionSnapshot) -> Result<()>;
    fn load(&self) -> Result<Option<SessionSnapshot>>;
    #[allow(dead_code)]
    fn clear(&self) -> Result<()>;
}

/// No-op store that keeps everything in memory only (original behavior).
pub struct InMemoryStore;

impl SessionStore for InMemoryStore {
    fn save(&self, _snapshot: &SessionSnapshot) -> Result<()> {
        Ok(())
    }

    fn load(&self) -> Result<Option<SessionSnapshot>> {
        Ok(None)
    }

    fn clear(&self) -> Result<()> {
        Ok(())
    }
}

/// Encrypted JSON file store.
pub struct DiskStore {
    path: PathBuf,
    key: [u8; 32],
}

impl DiskStore {
    pub fn new(path: PathBuf, data_dir: &std::path::Path) -> Result<Self> {
        let key = persistence::get_or_create_key(data_dir)?;
        Ok(Self { path, key })
    }
}

impl SessionStore for DiskStore {
    fn save(&self, snapshot: &SessionSnapshot) -> Result<()> {
        let json = serde_json::to_vec(snapshot).context("serializing session snapshot")?;
        let encrypted = persistence::encrypt(&json, &self.key)?;
        persistence::atomic_write(&self.path, &encrypted)
    }

    fn load(&self) -> Result<Option<SessionSnapshot>> {
        let data = match std::fs::read(&self.path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("reading session file: {}", self.path.display()))
            }
        };

        let decrypted = match persistence::decrypt(&data, &self.key) {
            Ok(d) => d,
            Err(e) => {
                eprintln!(
                    "warning: could not decrypt session file ({}), starting fresh: {e}",
                    self.path.display()
                );
                return Ok(None);
            }
        };

        let snapshot: SessionSnapshot = match serde_json::from_slice(&decrypted) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "warning: could not parse session file ({}), starting fresh: {e}",
                    self.path.display()
                );
                return Ok(None);
            }
        };

        if snapshot.version > SessionSnapshot::CURRENT_VERSION {
            eprintln!(
                "warning: session file version {} is newer than supported ({}), starting fresh",
                snapshot.version,
                SessionSnapshot::CURRENT_VERSION
            );
            return Ok(None);
        }

        Ok(Some(snapshot))
    }

    fn clear(&self) -> Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => {
                Err(e).with_context(|| format!("removing session file: {}", self.path.display()))
            }
        }
    }
}

/// Create the appropriate session store based on config.
pub fn create_session_store(
    persistence: &super::config::PersistenceMode,
    session_path: Option<&str>,
) -> Result<Box<dyn SessionStore>> {
    match persistence {
        super::config::PersistenceMode::Memory => Ok(Box::new(InMemoryStore)),
        super::config::PersistenceMode::Disk => {
            let data_dir = dirs::data_dir()
                .unwrap_or_else(|| std::path::PathBuf::from(".local/share"))
                .join("browser39");

            let path = match session_path {
                Some(p) => PathBuf::from(p),
                None => data_dir.join("session.enc"),
            };

            Ok(Box::new(DiskStore::new(path, &data_dir)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_in_memory_store() {
        let store = InMemoryStore;
        assert!(store.load().unwrap().is_none());
        let snapshot = SessionSnapshot {
            version: SessionSnapshot::CURRENT_VERSION,
            cookies_json: String::new(),
            storage: HashMap::new(),
            history: vec![],
            history_index: 0,
        };
        store.save(&snapshot).unwrap();
        assert!(store.load().unwrap().is_none()); // still None, it's in-memory
        store.clear().unwrap();
    }

    #[test]
    fn test_disk_store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.enc");
        let store = DiskStore::new(path, dir.path()).unwrap();

        assert!(store.load().unwrap().is_none());

        let mut storage = HashMap::new();
        let mut origin = HashMap::new();
        origin.insert("token".to_string(), "abc123".to_string());
        storage.insert("https://example.com".to_string(), origin);

        let snapshot = SessionSnapshot {
            version: SessionSnapshot::CURRENT_VERSION,
            cookies_json: r#"[{"raw_cookie":"a=1"}]"#.to_string(),
            storage,
            history: vec![HistoryEntry {
                url: "https://example.com".to_string(),
                title: Some("Example".to_string()),
                status: 200,
            }],
            history_index: 0,
        };

        store.save(&snapshot).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.version, snapshot.version);
        assert_eq!(loaded.cookies_json, snapshot.cookies_json);
        assert_eq!(loaded.storage, snapshot.storage);
        assert_eq!(loaded.history.len(), 1);
        assert_eq!(loaded.history[0].url, "https://example.com");
        assert_eq!(loaded.history_index, 0);
    }

    #[test]
    fn test_disk_store_clear() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.enc");
        let store = DiskStore::new(path.clone(), dir.path()).unwrap();

        let snapshot = SessionSnapshot {
            version: SessionSnapshot::CURRENT_VERSION,
            cookies_json: String::new(),
            storage: HashMap::new(),
            history: vec![],
            history_index: 0,
        };
        store.save(&snapshot).unwrap();
        assert!(path.exists());
        store.clear().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn test_disk_store_corrupted_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.enc");
        std::fs::write(&path, b"not valid encrypted data").unwrap();
        let store = DiskStore::new(path, dir.path()).unwrap();
        // Should gracefully return None, not error
        assert!(store.load().unwrap().is_none());
    }
}
