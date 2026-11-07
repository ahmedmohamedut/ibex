//! Synthetic backup builder, behind the `testutil` feature.
//!
//! Writes a structurally valid backup folder — `Info.plist`,
//! `Manifest.plist`, `Status.plist`, a real `Manifest.db` with binary
//! `NSKeyedArchiver` metadata blobs, and payload files at their hashed paths.
//! Every unit test in the crate runs against one of these, so no real backup
//! and no personal data is ever needed (AGENTS.md hard rule 2).
//!
//! ```no_run
//! use ibex_core::testutil::FixtureBackup;
//!
//! let tmp = std::env::temp_dir().join("ibex-doc-example");
//! let root = FixtureBackup::new()
//!     .with_device_name("Test iPhone")
//!     .with_file("HomeDomain", "Library/SMS/sms.db", b"bytes")
//!     .build(&tmp)
//!     .unwrap();
//! ```

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use plist::{Dictionary, Uid, Value};
use rusqlite::Connection;

use crate::error::{IoResultExt, Result};
use crate::util::paths;

/// What a fixture entry holds.
enum Payload {
    /// Literal bytes.
    Bytes(Vec<u8>),
    /// A SQLite database built by running `setup` against it.
    Db {
        setup: String,
        /// Statements left unchecked-pointed in a `-wal` sidecar.
        wal: Option<String>,
    },
    /// Listed in the manifest with this size, but no bytes written: the
    /// "attachment missing from the backup" case.
    Phantom { size: u64 },
}

struct FixtureEntry {
    domain: String,
    relative_path: String,
    payload: Payload,
}

/// Builder for a synthetic backup.
pub struct FixtureBackup {
    device_name: String,
    product_name: String,
    product_type: String,
    product_version: String,
    build_version: String,
    udid: String,
    serial_number: String,
    backup_date: DateTime<Utc>,
    encrypted: bool,
    installed_apps: Vec<String>,
    entries: Vec<FixtureEntry>,
}

impl Default for FixtureBackup {
    fn default() -> Self {
        Self::new()
    }
}

impl FixtureBackup {
    pub fn new() -> Self {
        Self {
            device_name: "Fixture iPhone".to_string(),
            product_name: "iPhone 15".to_string(),
            product_type: "iPhone15,4".to_string(),
            product_version: "18.1".to_string(),
            build_version: "22B83".to_string(),
            udid: "0123456789abcdef0123456789abcdef01234567".to_string(),
            serial_number: "FIXTURE00001".to_string(),
            // Fixed, so a fixture's backup id is reproducible.
            backup_date: Utc
                .timestamp_opt(1_793_491_200, 0)
                .single()
                .expect("1793491200 is a valid timestamp"),
            encrypted: false,
            installed_apps: Vec::new(),
            entries: Vec::new(),
        }
    }

    pub fn with_device_name(mut self, name: &str) -> Self {
        self.device_name = name.to_string();
        self
    }

    pub fn with_product_name(mut self, name: &str) -> Self {
        self.product_name = name.to_string();
        self
    }

    pub fn with_product_version(mut self, version: &str) -> Self {
        self.product_version = version.to_string();
        self
    }

    pub fn with_udid(mut self, udid: &str) -> Self {
        self.udid = udid.to_string();
        self
    }

    pub fn with_backup_date(mut self, date: DateTime<Utc>) -> Self {
        self.backup_date = date;
        self
    }

    pub fn with_installed_app(mut self, bundle_id: &str) -> Self {
        self.installed_apps.push(bundle_id.to_string());
        self
    }

    /// Mark the backup encrypted in `Manifest.plist`.
    ///
    /// The contents are *not* encrypted: this exercises the detection and the
    /// password-prompt path only. A fixture that really encrypts arrives with
    /// milestone M7.
    pub fn marked_encrypted(mut self) -> Self {
        self.encrypted = true;
        self
    }

    /// Add a file with literal contents.
    pub fn with_file(mut self, domain: &str, relative_path: &str, bytes: &[u8]) -> Self {
        self.entries.push(FixtureEntry {
            domain: domain.to_string(),
            relative_path: relative_path.trim_start_matches('/').to_string(),
            payload: Payload::Bytes(bytes.to_vec()),
        });
        self
    }

    /// Add a SQLite database, built by running `setup_sql` against it.
    pub fn with_db(mut self, domain: &str, relative_path: &str, setup_sql: &str) -> Self {
        self.entries.push(FixtureEntry {
            domain: domain.to_string(),
            relative_path: relative_path.trim_start_matches('/').to_string(),
            payload: Payload::Db {
                setup: setup_sql.to_string(),
                wal: None,
            },
        });
        self
    }

