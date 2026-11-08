//! SQLite working copies.
//!
//! A database in a backup often has `-wal` and `-shm` siblings holding the most
//! recent writes, and SQLite needs to touch those files to read the database.
//! Since the backup folder is strictly read-only (AGENTS.md hard rule 1), we
//! never open a database in place: all three files are copied into the cache
//! first and opened there.
//!
//! This module takes plain paths rather than a [`crate::backup::Backup`], so it
//! serves both callers: databases listed in the manifest, and `Manifest.db`
//! itself, which is a top-level file that no manifest lists.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDir;
use crate::error::{IoResultExt, Result};

/// The files that make up one SQLite database in a backup.
#[derive(Debug, Clone)]
pub struct SourceFiles {
    /// The database file itself.
    pub db: PathBuf,
    /// Write-ahead log, when the backup contains one.
    pub wal: Option<PathBuf>,
    /// Shared-memory file, when the backup contains one.
    pub shm: Option<PathBuf>,
    /// File name to use inside the cache. SQLite derives the `-wal` and `-shm`
    /// names from it, so it must stay a plain base name.
    pub name: String,
}

impl SourceFiles {
    /// A database with no sidecars.
    pub fn new(db: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        Self {
            db: db.into(),
            wal: None,
            shm: None,
            name: name.into(),
        }
    }

    /// Add `-wal` / `-shm` siblings, keeping only the ones that exist.
    pub fn with_sidecars(mut self, wal: Option<PathBuf>, shm: Option<PathBuf>) -> Self {
        self.wal = wal.filter(|p| p.exists());
        self.shm = shm.filter(|p| p.exists());
        self
    }
}

/// A database copied into the cache, ready to open.
#[derive(Debug, Clone)]
pub struct WorkCopy {
    dir: PathBuf,
    db: PathBuf,
}

impl WorkCopy {
    /// Copy `src` into `cache/db/<key>/`, skipping the copy when an up-to-date
    /// one is already there.
    ///
    /// `key` is usually the file ID; for `Manifest.db` it is a fixed name.
    pub fn materialize(cache: &CacheDir, key: &str, src: &SourceFiles) -> Result<Self> {
        let dir = cache.db_dir(key);
        CacheDir::ensure(&dir)?;

        let db = dir.join(&src.name);
        let stamp_path = dir.join(".stamp");

        // Hold a lock for this one working copy's directory, for the whole
        // check-then-copy sequence below. Two callers materializing the same
        // database concurrently — two IPC commands racing on first open, or
        // (how this was actually found) two parallel integration tests
        // against the one real backup every `real_backup.rs` test shares —
        // must not interleave their remove/copy steps. Without this, one
        // caller's SQLite connection can open mid-replacement and find a
        // missing or half-written database: exactly the "no Files table" and
        // "i/o error" failures a cold, parallel test run produced before this
        // lock existed. Other databases are not affected: the lock is keyed
        // per directory, not global.
        let lock = lock_for(&dir);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());

        let stamp = source_stamp(src)?;

        // A stamp file rather than comparing the copies themselves: opening a
        // WAL database read-write checkpoints it, which legitimately changes
        // the copy's size and would otherwise force a re-copy every time.
        let current = std::fs::read_to_string(&stamp_path).ok();
        if current.as_deref() == Some(stamp.as_str()) && db.exists() {
            return Ok(Self { dir, db });
        }

        // Remove a stale copy and its sidecars before writing a new one, so a
        // leftover WAL can never be applied to a newer database.
        for suffix in ["", "-wal", "-shm"] {
            let path = sidecar(&db, suffix);
            if path.exists() {
                std::fs::remove_file(&path).at(&path)?;
            }
        }
        let _ = std::fs::remove_file(&stamp_path);

        std::fs::copy(&src.db, &db).at(&src.db)?;
        if let Some(wal) = &src.wal {
            let dest = sidecar(&db, "-wal");
            std::fs::copy(wal, &dest).at(wal)?;
        }
        if let Some(shm) = &src.shm {
            let dest = sidecar(&db, "-shm");
            std::fs::copy(shm, &dest).at(shm)?;
        }

        std::fs::write(&stamp_path, &stamp).at(&stamp_path)?;
        Ok(Self { dir, db })
    }

    /// The copied database's path.
    pub fn path(&self) -> &Path {
        &self.db
    }

    /// The directory holding the copy and its sidecars.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Open the working copy.
    ///
    /// Read-only first. If the database needs WAL recovery, SQLite cannot do
    /// that without writing, so we retry read-write — on the *copy*, never on
    /// the backup.
    pub fn open(&self) -> Result<Connection> {
        open_db(&self.db)
    }
}

