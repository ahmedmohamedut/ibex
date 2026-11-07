//! `Manifest.db`: the index of every file in a backup.
//!
//! ```sql
//! CREATE TABLE Files (
//!   fileID       TEXT PRIMARY KEY,  -- SHA1 hex of "<domain>-<relativePath>"
//!   domain       TEXT,
//!   relativePath TEXT,
//!   flags        INTEGER,           -- 1 = file, 2 = directory, 4 = symlink
//!   file         BLOB               -- NSKeyedArchiver plist (MBFile)
//! );
//! ```
//!
//! See `docs/DATA_SOURCES.md`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use plist::Value;
use rusqlite::{Connection, OpenFlags, Row};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{BackupError, Result};
use crate::page::{Page, Paged};
use crate::util::{nskeyedarchiver, paths, time};

/// What a manifest row describes, from the `flags` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    /// A flags value we don't recognise. Kept rather than dropped so the Raw
    /// browser can still show the row.
    Other(i64),
}

impl EntryKind {
    fn from_flags(flags: i64) -> Self {
        match flags {
            1 => EntryKind::File,
            2 => EntryKind::Directory,
            4 => EntryKind::Symlink,
            other => EntryKind::Other(other),
        }
    }

    pub fn is_dir(self) -> bool {
        matches!(self, EntryKind::Directory)
    }

    pub fn is_file(self) -> bool {
        matches!(self, EntryKind::File)
    }
}

/// Filesystem metadata, decoded from the archived `MBFile` in the `file` blob.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileMeta {
    pub size: u64,
    pub mtime: Option<DateTime<Utc>>,
    pub birth: Option<DateTime<Utc>>,
    pub mode: u32,
    pub protection_class: Option<u32>,
    /// The wrapped per-file key, for encrypted backups (milestone M7).
    ///
    /// Not serialized: key material must never cross IPC or reach a log.
    #[serde(skip)]
    pub encryption_key: Option<Vec<u8>>,
}

impl FileMeta {
    /// Read an `MBFile` dictionary that has already been UID-resolved.
    fn from_mbfile(value: &Value) -> Option<Self> {
        let dict = value.as_dictionary()?;

        let size = dict
            .get("Size")
            .and_then(Value::as_signed_integer)
            .and_then(|s| u64::try_from(s).ok())
            .unwrap_or(0);

        let mode = dict
            .get("Mode")
            .and_then(Value::as_signed_integer)
            .and_then(|m| u32::try_from(m).ok())
            .unwrap_or(0);

        Some(FileMeta {
            size,
            // MBFile stores filesystem times in the *Unix* epoch, unlike the
            // Core Data databases inside the backup.
            mtime: dict
                .get("LastModified")
                .and_then(Value::as_signed_integer)
                .and_then(time::from_unix_secs),
            birth: dict
                .get("Birth")
                .and_then(Value::as_signed_integer)
                .and_then(time::from_unix_secs),
            mode,
            protection_class: dict
                .get("ProtectionClass")
                .and_then(Value::as_signed_integer)
                .and_then(|c| u32::try_from(c).ok()),
            encryption_key: dict
                .get("EncryptionKey")
                .and_then(Value::as_data)
                .map(<[u8]>::to_vec),
        })
    }

    /// The Unix permission bits, without the file-type bits.
    pub fn permissions(&self) -> u32 {
        self.mode & 0o7777
    }
}

/// One row of the `Files` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManifestEntry {
    pub file_id: String,
    pub domain: String,
    pub relative_path: String,
    pub kind: EntryKind,
    /// `None` when the `file` blob is absent or could not be decoded. The row
    /// is still listed — tolerant parsing, per AGENTS.md.
    pub meta: Option<FileMeta>,
}

impl ManifestEntry {
    /// The last path component.
    pub fn name(&self) -> &str {
        paths::split_relative(&self.relative_path).1
    }

    /// The containing directory, or `None` at a domain's root.
    pub fn parent(&self) -> Option<&str> {
        paths::split_relative(&self.relative_path).0
    }

    pub fn size(&self) -> u64 {
        self.meta.as_ref().map_or(0, |m| m.size)
    }

    /// Where this entry's bytes live inside the backup folder.
    ///
    /// `None` for anything that has no bytes of its own (directories), or a
    /// malformed file ID.
    pub fn physical_path(&self, backup_root: &Path) -> Option<std::path::PathBuf> {
        if self.kind.is_dir() {
            return None;
        }
        paths::physical_path(backup_root, &self.file_id)
    }
}