    /// Add a SQLite database whose most recent writes sit in a `-wal`
    /// sidecar, as they do in a real backup taken while apps were running.
    pub fn with_wal_db(
        mut self,
        domain: &str,
        relative_path: &str,
        setup_sql: &str,
        wal_sql: &str,
    ) -> Self {
        self.entries.push(FixtureEntry {
            domain: domain.to_string(),
            relative_path: relative_path.trim_start_matches('/').to_string(),
            payload: Payload::Db {
                setup: setup_sql.to_string(),
                wal: Some(wal_sql.to_string()),
            },
        });
        self
    }

    /// Add a manifest row whose bytes are deliberately absent.
    pub fn with_phantom_file(mut self, domain: &str, relative_path: &str, size: u64) -> Self {
        self.entries.push(FixtureEntry {
            domain: domain.to_string(),
            relative_path: relative_path.trim_start_matches('/').to_string(),
            payload: Payload::Phantom { size },
        });
        self
    }

    /// Write the backup into `parent`, returning the backup root — the folder
    /// named after the UDID, which is what the user picks in the app.
    pub fn build(self, parent: &Path) -> Result<PathBuf> {
        let root = parent.join(&self.udid);
        std::fs::create_dir_all(&root).at(&root)?;

        // Databases are built here first so their bytes can be copied to the
        // hashed paths. Kept outside the backup root, so it cannot show up in
        // listings or in "was the backup modified?" checks.
        let scratch = parent.join(".fixture-scratch");
        std::fs::create_dir_all(&scratch).at(&scratch)?;

        self.write_plists(&root)?;

        let conn = Connection::open(root.join("Manifest.db"))?;
        conn.execute_batch(
            "CREATE TABLE Files (
                 fileID       TEXT PRIMARY KEY,
                 domain       TEXT,
                 relativePath TEXT,
                 flags        INTEGER,
                 file         BLOB
             );
             CREATE TABLE Properties (key TEXT PRIMARY KEY, value BLOB);",
        )?;

        // Directories every entry implies, so listings have something to walk.
        let mut dirs: BTreeSet<(String, String)> = BTreeSet::new();
        for entry in &self.entries {
            let mut path = entry.relative_path.as_str();
            while let (Some(parent), _) = paths::split_relative(path) {
                dirs.insert((entry.domain.clone(), parent.to_string()));
                path = parent;
            }
        }
        for (domain, dir) in &dirs {
            insert_row(&conn, domain, dir, 2, 0, 0o40_755, None)?;
        }

        for entry in &self.entries {
            let (bytes, sidecars, size) = match &entry.payload {
                Payload::Bytes(bytes) => (Some(bytes.clone()), Vec::new(), bytes.len() as u64),
                Payload::Phantom { size } => (None, Vec::new(), *size),
                Payload::Db { setup, wal } => {
                    let (db, sidecars) = build_db(&scratch, entry, setup, wal.as_deref())?;
                    let size = db.len() as u64;
                    (Some(db), sidecars, size)
                }
            };

            if let Some(bytes) = &bytes {
                write_payload(&root, &entry.domain, &entry.relative_path, bytes)?;
            }
            insert_row(
                &conn,
                &entry.domain,
                &entry.relative_path,
                1,
                size,
                0o100_644,
                None,
            )?;

            // `-wal` / `-shm` are ordinary manifest entries of their own.
            for (suffix, bytes) in sidecars {
                let path = format!("{}{}", entry.relative_path, suffix);
                write_payload(&root, &entry.domain, &path, &bytes)?;
                insert_row(
                    &conn,
                    &entry.domain,
                    &path,
                    1,
                    bytes.len() as u64,
                    0o100_644,
                    None,
                )?;
            }
        }

