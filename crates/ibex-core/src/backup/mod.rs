//! Opening a backup and reading files out of it.
//!
//! The backup folder is never written to and never opened for writing. Where a
//! real file on disk is needed — SQLite, the media protocol — the file is used
//! in place for an unencrypted backup, and decrypted into the cache for an
//! encrypted one (milestone M7).

pub mod info;
pub mod manifest;

use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

pub use info::BackupInfo;
pub use manifest::{DomainSummary, EntryKind, FileMeta, Manifest, ManifestEntry};

use crate::cache::{self, CacheDir};
use crate::error::{BackupError, IoResultExt, Result};
use crate::page::{Page, Paged};
use crate::util::{mime, paths};
use crate::workcopy::{SourceFiles, WorkCopy};

/// A readable, seekable stream of one file's bytes.
pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// An open backup.
#[derive(Debug)]
pub struct Backup {
    root: PathBuf,
    id: String,
    info: BackupInfo,
    manifest: Manifest,
    cache: CacheDir,
}

impl Backup {
    /// Open a backup folder.
    ///
    /// `path` is the folder containing `Manifest.db` — the one named after the
    /// device UDID.
    ///
    /// `password` is accepted now and used from milestone M7; until then an
    /// encrypted backup reports [`BackupError::EncryptionUnsupported`] rather
    /// than pretending to open.
    pub fn open(path: &Path, password: Option<&str>) -> Result<Self> {
        Self::open_inner(path, password, None)
    }

    /// Open a backup with its cache redirected to `cache`.
    ///
    /// Tests use this so they never touch the user's real data directory, and
    /// it lets the app put the cache somewhere other than the XDG default.
    pub fn open_with_cache(path: &Path, password: Option<&str>, cache: CacheDir) -> Result<Self> {
        Self::open_inner(path, password, Some(cache))
    }

    fn open_inner(path: &Path, password: Option<&str>, cache: Option<CacheDir>) -> Result<Self> {
        if !path.is_dir() {
            return Err(BackupError::NotADirectory {
                path: path.to_path_buf(),
            }
            .into());
        }
        let manifest_db = path.join("Manifest.db");
        if !manifest_db.is_file() {
            return Err(BackupError::NotABackup {
                path: path.to_path_buf(),
            }
            .into());
        }

        // Canonicalize so the same backup reached by different paths shares one
        // cache entry.
        let root = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let info = BackupInfo::read(&root)?;

        if info.encrypted {
            return Err(if password.is_none() {
                BackupError::PasswordRequired.into()
            } else {
                BackupError::EncryptionUnsupported.into()
            });
        }

        let id = cache::backup_id(&root, info.udid_key(), &info.date_key());
        let cache = match cache {
            Some(cache) => cache,
            None => CacheDir::for_backup(&id)?,
        };

        // Manifest.db is a top-level file that the manifest does not list, so
        // it is copied by path. It can carry a -wal from an interrupted backup.
        let source = SourceFiles::new(&manifest_db, "Manifest.db").with_sidecars(
            Some(path.join("Manifest.db-wal")),
            Some(path.join("Manifest.db-shm")),
        );
        let copy = WorkCopy::materialize(&cache, "manifest", &source)?;
        let manifest = Manifest::from_connection(copy.open()?)?;

        Ok(Self {
            root,
            id,
            info,
            manifest,
            cache,
        })
    }

