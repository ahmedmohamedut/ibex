//! Voice Memos, from `CloudRecordings.db` when it can be read, or straight
//! from the recording files when it can't.
//!
//! Planned against a real iOS 27.2 backup. Three things that backup's own
//! `Recordings/` folder shaped this module, none of them from the database
//! itself — its `CloudRecordings.db` turned out to be genuinely corrupted
//! (independently confirmed with plain `sqlite3`, no `-wal`/`-shm` sidecar to
//! explain it), so the fallback below is what the real backup actually
//! exercises:
//!
//! - **`.qta` is a QuickTime container** (`ftyp` says `qt  `), the same
//!   family `.mov` already plays through via GStreamer's `qtdemux` — see the
//!   WhatsApp voice-note finding in `docs/DATA_SOURCES.md`. It is Voice
//!   Memos' own "edited recording" format, written once noise reduction or
//!   multi-take editing has been applied.
//! - **`.composition` directories hold no audio at all** — only an editing
//!   `manifest.plist`. The real recording is the sibling `.m4a`/`.qta` with
//!   the same timestamp base name.
//! - **`-trackN.waveform` files are UI waveform data**, never audio, and
//!   must not be listed as recordings.

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::Result;
use crate::extractors::{Availability, Extractor};
use crate::page::{Page, Paged};
use crate::util::mime::{self, PreviewKind};
use crate::util::time::from_apple;
use crate::{Backup, ManifestEntry};

/// Containers to probe, oldest-to-newest per `docs/DATA_SOURCES.md`. Only the
/// first is exercised by the real backup this was written against; the
/// second is fixture-only.
const CONTAINERS: &[(&str, &str)] = &[
    (
        "AppDomainGroup-group.com.apple.VoiceMemos.shared",
        "Recordings",
    ),
    ("MediaDomain", "Media/Recordings"),
];

const DB_NAME: &str = "CloudRecordings.db";

/// Extensions a recording's audio file is seen with.
const AUDIO_EXTENSIONS: &[&str] = &["m4a", "M4A", "qta", "QTA"];

/// Whether a recording's bytes are actually present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RecordingAvailability {
    InBackup,
    Missing,
}

/// One recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceRecording {
    /// Stable id: the audio file's own file id, so pagination and export
    /// selections survive a re-list.
    pub id: String,
    /// From the database. `None` in fallback mode — there is nowhere else a
    /// title could come from once the database can't be read.
    pub title: Option<String>,
    pub file_id: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub duration: Option<f64>,
    pub size: Option<u64>,
    pub mime: Option<String>,
    pub preview: Option<PreviewKind>,
    pub availability: RecordingAvailability,
}

/// The Voice Memos extractor.
pub struct VoiceMemos;

impl Extractor for VoiceMemos {
    const ID: &'static str = "voice";

    fn detect(backup: &Backup) -> Availability {
        for (domain, dir) in CONTAINERS {
            if container_has_recordings(backup, domain, dir) {
                return Availability::Present { version_hint: None };
            }
        }
        Availability::Absent
    }
}

/// Whether a container holds anything worth listing: the database, or a
/// recording file directly.
fn container_has_recordings(backup: &Backup, domain: &str, dir: &str) -> bool {
    if backup
        .find(domain, &format!("{dir}/{DB_NAME}"))
        .ok()
        .flatten()
        .is_some_and(|e| backup.has_data(&e))
    {
        return true;
    }
    list_audio_files(backup, domain, dir)
        .map(|files| !files.is_empty())
        .unwrap_or(false)
}

/// An open Voice Memos source.
pub struct VoiceMemosDb<'a> {
    backup: &'a Backup,
    domain: &'static str,
    dir: &'static str,
    /// `Some` only when the database opened *and* its schema looks usable —
    /// otherwise every query goes through the file-listing fallback, which is
    /// what the real backup this was built against actually needs.
    conn: Option<Connection>,
}

impl<'a> VoiceMemosDb<'a> {
    /// Open the first container that has anything to show.
    pub fn open(backup: &'a Backup) -> Result<Self> {
        let (domain, dir) = CONTAINERS
            .iter()
            .find(|(domain, dir)| container_has_recordings(backup, domain, dir))
            .copied()
            .ok_or(crate::error::ExtractError::SourceAbsent {
                source_id: VoiceMemos::ID,
            })?;

        // A connection that opens but can't run a basic query against the
        // expected table is treated exactly like one that didn't open at
        // all: both fall back to listing files directly. This is not
        // theoretical — it is this real backup's actual state.
        let conn = backup
            .find(domain, &format!("{dir}/{DB_NAME}"))
            .ok()
            .flatten()
            .filter(|e| backup.has_data(e))
            .and_then(|_| backup.open_db(domain, &format!("{dir}/{DB_NAME}")).ok())
            .filter(|conn| table_is_queryable(conn, "ZCLOUDRECORDING"));

        Ok(Self {
            backup,
            domain,
            dir,
            conn,
        })
    }

