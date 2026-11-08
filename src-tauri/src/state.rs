//! Application state: the backups currently open.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use ibex_core::Backup;

use crate::error::IpcError;
use crate::jobs::JobRegistry;

/// Opaque handle the frontend passes back on every call.
pub type BackupHandle = String;

/// Shared state, managed by Tauri.
#[derive(Default)]
pub struct AppState {
    backups: RwLock<HashMap<BackupHandle, Arc<Backup>>>,
    /// Overrides the XDG data directory for the cache and the recents list.
    ///
    /// `None` means the real one. The integration tests set it to a tempdir, so
    /// the suite never writes decrypted data or fake recents into the user's
    /// home — which is doubly important here, since a decrypted cache is the
    /// most sensitive thing ibex produces.
    data_root: Option<PathBuf>,
    /// Long-running work: exports now, the search index in M8.
    jobs: JobRegistry,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    /// State whose cache and recents live under `root` instead of the XDG data
    /// directory.
    pub fn with_data_root(root: impl Into<PathBuf>) -> Self {
        Self {
            backups: RwLock::new(HashMap::new()),
            data_root: Some(root.into()),
            jobs: JobRegistry::new(),
        }
    }

    /// The job registry.
    pub fn jobs(&self) -> &JobRegistry {
        &self.jobs
    }

    /// The data directory override, if any.
    pub fn data_root(&self) -> Option<&Path> {
        self.data_root.as_deref()
    }

    /// Register an open backup and return its handle.
    ///
    /// The handle is the backup's cache id, so re-opening the same backup
    /// replaces its entry instead of leaking a second copy.
    pub fn insert(&self, backup: Backup) -> BackupHandle {
        let handle = backup.id().to_string();
        let mut backups = self.write();
        backups.insert(handle.clone(), Arc::new(backup));
        handle
    }

    /// Look up an open backup.
    pub fn get(&self, handle: &str) -> Result<Arc<Backup>, IpcError> {
        self.read()
            .get(handle)
            .cloned()
            .ok_or_else(IpcError::unknown_handle)
    }

    /// Forget a backup. Returns whether it was open.
    pub fn remove(&self, handle: &str) -> bool {
        self.write().remove(handle).is_some()
    }

    pub fn open_count(&self) -> usize {
        self.read().len()
    }

    /// A poisoned lock means another thread panicked while holding it. The map
    /// is just a registry of handles, so recovering is safe and far better than
    /// making every later command fail.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<BackupHandle, Arc<Backup>>> {
        self.backups.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<BackupHandle, Arc<Backup>>> {
        self.backups.write().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibex_core::CacheDir;
    use ibex_core::testutil::FixtureBackup;

    fn fixture_backup(tmp: &std::path::Path) -> Backup {
        let root = FixtureBackup::new()
            .with_file("HomeDomain", "a.txt", b"x")
            .build(tmp)
            .unwrap();
        Backup::open_with_cache(&root, None, CacheDir::at(tmp.join("cache"))).unwrap()
    }

    #[test]
    fn handles_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new();

        let handle = state.insert(fixture_backup(tmp.path()));
        assert_eq!(state.open_count(), 1);
        assert!(state.get(&handle).is_ok());

        assert!(state.remove(&handle));
        assert_eq!(state.open_count(), 0);
        assert!(!state.remove(&handle), "removing twice is not an error");
    }

    #[test]
    fn an_unknown_handle_is_a_typed_error() {
        let state = AppState::new();
        let err = state.get("nope").unwrap_err();
        assert_eq!(err.code, "unknown_handle");
    }

    #[test]
    fn reopening_the_same_backup_does_not_leak_a_second_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new();

        let first = state.insert(fixture_backup(tmp.path()));
        let second = state.insert(fixture_backup(tmp.path()));
        assert_eq!(first, second, "the handle is the backup's cache id");
        assert_eq!(state.open_count(), 1);
    }
}
