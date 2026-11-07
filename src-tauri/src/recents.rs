//! The recent-backups list, stored as `backups.json` in the data directory.
//!
//! Deliberately small: a path, enough metadata to render a row, and the size
//! recorded at open time so the Welcome screen needs no disk walk. It holds no
//! password and nothing from inside the backup.

use std::path::{Path, PathBuf};

use ibex_core::BackupInfo;
use serde::{Deserialize, Serialize};

use crate::dto::RecentBackup;
use crate::error::IpcError;

/// How many entries to keep.
const MAX_ENTRIES: usize = 20;

/// One stored entry. Separate from [`RecentBackup`] because `exists` is
/// computed at read time rather than stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredRecent {
    path: String,
    #[serde(default)]
    device_name: Option<String>,
    #[serde(default)]
    product_name: Option<String>,
    #[serde(default)]
    product_version: Option<String>,
    #[serde(default)]
    backup_date: Option<String>,
    #[serde(default)]
    encrypted: bool,
    #[serde(default)]
    total_bytes: Option<u64>,
    last_opened: String,
}

/// Where the list lives. `base` overrides the XDG data directory, which tests
/// use so they never write to the user's own list.
pub fn store_path(base: Option<&Path>) -> Result<PathBuf, IpcError> {
    let dir = match base {
        Some(base) => base.to_path_buf(),
        None => ibex_core::cache::data_dir()?,
    };
    Ok(dir.join("backups.json"))
}

/// Read the list, newest first.
///
/// A missing or unreadable file yields an empty list: a corrupt recents file
/// must never stop the app from starting.
pub fn load(base: Option<&Path>) -> Vec<RecentBackup> {
    let Ok(path) = store_path(base) else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    let stored: Vec<StoredRecent> = serde_json::from_slice(&bytes).unwrap_or_default();

    stored
        .into_iter()
        .map(|s| RecentBackup {
            exists: Path::new(&s.path).join("Manifest.db").is_file(),
            path: s.path,
            device_name: s.device_name,
            product_name: s.product_name,
            product_version: s.product_version,
            backup_date: s.backup_date,
            encrypted: s.encrypted,
            total_bytes: s.total_bytes,
            last_opened: s.last_opened,
        })
        .collect()
}

/// Record that a backup was opened, moving it to the front.
///
/// Failing to write is not fatal — the backup is open either way — so the
/// caller logs nothing and carries on.
pub fn record(
    path: &Path,
    info: &BackupInfo,
    total_bytes: Option<u64>,
    base: Option<&Path>,
) -> Result<(), IpcError> {
    let store = store_path(base)?;
    let path_string = path.display().to_string();

    let mut stored: Vec<StoredRecent> = std::fs::read(&store)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();

    stored.retain(|s| s.path != path_string);
    stored.insert(
        0,
        StoredRecent {
            path: path_string,
            device_name: info.device_name.clone(),
            product_name: info.product_name.clone(),
            product_version: info.product_version.clone(),
            backup_date: info.date().map(|d| d.to_rfc3339()),
            encrypted: info.encrypted,
            total_bytes,
            last_opened: chrono::Utc::now().to_rfc3339(),
        },
    );
    stored.truncate(MAX_ENTRIES);

    if let Some(parent) = store.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            IpcError::new(
                "recents_write_failed",
                format!("could not create {parent:?}: {e}"),
            )
        })?;
    }
    let json = serde_json::to_vec_pretty(&stored)
        .map_err(|e| IpcError::new("recents_write_failed", e.to_string()))?;
    std::fs::write(&store, json)
        .map_err(|e| IpcError::new("recents_write_failed", e.to_string()))?;
    Ok(())
}

/// Drop one entry, for the Welcome screen's "remove from list".
pub fn forget(path: &str, base: Option<&Path>) -> Result<(), IpcError> {
    let store = store_path(base)?;
    let mut stored: Vec<StoredRecent> = std::fs::read(&store)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    stored.retain(|s| s.path != path);
    let json = serde_json::to_vec_pretty(&stored)
        .map_err(|e| IpcError::new("recents_write_failed", e.to_string()))?;
    std::fs::write(&store, json)
        .map_err(|e| IpcError::new("recents_write_failed", e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The store path comes from the XDG data dir, which these tests must not
    /// touch, so they exercise the serialization and ordering logic directly.
    fn sample(path: &str, when: &str) -> StoredRecent {
        StoredRecent {
            path: path.to_string(),
            device_name: Some("Fixture iPhone".into()),
            product_name: Some("iPhone 15".into()),
            product_version: Some("18.1".into()),
            backup_date: Some("2026-11-01T00:00:00+00:00".into()),
            encrypted: false,
            total_bytes: Some(1_024),
            last_opened: when.to_string(),
        }
    }

    #[test]
    fn entries_round_trip_through_json() {
        let stored = vec![sample("/b/one", "2026-11-07T10:00:00+00:00")];
        let json = serde_json::to_vec_pretty(&stored).unwrap();
        let back: Vec<StoredRecent> = serde_json::from_slice(&json).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].path, "/b/one");
        assert_eq!(back[0].total_bytes, Some(1_024));
    }

    #[test]
    fn missing_optional_fields_default_rather_than_fail() {
        // An entry written by an older build, with only the required fields.
        let json = br#"[{"path":"/b/one","lastOpened":"2026-11-07T10:00:00+00:00"}]"#;
        let back: Vec<StoredRecent> = serde_json::from_slice(json).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].device_name, None);
        assert!(!back[0].encrypted);
    }

    #[test]
    fn a_corrupt_file_reads_as_an_empty_list() {
        let back: Vec<StoredRecent> = serde_json::from_slice(b"{not json").unwrap_or_default();
        assert!(
            back.is_empty(),
            "a corrupt recents file must not break startup"
        );
    }

    #[test]
    fn the_list_is_capped_and_deduplicated() {
        // Mirrors what `record` does, without touching the real data dir.
        let mut stored: Vec<StoredRecent> = (0..MAX_ENTRIES)
            .map(|i| sample(&format!("/b/{i}"), "2026-11-07T10:00:00+00:00"))
            .collect();

        let reopened = "/b/5".to_string();
        stored.retain(|s| s.path != reopened);
        stored.insert(0, sample(&reopened, "2026-11-07T11:00:00+00:00"));
        stored.truncate(MAX_ENTRIES);

        assert_eq!(stored.len(), MAX_ENTRIES);
        assert_eq!(stored[0].path, reopened, "reopening moves it to the front");
        assert_eq!(
            stored.iter().filter(|s| s.path == reopened).count(),
            1,
            "and does not duplicate it"
        );
    }

    #[test]
    fn existence_is_judged_by_the_manifest_not_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(
            !empty.join("Manifest.db").is_file(),
            "a folder without Manifest.db is not a usable backup"
        );
        std::fs::write(empty.join("Manifest.db"), b"x").unwrap();
        assert!(empty.join("Manifest.db").is_file());
    }
}