    /// Whether this source is reading the database or the fallback. Exposed
    /// for diagnostics and tests, not for callers to branch on.
    pub fn is_database_backed(&self) -> bool {
        self.conn.is_some()
    }

    /// One page, newest first.
    pub fn list(&self, page: Page) -> Result<Paged<VoiceRecording>> {
        match &self.conn {
            Some(conn) => self.list_from_db(conn, page),
            None => self.list_from_files(page),
        }
    }

    fn list_from_db(&self, conn: &Connection, page: Page) -> Result<Paged<VoiceRecording>> {
        let columns = table_columns(conn, "ZCLOUDRECORDING");
        let has = |c: &str| columns.contains(c);
        let col = |c: &str| {
            if has(c) {
                format!("r.{c}")
            } else {
                "NULL".to_string()
            }
        };

        let total: i64 =
            conn.query_one("SELECT count(*) FROM ZCLOUDRECORDING", [], |row| row.get(0))?;

        let sql = format!(
            "SELECT {}, {}, {}, {}, {} FROM ZCLOUDRECORDING r \
             ORDER BY r.{} IS NULL, r.{} DESC LIMIT ?1 OFFSET ?2",
            col("ZPATH"),
            col("ZENCRYPTEDTITLE"),
            col("ZCUSTOMLABEL"),
            col("ZDATE"),
            col("ZDURATION"),
            if has("ZDATE") { "ZDATE" } else { "ZUNIQUEID" },
            if has("ZDATE") { "ZDATE" } else { "ZUNIQUEID" },
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params![page.sql_limit(), page.sql_offset()],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<f64>>(3)?,
                    r.get::<_, Option<f64>>(4)?,
                ))
            },
        )?;

        let mut items = Vec::new();
        for row in rows {
            let (path, encrypted_title, custom_label, date, duration) = row?;
            items.push(self.hydrate_from_db(path, encrypted_title, custom_label, date, duration));
        }
        Ok(Paged::new(items, page, Some(total.max(0) as u64)))
    }

    fn hydrate_from_db(
        &self,
        path: Option<String>,
        encrypted_title: Option<String>,
        custom_label: Option<String>,
        date: Option<f64>,
        duration: Option<f64>,
    ) -> VoiceRecording {
        // `ZCUSTOMLABEL` (a user rename) wins over `ZENCRYPTEDTITLE` (the
        // auto-generated one — "ZENCRYPTED" is historical; it is plain text).
        let title = custom_label
            .or(encrypted_title)
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());

        let relative = path
            .as_deref()
            .map(|p| format!("{}/{}", self.dir, p.trim_start_matches('/')));
        let entry = relative.as_deref().and_then(|p| {
            self.backup
                .find(self.domain, p)
                .ok()
                .flatten()
                .filter(|e| self.backup.has_data(e))
        });

        self.describe(entry, title, date.and_then(from_apple), duration)
    }

    /// The fallback: list `.m4a`/`.qta` files directly, skipping waveform
    /// sidecars and `.composition` packages.
    fn list_from_files(&self, page: Page) -> Result<Paged<VoiceRecording>> {
        let mut all = list_audio_files(self.backup, self.domain, self.dir)?;
        // Newest first, by the file's own mtime — the only ordering signal
        // available with no database.
        all.sort_by(|a, b| {
            let ta = a.meta.as_ref().and_then(|m| m.mtime);
            let tb = b.meta.as_ref().and_then(|m| m.mtime);
            tb.cmp(&ta)
        });

        let total = all.len() as u64;
        let start = (page.offset as usize).min(all.len());
        let end = (start + page.limit as usize).min(all.len());
        let items = all[start..end]
            .iter()
            .map(|entry| {
                self.describe(
                    Some(entry.clone()),
                    None,
                    entry.meta.as_ref().and_then(|m| m.mtime),
                    None,
                )
            })
            .collect();
        Ok(Paged::new(items, page, Some(total)))
    }

    fn describe(
        &self,
        entry: Option<ManifestEntry>,
        title: Option<String>,
        created_at: Option<DateTime<Utc>>,
        duration: Option<f64>,
    ) -> VoiceRecording {
        let sniffed = entry.as_ref().and_then(|e| self.backup.sniff(e).ok());
        let by_extension = entry
            .as_ref()
            .and_then(|e| mime_guess::from_path(e.name()).first_raw())
            .map(str::to_string);
        let mime = sniffed
            .as_ref()
            .map(|s| s.mime.clone())
            .or_else(|| by_extension.clone());
        let preview = sniffed
            .as_ref()
            .map(|s| s.preview)
            .or_else(|| by_extension.as_deref().map(mime::preview_for));

        VoiceRecording {
            id: entry
                .as_ref()
                .map(|e| e.file_id.clone())
                .unwrap_or_else(|| format!("missing-{}", title.as_deref().unwrap_or("untitled"))),
            title,
            file_id: entry.as_ref().map(|e| e.file_id.clone()),
            created_at,
            duration: duration.filter(|d| *d > 0.0),
            size: entry.as_ref().and_then(|e| e.meta.as_ref()).map(|m| m.size),
            mime,
            preview,
            availability: if entry.is_some() {
                RecordingAvailability::InBackup
            } else {
                RecordingAvailability::Missing
            },
        }
    }
}

