//! Device and backup metadata, read from `Info.plist`, `Manifest.plist` and
//! `Status.plist`.
//!
//! All three are optional in practice except `Manifest.plist`, which tells us
//! whether the backup is encrypted. Missing or unexpected keys degrade to
//! `None` rather than failing the open — a backup we can partly describe is far
//! more useful than an error. See `docs/DATA_SOURCES.md`.

use std::path::Path;

use chrono::{DateTime, Utc};
use plist::Value;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{BackupError, Result};

/// Everything we know about a backup before reading its manifest.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BackupInfo {
    /// The name the user gave the device.
    pub device_name: Option<String>,
    /// Marketing name, e.g. "iPhone 15 Pro".
    pub product_name: Option<String>,
    /// Model identifier, e.g. "iPhone16,1".
    pub product_type: Option<String>,
    /// iOS version, e.g. "18.1".
    pub product_version: Option<String>,
    pub build_version: Option<String>,
    /// The device UDID. Identifying: mask it before putting it in a log.
    pub udid: Option<String>,
    /// Identifying, like the UDID.
    pub serial_number: Option<String>,
    pub phone_number: Option<String>,
    pub last_backup_date: Option<DateTime<Utc>>,
    /// Bundle identifiers of the apps installed at backup time.
    pub installed_apps: Vec<String>,
    /// From `Manifest.plist`. Drives the password prompt.
    pub encrypted: bool,
    /// Whether a passcode was set on the device.
    pub passcode_set: Option<bool>,
    pub manifest_version: Option<String>,
    /// From `Status.plist`.
    pub is_full_backup: Option<bool>,
    pub snapshot_state: Option<String>,
    pub backup_date: Option<DateTime<Utc>>,
}

impl BackupInfo {
    /// Read the three metadata plists from a backup folder.
    pub fn read(root: &Path) -> Result<Self> {
        // Manifest.plist is the one file we insist on besides Manifest.db: it
        // carries the encryption flag, and guessing that would be unsafe.
        let manifest_plist =
            read_plist(root, "Manifest.plist")?.ok_or(BackupError::MissingMetadataFile {
                file: "Manifest.plist",
            })?;
        let info_plist = read_plist(root, "Info.plist")?;
        let status_plist = read_plist(root, "Status.plist")?;

        let mut info = BackupInfo {
            encrypted: dict_bool(&manifest_plist, "IsEncrypted").unwrap_or(false),
            passcode_set: dict_bool(&manifest_plist, "WasPasscodeSet"),
            manifest_version: dict_string(&manifest_plist, "Version"),
            ..Default::default()
        };

        if let Some(plist) = &info_plist {
            info.device_name = dict_string(plist, "Device Name");
            info.product_name = dict_string(plist, "Product Name");
            info.product_type = dict_string(plist, "Product Type");
            info.product_version = dict_string(plist, "Product Version");
            info.build_version = dict_string(plist, "Build Version");
            info.udid = dict_string(plist, "Unique Identifier")
                .or_else(|| dict_string(plist, "Target Identifier"));
            info.serial_number = dict_string(plist, "Serial Number");
            info.phone_number = dict_string(plist, "Phone Number");
            info.last_backup_date = dict_date(plist, "Last Backup Date");
            info.installed_apps = string_array(plist, "Installed Applications");
        }

        if let Some(plist) = &status_plist {
            info.is_full_backup = dict_bool(plist, "IsFullBackup");
            info.snapshot_state = dict_string(plist, "SnapshotState");
            info.backup_date = dict_date(plist, "Date");
        }

        Ok(info)
    }

    /// Short description for the top bar: "iPhone 15 Pro · iOS 18.1".
    pub fn display_line(&self) -> String {
        let device = self
            .product_name
            .as_deref()
            .or(self.device_name.as_deref())
            .or(self.product_type.as_deref())
            .unwrap_or("Unknown device");
        match &self.product_version {
            Some(v) => format!("{device} · iOS {v}"),
            None => device.to_string(),
        }
    }

    /// The most trustworthy date we have for this backup.
    pub fn date(&self) -> Option<DateTime<Utc>> {
        self.last_backup_date.or(self.backup_date)
    }

    /// A stable string for the cache id, even when no date is recorded.
    pub(crate) fn date_key(&self) -> String {
        self.date().map(|d| d.to_rfc3339()).unwrap_or_default()
    }