        drop(conn);
        // Leave no trace next to the backup.
        let _ = std::fs::remove_dir_all(&scratch);
        Ok(root)
    }

    fn write_plists(&self, root: &Path) -> Result<()> {
        let date = Value::Date(plist::Date::from(std::time::SystemTime::from(
            self.backup_date,
        )));

        let mut info = Dictionary::new();
        info.insert("Device Name".into(), self.device_name.clone().into());
        info.insert("Display Name".into(), self.device_name.clone().into());
        info.insert("Product Name".into(), self.product_name.clone().into());
        info.insert("Product Type".into(), self.product_type.clone().into());
        info.insert(
            "Product Version".into(),
            self.product_version.clone().into(),
        );
        info.insert("Build Version".into(), self.build_version.clone().into());
        info.insert("Unique Identifier".into(), self.udid.clone().into());
        info.insert("Target Identifier".into(), self.udid.clone().into());
        info.insert("Serial Number".into(), self.serial_number.clone().into());
        info.insert("Last Backup Date".into(), date.clone());
        info.insert(
            "Installed Applications".into(),
            Value::Array(
                self.installed_apps
                    .iter()
                    .map(|a| Value::String(a.clone()))
                    .collect(),
            ),
        );
        write_plist(&root.join("Info.plist"), Value::Dictionary(info))?;

        let mut manifest = Dictionary::new();
        manifest.insert("IsEncrypted".into(), Value::Boolean(self.encrypted));
        manifest.insert("WasPasscodeSet".into(), Value::Boolean(true));
        manifest.insert("Version".into(), "10.0".into());
        manifest.insert("Date".into(), date.clone());
        if self.encrypted {
            // Shape only; M7 replaces this with a real keybag.
            manifest.insert("BackupKeyBag".into(), Value::Data(vec![0; 32]));
            manifest.insert("ManifestKey".into(), Value::Data(vec![0; 44]));
        }
        write_plist(&root.join("Manifest.plist"), Value::Dictionary(manifest))?;

        let mut status = Dictionary::new();
        status.insert("IsFullBackup".into(), Value::Boolean(true));
        status.insert("SnapshotState".into(), "finished".into());
        status.insert("Version".into(), "3.3".into());
        status.insert("Date".into(), date);
        status.insert("BackupState".into(), "new".into());
        write_plist(&root.join("Status.plist"), Value::Dictionary(status))?;

        Ok(())
    }
}

fn write_plist(path: &Path, value: Value) -> Result<()> {
    value
        .to_file_binary(path)
        .map_err(|source| crate::error::BackupError::Plist {
            file: path.display().to_string(),
            source,
        })?;
    Ok(())
}

/// Write bytes to the hashed physical path, creating the shard directory.
fn write_payload(root: &Path, domain: &str, relative_path: &str, bytes: &[u8]) -> Result<()> {
    let id = paths::file_id(domain, relative_path);
    let path = paths::physical_path(root, &id).expect("a SHA-1 hex id always has two characters");
    let dir = path
        .parent()
        .expect("physical paths have a shard directory");
    std::fs::create_dir_all(dir).at(dir)?;
    std::fs::write(&path, bytes).at(&path)
}

/// Insert one `Files` row with a realistic `MBFile` blob.
fn insert_row(
    conn: &Connection,
    domain: &str,
    relative_path: &str,
    flags: i64,
    size: u64,
    mode: u32,
    encryption_key: Option<Vec<u8>>,
) -> Result<()> {
    let blob = mbfile_blob(relative_path, size, mode, encryption_key)?;
    conn.execute(
        "INSERT OR REPLACE INTO Files (fileID, domain, relativePath, flags, file)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            paths::file_id(domain, relative_path),
            domain,
            relative_path,
            flags,
            blob
        ],
    )?;
    Ok(())
}

/// Build the binary `NSKeyedArchiver` plist that a real `Manifest.db` stores in
/// its `file` column, complete with UID indirection.
fn mbfile_blob(
    relative_path: &str,
    size: u64,
    mode: u32,
    encryption_key: Option<Vec<u8>>,
) -> Result<Vec<u8>> {
    // $objects[0] is always the "$null" placeholder.
    let mut objects = vec![Value::String("$null".into())];

    // Reserve index 1 for the MBFile itself; its fields reference later slots.
    objects.push(Value::String(String::new()));

    let path_uid = push(&mut objects, Value::String(relative_path.to_string()));
    let mbfile_class_uid = push(&mut objects, class_dict("MBFile", &["MBFile", "NSObject"]));
    let key_uid = encryption_key.map(|key| {
        let data_class = class_dict("NSMutableData", &["NSMutableData", "NSData", "NSObject"]);
        let class_uid = push(&mut objects, data_class);
        let mut wrapper = Dictionary::new();
        wrapper.insert("$class".into(), Value::Uid(Uid::new(class_uid)));
        wrapper.insert("NS.data".into(), Value::Data(key));
        push(&mut objects, Value::Dictionary(wrapper))
    });

    let mtime = 1_793_491_200_i64; // Unix seconds, as MBFile stores them.
    let mut mbfile = Dictionary::new();
    mbfile.insert("$class".into(), Value::Uid(Uid::new(mbfile_class_uid)));
    mbfile.insert("RelativePath".into(), Value::Uid(Uid::new(path_uid)));
    mbfile.insert(
        "Size".into(),
        Value::Integer(i64::try_from(size).unwrap_or(i64::MAX).into()),
    );
    mbfile.insert("Mode".into(), Value::Integer(i64::from(mode).into()));
    mbfile.insert("LastModified".into(), Value::Integer(mtime.into()));
    mbfile.insert("Birth".into(), Value::Integer(mtime.into()));
    mbfile.insert("LastStatusChange".into(), Value::Integer(mtime.into()));
    mbfile.insert("UserID".into(), Value::Integer(501.into()));
    mbfile.insert("GroupID".into(), Value::Integer(501.into()));
    mbfile.insert("InodeNumber".into(), Value::Integer(1_234_567.into()));
    mbfile.insert("ProtectionClass".into(), Value::Integer(3.into()));
    mbfile.insert("Flags".into(), Value::Integer(4.into()));
    if let Some(key_uid) = key_uid {
        mbfile.insert("EncryptionKey".into(), Value::Uid(Uid::new(key_uid)));
    }
    objects[1] = Value::Dictionary(mbfile);

    let mut top = Dictionary::new();
    top.insert("root".into(), Value::Uid(Uid::new(1)));

    let mut archive = Dictionary::new();
    archive.insert("$version".into(), Value::Integer(100_000.into()));
    archive.insert("$archiver".into(), "NSKeyedArchiver".into());
    archive.insert("$top".into(), Value::Dictionary(top));
    archive.insert("$objects".into(), Value::Array(objects));

    let mut buf = Vec::new();
    Value::Dictionary(archive)
        .to_writer_binary(&mut buf)
        .map_err(|source| crate::error::BackupError::Plist {
            file: "fixture MBFile".to_string(),
            source,
        })?;
    Ok(buf)
}