/// A domain and what it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DomainSummary {
    pub domain: String,
    pub file_count: u64,
    /// Sum of file sizes, present only when sizes have been scanned.
    ///
    /// Sizes live inside each row's archived `file` blob, so totalling them
    /// means decoding every blob in the backup. [`Manifest::domains`] therefore
    /// leaves this `None` and [`Manifest::domains_with_sizes`] fills it in.
    pub total_size: Option<u64>,
}

/// An open, read-only connection to a backup's `Manifest.db`.
///
/// The connection sits behind a `Mutex` because `rusqlite::Connection` is
/// `Send` but not `Sync`, and the app shares one open backup across threads:
/// Tauri commands run on blocking workers and the `ibex://` protocol serves
/// files from its own thread. Reads here are short and indexed, so serializing
/// them costs little next to keeping `Backup` shareable.
pub struct Manifest {
    conn: Mutex<Connection>,
    /// Whether the `file` blob column exists. Checked once, at open.
    has_file_column: bool,
}

impl std::fmt::Debug for Manifest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Manifest")
            .field("has_file_column", &self.has_file_column)
            .finish_non_exhaustive()
    }
}

/// Columns selected for a [`ManifestEntry`], in the order [`row_to_entry`] reads them.
const ENTRY_COLUMNS: &str = "fileID, domain, relativePath, flags, file";

