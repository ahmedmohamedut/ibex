//! Synthetic `ChatStorage.sqlite` and `ContactsV2.sqlite` for tests.
//!
//! The schema mirrors what a real iOS 27.2 backup was observed to have, and the
//! content covers the cases real data showed to be easy to get wrong: a type-0
//! message that carries a link-preview media item, an undocumented type code,
//! a deleted message, a group with named members, a status session that the
//! default listing must exclude, and a media path absent from the backup.
//!
//! Every name, number and message is invented (AGENTS.md hard rule 2).

use crate::util::time::to_apple_secs;

/// Session primary key of the one-to-one chat.
pub const DIRECT_SESSION: i64 = 1;
/// Session primary key of the group chat.
pub const GROUP_SESSION: i64 = 2;
/// Session primary key of the status feed, excluded from the default list.
pub const STATUS_SESSION: i64 = 3;
/// A session flagged `ZHIDDEN`, which must still be listed: real data showed
/// hidden sessions hold genuine messages.
pub const HIDDEN_SESSION: i64 = 4;

/// Stanza id of a plain text message.
pub const TEXT_ID: &str = "FIXTUREWA0001";
/// A type-0 message that also has a media item: a link preview, which must
/// stay a text message rather than becoming an image.
pub const LINK_PREVIEW_ID: &str = "FIXTUREWA0002";
/// An image message.
pub const IMAGE_ID: &str = "FIXTUREWA0003";
/// An Ogg/Opus voice note.
pub const VOICE_ID: &str = "FIXTUREWA0004";
/// A PDF document.
pub const DOCUMENT_ID: &str = "FIXTUREWA0005";
/// A deleted (revoked) message.
pub const DELETED_ID: &str = "FIXTUREWA0006";
/// A reply to [`TEXT_ID`].
pub const REPLY_ID: &str = "FIXTUREWA0007";
/// A location message.
pub const LOCATION_ID: &str = "FIXTUREWA0008";
/// A message whose `ZMESSAGETYPE` is one of the many undocumented values.
pub const UNKNOWN_TYPE_ID: &str = "FIXTUREWA0009";
/// A message whose media is deliberately missing from the backup.
pub const MISSING_MEDIA_ID: &str = "FIXTUREWA0010";
/// A message in the group chat, from a named member.
pub const GROUP_MESSAGE_ID: &str = "FIXTUREWA0011";
/// An ordinary text message that also carries `ZGROUPEVENTTYPE` 2.
///
/// Real data has 35,467 of these — over half the database — so treating a
/// non-zero group-event type as a system message misclassifies most of a
/// conversation.
pub const GROUP_EVENT_TYPE_TWO_ID: &str = "FIXTUREWA0015";

/// The JIDs the fixture uses. Synthetic numbers in a reserved-looking range.
pub const DIRECT_JID: &str = "15550001001@s.whatsapp.net";
pub const GROUP_JID: &str = "15550001001-1545900921@g.us";
pub const MEMBER_JID: &str = "15550001002@lid";

/// `ZMEDIALOCALPATH` values, as WhatsApp stores them — note there is no
/// `Message/` prefix here; the extractor adds it.
pub const IMAGE_PATH: &str = "Media/15550001001@s.whatsapp.net/a/1/FIXTURE_IMG.png";
pub const VOICE_PATH: &str = "Media/15550001001@s.whatsapp.net/b/2/FIXTURE_VOICE.opus";
pub const DOCUMENT_PATH: &str = "Media/15550001001@s.whatsapp.net/c/3/FIXTURE_DOC.pdf";
pub const PREVIEW_PATH: &str = "Media/15550001001@s.whatsapp.net/d/4/FIXTURE_PREVIEW.png";
/// Referenced by a message but deliberately not written into the backup.
pub const MISSING_PATH: &str = "Media/15550001001@s.whatsapp.net/e/5/FIXTURE_GONE.mp4";

/// The relative path, inside the WhatsApp container, that a media path resolves
/// to once the extractor's prefix is applied.
pub fn stored_path(media_path: &str) -> String {
    format!("Message/{media_path}")
}