    /// A stable string for the cache id, even when no UDID is recorded.
    pub(crate) fn udid_key(&self) -> &str {
        self.udid.as_deref().unwrap_or_default()
    }
}

/// The key material from `Manifest.plist`, for an encrypted backup.
///
/// Kept apart from [`BackupInfo`] so it never crosses IPC: `BackupInfo` derives
/// `Serialize` and `TS`, this does not.
#[derive(Clone)]
pub struct EncryptionBlobs {
    /// The TLV keybag holding every wrapped class key.
    pub keybag: Vec<u8>,
    /// 4-byte protection class plus the wrapped key for `Manifest.db`.
    pub manifest_key: Vec<u8>,
}

impl std::fmt::Debug for EncryptionBlobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Lengths only: these are key material.
        f.debug_struct("EncryptionBlobs")
            .field(
                "keybag",
                &format_args!("{} bytes, redacted", self.keybag.len()),
            )
            .field(
                "manifest_key",
                &format_args!("{} bytes, redacted", self.manifest_key.len()),
            )
            .finish()
    }
}

/// Read the keybag and manifest key out of `Manifest.plist`.
///
/// `Ok(None)` when the backup is not encrypted.
pub fn read_encryption_blobs(root: &Path) -> Result<Option<EncryptionBlobs>> {
    let Some(plist) = read_plist(root, "Manifest.plist")? else {
        return Err(BackupError::MissingMetadataFile {
            file: "Manifest.plist",
        }
        .into());
    };
    if !dict_bool(&plist, "IsEncrypted").unwrap_or(false) {
        return Ok(None);
    }
    let keybag = dict_get(&plist, "BackupKeyBag")
        .and_then(Value::as_data)
        .map(<[u8]>::to_vec)
        .ok_or(crate::error::CryptoError::MissingKeybag)?;
    let manifest_key = dict_get(&plist, "ManifestKey")
        .and_then(Value::as_data)
        .map(<[u8]>::to_vec)
        .ok_or(crate::error::CryptoError::MissingKeybag)?;
    Ok(Some(EncryptionBlobs {
        keybag,
        manifest_key,
    }))
}

/// Read one plist from the backup folder. `Ok(None)` when it isn't there.
fn read_plist(root: &Path, name: &str) -> Result<Option<Value>> {
    let path = root.join(name);
    if !path.exists() {
        return Ok(None);
    }
    match Value::from_file(&path) {
        Ok(value) => Ok(Some(value)),
        Err(source) => Err(BackupError::Plist {
            file: name.to_string(),
            source,
        }
        .into()),
    }
}

fn dict_get<'a>(plist: &'a Value, key: &str) -> Option<&'a Value> {
    plist.as_dictionary()?.get(key)
}

fn dict_string(plist: &Value, key: &str) -> Option<String> {
    let value = dict_get(plist, key)?;
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        // Some writers store version numbers as integers.
        Value::Integer(i) => Some(i.to_string()),
        Value::Real(r) => Some(r.to_string()),
        _ => None,
    }
}

fn dict_bool(plist: &Value, key: &str) -> Option<bool> {
    let value = dict_get(plist, key)?;
    value.as_boolean().or_else(|| match value {
        // iTunes has been known to write 0/1 and "true"/"false" here.
        Value::Integer(i) => i.as_signed().map(|i| i != 0),
        Value::String(s) => match s.to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    })
}

fn dict_date(plist: &Value, key: &str) -> Option<DateTime<Utc>> {
    let value = dict_get(plist, key)?;
    let date = value.as_date()?;
    Some(DateTime::<Utc>::from(std::time::SystemTime::from(date)))
}