impl Manifest {
    /// Open a `Manifest.db` read-only.
    ///
    /// `path` must point at a working copy in the cache, never at the file in
    /// the backup folder — see [`crate::workcopy`].
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Self::from_connection(conn)
    }

    /// Wrap an existing connection. Used by tests and by the encrypted-backup
    /// path, which decrypts the manifest into the cache first.
    pub fn from_connection(conn: Connection) -> Result<Self> {
        // Hard rule 6: check the schema before selecting from it.
        let columns = Self::columns_of(&conn, "Files")?;
        if columns.is_empty() {
            return Err(BackupError::MalformedMetadata {
                file: "Manifest.db".to_string(),
                detail: "no Files table".to_string(),
            }
            .into());
        }
        for required in ["fileID", "domain", "relativePath"] {
            if !columns.iter().any(|c| c == required) {
                return Err(BackupError::MalformedMetadata {
                    file: "Manifest.db".to_string(),
                    detail: format!("Files table has no {required} column"),
                }
                .into());
            }
        }
        Ok(Self {
            has_file_column: columns.iter().any(|c| c == "file"),
            conn: Mutex::new(conn),
        })
    }

    /// Column names of a table, empty if the table does not exist.
    fn columns_of(conn: &Connection, table: &str) -> Result<Vec<String>> {
        let mut stmt = conn.prepare("SELECT name FROM pragma_table_info(?1)")?;
        let names = stmt
            .query_map([table], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(names)
    }

    /// The `SELECT` list, omitting `file` when the column is absent.
    fn entry_columns(&self) -> &'static str {
        if self.has_file_column {
            ENTRY_COLUMNS
        } else {
            "fileID, domain, relativePath, flags, NULL"
        }
    }

    /// Look up one entry by domain and relative path.
    ///
    /// Tries the computed file ID first (a primary-key hit), then falls back to
    /// querying the columns directly — the `"<domain>-<path>"` join Apple
    /// hashes is ambiguous, so the computed ID is only a fast path. See
    /// `util::paths::file_id`.
    pub fn find(&self, domain: &str, relative_path: &str) -> Result<Option<ManifestEntry>> {
        let relative_path = relative_path.trim_start_matches('/');
        let id = paths::file_id(domain, relative_path);

        if let Some(entry) = self.get(&id)?
            && entry.domain == domain
            && entry.relative_path == relative_path
        {
            return Ok(Some(entry));
        }

        let sql = format!(
            "SELECT {} FROM Files WHERE domain = ?1 AND relativePath = ?2 LIMIT 1",
            self.entry_columns()
        );
        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query((domain, relative_path))?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_entry(row)?)),
            None => Ok(None),
        }
    }

    /// Look up one entry by its file ID.
    pub fn get(&self, file_id: &str) -> Result<Option<ManifestEntry>> {
        let sql = format!(
            "SELECT {} FROM Files WHERE fileID = ?1",
            self.entry_columns()
        );
        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query([file_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_entry(row)?)),
            None => Ok(None),
        }
    }

    /// Direct children of a directory within one domain.
    ///
    /// `dir` is relative to the domain root; `""` lists the root itself.
    /// Directories sort before files, then by path, so paging is stable.
    pub fn list_dir(&self, domain: &str, dir: &str, page: Page) -> Result<Paged<ManifestEntry>> {
        let dir = paths::normalize_dir(dir);

        // Filter exactly in SQL so LIMIT/OFFSET page over the right set: the
        // direct children of "a/b" are the paths matching "a/b/%" but not
        // "a/b/%/%". At the root the same shape with "%" and "%/%" selects the
        // paths that contain no slash at all.
        let (prefix, deeper) = if dir.is_empty() {
            ("%".to_string(), "%/%".to_string())
        } else {
            let escaped = escape_like(dir);
            (format!("{escaped}/%"), format!("{escaped}/%/%"))
        };
        // One line: a raw string cannot use `\` continuations, and the LIKE
        // escape character has to stay a literal backslash.
        const PREDICATE: &str = r"domain = ?1 AND relativePath <> '' AND relativePath LIKE ?2 ESCAPE '\' AND relativePath NOT LIKE ?3 ESCAPE '\'";

        let total = {
            let sql = format!("SELECT count(*) FROM Files WHERE {PREDICATE}");
            let n: i64 =
                self.conn()
                    .query_one(&sql, rusqlite::params![domain, &prefix, &deeper], |r| {
                        r.get(0)
                    })?;
            n.max(0) as u64
        };

        let sql = format!(
            "SELECT {cols} FROM Files WHERE {PREDICATE} \
             ORDER BY CASE WHEN flags = 2 THEN 0 ELSE 1 END, relativePath \
             LIMIT ?4 OFFSET ?5",
            cols = self.entry_columns(),
        );
        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let items = stmt
            .query_and_then(
                rusqlite::params![
                    domain,
                    &prefix,
                    &deeper,
                    page.sql_limit(),
                    page.sql_offset()
                ],
                row_to_entry,
            )?
            .collect::<Result<Vec<_>>>()?;

        Ok(Paged::new(items, page, Some(total)))
    }

    /// Every domain, with its file count.
    ///
    /// Cheap: one `GROUP BY`. Sizes need [`Self::domains_with_sizes`].
    pub fn domains(&self) -> Result<Vec<DomainSummary>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT domain, count(*) FROM Files WHERE domain IS NOT NULL \
             GROUP BY domain ORDER BY domain",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(DomainSummary {
                    domain: row.get(0)?,
                    file_count: row.get::<_, i64>(1)?.max(0) as u64,
                    total_size: None,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Every domain with its file count *and* total size.
    ///
    /// Decodes every `file` blob in the backup, so this is a job with progress,
    /// not something to call on a UI thread. `progress` is called with
    /// (rows done, rows total).
    pub fn domains_with_sizes(
        &self,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<Vec<DomainSummary>> {
        let mut summaries = self.domains()?;
        if !self.has_file_column {
            return Ok(summaries);
        }

        let total_rows = self.count_files()?;
        let mut sizes: HashMap<String, u64> = HashMap::new();
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT domain, file FROM Files WHERE flags = 1")?;
        let mut rows = stmt.query([])?;
        let mut done = 0u64;
        while let Some(row) = rows.next()? {
            let domain: Option<String> = row.get(0)?;
            let blob: Option<Vec<u8>> = row.get(1)?;
            if let (Some(domain), Some(blob)) = (domain, blob) {
                let size = nskeyedarchiver::resolve_bytes(&blob)
                    .ok()
                    .as_ref()
                    .and_then(FileMeta::from_mbfile)
                    .map_or(0, |m| m.size);
                *sizes.entry(domain).or_default() += size;
            }
            done += 1;
            if done.is_multiple_of(2_000) {
                progress(done, total_rows);
            }
        }
        progress(total_rows, total_rows);

        for summary in &mut summaries {
            summary.total_size = Some(sizes.get(&summary.domain).copied().unwrap_or(0));
        }
        Ok(summaries)
    }

    /// Number of rows in the manifest.
    pub fn count_files(&self) -> Result<u64> {
        let n: i64 = self
            .conn()
            .query_one("SELECT count(*) FROM Files", [], |r| r.get(0))?;
        Ok(n.max(0) as u64)
    }

    /// Number of rows in one domain.
    pub fn count_in_domain(&self, domain: &str) -> Result<u64> {
        let n: i64 = self.conn().query_one(
            "SELECT count(*) FROM Files WHERE domain = ?1",
            [domain],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as u64)
    }

    /// Run a closure with the underlying connection, for queries this module
    /// does not wrap — the integration tests' structure-only checks, for
    /// instance.
    pub fn with_connection<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        f(&self.conn())
    }

    /// Lock the connection.
    ///
    /// A poisoned lock means a previous query panicked. The connection is still
    /// usable for reads, and failing every later query would be worse than
    /// carrying on, so the guard is recovered.
    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Build an entry from a row selecting [`ENTRY_COLUMNS`].