    /// The backup folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Opaque id, also the name of this backup's cache directory.
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn info(&self) -> &BackupInfo {
        &self.info
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn cache(&self) -> &CacheDir {
        &self.cache
    }

    pub fn is_encrypted(&self) -> bool {
        self.info.encrypted
    }

    /// Look up one entry by domain and relative path.
    pub fn find(&self, domain: &str, relative_path: &str) -> Result<Option<ManifestEntry>> {
        self.manifest.find(domain, relative_path)
    }

    /// Look up one entry by file ID.
    pub fn get(&self, file_id: &str) -> Result<Option<ManifestEntry>> {
        self.manifest.get(file_id)
    }

    /// Direct children of a directory in one domain.
    pub fn list_dir(&self, domain: &str, dir: &str, page: Page) -> Result<Paged<ManifestEntry>> {
        self.manifest.list_dir(domain, dir, page)
    }

    /// Every domain with its file count.
    pub fn domains(&self) -> Result<Vec<DomainSummary>> {
        self.manifest.domains()
    }

    /// Every domain with file count and total size. Decodes every metadata
    /// blob, so run it as a job.
    pub fn domains_with_sizes(
        &self,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<Vec<DomainSummary>> {
        self.manifest.domains_with_sizes(progress)
    }

    /// Open one file's bytes for reading.
    pub fn open_file(&self, entry: &ManifestEntry) -> Result<Box<dyn ReadSeek>> {
        let path = self.materialize(entry)?;
        let file = File::open(&path).at(&path)?;
        Ok(Box::new(file))
    }

    /// Read a whole file into memory.
    ///
    /// Only for files known to be small — previews, plists, metadata. Anything
    /// that could be large goes through [`Self::open_file`] or the media
    /// protocol instead.
    pub fn read_file(&self, entry: &ManifestEntry) -> Result<Vec<u8>> {
        let path = self.materialize(entry)?;
        std::fs::read(&path).at(&path)
    }

    /// Guarantee a real path on disk for an entry's bytes.
    ///
    /// For an unencrypted backup this is the file inside the backup folder,
    /// returned without copying — it is only ever opened for reading. From M7,
    /// an encrypted backup decrypts into the cache here instead.
    pub fn materialize(&self, entry: &ManifestEntry) -> Result<PathBuf> {
        if entry.kind.is_dir() {
            return Err(BackupError::DataNotInBackup {
                domain: entry.domain.clone(),
                path: entry.relative_path.clone(),
            }
            .into());
        }
        let path = entry
            .physical_path(&self.root)
            .ok_or_else(|| BackupError::DataNotInBackup {
                domain: entry.domain.clone(),
                path: entry.relative_path.clone(),
            })?;
        if !path.is_file() {
            // In the manifest but not in the backup: a real and common state,
            // reported as such rather than as an I/O error.
            return Err(BackupError::DataNotInBackup {
                domain: entry.domain.clone(),
                path: entry.relative_path.clone(),
            }
            .into());
        }
        Ok(path)
    }

    /// Whether an entry's bytes are actually present in the backup.
    pub fn has_data(&self, entry: &ManifestEntry) -> bool {
        self.materialize(entry).is_ok()
    }

    /// Open a SQLite database from the backup through a cached working copy.
    ///
    /// Finds the `-wal` and `-shm` siblings in the manifest, copies all three
    /// into the cache and opens the copy. The database in the backup is never
    /// opened.
    pub fn open_db(&self, domain: &str, relative_path: &str) -> Result<rusqlite::Connection> {
        self.db_workcopy(domain, relative_path)?.open()
    }

    /// The working copy backing [`Self::open_db`], for callers that need the
    /// path (attaching a second database, for instance).
    pub fn db_workcopy(&self, domain: &str, relative_path: &str) -> Result<WorkCopy> {
        let relative_path = relative_path.trim_start_matches('/');
        let entry = self
            .find(domain, relative_path)?
            .ok_or_else(|| BackupError::NoSuchEntry {
                domain: domain.to_string(),
                path: relative_path.to_string(),
            })?;
        let db = self.materialize(&entry)?;

        // The sidecars are separate manifest entries, and either may be absent.
        let sidecar = |suffix: &str| -> Option<PathBuf> {
            let path = format!("{relative_path}{suffix}");
            self.find(domain, &path)
                .ok()
                .flatten()
                .and_then(|e| self.materialize(&e).ok())
        };

        let name = paths::split_relative(&entry.relative_path).1.to_string();
        let source = SourceFiles {
            db,
            wal: sidecar("-wal"),
            shm: sidecar("-shm"),
            name,
        };
        WorkCopy::materialize(&self.cache, &entry.file_id, &source)
    }

    /// Read at most `max_bytes` from the start of a file.
    ///
    /// For previews and sniffing: bounded, so a multi-gigabyte video cannot be
    /// pulled into memory by a click.
    pub fn read_prefix(&self, entry: &ManifestEntry, max_bytes: usize) -> Result<Vec<u8>> {
        let path = self.materialize(entry)?;
        let file = File::open(&path).at(&path)?;
        let mut buf = Vec::new();
        std::io::Read::take(file, max_bytes as u64)
            .read_to_end(&mut buf)
            .at(&path)?;
        Ok(buf)
    }

    /// Read a window of bytes, for the hex view.
    pub fn read_range(&self, entry: &ManifestEntry, offset: u64, len: usize) -> Result<Vec<u8>> {
        let path = self.materialize(entry)?;
        let mut file = File::open(&path).at(&path)?;
        file.seek(std::io::SeekFrom::Start(offset)).at(&path)?;
        let mut buf = Vec::new();
        std::io::Read::take(file, len as u64)
            .read_to_end(&mut buf)
            .at(&path)?;
        Ok(buf)
    }

    /// Sniff an entry's content type, reading only a short prefix.
    pub fn sniff(&self, entry: &ManifestEntry) -> Result<mime::Sniffed> {
        let prefix = self.read_prefix(entry, mime::SNIFF_BYTES)?;
        Ok(mime::sniff(&prefix, entry.name()))
    }

    /// The real size of an entry's bytes on disk.
    ///
    /// The manifest's recorded size can disagree with reality — a truncated
    /// backup, for instance — and the media protocol must serve what is
    /// actually there.
    pub fn data_len(&self, entry: &ManifestEntry) -> Result<u64> {
        let path = self.materialize(entry)?;
        Ok(std::fs::metadata(&path).at(&path)?.len())
    }

    /// Copy a file out of the backup, restoring its modification time.
    ///
    /// This is the "Save as…" primitive; M4's `export::files` reuses it.
    /// Returns the number of bytes written and whether the mtime was restored
    /// (it cannot be when the manifest records none).
    pub fn copy_file_to(&self, entry: &ManifestEntry, dest: &Path) -> Result<(u64, bool)> {
        let source = self.materialize(entry)?;
        if let Some(parent) = dest.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).at(parent)?;
        }
        let bytes = std::fs::copy(&source, dest).at(dest)?;

        let mtime_restored = match entry.meta.as_ref().and_then(|m| m.mtime) {
            Some(mtime) => {
                let ft = filetime::FileTime::from_unix_time(
                    mtime.timestamp(),
                    mtime.timestamp_subsec_nanos(),
                );
                // A filesystem that refuses the timestamp should not fail the
                // save: the bytes are already safely written.
                filetime::set_file_mtime(dest, ft).is_ok()
            }
            None => false,
        };
        Ok((bytes, mtime_restored))
    }

