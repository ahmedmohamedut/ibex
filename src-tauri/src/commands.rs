//! The IPC commands. See the contract table in `docs/ARCHITECTURE.md`.
//!
//! Every command is `async` and pushes its blocking work — SQLite, file I/O —
//! onto `spawn_blocking`, so the UI thread never stalls. Each one is a thin
//! wrapper: the logic lives in ibex-core.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ibex_core::export::{self, ExportFormat, ExportOptions, Selection};
use ibex_core::extractors::photos::{Photos, PhotosDb};
use ibex_core::extractors::sms::{Sms, SmsDb};
use ibex_core::extractors::voice::{VoiceMemos, VoiceMemosDb};
use ibex_core::extractors::whatsapp::{WhatsApp, WhatsAppDb};
use ibex_core::extractors::{
    ConversationSummary, DocSource, Extractor, MessageCursor, MessagePage, PhotoAsset, PhotoCursor,
    PhotoFilter, PhotoPage, VoiceRecording,
};
use ibex_core::util::{mime, plist_text};
use ibex_core::{Backup, CacheDir, DomainSummary, ManifestEntry, Page, Paged};
use tauri::State;

use crate::dto::{
    BackupPeek, BackupStats, BackupSummary, FileInfo, HexPreview, RecentBackup, SaveReport,
    SourceAvailability, TextPreview,
};
use crate::error::IpcError;
use crate::jobs::{JobFailedEvent, JobFinishedEvent, JobInfo, ProgressThrottle};
use crate::recents;
use crate::state::AppState;

/// Run blocking work off the UI thread, turning a panic into a typed error.
async fn blocking<T, F>(f: F) -> Result<T, IpcError>
where
    F: FnOnce() -> Result<T, IpcError> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|_| IpcError::task_failed())?
}

/// Look up an entry by file ID, or fail with a typed error.
fn entry_of(backup: &Backup, file_id: &str) -> Result<ManifestEntry, IpcError> {
    backup.get(file_id)?.ok_or_else(IpcError::unknown_file)
}

/// Which data sources this backup contains.
///
/// M1 only probes for the databases; the extractors land in M2 onwards, so
/// `implemented` is false throughout and the sidebar greys them out.
fn detect_sources(backup: &Backup) -> Vec<SourceAvailability> {
    // (id, label, domain, path, milestone)
    const PROBES: &[(&str, &str, &str, &str, &str)] = &[
        ("sms", "Messages", "HomeDomain", "Library/SMS/sms.db", "M2"),
        (
            "whatsapp",
            "WhatsApp",
            "AppDomainGroup-group.net.whatsapp.WhatsApp.shared",
            "ChatStorage.sqlite",
            "M3",
        ),
        (
            "photos",
            "Photos",
            "CameraRollDomain",
            "Media/PhotoData/Photos.sqlite",
            "M5",
        ),
        (
            "voice",
            "Voice Memos",
            "AppDomainGroup-group.com.apple.VoiceMemos.shared",
            "Recordings/CloudRecordings.db",
            "M6",
        ),
        (
            "documents",
            "Files",
            "AppDomainGroup-group.com.apple.FileProvider.LocalStorage",
            "File Provider Storage",
            "M6",
        ),
    ];

    PROBES
        .iter()
        .map(|(id, label, domain, path, milestone)| {
            let present = backup
                .find(domain, path)
                .ok()
                .flatten()
                .is_some_and(|entry| entry.kind.is_dir() || backup.has_data(&entry));
            // Only SMS has a viewer so far; the rest stay greyed out with a
            // tooltip saying which milestone brings them.
            let implemented = *id == Sms::ID
                || *id == WhatsApp::ID
                || *id == Photos::ID
                || *id == VoiceMemos::ID
                || *id == "documents";
            SourceAvailability {
                id: (*id).to_string(),
                label: (*label).to_string(),
                available: present,
                reason: Some(match (present, implemented) {
                    (true, true) => "Found in this backup".to_string(),
                    (true, false) => {
                        format!("Found in this backup; the viewer arrives in milestone {milestone}")
                    }
                    (false, _) => {
                        format!("Not in this backup, or the app was not installed ({milestone})")
                    }
                }),
                implemented,
            }
        })
        .collect()
}