fn string_array(plist: &Value, key: &str) -> Vec<String> {
    dict_get(plist, key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_string)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_plist(path: &Path, value: &Value) {
        value.to_file_xml(path).unwrap();
    }

    fn dict(pairs: Vec<(&str, Value)>) -> Value {
        let mut d = plist::Dictionary::new();
        for (k, v) in pairs {
            d.insert(k.to_string(), v);
        }
        Value::Dictionary(d)
    }

    #[test]
    fn reads_all_three_plists() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        write_plist(
            &root.join("Info.plist"),
            &dict(vec![
                ("Device Name", Value::String("Test iPhone".into())),
                ("Product Name", Value::String("iPhone 15 Pro".into())),
                ("Product Type", Value::String("iPhone16,1".into())),
                ("Product Version", Value::String("18.1".into())),
                ("Build Version", Value::String("22B83".into())),
                (
                    "Unique Identifier",
                    Value::String("ABCD1234EFAB5678".into()),
                ),
                ("Serial Number", Value::String("F2LX00000001".into())),
                (
                    "Installed Applications",
                    Value::Array(vec![
                        Value::String("net.whatsapp.WhatsApp".into()),
                        Value::String("com.example.app".into()),
                    ]),
                ),
            ]),
        );
        write_plist(
            &root.join("Manifest.plist"),
            &dict(vec![
                ("IsEncrypted", Value::Boolean(false)),
                ("WasPasscodeSet", Value::Boolean(true)),
                ("Version", Value::String("10.0".into())),
            ]),
        );
        write_plist(
            &root.join("Status.plist"),
            &dict(vec![
                ("IsFullBackup", Value::Boolean(true)),
                ("SnapshotState", Value::String("finished".into())),
            ]),
        );

        let info = BackupInfo::read(root).unwrap();
        assert_eq!(info.device_name.as_deref(), Some("Test iPhone"));
        assert_eq!(info.product_version.as_deref(), Some("18.1"));
        assert_eq!(info.display_line(), "iPhone 15 Pro · iOS 18.1");
        assert!(!info.encrypted);
        assert_eq!(info.passcode_set, Some(true));
        assert_eq!(info.is_full_backup, Some(true));
        assert_eq!(info.installed_apps.len(), 2);
    }

    #[test]
    fn manifest_plist_is_required() {
        let tmp = tempfile::tempdir().unwrap();
        let err = BackupInfo::read(tmp.path()).unwrap_err();
        assert_eq!(err.code(), "missing_metadata");
    }

    #[test]
    fn info_and_status_are_optional() {
        let tmp = tempfile::tempdir().unwrap();
        write_plist(
            &tmp.path().join("Manifest.plist"),
            &dict(vec![("IsEncrypted", Value::Boolean(true))]),
        );

        let info = BackupInfo::read(tmp.path()).unwrap();
        assert!(info.encrypted);
        assert_eq!(info.device_name, None);
        assert_eq!(info.display_line(), "Unknown device");
        assert!(info.installed_apps.is_empty());
    }

    #[test]
    fn tolerates_unexpected_value_types() {
        let tmp = tempfile::tempdir().unwrap();
        write_plist(
            &tmp.path().join("Manifest.plist"),
            // 1 instead of true, an integer version, and a dictionary where a
            // string belongs.
            &dict(vec![
                ("IsEncrypted", Value::Integer(1.into())),
                ("Version", Value::Real(10.0)),
            ]),
        );
        write_plist(
            &tmp.path().join("Info.plist"),
            &dict(vec![
                ("Device Name", dict(vec![("nope", Value::Boolean(true))])),
                ("Product Version", Value::Integer(18.into())),
                ("Installed Applications", Value::String("not a list".into())),
            ]),
        );

        let info = BackupInfo::read(tmp.path()).unwrap();
        assert!(info.encrypted, "1 should read as true");
        assert_eq!(info.manifest_version.as_deref(), Some("10"));
        assert_eq!(info.device_name, None, "a dictionary is not a name");
        assert_eq!(info.product_version.as_deref(), Some("18"));
        assert!(info.installed_apps.is_empty());
    }

    #[test]
    fn a_corrupt_plist_is_an_error_not_a_panic() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Manifest.plist"), b"not a plist at all").unwrap();
        let err = BackupInfo::read(tmp.path()).unwrap_err();
        assert_eq!(err.code(), "bad_plist");
    }

    #[test]
    fn dates_convert_to_utc() {
        let tmp = tempfile::tempdir().unwrap();
        write_plist(
            &tmp.path().join("Manifest.plist"),
            &dict(vec![("IsEncrypted", Value::Boolean(false))]),
        );
        let when = DateTime::parse_from_rfc3339("2026-11-01T12:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        write_plist(
            &tmp.path().join("Info.plist"),
            &dict(vec![(
                "Last Backup Date",
                Value::Date(plist::Date::from(std::time::SystemTime::from(when))),
            )]),
        );

        let info = BackupInfo::read(tmp.path()).unwrap();
        assert_eq!(info.last_backup_date, Some(when));
        assert_eq!(info.date(), Some(when));
    }
}
