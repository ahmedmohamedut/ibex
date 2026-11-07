//! Synthetic `sms.db` and `AddressBook.sqlitedb` content for tests.
//!
//! Every name, number and message here is invented (AGENTS.md hard rule 2).
//! The schema follows `docs/DATA_SOURCES.md` and covers the cases that are easy
//! to get wrong: a message whose text lives only in `attributedBody`, tapbacks,
//! replies, an edited message, a fully-unsent one, an attachment missing from
//! the backup, and SMS sitting alongside iMessage.

use crate::util::time::to_apple_nanos;

/// Handles used by the fixture, as `(id, service)`.
pub const HANDLES: &[(&str, &str)] = &[
    ("+15550001001", "iMessage"),
    ("+447700900123", "iMessage"),
    ("+15550001003", "SMS"),
    ("ada@example.com", "iMessage"),
];

/// GUID of the message whose text exists only as an `attributedBody` blob.
pub const ATTRIBUTED_ONLY_GUID: &str = "FIXTURE-ATTRIBUTED-0001";
/// The text that blob decodes to.
pub const ATTRIBUTED_ONLY_TEXT: &str = "Text that lives only in attributedBody";
/// GUID of the message a tapback targets.
pub const TAPBACK_TARGET_GUID: &str = "FIXTURE-MSG-0002";
/// GUID of the message a reply points at.
pub const REPLY_PARENT_GUID: &str = "FIXTURE-MSG-0001";
/// GUID of the edited message.
pub const EDITED_GUID: &str = "FIXTURE-MSG-0005";
/// GUID of the fully-unsent message.
pub const UNSENT_GUID: &str = "FIXTURE-MSG-0006";
/// GUID of the message carrying two attachments, one of them absent.
pub const ATTACHMENTS_GUID: &str = "FIXTURE-MSG-0007";
/// GUID of a message with `associated_message_type` 2.
///
/// Real iOS 27 backups contain these: ordinary messages that are associated
/// with another one but are *not* tapbacks. Treating "non-zero" as "tapback"
/// drops them from the thread, so the fixture carries one.
pub const ASSOCIATED_TYPE_TWO_GUID: &str = "FIXTURE-MSG-0009";

/// Relative path, inside `MediaDomain`, of the attachment that *is* present.
pub const PRESENT_ATTACHMENT: &str = "Library/SMS/Attachments/ab/01/FIXTURE_IMG.png";
/// ...and of the one deliberately left out of the backup.
pub const MISSING_ATTACHMENT: &str = "Library/SMS/Attachments/cd/02/FIXTURE_GONE.pdf";

/// Build an `attributedBody` blob that the parser decodes to `text`.
///
/// Mirrors the real layout closely enough for the library's `streamtyped`
/// reader: it looks for `[0x01, 0x2b]`, then takes everything up to
/// `[0x86, 0x84]`, dropping one length byte from the front.
pub fn attributed_body(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\x04\x0bstreamtyped");
    out.extend_from_slice(&[0x81, 0xe8, 0x03, 0x84, 0x01, 0x40, 0x84, 0x84, 0x84]);
    out.extend_from_slice(
        b"\x12NSAttributedString\x00\x84\x84\x08NSObject\x00\x85\x92\x84\x84\x84",
    );
    out.extend_from_slice(b"\x0fNSMutableString\x01\x84\x84\x08NSString\x01\x95\x84");
    // The start pattern, then a length byte the reader discards, then the text.
    out.extend_from_slice(&[0x01, 0x2b]);
    let len = u8::try_from(text.len()).unwrap_or(0x7f);
    out.push(len.min(0x7f));
    out.extend_from_slice(text.as_bytes());
    // The end pattern, plus trailing bytes: the reader requires room after it.
    out.extend_from_slice(&[0x86, 0x84]);
    out.extend_from_slice(b"\x02iI\x00\x84\x84\x0cNSDictionary");
    out
}

