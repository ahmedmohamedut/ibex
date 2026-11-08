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

mod encryption;
pub mod sms;
pub mod whatsapp;

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
    /// Set by `encrypted_with_password`: the fixture's contents really are
    /// encrypted with keys derived from this password.
    password: Option<String>,
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
            password: None,
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

    /// Mark the backup encrypted in `Manifest.plist` **without** encrypting
    /// anything.
    ///
    /// Only useful for exercising detection and the password prompt. For a
    /// backup that really is encrypted, use [`Self::encrypted_with_password`].
    pub fn marked_encrypted(mut self) -> Self {
        self.encrypted = true;
        self.password = None;
        self
    }

    /// Really encrypt the fixture, the way a device does.
    ///
    /// Builds a genuine keybag, wraps each class key with a key derived from
    /// `password`, and encrypts `Manifest.db` and every payload with
    /// AES-256-CBC. This is the wrap-and-encrypt side of the scheme, written
    /// for tests, and it is what makes the decrypt side trustworthy.
    ///
    /// The keybag uses low iteration counts so the suite stays fast; a real
    /// backup's ten million rounds are only exercised by hand.
    pub fn encrypted_with_password(mut self, password: &str) -> Self {
        self.encrypted = true;
        self.password = Some(password.to_string());
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

    /// Add one of each sample file, for exercising the previews.
    ///
    /// Lands in `HomeDomain` under `Library/IbexSamples/`.
    pub fn with_sample_media(self) -> Self {
        const DIR: &str = "Library/IbexSamples";
        self.with_file("HomeDomain", &format!("{DIR}/tone.wav"), &samples::wav(5))
            .with_file("HomeDomain", &format!("{DIR}/pixel.png"), &samples::png())
            .with_file(
                "HomeDomain",
                &format!("{DIR}/preferences.plist"),
                &samples::binary_plist(),
            )
            .with_file("HomeDomain", &format!("{DIR}/notes.txt"), &samples::text())
            .with_file("HomeDomain", &format!("{DIR}/data.json"), &samples::json())
            // No extension and no magic bytes: exercises the hex view.
            .with_file(
                "HomeDomain",
                &format!("{DIR}/opaque"),
                &[0u8, 1, 2, 255, 254, 253, 0, 7],
            )
    }

    /// Add a realistic `sms.db` plus the attachment that goes with it.
    ///
    /// See [`sms`] for what the content covers. The attachment referenced as
    /// present is written into `MediaDomain`; the second one is deliberately
    /// left out, so the "not in backup" path is exercised.
    pub fn with_sms(self) -> Self {
        self.with_db("HomeDomain", "Library/SMS/sms.db", &sms::setup_sql())
            .with_file("MediaDomain", sms::PRESENT_ATTACHMENT, &samples::png())
    }

    /// Add a realistic WhatsApp container: `ChatStorage.sqlite`,
    /// `ContactsV2.sqlite` and the media those chats reference.
    ///
    /// One media path is deliberately left out of the backup, so the
    /// "not in backup" path is exercised.
    pub fn with_whatsapp(self) -> Self {
        use whatsapp as wa;
        const DOMAIN: &str = "AppDomainGroup-group.net.whatsapp.WhatsApp.shared";
        self.with_db(DOMAIN, "ChatStorage.sqlite", &wa::chat_storage_sql())
            .with_db(DOMAIN, "ContactsV2.sqlite", wa::contacts_sql())
            .with_file(DOMAIN, &wa::stored_path(wa::IMAGE_PATH), &samples::png())
            .with_file(DOMAIN, &wa::stored_path(wa::VOICE_PATH), &wa::ogg_opus())
            .with_file(
                DOMAIN,
                &wa::stored_path(wa::DOCUMENT_PATH),
                b"%PDF-1.4\n% fixture document\n",
            )
            .with_file(DOMAIN, &wa::stored_path(wa::PREVIEW_PATH), &samples::png())
        // MISSING_PATH is intentionally not written.
    }

    /// Add an `AddressBook.sqlitedb` with a few synthetic contacts.
    pub fn with_address_book(self) -> Self {
        self.with_db(
            "HomeDomain",
            "Library/AddressBook/AddressBook.sqlitedb",
            sms::address_book_sql(),
        )
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

        // Building into a directory that already holds a fixture replaces it,
        // so a builder call is idempotent and tests can rebuild in place.
        let manifest_db = root.join("Manifest.db");
        for suffix in ["", "-wal", "-shm"] {
            let path = root.join(format!("Manifest.db{suffix}"));
            if path.exists() {
                std::fs::remove_file(&path).at(&path)?;
            }
        }

        // An encrypted fixture needs the keybag before any row is written,
        // because each file's wrapped key goes into its own MBFile.
        let crypt = match &self.password {
            Some(password) => Some(encryption::Encryption::new(password)?),
            None => None,
        };

        // Build the manifest in the scratch area. For an encrypted fixture the
        // copy that lands in the backup folder is the ciphertext.
        let plain_manifest = if crypt.is_some() {
            scratch.join("Manifest.plain.db")
        } else {
            manifest_db.clone()
        };
        if plain_manifest.exists() {
            std::fs::remove_file(&plain_manifest).at(&plain_manifest)?;
        }

        let conn = Connection::open(&plain_manifest)?;
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
            // Directories hold no bytes, so they get no file key.
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

            // Each file gets its own key, wrapped under a class key, exactly
            // as iOS does. The plaintext length stays in the MBFile `Size`,
            // which is what the decrypt path truncates to.
            let key_blob = match (&crypt, &bytes) {
                (Some(crypt), Some(_)) => Some(crypt.wrap_new_file_key(&entry.relative_path)?),
                _ => None,
            };

            if let Some(bytes) = &bytes {
                let stored = match (&crypt, &key_blob) {
                    (Some(crypt), Some(blob)) => crypt.encrypt_for(blob, bytes)?,
                    _ => bytes.clone(),
                };
                write_payload(&root, &entry.domain, &entry.relative_path, &stored)?;
            }
            insert_row(
                &conn,
                &entry.domain,
                &entry.relative_path,
                1,
                size,
                0o100_644,
                key_blob.clone(),
            )?;

            // `-wal` / `-shm` are ordinary manifest entries of their own.
            for (suffix, bytes) in sidecars {
                let path = format!("{}{}", entry.relative_path, suffix);
                let sidecar_blob = match &crypt {
                    Some(crypt) => Some(crypt.wrap_new_file_key(&path)?),
                    None => None,
                };
                let stored = match (&crypt, &sidecar_blob) {
                    (Some(crypt), Some(blob)) => crypt.encrypt_for(blob, &bytes)?,
                    _ => bytes.clone(),
                };
                write_payload(&root, &entry.domain, &path, &stored)?;
                insert_row(
                    &conn,
                    &entry.domain,
                    &path,
                    1,
                    bytes.len() as u64,
                    0o100_644,
                    sidecar_blob,
                )?;
            }
        }

        drop(conn);

        // Encrypt the finished manifest into the backup folder.
        if let Some(crypt) = &crypt {
            let plaintext = std::fs::read(&plain_manifest).at(&plain_manifest)?;
            let ciphertext = crypt.encrypt_manifest(&plaintext)?;
            std::fs::write(&manifest_db, &ciphertext).at(&manifest_db)?;
        }

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
        if let Some(password) = &self.password {
            // A real keybag, with keys actually derived from the password.
            let crypt = encryption::Encryption::new(password)?;
            manifest.insert("BackupKeyBag".into(), Value::Data(crypt.keybag_blob()));
            manifest.insert("ManifestKey".into(), Value::Data(crypt.manifest_key_blob()));
        } else if self.encrypted {
            // Flagged encrypted but with nothing behind it, for testing the
            // detection path alone.
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

/// Generators for small, valid sample files.
///
/// Real media, synthesized rather than committed, so previews and the media
/// protocol can be exercised without shipping binaries in the repo or using
/// anyone's data.
pub mod samples {
    use plist::{Dictionary, Value};

    /// A playable mono 8-bit PCM WAV of `seconds` duration at 8 kHz, holding a
    /// quiet 440 Hz tone.
    ///
    /// Long enough to seek in, which is what makes it useful: scrubbing it in
    /// the UI is what proves the media protocol's Range support works.
    pub fn wav(seconds: u32) -> Vec<u8> {
        const RATE: u32 = 8_000;
        let samples = RATE * seconds;
        let data_len = samples; // one byte per sample
        let mut out = Vec::with_capacity(44 + data_len as usize);

        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes()); // chunk size
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // mono
        out.extend_from_slice(&RATE.to_le_bytes());
        out.extend_from_slice(&RATE.to_le_bytes()); // byte rate
        out.extend_from_slice(&1u16.to_le_bytes()); // block align
        out.extend_from_slice(&8u16.to_le_bytes()); // bits per sample
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());

        for i in 0..samples {
            // 440 Hz sine, scaled quietly around the 8-bit midpoint.
            let t = f64::from(i) / f64::from(RATE);
            let v = (t * 440.0 * std::f64::consts::TAU).sin() * 40.0;
            out.push((128.0 + v) as u8);
        }
        out
    }

    /// A 1x1 opaque PNG, built chunk by chunk with correct CRCs.
    pub fn png() -> Vec<u8> {
        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc = 0xffff_ffffu32;
            for &byte in bytes {
                crc ^= u32::from(byte);
                for _ in 0..8 {
                    let mask = (crc & 1).wrapping_neg();
                    crc = (crc >> 1) ^ (0xedb8_8320 & mask);
                }
            }
            !crc
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut crc_input = kind.to_vec();
            crc_input.extend_from_slice(data);
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
        }

        let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        // 1x1, 8-bit, greyscale
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        // One zlib-wrapped stored block: filter byte 0 plus one pixel.
        let raw = [0u8, 0x80];
        let mut idat = vec![0x78, 0x01, 0x01, 0x02, 0x00, 0xfd, 0xff];
        idat.extend_from_slice(&raw);
        let adler = {
            let mut a = 1u32;
            let mut b = 0u32;
            for &byte in &raw {
                a = (a + u32::from(byte)) % 65_521;
                b = (b + a) % 65_521;
            }
            (b << 16) | a
        };
        idat.extend_from_slice(&adler.to_be_bytes());
        chunk(&mut out, b"IDAT", &idat);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    /// A binary property list with a few typed values, as iOS writes them.
    pub fn binary_plist() -> Vec<u8> {
        let mut dict = Dictionary::new();
        dict.insert("Label".into(), Value::String("fixture preference".into()));
        dict.insert("Count".into(), Value::Integer(42.into()));
        dict.insert("Enabled".into(), Value::Boolean(true));
        dict.insert(
            "Items".into(),
            Value::Array(vec![
                Value::String("first".into()),
                Value::String("second".into()),
            ]),
        );
        let mut out = Vec::new();
        Value::Dictionary(dict)
            .to_writer_binary(&mut out)
            .expect("a dictionary of plain values always serializes");
        out
    }

    /// A few lines of plain text.
    pub fn text() -> Vec<u8> {
        b"ibex synthetic fixture\nline two\nline three, with a tab\there\n".to_vec()
    }

    /// A small JSON document.
    pub fn json() -> Vec<u8> {
        br#"{"source":"ibex fixture","values":[1,2,3],"nested":{"ok":true}}"#.to_vec()
    }
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
    fn the_sample_wav_is_a_valid_riff_file() {
        let wav = samples::wav(2);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        // Declared RIFF size must match the real one.
        let declared = u32::from_le_bytes(wav[4..8].try_into().unwrap());
        assert_eq!(declared as usize, wav.len() - 8);
        // Declared data size must match the samples that follow.
        assert_eq!(&wav[36..40], b"data");
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap());
        assert_eq!(data_len as usize, wav.len() - 44);
        assert_eq!(data_len, 8_000 * 2, "8 kHz mono, one byte per sample");
        // And it must sniff as audio, which is what the preview switches on.
        let sniffed = crate::util::mime::sniff(&wav, "tone.wav");
        assert_eq!(sniffed.preview, crate::util::mime::PreviewKind::Audio);
    }

    #[test]
    fn the_sample_png_is_a_valid_png() {
        let png = samples::png();
        assert_eq!(
            &png[0..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
        );
        assert_eq!(&png[12..16], b"IHDR");
        assert!(png.ends_with(b"\x00\x00\x00\x00IEND\xae\x42\x60\x82"));

        // Walk the chunks and verify every CRC, so a malformed sample cannot
        // pass as a working image.
        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc = 0xffff_ffffu32;
            for &byte in bytes {
                crc ^= u32::from(byte);
                for _ in 0..8 {
                    let mask = (crc & 1).wrapping_neg();
                    crc = (crc >> 1) ^ (0xedb8_8320 & mask);
                }
            }
            !crc
        }
        let mut at = 8;
        let mut chunks = 0;
        while at + 12 <= png.len() {
            let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            let body = &png[at + 4..at + 8 + len];
            let stored = u32::from_be_bytes(png[at + 8 + len..at + 12 + len].try_into().unwrap());
            assert_eq!(crc32(body), stored, "bad CRC in chunk {chunks}");
            at += 12 + len;
            chunks += 1;
        }
        assert_eq!(chunks, 3, "IHDR, IDAT, IEND");
        assert_eq!(at, png.len());

        let sniffed = crate::util::mime::sniff(&png, "pixel.png");
        assert_eq!(sniffed.mime, "image/png");
    }

    #[test]
    fn the_sample_plist_round_trips_to_readable_xml() {
        let bytes = samples::binary_plist();
        assert!(bytes.starts_with(b"bplist"));
        let xml = crate::util::plist_text::to_xml(&bytes).unwrap();
        assert!(xml.contains("<key>Label</key>"));
        assert!(xml.contains("<integer>42</integer>"));
    }

    #[test]
    fn sample_media_lands_in_the_backup_and_sniffs_correctly() {
        use crate::util::mime::PreviewKind;

        let tmp = tempfile::tempdir().unwrap();
        let root = FixtureBackup::new()
            .with_sample_media()
            .build(tmp.path())
            .unwrap();
        let backup = open(&root);

        let expected = [
            ("tone.wav", PreviewKind::Audio),
            ("pixel.png", PreviewKind::Image),
            ("preferences.plist", PreviewKind::Plist),
            ("notes.txt", PreviewKind::Text),
            ("data.json", PreviewKind::Text),
            ("opaque", PreviewKind::Binary),
        ];
        for (name, preview) in expected {
            let entry = backup
                .find("HomeDomain", &format!("Library/IbexSamples/{name}"))
                .unwrap()
                .unwrap_or_else(|| panic!("{name} should be in the fixture"));
            assert_eq!(
                backup.sniff(&entry).unwrap().preview,
                preview,
                "{name} sniffed wrongly"
            );
        }
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
