//! Wire types that exist only at the IPC boundary.
//!
//! Types ibex-core already defines (`ManifestEntry`, `FileMeta`, `EntryKind`,
//! `DomainSummary`, `BackupInfo`, `Page`, `Paged`) cross IPC as themselves —
//! they already derive `Serialize` and `TS`. Only genuinely new shapes live
//! here.

use ibex_core::backup::BackupInfo;
use ibex_core::util::mime::{PreviewKind, Sniffed};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Returned by `open_backup`: the handle plus everything the shell needs to
/// render the top bar and sidebar.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BackupSummary {
    /// Opaque id for every later call.
    pub handle: String,
    /// The folder that was opened.
    pub path: String,
    pub info: BackupInfo,
    pub encrypted: bool,
    /// Which sidebar sections have data behind them.
    pub sources: Vec<SourceAvailability>,
}

/// Whether one data source is present in this backup.
///
/// M1 only detects whether the underlying database exists; the extractors that
/// read them arrive in M2 onwards. `available: false` greys the sidebar entry
/// and `reason` becomes its tooltip.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceAvailability {
    /// Stable id: `sms`, `whatsapp`, `photos`, `voice`, `documents`.
    pub id: String,
    pub label: String,
    pub available: bool,
    /// Why it is unavailable, or which milestone implements it.
    pub reason: Option<String>,
    /// True once a milestone has shipped the view for it.
    pub implemented: bool,
}

/// One entry in the Welcome screen's recent list.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecentBackup {
    pub path: String,
    pub device_name: Option<String>,
    pub product_name: Option<String>,
    pub product_version: Option<String>,
    /// RFC 3339, from the backup's own metadata.
    pub backup_date: Option<String>,
    pub encrypted: bool,
    /// Size recorded when it was last opened, so the list needs no disk walk.
    pub total_bytes: Option<u64>,
    /// RFC 3339, when ibex last opened it.
    pub last_opened: String,
    /// False when the folder has since been moved or deleted.
    pub exists: bool,
}

/// Everything the detail pane shows about one file.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileInfo {
    pub file_id: String,
    pub domain: String,
    pub relative_path: String,
    pub name: String,
    pub kind: ibex_core::EntryKind,
    pub meta: Option<ibex_core::FileMeta>,
    /// Whether the bytes are actually in the backup. The manifest can list a
    /// file whose data was never captured.
    pub data_present: bool,
    /// Real size on disk, which can differ from the manifest's record.
    pub data_bytes: Option<u64>,
    /// Sniffed type and how to show it. `None` when the data is absent.
    pub sniffed: Option<Sniffed>,
    /// Path inside the backup folder, for "Open containing folder".
    pub physical_path: Option<String>,
}

/// A bounded text preview.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TextPreview {
    pub text: String,
    /// True when the file is longer than what was read.
    pub truncated: bool,
    /// Bytes read for this preview.
    pub bytes_read: u64,
    /// Total size of the file.
    pub total_bytes: u64,
    /// How the text was produced, so the UI can label it.
    pub kind: PreviewKind,
    /// True when a plist was successfully rendered as XML. False means the
    /// bytes are being shown as-is because the plist would not parse.
    pub plist_rendered: bool,
    /// True when the bytes were not valid UTF-8 and were shown lossily.
    pub lossy: bool,
}

/// A window of bytes for the hex view.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HexPreview {
    pub offset: u64,
    /// The bytes themselves, as a lowercase hex string. Rendering the rows and
    /// the ASCII gutter is the frontend's job.
    pub hex: String,
    pub len: u64,
    pub total_bytes: u64,
}

/// Overview numbers.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BackupStats {
    /// Real size of the backup folder on disk.
    pub total_bytes: u64,
    /// Rows in the manifest, directories included.
    pub file_count: u64,
    pub domain_count: u64,
    /// Size of this backup's cache directory.
    pub cache_bytes: u64,
}

/// Result of "Save as…".
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SaveReport {
    pub path: String,
    pub bytes: u64,
    /// False when the manifest recorded no mtime, or the filesystem refused it.
    pub mtime_restored: bool,
}