/// SQL creating the `sms.db` schema and the fixture's content.
///
/// Columns follow `docs/DATA_SOURCES.md`; the extras the library probes for
/// (`thread_originator_guid`, `date_edited`, `associated_message_emoji`, ...)
/// are present so the schema looks like a current iOS.
pub fn setup_sql() -> String {
    // Fixed dates, so fixtures are reproducible: 2026-11-01T10:00:00Z onwards.
    let base = chrono::DateTime::parse_from_rfc3339("2026-11-01T10:00:00Z")
        .expect("a valid literal")
        .with_timezone(&chrono::Utc);
    let at = |minutes: i64| to_apple_nanos(base + chrono::Duration::minutes(minutes));

    let mut sql = String::from(
        "CREATE TABLE handle (
             ROWID INTEGER PRIMARY KEY AUTOINCREMENT,
             id TEXT NOT NULL,
             country TEXT,
             service TEXT NOT NULL,
             uncanonicalized_id TEXT,
             person_centric_id TEXT
         );
         CREATE TABLE chat (
             ROWID INTEGER PRIMARY KEY AUTOINCREMENT,
             guid TEXT NOT NULL,
             style INTEGER,
             state INTEGER,
             account_id TEXT,
             properties BLOB,
             chat_identifier TEXT,
             service_name TEXT,
             room_name TEXT,
             account_login TEXT,
             is_archived INTEGER DEFAULT 0,
             last_addressed_handle TEXT,
             display_name TEXT,
             group_id TEXT,
             is_filtered INTEGER DEFAULT 0,
             successful_query INTEGER,
             engram_id TEXT,
             server_change_token TEXT,
             ck_sync_state INTEGER DEFAULT 0
         );
         CREATE TABLE message (
             ROWID INTEGER PRIMARY KEY AUTOINCREMENT,
             guid TEXT NOT NULL,
             text TEXT,
             replace INTEGER DEFAULT 0,
             service_center TEXT,
             handle_id INTEGER DEFAULT 0,
             subject TEXT,
             country TEXT,
             attributedBody BLOB,
             version INTEGER DEFAULT 0,
             type INTEGER DEFAULT 0,
             service TEXT,
             account TEXT,
             account_guid TEXT,
             error INTEGER DEFAULT 0,
             date INTEGER,
             date_read INTEGER,
             date_delivered INTEGER,
             is_delivered INTEGER DEFAULT 0,
             is_finished INTEGER DEFAULT 0,
             is_from_me INTEGER DEFAULT 0,
             is_read INTEGER DEFAULT 0,
             is_sent INTEGER DEFAULT 0,
             item_type INTEGER DEFAULT 0,
             other_handle INTEGER DEFAULT 0,
             group_title TEXT,
             group_action_type INTEGER DEFAULT 0,
             share_status INTEGER DEFAULT 0,
             share_direction INTEGER DEFAULT 0,
             is_expirable INTEGER DEFAULT 0,
             expire_state INTEGER DEFAULT 0,
             message_action_type INTEGER DEFAULT 0,
             associated_message_guid TEXT,
             associated_message_type INTEGER DEFAULT 0,
             balloon_bundle_id TEXT,
             payload_data BLOB,
             expressive_send_style_id TEXT,
             associated_message_range_location INTEGER DEFAULT 0,
             associated_message_range_length INTEGER DEFAULT 0,
             time_expressive_send_played INTEGER,
             message_summary_info BLOB,
             thread_originator_guid TEXT,
             thread_originator_part TEXT,
             date_edited INTEGER DEFAULT 0,
             date_retracted INTEGER DEFAULT 0,
             associated_message_emoji TEXT,
             cache_has_attachments INTEGER DEFAULT 0,
             num_replies INTEGER DEFAULT 0,
             was_deduplicated INTEGER DEFAULT 0
         );
         CREATE TABLE attachment (
             ROWID INTEGER PRIMARY KEY AUTOINCREMENT,
             guid TEXT NOT NULL,
             created_date INTEGER DEFAULT 0,
             filename TEXT,
             uti TEXT,
             mime_type TEXT,
             transfer_state INTEGER DEFAULT 0,
             is_outgoing INTEGER DEFAULT 0,
             total_bytes INTEGER DEFAULT 0,
             is_sticker INTEGER DEFAULT 0,
             hide_attachment INTEGER DEFAULT 0,
             emoji_image_short_description TEXT
         );
         CREATE TABLE chat_message_join (
             chat_id INTEGER, message_id INTEGER, message_date INTEGER DEFAULT 0,
             PRIMARY KEY (chat_id, message_id)
         );
         CREATE TABLE chat_handle_join (
             chat_id INTEGER, handle_id INTEGER,
             UNIQUE (chat_id, handle_id)
         );
         CREATE TABLE message_attachment_join (
             message_id INTEGER, attachment_id INTEGER,
             PRIMARY KEY (message_id, attachment_id)
         );
         CREATE TABLE deleted_messages (ROWID INTEGER PRIMARY KEY AUTOINCREMENT, guid TEXT NOT NULL);
         ",
    );

    // Handles.
    for (id, service) in HANDLES {
        sql.push_str(&format!(
            "INSERT INTO handle (id, service) VALUES ('{id}', '{service}');\n"
        ));
    }

    // Two chats: a 1:1 with handle 1 and a named group with handles 1 and 2.
    sql.push_str(
        "INSERT INTO chat (guid, style, chat_identifier, service_name, display_name)
         VALUES ('iMessage;-;+15550001001', 45, '+15550001001', 'iMessage', NULL);
         INSERT INTO chat (guid, style, chat_identifier, service_name, display_name)
         VALUES ('iMessage;+;chat999', 43, 'chat999', 'iMessage', 'Fixture Book Club');
         INSERT INTO chat_handle_join (chat_id, handle_id) VALUES (1, 1), (2, 1), (2, 2);\n",
    );

    // Message rows. `m` pushes one and joins it to a chat.
    let mut m = |guid: &str,
                 chat: i64,
                 text: Option<&str>,
                 handle: i64,
                 from_me: i64,
                 date: i64,
                 extra: &str| {
        let text_sql = match text {
            Some(t) => format!("'{}'", t.replace('\'', "''")),
            None => "NULL".to_string(),
        };
        sql.push_str(&format!(
            "INSERT INTO message (guid, text, handle_id, is_from_me, service, date{extra_cols})
             VALUES ('{guid}', {text_sql}, {handle}, {from_me}, 'iMessage', {date}{extra_vals});\n\
             INSERT INTO chat_message_join (chat_id, message_id, message_date)
             VALUES ({chat}, (SELECT ROWID FROM message WHERE guid = '{guid}'), {date});\n",
            extra_cols = if extra.is_empty() {
                ""
            } else {
                extra.split('|').next().unwrap_or("")
            },
            extra_vals = if extra.is_empty() {
                ""
            } else {
                extra.split('|').nth(1).unwrap_or("")
            },
        ));
    };

    // A plain incoming message, and our reply to it.
    m(
        REPLY_PARENT_GUID,
        1,
        Some("Fixture incoming message"),
        1,
        0,
        at(0),
        "",
    );
    m(
        TAPBACK_TARGET_GUID,
        1,
        Some("Fixture outgoing message"),
        0,
        1,
        at(1),
        "",
    );
    // Text only in attributedBody, which is the iOS 16+ norm.
    m(ATTRIBUTED_ONLY_GUID, 1, None, 1, 0, at(2), "");
    // A threaded reply to the first message.
    m(
        "FIXTURE-MSG-0004",
        1,
        Some("Fixture threaded reply"),
        1,
        0,
        at(3),
        ", thread_originator_guid, thread_originator_part|, 'FIXTURE-MSG-0001', '0:0:0'",
    );
    // An edited message.
    m(
        EDITED_GUID,
        1,
        Some("Fixture edited message"),
        0,
        1,
        at(4),
        ", date_edited|, 1",
    );
    // A fully-unsent message: iOS keeps the row with the text removed.
    m(UNSENT_GUID, 1, None, 0, 1, at(5), ", date_retracted|, 1");
    // A message with two attachments.
    m(
        ATTACHMENTS_GUID,
        1,
        Some("Fixture message with attachments"),
        1,
        0,
        at(6),
        ", cache_has_attachments|, 1",
    );
    // An SMS in the group chat, so both services appear.
    sql.push_str(&format!(
        "INSERT INTO message (guid, text, handle_id, is_from_me, service, date)
         VALUES ('FIXTURE-MSG-0008', 'Fixture SMS in a group', 3, 0, 'SMS', {});
         INSERT INTO chat_message_join (chat_id, message_id, message_date)
         VALUES (2, (SELECT ROWID FROM message WHERE guid = 'FIXTURE-MSG-0008'), {});\n",
        at(7),
        at(7)
    ));

    // A message with associated_message_type 2: associated with another
    // message, but still a message in its own right.
    sql.push_str(&format!(
        "INSERT INTO message (guid, text, handle_id, is_from_me, service, date,
                              associated_message_guid, associated_message_type)
         VALUES ('{ASSOCIATED_TYPE_TWO_GUID}', 'Fixture associated type two', 1, 0,
                 'iMessage', {}, 'p:0/{REPLY_PARENT_GUID}', 2);
         INSERT INTO chat_message_join (chat_id, message_id, message_date)
         VALUES (1, (SELECT ROWID FROM message WHERE guid = '{ASSOCIATED_TYPE_TWO_GUID}'), {});\n",
        at(9),
        at(9)
    ));

    // The attributedBody blob, written as a hex literal.
    sql.push_str(&format!(
        "UPDATE message SET attributedBody = x'{}' WHERE guid = '{ATTRIBUTED_ONLY_GUID}';\n",
        hex::encode(attributed_body(ATTRIBUTED_ONLY_TEXT))
    ));

    // A tapback: a message in its own right, which must *not* be listed as one.
    // 2000 is "loved"; 3000 would be its removal.
    sql.push_str(&format!(
        "INSERT INTO message (guid, text, handle_id, is_from_me, service, date,
                              associated_message_guid, associated_message_type)
         VALUES ('FIXTURE-TAPBACK-0001', NULL, 1, 0, 'iMessage', {}, 'p:0/{TAPBACK_TARGET_GUID}', 2000);
         INSERT INTO chat_message_join (chat_id, message_id, message_date)
         VALUES (1, (SELECT ROWID FROM message WHERE guid = 'FIXTURE-TAPBACK-0001'), {});\n",
        at(8),
        at(8)
    ));

    // Attachments: the first is in the backup, the second deliberately is not.
    sql.push_str(&format!(
        "INSERT INTO attachment (guid, filename, mime_type, uti, total_bytes)
         VALUES ('FIXTURE-ATT-0001', '~/{PRESENT_ATTACHMENT}', 'image/png', 'public.png', 70);
         INSERT INTO attachment (guid, filename, mime_type, uti, total_bytes)
         VALUES ('FIXTURE-ATT-0002', '~/{MISSING_ATTACHMENT}', 'application/pdf', 'com.adobe.pdf', 4242);
         INSERT INTO message_attachment_join (message_id, attachment_id)
         VALUES ((SELECT ROWID FROM message WHERE guid = '{ATTACHMENTS_GUID}'), 1),
                ((SELECT ROWID FROM message WHERE guid = '{ATTACHMENTS_GUID}'), 2);\n"
    ));

    sql
}