/// Every recording's audio file in a container, unordered — `.m4a`/`.qta`
/// only, not their `.waveform` sidecars or `.composition` packages.
fn list_audio_files(backup: &Backup, domain: &str, dir: &str) -> Result<Vec<ManifestEntry>> {
    let mut out = Vec::new();
    let mut offset = 0u64;
    loop {
        let page = backup.list_dir(domain, dir, Page::new(offset, Page::MAX_LIMIT))?;
        let count = page.items.len() as u64;
        for entry in page.items {
            if !entry.kind.is_file() {
                continue; // skips .composition directories too
            }
            let name = entry.name();
            let is_audio = AUDIO_EXTENSIONS
                .iter()
                .any(|ext| name.ends_with(&format!(".{ext}")));
            // A waveform sidecar shares its recording's base name with a
            // `-trackN` suffix before the extension, which no real recording
            // file has.
            if is_audio && !name.contains("-track") {
                out.push(entry);
            }
        }
        match page.next_offset {
            Some(next) if count > 0 => offset = next,
            _ => break,
        }
    }
    Ok(out)
}

/// Whether a connection can actually run a basic query against `table` —
/// distinguishing a database that opened but is corrupt from one that is
/// genuinely usable. `has_file_column`-style probing (hard rule 6) checks a
/// column exists; this checks the *data* is reachable at all, which a
/// malformed file fails even though SQLite is happy to open the handle.
fn table_is_queryable(conn: &Connection, table: &str) -> bool {
    conn.query_one(&format!("SELECT count(*) FROM {table}"), [], |row| {
        row.get::<_, i64>(0)
    })
    .is_ok()
}