/// A minimal valid Ogg/Opus stream, for the voice-note case.
///
/// Real WhatsApp voice notes are `Ogg data, Opus audio, mono 16 kHz`. This is
/// a hand-built Ogg page carrying an OpusHead identification header, which is
/// what makes content sniffing classify it as audio.
pub fn ogg_opus() -> Vec<u8> {
    fn crc32_ogg(bytes: &[u8]) -> u32 {
        // Ogg uses a non-reflected CRC-32 with polynomial 0x04c11db7.
        let mut crc: u32 = 0;
        for &byte in bytes {
            crc ^= (byte as u32) << 24;
            for _ in 0..8 {
                crc = if crc & 0x8000_0000 != 0 {
                    (crc << 1) ^ 0x04c1_1db7
                } else {
                    crc << 1
                };
            }
        }
        crc
    }

    // The OpusHead identification header: magic, version, channels, pre-skip,
    // input sample rate, output gain, channel mapping.
    let mut head = b"OpusHead".to_vec();
    head.push(1); // version
    head.push(1); // mono
    head.extend_from_slice(&312u16.to_le_bytes()); // pre-skip
    head.extend_from_slice(&16_000u32.to_le_bytes()); // input sample rate
    head.extend_from_slice(&0i16.to_le_bytes()); // output gain
    head.push(0); // channel mapping family

    let mut page = Vec::new();
    page.extend_from_slice(b"OggS");
    page.push(0); // stream structure version
    page.push(0x02); // header type: first page of the stream
    page.extend_from_slice(&0u64.to_le_bytes()); // granule position
    page.extend_from_slice(&1u32.to_le_bytes()); // bitstream serial
    page.extend_from_slice(&0u32.to_le_bytes()); // page sequence
    let crc_at = page.len();
    page.extend_from_slice(&0u32.to_le_bytes()); // CRC, filled in below
    page.push(1); // one segment
    page.push(head.len() as u8); // segment length
    page.extend_from_slice(&head);

    let crc = crc32_ogg(&page);
    page[crc_at..crc_at + 4].copy_from_slice(&crc.to_le_bytes());
    page
}