/// Read a backup's metadata without opening it.
///
/// The password modal uses this to name the device being unlocked: `Info.plist`
/// is readable even when everything else in the backup is encrypted.
#[tauri::command]
pub async fn peek_backup(path: String) -> Result<BackupPeek, IpcError> {
    blocking(move || {
        let root = PathBuf::from(&path);
        let info = Backup::peek(&root)?;
        Ok(BackupPeek {
            path: root.display().to_string(),
            encrypted: info.encrypted,
            info,
        })
    })
    .await
}

#[tauri::command]
pub async fn open_backup(
    state: State<'_, AppState>,
    path: String,
    password: Option<String>,
) -> Result<BackupSummary, IpcError> {
    let root = PathBuf::from(&path);
    let data_root = state.data_root().map(Path::to_path_buf);

    // Opening reads the plists and copies Manifest.db into the cache, and for
    // an encrypted backup also derives the keys — about a second of deliberate
    // work. All of it belongs off the UI thread.
    let opened = blocking(move || {
        let backup = match &data_root {
            // A data-root override has to be turned into this backup's own
            // cache directory, which needs its id.
            Some(dir) => {
                let id = Backup::cache_id(&root)?;
                let cache = CacheDir::at(dir.join("cache").join(id));
                Backup::open_with_cache(&root, password.as_deref(), cache)?
            }
            None => Backup::open(&root, password.as_deref())?,
        };
        let total_bytes = backup.total_bytes().ok();
        let sources = detect_sources(&backup);
        // Best effort: a failure to record a recent must not fail the open.
        let _ = recents::record(
            backup.root(),
            backup.info(),
            total_bytes,
            data_root.as_deref(),
        );
        Ok((backup, sources))
    })
    .await?;

    let (backup, sources) = opened;
    let summary = BackupSummary {
        handle: String::new(), // filled in below, once registered
        path: backup.root().display().to_string(),
        info: backup.info().clone(),
        encrypted: backup.is_encrypted(),
        sources,
    };
    let handle = state.insert(backup);
    Ok(BackupSummary { handle, ..summary })
}

#[tauri::command]
pub async fn close_backup(state: State<'_, AppState>, handle: String) -> Result<(), IpcError> {
    state.remove(&handle);
    Ok(())
}

#[tauri::command]
pub async fn recent_backups(state: State<'_, AppState>) -> Result<Vec<RecentBackup>, IpcError> {
    let data_root = state.data_root().map(Path::to_path_buf);
    blocking(move || Ok(recents::load(data_root.as_deref()))).await
}

#[tauri::command]
pub async fn forget_recent(state: State<'_, AppState>, path: String) -> Result<(), IpcError> {
    let data_root = state.data_root().map(Path::to_path_buf);
    blocking(move || recents::forget(&path, data_root.as_deref())).await
}

#[tauri::command]
pub async fn list_domains(
    state: State<'_, AppState>,
    handle: String,
) -> Result<Vec<DomainSummary>, IpcError> {
    let backup = state.get(&handle)?;
    blocking(move || Ok(backup.domains()?)).await
}

#[tauri::command]
pub async fn list_dir(
    state: State<'_, AppState>,
    handle: String,
    domain: String,
    path: String,
    page: Option<Page>,
) -> Result<Paged<ManifestEntry>, IpcError> {
    let backup = state.get(&handle)?;
    let page = page.unwrap_or_default();
    blocking(move || Ok(backup.list_dir(&domain, &path, page)?)).await
}

#[tauri::command]
pub async fn file_info(
    state: State<'_, AppState>,
    handle: String,
    file_id: String,
) -> Result<FileInfo, IpcError> {
    let backup = state.get(&handle)?;
    blocking(move || {
        let entry = entry_of(&backup, &file_id)?;
        let data_present = !entry.kind.is_dir() && backup.has_data(&entry);
        Ok(FileInfo {
            file_id: entry.file_id.clone(),
            domain: entry.domain.clone(),
            relative_path: entry.relative_path.clone(),
            name: entry.name().to_string(),
            kind: entry.kind,
            data_bytes: if data_present {
                backup.data_len(&entry).ok()
            } else {
                None
            },
            sniffed: if data_present {
                backup.sniff(&entry).ok()
            } else {
                None
            },
            physical_path: entry
                .physical_path(backup.root())
                .map(|p| p.display().to_string()),
            data_present,
            meta: entry.meta,
        })
    })
    .await
}