fn table_columns(conn: &Connection, table: &str) -> std::collections::HashSet<String> {
    let Ok(mut stmt) = conn.prepare("SELECT name FROM pragma_table_info(?1)") else {
        return std::collections::HashSet::new();
    };
    stmt.query_map([table], |row| row.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{FixtureBackup, voice as fixture};
    use crate::{Backup, CacheDir};

    fn open_fixture(tmp: &std::path::Path) -> Backup {
        let root = FixtureBackup::new().with_voice_memos().build(tmp).unwrap();
        Backup::open_with_cache(&root, None, CacheDir::at(tmp.join("cache"))).unwrap()
    }

    fn open_corrupted_fixture(tmp: &std::path::Path) -> Backup {
        let root = FixtureBackup::new()
            .with_corrupted_voice_memos()
            .build(tmp)
            .unwrap();
        Backup::open_with_cache(&root, None, CacheDir::at(tmp.join("cache"))).unwrap()
    }

    #[test]
    fn detection_finds_the_library() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        assert!(VoiceMemos::detect(&backup).is_present());

        let bare = FixtureBackup::new()
            .with_file("HomeDomain", "a.txt", b"x")
            .build(&tmp.path().join("bare"))
            .unwrap();
        let bare =
            Backup::open_with_cache(&bare, None, CacheDir::at(tmp.path().join("c2"))).unwrap();
        assert_eq!(VoiceMemos::detect(&bare), Availability::Absent);
    }

    #[test]
    fn a_readable_database_is_used_and_lists_every_recording() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = VoiceMemosDb::open(&backup).unwrap();
        assert!(db.is_database_backed());

        let page = db.list(Page::first(50)).unwrap();
        assert_eq!(page.items.len(), 4);
        assert_eq!(page.total, Some(4));
    }

    #[test]
    fn a_custom_label_wins_over_the_auto_generated_title() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = VoiceMemosDb::open(&backup).unwrap();
        let page = db.list(Page::first(50)).unwrap();

        let titled = page
            .items
            .iter()
            .find(|r| r.title.as_deref() == Some("Fixture Title"))
            .expect("the custom label should be the title");
        assert!(titled.file_id.is_some());
    }

    #[test]
    fn an_untitled_recording_has_no_title_rather_than_an_empty_string() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = VoiceMemosDb::open(&backup).unwrap();
        let page = db.list(Page::first(50)).unwrap();

        let untitled = page
            .items
            .iter()
            .find(|r| r.duration == Some(12.0))
            .unwrap();
        assert_eq!(untitled.title, None);
    }

    #[test]
    fn a_recording_missing_from_the_backup_is_reported_not_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = VoiceMemosDb::open(&backup).unwrap();
        let page = db.list(Page::first(50)).unwrap();

        let missing = page
            .items
            .iter()
            .find(|r| r.availability == RecordingAvailability::Missing)
            .expect("the database-listed, backup-absent recording should still appear");
        assert!(missing.file_id.is_none());
    }

    #[test]
    fn a_corrupted_database_falls_back_to_listing_files_directly() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_corrupted_fixture(tmp.path());
        let db = VoiceMemosDb::open(&backup).unwrap();
        assert!(
            !db.is_database_backed(),
            "a connection that opens but can't query must still fall back"
        );

        let page = db.list(Page::first(50)).unwrap();
        // The titled .m4a and the .qta — not the waveform sidecar or the
        // composition folder, and no titles since there is no database.
        assert_eq!(page.items.len(), 2);
        assert!(page.items.iter().all(|r| r.title.is_none()));
        assert!(
            page.items
                .iter()
                .all(|r| r.availability == RecordingAvailability::InBackup)
        );
    }

    #[test]
    fn a_missing_database_also_falls_back_to_listing_files() {
        let tmp = tempfile::tempdir().unwrap();
        use fixture as vm;
        let root = FixtureBackup::new()
            .with_file(
                vm::DOMAIN,
                &format!("{}/{}", vm::DIR, vm::TITLED_PATH),
                &vm::m4a(),
            )
            .build(tmp.path())
            .unwrap();
        let backup =
            Backup::open_with_cache(&root, None, CacheDir::at(tmp.path().join("cache"))).unwrap();

        let db = VoiceMemosDb::open(&backup).unwrap();
        assert!(!db.is_database_backed());
        let page = db.list(Page::first(50)).unwrap();
        assert_eq!(page.items.len(), 1);
    }

    #[test]
    fn waveform_sidecars_and_composition_folders_are_never_listed() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_corrupted_fixture(tmp.path());
        let db = VoiceMemosDb::open(&backup).unwrap();
        let page = db.list(Page::first(50)).unwrap();
        assert!(
            page.items
                .iter()
                .all(|r| r.file_id.as_deref().is_some_and(|id| {
                    // The id is a file id, not a path, but the count check
                    // above is the real guard; this just documents the
                    // intent that a .waveform file never resolves to a
                    // "recording" at all.
                    !id.is_empty()
                })),
        );
        assert_eq!(page.items.len(), 2, "waveform and composition excluded");
    }

    #[test]
    fn qta_files_are_recognised_alongside_m4a() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = VoiceMemosDb::open(&backup).unwrap();
        let page = db.list(Page::first(50)).unwrap();
        let qta = page
            .items
            .iter()
            .find(|r| r.duration == Some(97.25))
            .unwrap();
        assert!(qta.file_id.is_some());
    }

    #[test]
    fn listing_is_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = VoiceMemosDb::open(&backup).unwrap();
        let page = db.list(Page::first(50)).unwrap();
        let dates: Vec<_> = page.items.iter().filter_map(|r| r.created_at).collect();
        assert!(dates.windows(2).all(|w| w[0] >= w[1]));
    }

    #[test]
    fn pagination_does_not_lose_or_repeat_recordings() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = VoiceMemosDb::open(&backup).unwrap();

        let whole = db.list(Page::first(50)).unwrap().items;
        let mut paged = Vec::new();
        let mut offset = 0u64;
        loop {
            let page = db.list(Page::new(offset, 1)).unwrap();
            let got = page.items.len();
            paged.extend(page.items);
            match page.next_offset {
                Some(next) if got > 0 => offset = next,
                _ => break,
            }
        }
        assert_eq!(paged.len(), whole.len());
        let ids: std::collections::HashSet<_> = paged.iter().map(|r| &r.id).collect();
        assert_eq!(ids.len(), paged.len());
    }
}
