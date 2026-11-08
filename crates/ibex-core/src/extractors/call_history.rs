//! Call History, from `HomeDomain` / `Library/CallHistoryDB/CallHistory.storedata`.
//!
//! Core Data (`ZCALLRECORD`, `Z_PK`), so the dates are Apple-epoch and go
//! through `util::time::from_apple`, never inlined (hard rule 7). Two columns
//! this module deliberately does not pretend to understand fully:
//!
//! - `ZSERVICE_PROVIDER` is read as-is and matched only against the two known
//!   real values (`com.apple.Telephony`, `com.apple.FaceTime`) — this is what
//!   separates a cellular call from a FaceTime one, so audio vs. video
//!   FaceTime is not distinguished.
//! - `ZCALLTYPE` is carried through raw (`Option<i64>`), not mapped to an
//!   enum: real data shows at least three values with no confirmed meaning,
//!   and guessing wrong here would mislabel calls rather than just omit a
//!   detail.

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Backup;
use crate::error::Result;
use crate::extractors::{Availability, ContactResolver, Extractor, Participant};
use crate::page::{Page, Paged};
use crate::util::time::from_apple;

const DOMAIN: &str = "HomeDomain";
const DB_PATH: &str = "Library/CallHistoryDB/CallHistory.storedata";

/// Which service carried the call, so a FaceTime call isn't shown as cellular.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
#[ts(export)]
pub enum CallService {
    Telephony,
    FaceTime,
    /// Some other `ZSERVICE_PROVIDER` value, or an empty string when the
    /// column itself is absent from this backup's schema.
    Other(String),
}

/// One call record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CallRecord {
    /// `Z_PK`.
    pub id: String,
    /// Raw `ZADDRESS` — the number or handle, before contact resolution.
    pub address: Option<String>,
    pub contact: Participant,
    pub date: Option<DateTime<Utc>>,
    pub duration_seconds: u32,
    pub outgoing: bool,
    /// `None` when `ZANSWERED` is not a column in this backup's schema,
    /// rather than guessing at whether the call connected.
    pub answered: Option<bool>,
    pub service: CallService,
    /// Raw `ZCALLTYPE` — see the module doc for why this isn't an enum.
    pub call_type: Option<i64>,
}

/// The Call History extractor.
pub struct CallHistory;

impl Extractor for CallHistory {
    const ID: &'static str = "calls";

    fn detect(backup: &Backup) -> Availability {
        match backup.find(DOMAIN, DB_PATH) {
            Ok(Some(entry)) if backup.has_data(&entry) => {
                Availability::Present { version_hint: None }
            }
            _ => Availability::Absent,
        }
    }
}

/// Browse call history, newest first.
pub struct CallHistoryDb<'a> {
    #[allow(dead_code)]
    backup: &'a Backup,
    conn: Connection,
    contacts: ContactResolver,
}

impl<'a> CallHistoryDb<'a> {
    pub fn open(backup: &'a Backup) -> Result<Self> {
        let conn = backup.open_db(DOMAIN, DB_PATH)?;
        let contacts = ContactResolver::open(backup)?;
        Ok(Self {
            backup,
            conn,
            contacts,
        })
    }