#[tauri::command]
pub async fn read_text_preview(
    state: State<'_, AppState>,
    handle: String,
    file_id: String,
    max_bytes: Option<u64>,
) -> Result<TextPreview, IpcError> {
    const DEFAULT_MAX: u64 = 512 * 1024;
    let backup = state.get(&handle)?;
    let max_bytes = max_bytes.unwrap_or(DEFAULT_MAX).min(8 * 1024 * 1024);

    blocking(move || {
        let entry = entry_of(&backup, &file_id)?;
        let total_bytes = backup.data_len(&entry)?;
        let sniffed = backup.sniff(&entry)?;

        // A plist has to be parsed whole, so it is read in full up to the cap;
        // anything else is read as a bounded prefix.
        let bytes = backup.read_prefix(&entry, max_bytes as usize)?;
        let bytes_read = bytes.len() as u64;
        let truncated = bytes_read < total_bytes;

        let (text, plist_rendered, lossy) = if sniffed.preview == mime::PreviewKind::Plist {
            if truncated {
                // Half a plist will not parse; say so rather than show noise.
                (
                    format!(
                        "This property list is {total_bytes} bytes, larger than the \
                         {max_bytes}-byte preview limit, so it was not parsed.\n\
                         Use the hex view, or save the file out to inspect it."
                    ),
                    false,
                    false,
                )
            } else {
                let (text, parsed) = plist_text::to_xml_or_text(&bytes);
                (text, parsed, false)
            }
        } else {
            match String::from_utf8(bytes) {
                Ok(text) => (text, false, false),
                Err(e) => (
                    String::from_utf8_lossy(e.as_bytes()).into_owned(),
                    false,
                    true,
                ),
            }
        };

        Ok(TextPreview {
            text,
            truncated,
            bytes_read,
            total_bytes,
            kind: sniffed.preview,
            plist_rendered,
            lossy,
        })
    })
    .await
}

#[tauri::command]
pub async fn read_hex_preview(
    state: State<'_, AppState>,
    handle: String,
    file_id: String,
    offset: Option<u64>,
    len: Option<u64>,
) -> Result<HexPreview, IpcError> {
    const DEFAULT_LEN: u64 = 4 * 1024;
    let backup = state.get(&handle)?;
    let offset = offset.unwrap_or(0);
    // Capped: the hex view pages, so it never needs a whole file.
    let len = len.unwrap_or(DEFAULT_LEN).min(64 * 1024);

    blocking(move || {
        let entry = entry_of(&backup, &file_id)?;
        let total_bytes = backup.data_len(&entry)?;
        let bytes = backup.read_range(&entry, offset, len as usize)?;
        Ok(HexPreview {
            offset,
            len: bytes.len() as u64,
            hex: hex_encode(&bytes),
            total_bytes,
        })
    })
    .await
}

/// Lowercase hex, without pulling a dependency into src-tauri for it.
fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

#[tauri::command]
pub async fn sms_conversations(
    state: State<'_, AppState>,
    handle: String,
    page: Option<Page>,
) -> Result<Paged<ConversationSummary>, IpcError> {
    let backup = state.get(&handle)?;
    let page = page.unwrap_or_default();
    blocking(move || {
        let db = SmsDb::open(&backup)?;
        Ok(db.list_conversations(page)?)
    })
    .await
}

#[tauri::command]
pub async fn sms_messages(
    state: State<'_, AppState>,
    handle: String,
    conversation_id: String,
    cursor: Option<MessageCursor>,
) -> Result<MessagePage, IpcError> {
    let backup = state.get(&handle)?;
    let cursor = cursor.unwrap_or_default();
    blocking(move || {
        let db = SmsDb::open(&backup)?;
        Ok(db.list_messages(&conversation_id, cursor)?)
    })
    .await
}