/// SQL for a small `AddressBook.sqlitedb`.
pub fn address_book_sql() -> &'static str {
    "CREATE TABLE ABPerson (
         ROWID INTEGER PRIMARY KEY AUTOINCREMENT,
         First TEXT, Last TEXT, Middle TEXT, Nickname TEXT, Organization TEXT,
         Department TEXT, Note TEXT, Birthday TEXT, JobTitle TEXT
     );
     CREATE TABLE ABMultiValue (
         UID INTEGER PRIMARY KEY AUTOINCREMENT,
         record_id INTEGER, property INTEGER, identifier INTEGER, label INTEGER, value TEXT
     );
     INSERT INTO ABPerson (First, Last) VALUES ('Ada', 'Lovelace');
     INSERT INTO ABPerson (First, Last) VALUES ('Grace', 'Hopper');
     INSERT INTO ABPerson (Organization) VALUES ('Example Dental');
     -- property 3 = phone, 4 = email
     INSERT INTO ABMultiValue (record_id, property, value) VALUES (1, 3, '+1 (555) 000-1001');
     INSERT INTO ABMultiValue (record_id, property, value) VALUES (1, 4, 'Ada@Example.com');
     INSERT INTO ABMultiValue (record_id, property, value) VALUES (2, 3, '+44 7700 900123');
     INSERT INTO ABMultiValue (record_id, property, value) VALUES (3, 3, '+15550001003');"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_attributed_body_blob_decodes_to_its_text() {
        // Checked against the library's own reader, so the fixture cannot
        // claim to exercise a path it actually fails on.
        let blob = attributed_body(ATTRIBUTED_ONLY_TEXT);
        let decoded = imessage_database::util::streamtyped::parse(blob)
            .expect("the fixture blob should parse");
        assert_eq!(decoded, ATTRIBUTED_ONLY_TEXT);
    }

    #[test]
    fn the_blob_survives_multi_byte_text() {
        let text = "caf\u{e9} and \u{1f600}";
        let decoded = imessage_database::util::streamtyped::parse(attributed_body(text)).unwrap();
        assert_eq!(decoded, text);
    }

    #[test]
    fn the_setup_sql_is_valid_and_builds_the_expected_shape() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&setup_sql())
            .expect("fixture SQL should run");

        let chats: i64 = conn
            .query_one("SELECT count(*) FROM chat", [], |r| r.get(0))
            .unwrap();
        assert_eq!(chats, 2);

        // Nine message rows, one of which is a tapback.
        let messages: i64 = conn
            .query_one("SELECT count(*) FROM message", [], |r| r.get(0))
            .unwrap();
        assert_eq!(messages, 10);
        let tapbacks: i64 = conn
            .query_one(
                "SELECT count(*) FROM message WHERE associated_message_type >= 2000",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tapbacks, 1);

        // Every message is joined to a chat, or it would be invisible.
        let orphans: i64 = conn
            .query_one(
                "SELECT count(*) FROM message m
                 WHERE NOT EXISTS (SELECT 1 FROM chat_message_join j WHERE j.message_id = m.ROWID)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0);

        // The attributedBody-only row really has no text.
        let (text, body_len): (Option<String>, i64) = conn
            .query_one(
                "SELECT text, length(attributedBody) FROM message WHERE guid = ?1",
                [ATTRIBUTED_ONLY_GUID],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(text, None);
        assert!(body_len > 0);
    }

    #[test]
    fn the_address_book_sql_is_valid() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(address_book_sql()).unwrap();
        let people: i64 = conn
            .query_one("SELECT count(*) FROM ABPerson", [], |r| r.get(0))
            .unwrap();
        assert_eq!(people, 3);
    }
}