fn row_to_entry(row: &Row<'_>) -> Result<ManifestEntry> {
    let blob: Option<Vec<u8>> = row.get(4)?;
    // A blob that won't decode leaves `meta` as None; the row is still listed.
    let meta = blob
        .as_deref()
        .filter(|b| !b.is_empty())
        .and_then(|b| nskeyedarchiver::resolve_bytes(b).ok())
        .as_ref()
        .and_then(FileMeta::from_mbfile);

    Ok(ManifestEntry {
        file_id: row.get(0)?,
        domain: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
        relative_path: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        kind: EntryKind::from_flags(row.get::<_, Option<i64>>(3)?.unwrap_or(0)),
        meta,
    })
}

/// Escape the LIKE wildcards in a literal path fragment.
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_escaping_protects_wildcards() {
        assert_eq!(escape_like("a_b%c"), r"a\_b\%c");
        assert_eq!(escape_like(r"a\b"), r"a\\b");
        assert_eq!(escape_like("Library/SMS"), "Library/SMS");
    }

    #[test]
    fn flags_map_to_kinds() {
        assert_eq!(EntryKind::from_flags(1), EntryKind::File);
        assert_eq!(EntryKind::from_flags(2), EntryKind::Directory);
        assert_eq!(EntryKind::from_flags(4), EntryKind::Symlink);
        assert_eq!(EntryKind::from_flags(9), EntryKind::Other(9));
        assert!(EntryKind::Directory.is_dir());
        assert!(!EntryKind::Symlink.is_file());
    }

    #[test]
    fn mbfile_decoding_reads_what_it_can() {
        let mut dict = plist::Dictionary::new();
        dict.insert("Size".into(), Value::Integer(4096.into()));
        dict.insert("Mode".into(), Value::Integer(0o100_644.into()));
        // 2026-11-01T00:00:00Z as a Unix timestamp
        dict.insert("LastModified".into(), Value::Integer(1_793_491_200.into()));
        dict.insert("ProtectionClass".into(), Value::Integer(3.into()));

        let meta = FileMeta::from_mbfile(&Value::Dictionary(dict)).unwrap();
        assert_eq!(meta.size, 4096);
        assert_eq!(meta.permissions(), 0o644);
        assert_eq!(meta.protection_class, Some(3));
        assert_eq!(meta.encryption_key, None);
        assert_eq!(
            meta.mtime.unwrap().to_rfc3339(),
            "2026-11-01T00:00:00+00:00"
        );
        assert_eq!(meta.birth, None, "an absent key is None, not an error");
    }

    #[test]
    fn mbfile_decoding_survives_nonsense() {
        // Negative size, mode and class are all out of range for their target
        // types; none of them may panic or abort the decode.
        let mut dict = plist::Dictionary::new();
        dict.insert("Size".into(), Value::Integer((-1).into()));
        dict.insert("Mode".into(), Value::Integer((-5).into()));
        dict.insert("ProtectionClass".into(), Value::Integer((-2).into()));
        dict.insert("LastModified".into(), Value::String("nonsense".into()));

        let meta = FileMeta::from_mbfile(&Value::Dictionary(dict)).unwrap();
        assert_eq!(meta.size, 0);
        assert_eq!(meta.mode, 0);
        assert_eq!(meta.protection_class, None);
        assert_eq!(meta.mtime, None);
    }

    #[test]
    fn encryption_keys_are_never_serialized() {
        let meta = FileMeta {
            size: 1,
            encryption_key: Some(vec![0xde, 0xad, 0xbe, 0xef]),
            ..FileMeta::default()
        };
        let json = serde_json::to_string(&meta).unwrap();
        assert!(
            !json.contains("encryptionKey") && !json.contains("dead"),
            "key material must not cross a serialization boundary: {json}"
        );
    }

    #[test]
    fn entry_paths() {
        let entry = ManifestEntry {
            file_id: "3d0d7e5fb2ce288813306e4d4636395e047a3d28".into(),
            domain: "HomeDomain".into(),
            relative_path: "Library/SMS/sms.db".into(),
            kind: EntryKind::File,
            meta: None,
        };
        assert_eq!(entry.name(), "sms.db");
        assert_eq!(entry.parent(), Some("Library/SMS"));
        assert_eq!(
            entry.size(),
            0,
            "no metadata means unknown size, not a panic"
        );
        assert_eq!(
            entry.physical_path(Path::new("/b")).unwrap(),
            Path::new("/b/3d/3d0d7e5fb2ce288813306e4d4636395e047a3d28")
        );

        let dir = ManifestEntry {
            kind: EntryKind::Directory,
            ..entry
        };
        assert_eq!(
            dir.physical_path(Path::new("/b")),
            None,
            "directories have no bytes of their own"
        );
    }
}