    fn column_exists(&self, table: &str, column: &str) -> bool {
        self.conn
            .query_one(
                "SELECT count(*) FROM pragma_table_info(?1) WHERE name = ?2",
                rusqlite::params![table, column],
                |row| row.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    /// One page of calls, newest first.
    pub fn list(&self, page: Page) -> Result<Paged<CallRecord>> {
        let total: i64 = self
            .conn
            .query_one("SELECT count(*) FROM ZCALLRECORD", [], |row| row.get(0))?;

        let has_answered = self.column_exists("ZCALLRECORD", "ZANSWERED");
        let has_service = self.column_exists("ZCALLRECORD", "ZSERVICE_PROVIDER");
        let has_call_type = self.column_exists("ZCALLRECORD", "ZCALLTYPE");

        let sql = format!(
            "SELECT Z_PK, ZADDRESS, ZDATE, ZDURATION, ZORIGINATED, {answered}, {service}, {call_type}
             FROM ZCALLRECORD
             ORDER BY ZDATE DESC, Z_PK DESC
             LIMIT ?1 OFFSET ?2",
            answered = if has_answered { "ZANSWERED" } else { "NULL" },
            service = if has_service { "ZSERVICE_PROVIDER" } else { "NULL" },
            call_type = if has_call_type { "ZCALLTYPE" } else { "NULL" },
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![page.limit, page.offset as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<i64>>(7)?,
            ))
        })?;

        let mut items = Vec::new();
        for row in rows {
            let (pk, address, date, duration, originated, answered, service, call_type) = row?;
            let contact = self.contacts.participant(address.as_deref().unwrap_or(""));
            items.push(CallRecord {
                id: pk.to_string(),
                address,
                contact,
                date: date.and_then(from_apple),
                duration_seconds: duration
                    .filter(|d| d.is_finite() && *d > 0.0)
                    .unwrap_or(0.0) as u32,
                outgoing: originated == Some(1),
                answered: answered.map(|a| a != 0),
                service: match service.as_deref() {
                    Some("com.apple.Telephony") => CallService::Telephony,
                    Some("com.apple.FaceTime") => CallService::FaceTime,
                    Some(other) => CallService::Other(other.to_string()),
                    None => CallService::Other(String::new()),
                },
                call_type,
            });
        }

        Ok(Paged::new(items, page, Some(total.max(0) as u64)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CacheDir;
    use crate::testutil::FixtureBackup;

    fn open_fixture(tmp: &std::path::Path) -> Backup {
        let root = FixtureBackup::new()
            .with_address_book()
            .with_call_history()
            .build(tmp)
            .unwrap();
        Backup::open_with_cache(&root, None, CacheDir::at(tmp.join("cache"))).unwrap()
    }

    #[test]
    fn call_history_detects_as_present() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        assert_eq!(
            CallHistory::detect(&backup),
            Availability::Present { version_hint: None }
        );
    }

    #[test]
    fn calls_list_newest_first_and_resolve_contacts() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = CallHistoryDb::open(&backup).unwrap();
        let page = db.list(Page::default()).unwrap();

        assert_eq!(page.total, Some(5));
        // Newest first.
        let dates: Vec<_> = page.items.iter().map(|c| c.date).collect();
        let mut sorted = dates.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(dates, sorted, "calls must list newest first");

        // Ada's number is in the address book fixture.
        let ada_call = page
            .items
            .iter()
            .find(|c| c.address.as_deref() == Some("+1 (555) 000-1001"))
            .expect("a call to Ada's number");
        assert_eq!(
            ada_call.contact.display_name.as_deref(),
            Some("Ada Lovelace")
        );
    }

    #[test]
    fn cellular_and_facetime_calls_are_told_apart() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = CallHistoryDb::open(&backup).unwrap();
        let page = db.list(Page::default()).unwrap();

        let telephony = page
            .items
            .iter()
            .filter(|c| c.service == CallService::Telephony)
            .count();
        let facetime = page
            .items
            .iter()
            .filter(|c| c.service == CallService::FaceTime)
            .count();
        assert!(telephony > 0, "got {page:?}");
        assert_eq!(facetime, 1);
    }

    #[test]
    fn an_unanswered_call_and_an_unknown_call_type_degrade_gracefully() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = CallHistoryDb::open(&backup).unwrap();
        let page = db.list(Page::default()).unwrap();

        let missed = page
            .items
            .iter()
            .find(|c| c.answered == Some(false))
            .expect("an unanswered call in the fixture");
        assert!(!missed.outgoing, "the fixture's missed call is incoming");

        // The fixture includes an unrecognised ZCALLTYPE value; it must
        // survive the round trip raw rather than being dropped or crashing.
        assert!(
            page.items.iter().any(|c| c.call_type == Some(99)),
            "got {page:?}"
        );
    }

    #[test]
    fn an_unknown_caller_with_no_address_still_lists() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = CallHistoryDb::open(&backup).unwrap();
        let page = db.list(Page::default()).unwrap();

        let unknown = page
            .items
            .iter()
            .find(|c| c.address.is_none())
            .expect("a call with no recorded address");
        assert!(!unknown.contact.from_contacts);
    }
}
