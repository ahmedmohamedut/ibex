//! The cache directory: everything derived from a backup lives here, because
//! the backup folder itself is strictly read-only (AGENTS.md hard rule 1).
//!
//! ```text
//! ~/.local/share/ibex/
//!   backups.json                      # recently opened backups (written by the app)
//!   cache/<backup-id>/
//!     manifest.db                     # decrypted Manifest.db (encrypted backups only)
//!     db/<file_id>/<name>             # SQLite working copies
//!     files/<file_id>                 # decrypted files (encrypted backups only)
//!     thumbs/<size>/<file_id>.jpg
//!     search.db
//! ```

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{BackupError, IoResultExt, Result};

/// `$XDG_DATA_HOME/ibex`, falling back to `~/.local/share/ibex`.
///
/// Read from the environment directly rather than pulling in a dependency:
/// ibex is Linux-first, and this is exactly the XDG rule.
pub fn data_dir() -> Result<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(xdg).join("ibex"));
    }
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .ok_or(BackupError::NoCacheDir)?;
    Ok(PathBuf::from(home).join(".local/share/ibex"))
}

/// Identity of a backup, used as its cache directory name.
///
/// SHA-256 over the canonical path, the device UDID and the backup date, so two
/// backups of the same device taken at different times get separate caches and
/// a moved folder does not collide with the original.
pub fn backup_id(canonical_path: &Path, udid: &str, backup_date: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonical_path.as_os_str().as_encoded_bytes());
    hasher.update(b"\0");
    hasher.update(udid.as_bytes());
    hasher.update(b"\0");
    hasher.update(backup_date.as_bytes());
    // 32 hex chars is plenty for a directory name and keeps paths short.
    hex::encode(&hasher.finalize()[..16])
}

/// The cache directory belonging to one open backup.
#[derive(Debug, Clone)]
pub struct CacheDir {
    root: PathBuf,
}

impl CacheDir {
    /// Locate (without creating) the cache for a backup id.
    pub fn for_backup(backup_id: &str) -> Result<Self> {
        Ok(Self {
            root: data_dir()?.join("cache").join(backup_id),
        })
    }

    /// A cache rooted at an explicit path. Used by tests.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory holding the SQLite working copy of one manifest entry.
    pub fn db_dir(&self, file_id: &str) -> PathBuf {
        self.root.join("db").join(file_id)
    }

    /// Where a decrypted file goes (encrypted backups, milestone M7).
    pub fn file_path(&self, file_id: &str) -> PathBuf {
        self.root.join("files").join(file_id)
    }

    /// Where the decrypted `Manifest.db` goes (encrypted backups, M7).
    pub fn manifest_db(&self) -> PathBuf {
        self.root.join("manifest.db")
    }

    pub fn thumb_path(&self, size: u32, file_id: &str) -> PathBuf {
        self.root
            .join("thumbs")
            .join(size.to_string())
            .join(format!("{file_id}.jpg"))
    }

    pub fn search_db(&self) -> PathBuf {
        self.root.join("search.db")
    }

    /// Create `dir` and every missing parent.
    pub fn ensure(dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir).at(dir)
    }

    /// Total size of the cache in bytes, for the Overview screen.
    pub fn size_on_disk(&self) -> Result<u64> {
        fn walk(dir: &Path, total: &mut u64) -> Result<()> {
            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                // A cache that was never created has size zero.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => {
                    return Err(crate::error::Error::Io {
                        path: dir.to_path_buf(),
                        source: e,
                    });
                }
            };
            for entry in entries {
                let entry = entry.at(dir)?;
                let meta = entry.metadata().at(entry.path())?;
                if meta.is_dir() {
                    walk(&entry.path(), total)?;
                } else {
                    *total += meta.len();
                }
            }
            Ok(())
        }
        let mut total = 0;
        walk(&self.root, &mut total)?;
        Ok(total)
    }

    /// Delete everything derived from this backup.
    ///
    /// For an encrypted backup the cache holds decrypted data, so this is the
    /// privacy escape hatch offered in the UI.
    pub fn clear(&self) -> Result<()> {
        match std::fs::remove_dir_all(&self.root) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(crate::error::Error::Io {
                path: self.root.clone(),
                source: e,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_separate_backups_and_are_stable() {
        let a = backup_id(Path::new("/b/UDID1"), "UDID1", "2026-11-01T10:00:00Z");
        let again = backup_id(Path::new("/b/UDID1"), "UDID1", "2026-11-01T10:00:00Z");
        assert_eq!(a, again, "same inputs must give the same cache directory");

        // A later backup of the same device is a different cache.
        let later = backup_id(Path::new("/b/UDID1"), "UDID1", "2026-11-02T10:00:00Z");
        assert_ne!(a, later);
        // So is the same backup copied elsewhere.
        let moved = backup_id(Path::new("/other/UDID1"), "UDID1", "2026-11-01T10:00:00Z");
        assert_ne!(a, moved);
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn paths_are_laid_out_as_documented() {
        let cache = CacheDir::at("/cache/abc");
        assert_eq!(cache.db_dir("fid"), Path::new("/cache/abc/db/fid"));
        assert_eq!(cache.file_path("fid"), Path::new("/cache/abc/files/fid"));
        assert_eq!(cache.manifest_db(), Path::new("/cache/abc/manifest.db"));
        assert_eq!(
            cache.thumb_path(256, "fid"),
            Path::new("/cache/abc/thumbs/256/fid.jpg")
        );
        assert_eq!(cache.search_db(), Path::new("/cache/abc/search.db"));
    }

    #[test]
    fn size_and_clear_tolerate_a_missing_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = CacheDir::at(tmp.path().join("never-created"));
        assert_eq!(cache.size_on_disk().unwrap(), 0);
        assert!(cache.clear().is_ok());
    }

    #[test]
    fn clear_removes_everything() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = CacheDir::at(tmp.path().join("c"));
        let dir = cache.db_dir("fid");
        CacheDir::ensure(&dir).unwrap();
        std::fs::write(dir.join("sms.db"), b"0123456789").unwrap();

        assert_eq!(cache.size_on_disk().unwrap(), 10);
        cache.clear().unwrap();
        assert!(!cache.root().exists());
        // The tempdir itself is untouched.
        assert!(tmp.path().exists());
    }

    #[test]
    fn data_dir_follows_xdg() {
        // Not using std::env::set_var: it is unsafe in edition 2024 and would
        // race other tests. Checking the shape of whatever the environment
        // gives us is enough.
        let dir = data_dir().expect("HOME or XDG_DATA_HOME is set in the test env");
        assert!(dir.ends_with("ibex"), "got {dir:?}");
        assert!(dir.is_absolute());
    }
}
