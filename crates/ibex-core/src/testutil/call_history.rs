//! Synthetic `CallHistory.storedata`, for exercising `call_history.rs`
//! without a real backup.

use crate::util::time::to_apple_secs;

/// SQL for a small `CallHistory.storedata`: five records covering a resolved
/// cellular call (in and out), a FaceTime call, an unanswered call, and an
/// unknown caller with no recorded address and an unrecognised `ZCALLTYPE`.
pub fn call_history_sql() -> String {
    let base = chrono::DateTime::parse_from_rfc3339("2026-11-01T09:00:00Z")
        .expect("a valid literal")
        .with_timezone(&chrono::Utc);
    let at = |minutes: i64| to_apple_secs(base + chrono::Duration::minutes(minutes));

    format!(
        "CREATE TABLE ZCALLRECORD (
             Z_PK INTEGER PRIMARY KEY AUTOINCREMENT,
             ZADDRESS TEXT,
             ZDATE REAL,
             ZDURATION REAL,
             ZORIGINATED INTEGER,
             ZANSWERED INTEGER,
             ZSERVICE_PROVIDER TEXT,
             ZCALLTYPE INTEGER
         );
         -- Outgoing, answered, cellular, resolves to Ada Lovelace.
         INSERT INTO ZCALLRECORD
             (ZADDRESS, ZDATE, ZDURATION, ZORIGINATED, ZANSWERED, ZSERVICE_PROVIDER, ZCALLTYPE)
             VALUES ('+1 (555) 000-1001', {t5}, 125.0, 1, 1, 'com.apple.Telephony', 1);
         -- Incoming, answered, cellular, resolves to Grace Hopper.
         INSERT INTO ZCALLRECORD
             (ZADDRESS, ZDATE, ZDURATION, ZORIGINATED, ZANSWERED, ZSERVICE_PROVIDER, ZCALLTYPE)
             VALUES ('+44 7700 900123', {t4}, 42.0, 0, 1, 'com.apple.Telephony', 1);
         -- A FaceTime call.
         INSERT INTO ZCALLRECORD
             (ZADDRESS, ZDATE, ZDURATION, ZORIGINATED, ZANSWERED, ZSERVICE_PROVIDER, ZCALLTYPE)
             VALUES ('+1 (555) 000-1001', {t3}, 301.0, 1, 1, 'com.apple.FaceTime', 8);
         -- An unanswered (missed) call.
         INSERT INTO ZCALLRECORD
             (ZADDRESS, ZDATE, ZDURATION, ZORIGINATED, ZANSWERED, ZSERVICE_PROVIDER, ZCALLTYPE)
             VALUES ('+15559999999', {t2}, 0.0, 0, 0, 'com.apple.Telephony', 1);
         -- An unknown caller with no address and an unrecognised ZCALLTYPE.
         INSERT INTO ZCALLRECORD
             (ZADDRESS, ZDATE, ZDURATION, ZORIGINATED, ZANSWERED, ZSERVICE_PROVIDER, ZCALLTYPE)
             VALUES (NULL, {t1}, 0.0, 0, 0, 'com.apple.Telephony', 99);",
        t5 = at(5),
        t4 = at(4),
        t3 = at(3),
        t2 = at(2),
        t1 = at(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_call_history_sql_is_valid_and_has_the_expected_shape() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&call_history_sql()).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM ZCALLRECORD", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 5);
    }
}