    /// Total size of the backup on disk, in bytes.
    ///
    /// Walks the shard directories and sums real file sizes: `stat` calls only,
    /// with no metadata blobs decoded, so it stays fast enough to call when a
    /// backup is opened. That also makes it the true size of the folder rather
    /// than the sum of the manifest's recorded sizes, which can disagree.
    pub fn total_bytes(&self) -> Result<u64> {
        fn walk(dir: &Path, total: &mut u64) -> Result<()> {
            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(source) => {
                    return Err(crate::error::Error::Io {
                        path: dir.to_path_buf(),
                        source,
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

    /// Delete everything cached for this backup.
    pub fn clear_cache(&self) -> Result<()> {
        self.cache.clear()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FixtureBackup;

    /// Open a fixture with its cache inside the fixture's own tempdir, so the
    /// test suite never writes to the user's real data directory.
    fn open(root: &Path) -> Backup {
        let cache = root
            .parent()
            .expect("a fixture root always has a parent")
            .join("cache");
        Backup::open_with_cache(root, None, CacheDir::at(cache)).unwrap()
    }

    #[test]
    fn opens_a_fixture_and_reads_its_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = FixtureBackup::new()
            .with_device_name("Test iPhone")
            .with_product_version("18.1")
            .with_file("HomeDomain", "Library/SMS/sms.db", b"not really a db")
            .with_file("HomeDomain", "Library/Preferences/com.apple.x.plist", b"ab")
            .with_file(
                "MediaDomain",
                "Library/SMS/Attachments/a/1/IMG.HEIC",
                b"xyz",
            )
            .build(tmp.path())
            .unwrap();

        let backup = open(&fixture);
        assert_eq!(backup.info().device_name.as_deref(), Some("Test iPhone"));
        assert_eq!(backup.info().product_version.as_deref(), Some("18.1"));
        assert!(!backup.is_encrypted());
        assert_eq!(backup.id().len(), 32);

        let domains = backup.domains().unwrap();
        let names: Vec<_> = domains.iter().map(|d| d.domain.as_str()).collect();
        assert!(names.contains(&"HomeDomain"));
        assert!(names.contains(&"MediaDomain"));
        // Counts include the directory entries the builder synthesizes.
        assert!(domains.iter().all(|d| d.file_count > 0));
        assert!(
            domains.iter().all(|d| d.total_size.is_none()),
            "domains() must not pay for size decoding"
        );
    }

    #[test]
    fn finds_entries_and_reads_their_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = FixtureBackup::new()
            .with_file("HomeDomain", "Library/SMS/sms.db", b"payload bytes")
            .build(tmp.path())
            .unwrap();
        let backup = open(&fixture);

        let entry = backup
            .find("HomeDomain", "Library/SMS/sms.db")
            .unwrap()
            .expect("the entry should exist");
        assert_eq!(entry.kind, EntryKind::File);
        assert_eq!(entry.name(), "sms.db");
        assert_eq!(entry.size(), 13);
        assert_eq!(
            entry.file_id, "3d0d7e5fb2ce288813306e4d4636395e047a3d28",
            "the fixture must hash paths the way iOS does"
        );

        assert_eq!(backup.read_file(&entry).unwrap(), b"payload bytes");
        // A leading slash is tolerated.
        assert!(
            backup
                .find("HomeDomain", "/Library/SMS/sms.db")
                .unwrap()
                .is_some()
        );
        // Lookups by id agree with lookups by path.
        assert_eq!(backup.get(&entry.file_id).unwrap().unwrap(), entry);
        assert!(backup.find("HomeDomain", "nope/missing").unwrap().is_none());
    }

    #[test]
    fn lists_directories_one_level_at_a_time() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = FixtureBackup::new()
            .with_file("HomeDomain", "Library/SMS/sms.db", b"a")
            .with_file("HomeDomain", "Library/SMS/Drafts/draft.plist", b"b")
            .with_file("HomeDomain", "Library/Safari/History.db", b"c")
            .build(tmp.path())
            .unwrap();
        let backup = open(&fixture);

        let root = backup.list_dir("HomeDomain", "", Page::default()).unwrap();
        let names: Vec<_> = root.items.iter().map(|e| e.name()).collect();
        assert_eq!(names, vec!["Library"], "only direct children");

        let library = backup
            .list_dir("HomeDomain", "Library", Page::default())
            .unwrap();
        let names: Vec<_> = library.items.iter().map(|e| e.name()).collect();
        assert_eq!(names, vec!["SMS", "Safari"], "directories sort first");

        let sms = backup
            .list_dir("HomeDomain", "Library/SMS", Page::default())
            .unwrap();
        let names: Vec<_> = sms.items.iter().map(|e| e.name()).collect();
        assert_eq!(names, vec!["Drafts", "sms.db"]);
        assert_eq!(sms.total, Some(2));
        assert_eq!(sms.next_offset, None);

        // Trailing and leading slashes mean the same directory.
        let same = backup
            .list_dir("HomeDomain", "/Library/SMS/", Page::default())
            .unwrap();
        assert_eq!(same.items, sms.items);
    }

    #[test]
    fn listings_paginate() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fixture = FixtureBackup::new();
        for i in 0..5 {
            fixture = fixture.with_file("HomeDomain", &format!("Docs/f{i}.txt"), b"x");
        }
        let fixture = fixture.build(tmp.path()).unwrap();
        let backup = open(&fixture);

        let first = backup
            .list_dir("HomeDomain", "Docs", Page::new(0, 2))
            .unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(first.total, Some(5));
        assert_eq!(first.next_offset, Some(2));

        let last = backup
            .list_dir("HomeDomain", "Docs", Page::new(4, 2))
            .unwrap();
        assert_eq!(last.len(), 1);
        assert_eq!(last.next_offset, None);

        // Paging over the whole listing yields every entry exactly once.
        let mut seen = Vec::new();
        let mut offset = Some(0);
        while let Some(o) = offset {
            let page = backup
                .list_dir("HomeDomain", "Docs", Page::new(o, 2))
                .unwrap();
            seen.extend(page.items.iter().map(|e| e.relative_path.clone()));
            offset = page.next_offset;
        }
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 5);
    }

    #[test]
    fn wildcards_in_a_path_are_not_patterns() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = FixtureBackup::new()
            .with_file("HomeDomain", "100%_sure/a.txt", b"a")
            .with_file("HomeDomain", "100x_sure/b.txt", b"b")
            .build(tmp.path())
            .unwrap();
        let backup = open(&fixture);

        let listing = backup
            .list_dir("HomeDomain", "100%_sure", Page::default())
            .unwrap();
        let names: Vec<_> = listing.items.iter().map(|e| e.name()).collect();
        assert_eq!(names, vec!["a.txt"], "% and _ must be escaped, not matched");
    }