/// Open a SQLite file read-only, falling back to read-write if the WAL needs
/// recovering. `path` must be a file in the cache.
pub fn open_db(path: &Path) -> Result<Connection> {
    let read_only = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if let Ok(conn) = Connection::open_with_flags(path, read_only) {
        // A read-only open can succeed and then fail on the first read, when
        // recovering the WAL would require a write. Probe before handing it on.
        if conn
            .query_one("SELECT count(*) FROM sqlite_master", [], |r| {
                r.get::<_, i64>(0)
            })
            .is_ok()
        {
            return Ok(conn);
        }
    }

    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    Ok(conn)
}

/// One lock per working-copy directory, so concurrent materializations of
/// *different* databases never wait on each other, but two callers racing on
/// the *same* one are serialized rather than interleaved.
///
/// Process-local only — this is a desktop app with one process per running
/// instance, not a guard against another program touching the cache.
fn lock_for(dir: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let registry = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registry = registry.lock().unwrap_or_else(|e| e.into_inner());
    registry
        .entry(dir.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// `path` with a suffix appended to its file name.
fn sidecar(db: &Path, suffix: &str) -> PathBuf {
    if suffix.is_empty() {
        return db.to_path_buf();
    }
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    db.with_file_name(name)
}

/// A string describing the source files' size and mtime, used to decide whether
/// an existing copy is still current.
fn source_stamp(src: &SourceFiles) -> Result<String> {
    let mut out = String::new();
    for path in [Some(&src.db), src.wal.as_ref(), src.shm.as_ref()]
        .into_iter()
        .flatten()
    {
        let meta = std::fs::metadata(path).at(path)?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        out.push_str(&format!("{}:{}:{}\n", path.display(), meta.len(), mtime));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a small SQLite database at `path` with one row.
    fn make_db(path: &Path, journal_mode: &str) {
        let conn = Connection::open(path).unwrap();
        conn.pragma_update(None, "journal_mode", journal_mode)
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT);
             INSERT INTO t (v) VALUES ('one');",
        )
        .unwrap();
    }

    fn row_count(conn: &Connection) -> i64 {
        conn.query_one("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn copies_a_plain_database() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("backup");
        std::fs::create_dir_all(&src_dir).unwrap();
        let src_db = src_dir.join("plain.db");
        make_db(&src_db, "delete");

        let cache = CacheDir::at(tmp.path().join("cache"));
        let copy =
            WorkCopy::materialize(&cache, "fid", &SourceFiles::new(&src_db, "plain.db")).unwrap();

        assert_eq!(copy.path(), cache.db_dir("fid").join("plain.db"));
        assert_eq!(row_count(&copy.open().unwrap()), 1);
    }

    #[test]
    fn copies_wal_and_shm_and_sees_their_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("backup");
        std::fs::create_dir_all(&src_dir).unwrap();
        let src_db = src_dir.join("wal.db");

        // Write a row that stays in the WAL: the database file alone does not
        // contain it, so reading it back proves the sidecars were used.
        {
            let conn = Connection::open(&src_db).unwrap();
            conn.pragma_update(None, "journal_mode", "wal").unwrap();
            conn.execute_batch(
                "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT);
                 INSERT INTO t (v) VALUES ('one');",
            )
            .unwrap();
            conn.execute("INSERT INTO t (v) VALUES ('two')", [])
                .unwrap();
            // Leave the WAL unchecked-pointed.
            std::mem::forget(conn);
        }
        assert!(
            src_dir.join("wal.db-wal").exists(),
            "the test needs a real -wal file"
        );

        let cache = CacheDir::at(tmp.path().join("cache"));
        let src = SourceFiles::new(&src_db, "wal.db").with_sidecars(
            Some(src_dir.join("wal.db-wal")),
            Some(src_dir.join("wal.db-shm")),
        );
        let copy = WorkCopy::materialize(&cache, "fid", &src).unwrap();

        assert!(copy.dir().join("wal.db-wal").exists());
        assert_eq!(
            row_count(&copy.open().unwrap()),
            2,
            "rows still in the WAL must be visible"
        );
    }

    #[test]
    fn missing_sidecars_are_simply_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let src_db = tmp.path().join("plain.db");
        make_db(&src_db, "delete");

        let src = SourceFiles::new(&src_db, "plain.db").with_sidecars(
            Some(tmp.path().join("plain.db-wal")),
            Some(tmp.path().join("plain.db-shm")),
        );
        assert!(src.wal.is_none() && src.shm.is_none());

        let cache = CacheDir::at(tmp.path().join("cache"));
        let copy = WorkCopy::materialize(&cache, "fid", &src).unwrap();
        assert_eq!(row_count(&copy.open().unwrap()), 1);
    }

    #[test]
    fn an_unchanged_source_is_not_copied_twice() {
        let tmp = tempfile::tempdir().unwrap();
        let src_db = tmp.path().join("plain.db");
        make_db(&src_db, "delete");
        let cache = CacheDir::at(tmp.path().join("cache"));
        let src = SourceFiles::new(&src_db, "plain.db");

        let first = WorkCopy::materialize(&cache, "fid", &src).unwrap();
        // Mark the copy so we can tell whether it gets replaced.
        let marker = first.dir().join("marker");
        std::fs::write(&marker, b"x").unwrap();

        let second = WorkCopy::materialize(&cache, "fid", &src).unwrap();
        assert_eq!(first.path(), second.path());
        assert!(marker.exists(), "the copy should have been reused");
    }

    #[test]
    fn a_changed_source_replaces_the_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let src_db = tmp.path().join("plain.db");
        make_db(&src_db, "delete");
        let cache = CacheDir::at(tmp.path().join("cache"));
        let src = SourceFiles::new(&src_db, "plain.db");

        let copy = WorkCopy::materialize(&cache, "fid", &src).unwrap();
        assert_eq!(row_count(&copy.open().unwrap()), 1);

        // A newer backup of the same database, with another row.
        {
            let conn = Connection::open(&src_db).unwrap();
            conn.execute("INSERT INTO t (v) VALUES ('two')", [])
                .unwrap();
        }
        let copy = WorkCopy::materialize(&cache, "fid", &src).unwrap();
        assert_eq!(
            row_count(&copy.open().unwrap()),
            2,
            "a changed source must be re-copied"
        );
    }

    #[test]
    fn the_source_is_never_written_to() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("backup");
        std::fs::create_dir_all(&src_dir).unwrap();
        let src_db = src_dir.join("wal.db");
        {
            let conn = Connection::open(&src_db).unwrap();
            conn.pragma_update(None, "journal_mode", "wal").unwrap();
            conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT);")
                .unwrap();
            conn.execute("INSERT INTO t (v) VALUES ('one')", [])
                .unwrap();
            std::mem::forget(conn);
        }

        let before: Vec<_> = std::fs::read_dir(&src_dir)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let m = e.metadata().unwrap();
                (e.path(), m.len(), m.modified().unwrap())
            })
            .collect();

        let cache = CacheDir::at(tmp.path().join("cache"));
        let src = SourceFiles::new(&src_db, "wal.db").with_sidecars(
            Some(src_dir.join("wal.db-wal")),
            Some(src_dir.join("wal.db-shm")),
        );
        let copy = WorkCopy::materialize(&cache, "fid", &src).unwrap();
        // Force the open path that may fall back to read-write.
        let _ = row_count(&copy.open().unwrap());

        let after: Vec<_> = std::fs::read_dir(&src_dir)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let m = e.metadata().unwrap();
                (e.path(), m.len(), m.modified().unwrap())
            })
            .collect();
        assert_eq!(before, after, "the backup folder must not be modified");
    }

    /// Many threads materializing the same working copy at once must never
    /// see a half-replaced or missing database.
    ///
    /// This is the race a cold, parallel `cargo test` run against one real
    /// backup hit in practice: every integration test in `real_backup.rs`
    /// opens the same backup, so without the per-directory lock, concurrent
    /// first-time materializations of `sms.db`/`ChatStorage.sqlite` raced on
    /// removing and recopying the same files, and a reader could open a
    /// connection in the gap.
    #[test]
    fn concurrent_materialize_of_the_same_key_never_corrupts_it() {
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let src_db = src_dir.join("real.db");
        {
            let conn = Connection::open(&src_db).unwrap();
            conn.execute_batch("CREATE TABLE t (v TEXT); INSERT INTO t VALUES ('a'), ('b');")
                .unwrap();
        }

        let cache = Arc::new(CacheDir::at(tmp.path().join("cache")));
        let src = Arc::new(SourceFiles::new(&src_db, "real.db"));

        let handles: Vec<_> = (0..16)
            .map(|_| {
                let cache = Arc::clone(&cache);
                let src = Arc::clone(&src);
                std::thread::spawn(move || {
                    let copy = WorkCopy::materialize(&cache, "shared-key", &src).unwrap();
                    let conn = copy.open().unwrap();
                    row_count(&conn)
                })
            })
            .collect();

        for handle in handles {
            // Every racer must see the real table with its real rows — never
            // an empty database created by SQLite opening a path mid-removal,
            // and never an error from a half-copied file.
            assert_eq!(handle.join().unwrap(), 2);
        }
    }
}