#[tauri::command]
pub async fn wa_chats(
    state: State<'_, AppState>,
    handle: String,
    page: Option<Page>,
    include_broadcasts: Option<bool>,
) -> Result<Paged<ConversationSummary>, IpcError> {
    let backup = state.get(&handle)?;
    let page = page.unwrap_or_default();
    let include = include_broadcasts.unwrap_or(false);
    blocking(move || {
        let db = WhatsAppDb::open(&backup)?;
        Ok(db.list_conversations(page, include)?)
    })
    .await
}

#[tauri::command]
pub async fn wa_messages(
    state: State<'_, AppState>,
    handle: String,
    chat_id: String,
    cursor: Option<MessageCursor>,
) -> Result<MessagePage, IpcError> {
    let backup = state.get(&handle)?;
    let cursor = cursor.unwrap_or_default();
    blocking(move || {
        let db = WhatsAppDb::open(&backup)?;
        Ok(db.list_messages(&chat_id, cursor)?)
    })
    .await
}

#[tauri::command]
pub async fn photos_list(
    state: State<'_, AppState>,
    handle: String,
    filter: Option<PhotoFilter>,
    cursor: Option<PhotoCursor>,
) -> Result<PhotoPage, IpcError> {
    let backup = state.get(&handle)?;
    let filter = filter.unwrap_or_default();
    let cursor = cursor.unwrap_or_default();
    blocking(move || {
        let db = PhotosDb::open(&backup)?;
        Ok(db.list(&filter, cursor)?)
    })
    .await
}

#[tauri::command]
pub async fn photos_timeline(
    state: State<'_, AppState>,
    handle: String,
    filter: Option<PhotoFilter>,
) -> Result<Vec<ibex_core::extractors::MonthCount>, IpcError> {
    let backup = state.get(&handle)?;
    let filter = filter.unwrap_or_default();
    blocking(move || {
        let db = PhotosDb::open(&backup)?;
        Ok(db.timeline(&filter)?)
    })
    .await
}

#[tauri::command]
pub async fn photos_albums(
    state: State<'_, AppState>,
    handle: String,
) -> Result<Vec<ibex_core::extractors::AlbumSummary>, IpcError> {
    let backup = state.get(&handle)?;
    blocking(move || {
        let db = PhotosDb::open(&backup)?;
        Ok(db.albums()?)
    })
    .await
}

#[tauri::command]
pub async fn photo_detail(
    state: State<'_, AppState>,
    handle: String,
    id: String,
) -> Result<PhotoAsset, IpcError> {
    let backup = state.get(&handle)?;
    blocking(move || {
        let db = PhotosDb::open(&backup)?;
        db.asset(&id)?.ok_or_else(IpcError::unknown_file)
    })
    .await
}

#[tauri::command]
pub async fn voice_list(
    state: State<'_, AppState>,
    handle: String,
    page: Option<Page>,
) -> Result<Paged<VoiceRecording>, IpcError> {
    let backup = state.get(&handle)?;
    let page = page.unwrap_or_default();
    blocking(move || {
        let db = VoiceMemosDb::open(&backup)?;
        Ok(db.list(page)?)
    })
    .await
}

#[tauri::command]
pub async fn documents_sources(
    state: State<'_, AppState>,
    handle: String,
) -> Result<Vec<DocSource>, IpcError> {
    let backup = state.get(&handle)?;
    blocking(move || Ok(ibex_core::extractors::list_document_sources(&backup)?)).await
}