/// SQL creating the `ChatStorage.sqlite` schema and content.
///
/// Columns follow what the real database was observed to have.
pub fn chat_storage_sql() -> String {
    let base = chrono::DateTime::parse_from_rfc3339("2026-11-01T09:00:00Z")
        .expect("a valid literal")
        .with_timezone(&chrono::Utc);
    let at = |minutes: i64| to_apple_secs(base + chrono::Duration::minutes(minutes));

    let mut sql = String::from(
        "CREATE TABLE ZWACHATSESSION (
             Z_PK INTEGER PRIMARY KEY, Z_ENT INTEGER, Z_OPT INTEGER,
             ZARCHIVED INTEGER, ZFLAGS INTEGER, ZHIDDEN INTEGER, ZREMOVED INTEGER,
             ZMESSAGECOUNTER INTEGER, ZSESSIONTYPE INTEGER, ZUNREADCOUNT INTEGER,
             ZGROUPINFO INTEGER, ZLASTMESSAGE INTEGER,
             ZLASTMESSAGEDATE TIMESTAMP, ZCONTACTJID VARCHAR,
             ZLASTMESSAGETEXT VARCHAR, ZPARTNERNAME VARCHAR
         );
         CREATE TABLE ZWAMESSAGE (
             Z_PK INTEGER PRIMARY KEY, Z_ENT INTEGER, Z_OPT INTEGER,
             ZFLAGS INTEGER, ZGROUPEVENTTYPE INTEGER, ZISFROMME INTEGER,
             ZMESSAGESTATUS INTEGER, ZMESSAGETYPE INTEGER, ZSORT INTEGER,
             ZSTARRED INTEGER, ZCHATSESSION INTEGER, ZGROUPMEMBER INTEGER,
             ZMEDIAITEM INTEGER, ZMESSAGEINFO INTEGER, ZPARENTMESSAGE INTEGER,
             ZMESSAGEDATE TIMESTAMP, ZSENTDATE TIMESTAMP,
             ZFROMJID VARCHAR, ZPUSHNAME VARCHAR, ZSTANZAID VARCHAR,
             ZTEXT VARCHAR, ZTOJID VARCHAR
         );
         CREATE TABLE ZWAMEDIAITEM (
             Z_PK INTEGER PRIMARY KEY, Z_ENT INTEGER, Z_OPT INTEGER,
             ZFILESIZE INTEGER, ZMOVIEDURATION FLOAT, ZMESSAGE INTEGER,
             ZLATITUDE FLOAT, ZLONGITUDE FLOAT,
             ZMEDIALOCALPATH VARCHAR, ZTHUMBNAILLOCALPATH VARCHAR,
             ZTITLE VARCHAR, ZVCARDNAME VARCHAR, ZVCARDSTRING VARCHAR
         );
         CREATE TABLE ZWAGROUPMEMBER (
             Z_PK INTEGER PRIMARY KEY, Z_ENT INTEGER, Z_OPT INTEGER,
             ZISACTIVE INTEGER, ZISADMIN INTEGER, ZCHATSESSION INTEGER,
             ZCONTACTNAME VARCHAR, ZFIRSTNAME VARCHAR, ZMEMBERJID VARCHAR
         );
         CREATE TABLE ZWAGROUPINFO (
             Z_PK INTEGER PRIMARY KEY, Z_ENT INTEGER, Z_OPT INTEGER,
             ZCREATIONDATE TIMESTAMP, ZSUBJECT VARCHAR
         );
         CREATE TABLE ZWAMESSAGEINFO (
             Z_PK INTEGER PRIMARY KEY, Z_ENT INTEGER, Z_OPT INTEGER,
             ZMESSAGE INTEGER, ZRECEIPTINFO BLOB
         );
         ",
    );

    // Sessions: a one-to-one chat, a group, and a status feed.
    sql.push_str(&format!(
        "INSERT INTO ZWACHATSESSION
            (Z_PK, ZSESSIONTYPE, ZMESSAGECOUNTER, ZLASTMESSAGEDATE, ZCONTACTJID,
             ZLASTMESSAGETEXT, ZPARTNERNAME, ZHIDDEN, ZREMOVED)
         VALUES
            ({DIRECT_SESSION}, 0, 10, {}, '{DIRECT_JID}',
             'Fixture last message', 'Ada Lovelace', 0, 0),
            ({GROUP_SESSION}, 1, 1, {}, '{GROUP_JID}',
             'Fixture group message', 'Fixture Group', 0, 0),
            ({STATUS_SESSION}, 3, 5, {}, 'status@broadcast',
             'Fixture status', 'Status', 0, 0),
            ({HIDDEN_SESSION}, 0, 2, {}, '15550001004@lid',
             'Fixture hidden chat', NULL, 1, 0);
         INSERT INTO ZWAGROUPINFO (Z_PK, ZSUBJECT) VALUES (1, 'Fixture Group');
         INSERT INTO ZWAGROUPMEMBER
            (Z_PK, ZCHATSESSION, ZMEMBERJID, ZCONTACTNAME, ZISADMIN, ZISACTIVE)
         VALUES
            (1, {GROUP_SESSION}, '{MEMBER_JID}', 'Grace Hopper', 1, 1),
            (2, {GROUP_SESSION}, '{DIRECT_JID}', NULL, 0, 1);\n",
        at(20),
        at(11),
        at(5),
        at(3),
    ));

    // Media items, numbered so messages can reference them.
    sql.push_str(&format!(
        "INSERT INTO ZWAMEDIAITEM (Z_PK, ZMEDIALOCALPATH, ZTITLE, ZFILESIZE) VALUES
            (1, '{IMAGE_PATH}', NULL, 70),
            (2, '{VOICE_PATH}', NULL, 120),
            (3, '{DOCUMENT_PATH}', 'Fixture Document.pdf', 4096),
            (4, '{PREVIEW_PATH}', NULL, 70),
            (5, '{MISSING_PATH}', NULL, 98765);
         INSERT INTO ZWAMEDIAITEM (Z_PK, ZLATITUDE, ZLONGITUDE) VALUES
            (6, 51.50722, -0.12750);\n"
    ));

    // Messages in the one-to-one chat.
    let msg = |pk: i64,
               stanza: &str,
               text: Option<&str>,
               minutes: i64,
               from_me: i64,
               mtype: i64,
               media: Option<i64>,
               parent: Option<i64>| {
        let text_sql = match text {
            Some(t) => format!("'{}'", t.replace('\'', "''")),
            None => "NULL".to_string(),
        };
        format!(
            "INSERT INTO ZWAMESSAGE
                (Z_PK, ZSTANZAID, ZTEXT, ZMESSAGEDATE, ZISFROMME, ZMESSAGETYPE,
                 ZCHATSESSION, ZMEDIAITEM, ZPARENTMESSAGE, ZFROMJID)
             VALUES ({pk}, '{stanza}', {text_sql}, {date}, {from_me}, {mtype},
                     {DIRECT_SESSION}, {media}, {parent}, '{DIRECT_JID}');\n",
            date = at(minutes),
            media = media.map_or("NULL".to_string(), |m| m.to_string()),
            parent = parent.map_or("NULL".to_string(), |p| p.to_string()),
        )
    };

    sql.push_str(&msg(
        1,
        TEXT_ID,
        Some("Fixture plain text"),
        0,
        0,
        0,
        None,
        None,
    ));
    // Type 0 *with* media: a link preview. Must stay a text message.
    sql.push_str(&msg(
        2,
        LINK_PREVIEW_ID,
        Some("Look at https://example.invalid/page"),
        1,
        0,
        0,
        Some(4),
        None,
    ));
    sql.push_str(&msg(3, IMAGE_ID, None, 2, 0, 1, Some(1), None));
    sql.push_str(&msg(4, VOICE_ID, None, 3, 1, 3, Some(2), None));
    sql.push_str(&msg(5, DOCUMENT_ID, None, 4, 0, 8, Some(3), None));
    sql.push_str(&msg(
        6,
        DELETED_ID,
        Some("should not be shown"),
        5,
        0,
        10,
        None,
        None,
    ));
    sql.push_str(&msg(
        7,
        REPLY_ID,
        Some("Fixture reply"),
        6,
        1,
        0,
        None,
        Some(1),
    ));
    sql.push_str(&msg(8, LOCATION_ID, None, 7, 0, 5, Some(6), None));
    // One of the many undocumented type codes seen in real data.
    sql.push_str(&msg(9, UNKNOWN_TYPE_ID, None, 8, 0, 42, None, None));
    sql.push_str(&msg(10, MISSING_MEDIA_ID, None, 9, 0, 2, Some(5), None));

    // A message in the group, from a named member.
    sql.push_str(&format!(
        "INSERT INTO ZWAMESSAGE
            (Z_PK, ZSTANZAID, ZTEXT, ZMESSAGEDATE, ZISFROMME, ZMESSAGETYPE,
             ZCHATSESSION, ZGROUPMEMBER, ZFROMJID, ZPUSHNAME)
         VALUES (11, '{GROUP_MESSAGE_ID}', 'Fixture group message', {}, 0, 0,
                 {GROUP_SESSION}, 1, '{MEMBER_JID}', 'Pushname Ignored');\n",
        at(11)
    ));

    // Two messages in the hidden session, so it is a real conversation.
    sql.push_str(&format!(
        "INSERT INTO ZWAMESSAGE
            (Z_PK, ZSTANZAID, ZTEXT, ZMESSAGEDATE, ZISFROMME, ZMESSAGETYPE, ZCHATSESSION, ZFROMJID)
         VALUES (13, 'FIXTUREWA0013', 'Fixture hidden one', {}, 0, 0, {HIDDEN_SESSION},
                 '15550001004@lid'),
                (14, 'FIXTUREWA0014', 'Fixture hidden two', {}, 1, 0, {HIDDEN_SESSION},
                 '15550001004@lid');\n",
        at(2),
        at(3)
    ));

    // An ordinary message that also has ZGROUPEVENTTYPE = 2, which must stay a
    // text message rather than becoming a system notice.
    sql.push_str(&format!(
        "INSERT INTO ZWAMESSAGE
            (Z_PK, ZSTANZAID, ZTEXT, ZMESSAGEDATE, ZISFROMME, ZMESSAGETYPE,
             ZCHATSESSION, ZGROUPEVENTTYPE, ZFROMJID)
         VALUES (15, '{GROUP_EVENT_TYPE_TWO_ID}', 'Fixture ordinary message', {}, 0, 0,
                 {DIRECT_SESSION}, 2, '{DIRECT_JID}');\n",
        at(10)
    ));

    // A status post, which the default listing must not surface.
    sql.push_str(&format!(
        "INSERT INTO ZWAMESSAGE
            (Z_PK, ZSTANZAID, ZTEXT, ZMESSAGEDATE, ZISFROMME, ZMESSAGETYPE, ZCHATSESSION)
         VALUES (12, 'FIXTUREWA0012', 'Fixture status post', {}, 0, 0, {STATUS_SESSION});\n",
        at(5)
    ));

    sql
}

