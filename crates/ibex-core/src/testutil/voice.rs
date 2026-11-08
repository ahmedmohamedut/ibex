//! A synthetic `CloudRecordings.db` and the recording files it points at,
//! plus a deliberately corrupted variant for the fallback path.
//!
//! The real backup this was built against has a `CloudRecordings.db` that
//! independently confirmed `sqlite3` tools cannot open at all, so the
//! fallback is not a hypothetical path — it is what the real backup
//! actually needs. This fixture exercises both, so the database path stays
//! covered too.

use chrono::TimeZone;

use crate::util::time::to_apple_secs;

pub const DOMAIN: &str = "AppDomainGroup-group.com.apple.VoiceMemos.shared";
pub const DIR: &str = "Recordings";

/// A titled recording, found through `ZCUSTOMLABEL`.
pub const TITLED_PATH: &str = "20260301 120000-AAAA0001.m4a";
/// An untitled recording — title comes back `None`, not an empty string.
pub const UNTITLED_PATH: &str = "20260215 081500-BBBB0002.m4a";
/// The newer QuickTime-container "edited" format.
pub const QTA_PATH: &str = "20260410 093000-CCCC0003.qta";
/// Listed in the database, but its file is not in this backup.
pub const MISSING_PATH: &str = "20260501 100000-DDDD0004.m4a";

/// A `.composition` folder's editing metadata — holds no audio, must never
/// be listed as a recording even in fallback mode.
pub const COMPOSITION_DIR: &str = "20260301 120000.composition";
/// A waveform sidecar beside [`QTA_PATH`] — UI data, never audio.
pub const WAVEFORM_PATH: &str = "20260410 093000-track0.waveform";

/// A fixed anchor so dates are stable across runs, like the Photos fixture.
fn apple_date(days_before_anchor: i64) -> f64 {
    let anchor = chrono::Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap();
    to_apple_secs(anchor - chrono::Duration::days(days_before_anchor))
}

/// `CREATE TABLE` plus `INSERT` statements for a readable library.
pub fn cloud_recordings_sql() -> String {
    format!(
        "CREATE TABLE ZCLOUDRECORDING (
             Z_PK INTEGER PRIMARY KEY,
             ZPATH VARCHAR,
             ZENCRYPTEDTITLE VARCHAR,
             ZCUSTOMLABEL VARCHAR,
             ZDATE TIMESTAMP,
             ZDURATION FLOAT,
             ZUNIQUEID VARCHAR,
             ZFOLDER INTEGER);

         INSERT INTO ZCLOUDRECORDING
             (ZPATH, ZENCRYPTEDTITLE, ZCUSTOMLABEL, ZDATE, ZDURATION, ZUNIQUEID)
         VALUES
             ('{TITLED_PATH}', 'New Recording 1', 'Fixture Title', {t1}, 42.5, 'uuid-1'),
             ('{UNTITLED_PATH}', NULL, NULL, {t2}, 12.0, 'uuid-2'),
             ('{QTA_PATH}', 'New Recording 3', NULL, {t3}, 97.25, 'uuid-3'),
             ('{MISSING_PATH}', 'New Recording 4', NULL, {t4}, 5.0, 'uuid-4');
        ",
        t1 = apple_date(3),
        t2 = apple_date(5),
        t3 = apple_date(1),
        t4 = apple_date(7),
    )
}

/// Bytes that look like the start of a SQLite file but are not one — the
/// same failure shape the real backup's `CloudRecordings.db` has: a
/// connection may well *open*, but any real query fails. Independently
/// confirmed against the real file with both Python's `sqlite3` and the
/// system `sqlite3` CLI, neither of which could run even
/// `PRAGMA integrity_check` on it.
pub fn corrupted_db() -> Vec<u8> {
    let mut bytes = b"SQLite format 3\0".to_vec();
    bytes.extend(std::iter::repeat_n(0xFFu8, 4096));
    bytes
}

/// A minimal M4A header, enough for the type sniffer.
pub fn m4a() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&[0, 0, 0, 0x1c]);
    bytes.extend_from_slice(b"ftypM4A ");
    bytes.extend_from_slice(b"\0\0\0\0M4A isom");
    bytes.extend_from_slice(b"mp42");
    bytes.extend_from_slice(&[0, 0, 0, 8]);
    bytes.extend_from_slice(b"free");
    bytes
}

/// A minimal QuickTime-container header, as Voice Memos writes for `.qta`.
pub fn qta() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&[0, 0, 0, 0x14]);
    bytes.extend_from_slice(b"ftypqt  ");
    bytes.extend_from_slice(b"\0\0\x02\0qt  ");
    bytes.extend_from_slice(&[0, 0, 0, 8]);
    bytes.extend_from_slice(b"free");
    bytes
}