/// Start an export. Returns at once with a job id; progress arrives as events.
#[tauri::command]
pub async fn start_export<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    handle: String,
    selection: Selection,
    format: ExportFormat,
    out_dir: String,
    options: Option<ExportOptions>,
) -> Result<String, IpcError> {
    let backup = state.get(&handle)?;
    let options = options.unwrap_or_default();
    let out = PathBuf::from(&out_dir);

    let label = format!(
        "{} to {}",
        match selection.source.as_str() {
            "sms" => "Messages",
            "whatsapp" => "WhatsApp",
            other => other,
        },
        format.as_str()
    );
    let (job_id, cancel) = state.jobs().start("export", &label);

    // The work runs on a blocking worker; this command returns immediately so
    // the dialog can close and the toast can appear.
    let job_for_task = job_id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let registry_app = app.clone();
        let throttle = ProgressThrottle::new(app.clone(), job_for_task.clone());
        let state = tauri::Manager::state::<AppState>(&registry_app);

        let progress = |p: export::Progress| throttle.emit(&p, state.jobs());
        let result = export::run(
            &backup, &selection, format, &out, &options, &progress, &cancel,
        );

        match result {
            Ok(report) => {
                state.jobs().finished(&job_for_task, report.clone());
                let _ = tauri::Emitter::emit(
                    &app,
                    "job://finished",
                    JobFinishedEvent {
                        job_id: job_for_task,
                        report,
                    },
                );
            }
            Err(error) => {
                let ipc = IpcError::from(error);
                state.jobs().failed(&job_for_task, ipc.message.clone());
                let _ = tauri::Emitter::emit(
                    &app,
                    "job://failed",
                    JobFailedEvent {
                        job_id: job_for_task,
                        error: ipc.message,
                    },
                );
            }
        }
    });

    Ok(job_id)
}

/// Ask a running job to stop. The export keeps what it has already written and
/// its report says it was cancelled.
#[tauri::command]
pub async fn cancel_job(state: State<'_, AppState>, job_id: String) -> Result<bool, IpcError> {
    Ok(state.jobs().cancel(&job_id))
}

/// Every job this session has run, newest first.
#[tauri::command]
pub async fn list_jobs(state: State<'_, AppState>) -> Result<Vec<JobInfo>, IpcError> {
    Ok(state.jobs().list())
}

#[tauri::command]
pub async fn backup_stats(
    state: State<'_, AppState>,
    handle: String,
) -> Result<BackupStats, IpcError> {
    let backup = state.get(&handle)?;
    blocking(move || {
        Ok(BackupStats {
            total_bytes: backup.total_bytes()?,
            file_count: backup.manifest().count_files()?,
            domain_count: backup.domains()?.len() as u64,
            cache_bytes: backup.cache().size_on_disk()?,
        })
    })
    .await
}

#[tauri::command]
pub async fn save_file_as(
    state: State<'_, AppState>,
    handle: String,
    file_id: String,
    dest: String,
) -> Result<SaveReport, IpcError> {
    let backup = state.get(&handle)?;
    blocking(move || {
        let entry = entry_of(&backup, &file_id)?;
        let dest = PathBuf::from(&dest);
        let (bytes, mtime_restored) = backup.copy_file_to(&entry, &dest)?;
        Ok(SaveReport {
            path: dest.display().to_string(),
            bytes,
            mtime_restored,
        })
    })
    .await
}

#[tauri::command]
pub async fn clear_cache(state: State<'_, AppState>, handle: String) -> Result<u64, IpcError> {
    let backup: Arc<Backup> = state.get(&handle)?;
    blocking(move || {
        let freed = backup.cache().size_on_disk().unwrap_or(0);
        backup.clear_cache()?;
        Ok(freed)
    })
    .await
}