/// Append an object and return its UID index.
fn push(objects: &mut Vec<Value>, value: Value) -> u64 {
    objects.push(value);
    (objects.len() - 1) as u64
}

fn class_dict(name: &str, hierarchy: &[&str]) -> Value {
    let mut dict = Dictionary::new();
    dict.insert("$classname".into(), name.into());
    dict.insert(
        "$classes".into(),
        Value::Array(
            hierarchy
                .iter()
                .map(|c| Value::String((*c).into()))
                .collect(),
        ),
    );
    Value::Dictionary(dict)
}

/// A `-wal` / `-shm` suffix and the bytes of that sidecar.
type Sidecar = (&'static str, Vec<u8>);

/// Build a SQLite database and return its bytes, plus any sidecar bytes.
fn build_db(
    scratch: &Path,
    entry: &FixtureEntry,
    setup: &str,
    wal: Option<&str>,
) -> Result<(Vec<u8>, Vec<Sidecar>)> {
    let name = paths::split_relative(&entry.relative_path).1;
    let dir = scratch.join(paths::file_id(&entry.domain, &entry.relative_path));
    std::fs::create_dir_all(&dir).at(&dir)?;
    let db_path = dir.join(name);

    let conn = Connection::open(&db_path)?;
    if wal.is_some() {
        conn.pragma_update(None, "journal_mode", "wal")?;
        // Keep the writes below in the WAL instead of folding them in.
        conn.pragma_update(None, "wal_autocheckpoint", 0)?;
    }
    conn.execute_batch(setup)?;

    let mut sidecars = Vec::new();
    match wal {
        None => drop(conn),
        Some(wal_sql) => {
            conn.execute_batch(wal_sql)?;
            // Dropping the last connection checkpoints the WAL, which would
            // defeat the purpose, so the handle is leaked for the life of the
            // build instead.
            std::mem::forget(conn);
            for suffix in ["-wal", "-shm"] {
                let path = dir.join(format!("{name}{suffix}"));
                if path.exists() {
                    sidecars.push((suffix, std::fs::read(&path).at(&path)?));
                }
            }
        }
    }

    let bytes = std::fs::read(&db_path).at(&db_path)?;
    Ok((bytes, sidecars))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::Backup;
    use crate::cache::CacheDir;
    use crate::page::Page;

    /// Open a fixture with an isolated cache; see the note in `backup::tests`.
    fn open(root: &Path) -> Backup {
        let cache = root
            .parent()
            .expect("a fixture root always has a parent")
            .join("cache");
        Backup::open_with_cache(root, None, CacheDir::at(cache)).unwrap()
    }

    #[test]
    fn the_fixture_writes_a_real_backup_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = FixtureBackup::new()
            .with_file("HomeDomain", "Library/SMS/sms.db", b"hello")
            .build(tmp.path())
            .unwrap();

        assert!(root.join("Manifest.db").is_file());
        assert!(root.join("Info.plist").is_file());
        assert!(root.join("Manifest.plist").is_file());
        assert!(root.join("Status.plist").is_file());
        // Payload at its sharded, hashed path.
        assert!(
            root.join("3d/3d0d7e5fb2ce288813306e4d4636395e047a3d28")
                .is_file()
        );
        // The scratch area is gone and was never inside the backup.
        assert!(!tmp.path().join(".fixture-scratch").exists());
    }

    #[test]
    fn metadata_blobs_are_binary_archives_with_uid_indirection() {
        let tmp = tempfile::tempdir().unwrap();
        let root = FixtureBackup::new()
            .with_file("HomeDomain", "Library/SMS/sms.db", b"hello")
            .build(tmp.path())
            .unwrap();

        let conn = Connection::open(root.join("Manifest.db")).unwrap();
        let blob: Vec<u8> = conn
            .query_one(
                "SELECT file FROM Files WHERE relativePath = 'Library/SMS/sms.db'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // A binary plist, as iOS writes it.
        assert!(blob.starts_with(b"bplist"), "expected a binary plist");

        // Unresolved, RelativePath is a UID rather than the string.
        let raw = Value::from_reader(std::io::Cursor::new(&blob)).unwrap();
        let objects = raw
            .as_dictionary()
            .unwrap()
            .get("$objects")
            .unwrap()
            .as_array()
            .unwrap();
        let mbfile = objects[1].as_dictionary().unwrap();
        assert!(
            matches!(mbfile.get("RelativePath"), Some(Value::Uid(_))),
            "the fixture must exercise UID indirection, not inline the value"
        );

        // Resolved, it reads as a plain value.
        let resolved = crate::util::nskeyedarchiver::resolve_bytes(&blob).unwrap();
        let dict = resolved.as_dictionary().unwrap();
        assert_eq!(
            dict.get("RelativePath").unwrap().as_string(),
            Some("Library/SMS/sms.db")
        );
        assert_eq!(dict.get("$class").unwrap().as_string(), Some("MBFile"));
        assert_eq!(dict.get("Size").unwrap().as_signed_integer(), Some(5));
    }

    #[test]
    fn directories_are_synthesized_for_every_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = FixtureBackup::new()
            .with_file("HomeDomain", "a/b/c/file.txt", b"x")
            .build(tmp.path())
            .unwrap();
        let backup = open(&root);

        for dir in ["a", "a/b", "a/b/c"] {
            let entry = backup
                .find("HomeDomain", dir)
                .unwrap()
                .unwrap_or_else(|| panic!("{dir} should be listed"));
            assert!(entry.kind.is_dir(), "{dir} should be a directory");
        }
    }

    #[test]
    fn a_wal_database_keeps_its_recent_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = FixtureBackup::new()
            .with_wal_db(
                "HomeDomain",
                "Library/SMS/sms.db",
                "CREATE TABLE message (ROWID INTEGER PRIMARY KEY, n INT);
                 INSERT INTO message (n) VALUES (1);",
                "INSERT INTO message (n) VALUES (2), (3);",
            )
            .build(tmp.path())
            .unwrap();
        let backup = open(&root);

        // The sidecars are manifest entries in their own right.
        assert!(
            backup
                .find("HomeDomain", "Library/SMS/sms.db-wal")
                .unwrap()
                .is_some(),
            "the -wal sibling should be in the manifest"
        );

        let conn = backup.open_db("HomeDomain", "Library/SMS/sms.db").unwrap();
        let n: i64 = conn
            .query_one("SELECT count(*) FROM message", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3, "rows held in the -wal must be visible");

        // And the listing shows the database next to its sidecars.
        let listing = backup
            .list_dir("HomeDomain", "Library/SMS", Page::default())
            .unwrap();
        let names: Vec<_> = listing.items.iter().map(|e| e.name()).collect();
        assert!(names.contains(&"sms.db"));
        assert!(names.contains(&"sms.db-wal"));
    }

    #[test]
    fn fixtures_are_reproducible() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let build = |dir: &Path| {
            FixtureBackup::new()
                .with_file("HomeDomain", "a.txt", b"x")
                .build(dir)
                .unwrap()
        };
        let root_a = build(a.path());
        let root_b = build(b.path());

        // Same bytes for the metadata, so tests cannot depend on build time.
        assert_eq!(
            std::fs::read(root_a.join("Info.plist")).unwrap(),
            std::fs::read(root_b.join("Info.plist")).unwrap()
        );
        // The cache id depends on the path, so those differ by design.
        let id_a = open(&root_a).id().to_string();
        let id_b = open(&root_b).id().to_string();
        assert_ne!(id_a, id_b);
    }
}