    #[test]
    fn opens_a_database_through_a_working_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = FixtureBackup::new()
            .with_db(
                "HomeDomain",
                "Library/SMS/sms.db",
                "CREATE TABLE message (ROWID INTEGER PRIMARY KEY, text TEXT);
                 INSERT INTO message (text) VALUES ('a'), ('b');",
            )
            .build(tmp.path())
            .unwrap();
        let backup = open(&fixture);

        let conn = backup.open_db("HomeDomain", "Library/SMS/sms.db").unwrap();
        let n: i64 = conn
            .query_one("SELECT count(*) FROM message", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);

        // The copy lives in the cache, not in the backup.
        let copy = backup
            .db_workcopy("HomeDomain", "Library/SMS/sms.db")
            .unwrap();
        assert!(copy.path().starts_with(backup.cache().root()));
        assert!(!copy.path().starts_with(backup.root()));
    }

    #[test]
    fn rejects_folders_that_are_not_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let err = Backup::open(tmp.path(), None).unwrap_err();
        assert_eq!(err.code(), "not_a_backup");
        assert!(
            err.to_string().contains("Manifest.db"),
            "the message should tell the user what to look for: {err}"
        );

        let missing = tmp.path().join("nope");
        assert_eq!(
            Backup::open(&missing, None).unwrap_err().code(),
            "not_a_directory"
        );

        let file = tmp.path().join("afile");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(
            Backup::open(&file, None).unwrap_err().code(),
            "not_a_directory"
        );
    }

    #[test]
    fn an_encrypted_backup_asks_for_a_password_then_says_it_cannot_yet() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = FixtureBackup::new()
            .marked_encrypted()
            .with_file("HomeDomain", "a.txt", b"x")
            .build(tmp.path())
            .unwrap();

        // No password: the UI should prompt.
        assert_eq!(
            Backup::open(&fixture, None).unwrap_err().code(),
            "password_required"
        );
        // With one: honest about not implementing M7 yet, rather than
        // reporting a wrong password or returning garbage.
        assert_eq!(
            Backup::open(&fixture, Some("hunter2")).unwrap_err().code(),
            "encryption_unsupported"
        );
    }

    #[test]
    fn missing_data_is_reported_as_missing_not_as_io_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = FixtureBackup::new()
            .with_file("HomeDomain", "Library/SMS/sms.db", b"a")
            // Listed in the manifest, bytes deliberately left out.
            .with_phantom_file("HomeDomain", "Library/SMS/gone.db", 1234)
            .build(tmp.path())
            .unwrap();
        let backup = open(&fixture);

        let entry = backup
            .find("HomeDomain", "Library/SMS/gone.db")
            .unwrap()
            .expect("the manifest row should still be listed");
        assert_eq!(entry.size(), 1234, "metadata survives without the bytes");
        assert!(!backup.has_data(&entry));
        assert_eq!(
            backup.materialize(&entry).unwrap_err().code(),
            "data_not_in_backup"
        );
    }

    #[test]
    fn opening_never_writes_to_the_backup_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = FixtureBackup::new()
            .with_db(
                "HomeDomain",
                "Library/SMS/sms.db",
                "CREATE TABLE message (ROWID INTEGER PRIMARY KEY);",
            )
            .build(tmp.path())
            .unwrap();

        fn snapshot(dir: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
            let mut out = Vec::new();
            let mut stack = vec![dir.to_path_buf()];
            while let Some(d) = stack.pop() {
                for entry in std::fs::read_dir(&d).unwrap() {
                    let entry = entry.unwrap();
                    let meta = entry.metadata().unwrap();
                    if meta.is_dir() {
                        stack.push(entry.path());
                    } else {
                        out.push((entry.path(), meta.len(), meta.modified().unwrap()));
                    }
                }
            }
            out.sort();
            out
        }

        let before = snapshot(&fixture);
        let backup = open(&fixture);
        let _ = backup.domains().unwrap();
        let _ = backup.open_db("HomeDomain", "Library/SMS/sms.db").unwrap();
        let entry = backup
            .find("HomeDomain", "Library/SMS/sms.db")
            .unwrap()
            .unwrap();
        let _ = backup.read_file(&entry).unwrap();
        let after = snapshot(&fixture);

        assert_eq!(before, after, "hard rule 1: the backup folder is read-only");
    }

    #[test]
    fn sizes_are_summed_per_domain_on_request() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = FixtureBackup::new()
            .with_file("HomeDomain", "a.txt", b"12345")
            .with_file("HomeDomain", "b.txt", b"123")
            .with_file("MediaDomain", "c.txt", b"1")
            .build(tmp.path())
            .unwrap();
        let backup = open(&fixture);

        let mut calls = 0;
        let domains = backup
            .domains_with_sizes(&mut |_done, _total| calls += 1)
            .unwrap();
        let home = domains.iter().find(|d| d.domain == "HomeDomain").unwrap();
        let media = domains.iter().find(|d| d.domain == "MediaDomain").unwrap();
        assert_eq!(home.total_size, Some(8));
        assert_eq!(media.total_size, Some(1));
        assert!(calls > 0, "progress should be reported at least once");
    }
}