/// Size of the whole ibex cache directory, for the Overview's notice.
#[tauri::command]
pub async fn total_cache_size(state: State<'_, AppState>) -> Result<u64, IpcError> {
    let data_root = state.data_root().map(Path::to_path_buf);
    blocking(move || {
        let dir = match data_root {
            Some(dir) => dir,
            None => ibex_core::cache::data_dir()?,
        };
        Ok(CacheDir::at(dir.join("cache")).size_on_disk()?)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibex_core::testutil::FixtureBackup;

    fn fixture(tmp: &std::path::Path) -> Backup {
        let root = FixtureBackup::new()
            .with_sample_media()
            .with_db(
                "HomeDomain",
                "Library/SMS/sms.db",
                "CREATE TABLE message (ROWID INTEGER PRIMARY KEY);",
            )
            .build(tmp)
            .unwrap();
        Backup::open_with_cache(&root, None, CacheDir::at(tmp.join("cache"))).unwrap()
    }

    #[test]
    fn hex_encoding() {
        assert_eq!(hex_encode(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(hex_encode(&[]), "");
    }

    #[test]
    fn source_detection_finds_what_is_there_and_is_honest_about_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture(tmp.path());
        let sources = detect_sources(&backup);

        let sms = sources.iter().find(|s| s.id == "sms").unwrap();
        assert!(sms.available, "the fixture has an sms.db");
        assert!(
            sms.implemented,
            "M2 ships the Messages view, so the sidebar entry is usable"
        );

        let whatsapp = sources.iter().find(|s| s.id == "whatsapp").unwrap();
        assert!(!whatsapp.available);
        assert!(
            whatsapp.reason.as_ref().unwrap().contains("M3"),
            "the tooltip should say which milestone brings it"
        );

        assert_eq!(sources.len(), 5);
    }

    /// The command bodies need a Tauri `State`, so these exercise the logic
    /// the commands wrap, which is where the behaviour actually lives.
    #[test]
    fn previews_are_bounded_and_labelled() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture(tmp.path());

        // The text sample reads back as text.
        let entry = backup
            .find("HomeDomain", "Library/IbexSamples/notes.txt")
            .unwrap()
            .unwrap();
        let sniffed = backup.sniff(&entry).unwrap();
        assert_eq!(sniffed.preview, mime::PreviewKind::Text);
        let bytes = backup.read_prefix(&entry, 8).unwrap();
        assert_eq!(bytes.len(), 8, "a prefix read must respect its cap");

        // The plist sample renders as XML.
        let entry = backup
            .find("HomeDomain", "Library/IbexSamples/preferences.plist")
            .unwrap()
            .unwrap();
        assert_eq!(
            backup.sniff(&entry).unwrap().preview,
            mime::PreviewKind::Plist
        );
        let all = backup.read_prefix(&entry, 1024 * 1024).unwrap();
        let (xml, parsed) = plist_text::to_xml_or_text(&all);
        assert!(parsed);
        assert!(xml.contains("<key>Label</key>"));
    }

    #[test]
    fn hex_windows_respect_offset_and_length() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture(tmp.path());
        let entry = backup
            .find("HomeDomain", "Library/IbexSamples/tone.wav")
            .unwrap()
            .unwrap();

        let bytes = backup.read_range(&entry, 0, 4).unwrap();
        assert_eq!(hex_encode(&bytes), "52494646", "\"RIFF\"");

        // Reading past the end returns what exists, not an error.
        let total = backup.data_len(&entry).unwrap();
        let bytes = backup.read_range(&entry, total - 2, 100).unwrap();
        assert_eq!(bytes.len(), 2);
    }

    #[test]
    fn saving_a_file_out_preserves_bytes_and_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture(tmp.path());
        let entry = backup
            .find("HomeDomain", "Library/IbexSamples/pixel.png")
            .unwrap()
            .unwrap();

        let dest = tmp.path().join("saved/pixel.png");
        let (bytes, mtime_restored) = backup.copy_file_to(&entry, &dest).unwrap();

        assert!(dest.is_file(), "parent directories should be created");
        assert_eq!(bytes, backup.data_len(&entry).unwrap());
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            backup.read_file(&entry).unwrap(),
            "the saved file must be byte-identical"
        );

        assert!(mtime_restored);
        let expected = entry.meta.as_ref().unwrap().mtime.unwrap();
        let actual = std::fs::metadata(&dest).unwrap().modified().unwrap();
        let actual = chrono::DateTime::<chrono::Utc>::from(actual);
        assert_eq!(
            actual.timestamp(),
            expected.timestamp(),
            "the original modification time should survive the copy"
        );
    }

    #[test]
    fn stats_describe_the_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture(tmp.path());

        let total_bytes = backup.total_bytes().unwrap();
        let file_count = backup.manifest().count_files().unwrap();
        assert!(total_bytes > 0);
        assert!(file_count > 0);
        // The disk total includes Manifest.db and the plists, so it exceeds the
        // sum of the payloads alone.
        let payload_total: u64 = backup
            .domains_with_sizes(&mut |_, _| {})
            .unwrap()
            .iter()
            .filter_map(|d| d.total_size)
            .sum();
        assert!(
            total_bytes > payload_total,
            "disk size {total_bytes} should exceed payload size {payload_total}"
        );
    }
}