/// SQL for a small `ContactsV2.sqlite`.
pub fn contacts_sql() -> &'static str {
    "CREATE TABLE ZWAADDRESSBOOKCONTACT (
         Z_PK INTEGER PRIMARY KEY, Z_ENT INTEGER, Z_OPT INTEGER,
         ZABOUTTEXT VARCHAR, ZFULLNAME VARCHAR, ZGIVENNAME VARCHAR,
         ZLASTNAME VARCHAR, ZPHONENUMBER VARCHAR, ZWHATSAPPID VARCHAR,
         ZLID VARCHAR, ZBUSINESSNAME VARCHAR
     );
     -- Resolved by ZWHATSAPPID.
     INSERT INTO ZWAADDRESSBOOKCONTACT (Z_PK, ZFULLNAME, ZWHATSAPPID, ZPHONENUMBER)
     VALUES (1, 'Ada Lovelace', '15550001001@s.whatsapp.net', '+1 555 000 1001');
     -- Resolved by ZLID, the newer linked-identity form a real backup uses.
     INSERT INTO ZWAADDRESSBOOKCONTACT (Z_PK, ZFULLNAME, ZLID)
     VALUES (2, 'Grace Hopper', '15550001002@lid');
     -- A business contact with no personal name.
     INSERT INTO ZWAADDRESSBOOKCONTACT (Z_PK, ZBUSINESSNAME, ZWHATSAPPID)
     VALUES (3, 'Example Dental', '15550001003@s.whatsapp.net');"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ogg_opus_sample_sniffs_as_audio() {
        let bytes = ogg_opus();
        assert_eq!(&bytes[0..4], b"OggS");
        // The identification header has to be in the first page for a decoder
        // (or our sniffer) to recognise the stream.
        assert!(
            bytes.windows(8).any(|w| w == b"OpusHead"),
            "OpusHead missing"
        );

        let sniffed = crate::util::mime::sniff(&bytes, "voice.opus");
        assert_eq!(
            sniffed.preview,
            crate::util::mime::PreviewKind::Audio,
            "got {sniffed:?}"
        );
    }

    #[test]
    fn the_chat_storage_sql_is_valid_and_has_the_expected_shape() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&chat_storage_sql())
            .expect("fixture SQL should run");

        let sessions: i64 = conn
            .query_one("SELECT count(*) FROM ZWACHATSESSION", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sessions, 4);

        // Two conversations plus one status feed.
        let conversations: i64 = conn
            .query_one(
                "SELECT count(*) FROM ZWACHATSESSION WHERE ZSESSIONTYPE IN (0, 1)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(conversations, 3, "two visible plus one hidden");

        let messages: i64 = conn
            .query_one("SELECT count(*) FROM ZWAMESSAGE", [], |r| r.get(0))
            .unwrap();
        assert_eq!(messages, 15);

        // The link-preview case: type 0 with a media item, which real data
        // showed to be the common case rather than an oddity.
        let (mtype, media): (i64, Option<i64>) = conn
            .query_one(
                "SELECT ZMESSAGETYPE, ZMEDIAITEM FROM ZWAMESSAGE WHERE ZSTANZAID = ?1",
                [LINK_PREVIEW_ID],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(mtype, 0);
        assert!(media.is_some());

        // Media paths carry no Message/ prefix; the extractor adds it.
        let path: String = conn
            .query_one(
                "SELECT ZMEDIALOCALPATH FROM ZWAMEDIAITEM WHERE Z_PK = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(path.starts_with("Media/"), "{path}");
        assert_eq!(stored_path(&path), format!("Message/{path}"));
    }

    #[test]
    fn the_contacts_sql_is_valid() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(contacts_sql()).unwrap();
        let n: i64 = conn
            .query_one("SELECT count(*) FROM ZWAADDRESSBOOKCONTACT", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 3);
    }
}
