//! WhatsApp, from `ChatStorage.sqlite` in the shared app-group container.
//!
//! Core Data, so tables are prefixed `ZWA` and columns `Z`. Everything here is
//! designed against a real iOS 27.2 backup (66,407 messages, 1,192 sessions,
//! 18 groups); `docs/DATA_SOURCES.md` records what that inspection found.
//!
//! Two of those findings shape the code:
//!
//! - **`ZMESSAGETYPE` cannot decide the message kind.** The real database holds
//!   38 distinct values, most undocumented, and 27,813 of the 50,410 "text"
//!   (type 0) messages carry a media item — link previews. So the kind comes
//!   from the resolved *file*, with the type code consulted only for what a
//!   file cannot reveal: text, contact, location, system, deleted, sticker.
//! - **Media paths need exactly one prefix.** `Message/` + `ZMEDIALOCALPATH`
//!   resolved 97.5% of rows in the real backup; the raw value resolved none.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::error::{ExtractError, Result};
use crate::extractors::{
    AttachmentRef, Availability, ContactResolver, ConversationSummary, Extractor, Message,
    MessageCursor, MessageKind, MessagePage, Participant, Service,
};
use crate::page::{Page, Paged};
use crate::util::mime::{self, PreviewKind};
use crate::util::time::from_apple;
use crate::{Backup, ManifestEntry};

/// The consumer app's shared container, and WhatsApp Business's.
const DOMAINS: &[(&str, &str)] = &[
    (
        "AppDomainGroup-group.net.whatsapp.WhatsApp.shared",
        "WhatsApp",
    ),
    (
        "AppDomainGroup-group.net.whatsapp.WhatsAppSMB.shared",
        "WhatsApp Business",
    ),
];
const CHAT_DB: &str = "ChatStorage.sqlite";
const CONTACTS_DB: &str = "ContactsV2.sqlite";

/// `ZMEDIALOCALPATH` is stored relative to this.
const MEDIA_PREFIX: &str = "Message/";

/// `ZSESSIONTYPE` values that are conversations rather than broadcast feeds.
const CONVERSATION_SESSION_TYPES: &[i64] = &[0, 1];

/// `ZMESSAGETYPE` values whose meaning a file cannot reveal.
const TYPE_TEXT: i64 = 0;
const TYPE_CONTACT: i64 = 4;
const TYPE_LOCATION: i64 = 5;
const TYPE_SYSTEM: i64 = 6;
const TYPE_DELETED: i64 = 10;
const TYPE_STICKER: i64 = 15;

/// The WhatsApp extractor.
pub struct WhatsApp;

impl Extractor for WhatsApp {
    const ID: &'static str = "whatsapp";

    fn detect(backup: &Backup) -> Availability {
        for (domain, label) in DOMAINS {
            match backup.find(domain, CHAT_DB) {
                Ok(Some(entry)) if backup.has_data(&entry) => {
                    return Availability::Present {
                        version_hint: Some((*label).to_string()),
                    };
                }
                Ok(Some(_)) => {
                    return Availability::Unsupported(format!(
                        "{label}'s ChatStorage.sqlite is in the manifest but its contents are \
                         not in the backup"
                    ));
                }
                _ => {}
            }
        }
        Availability::Absent
    }
}

/// An open `ChatStorage.sqlite`.
pub struct WhatsAppDb<'a> {
    backup: &'a Backup,
    conn: Connection,
    /// Which container this came from, so media resolves in the right domain.
    domain: &'static str,
    /// JID to display name, from `ContactsV2.sqlite`.
    names: HashMap<String, String>,
    /// The iOS address book, as a last resort before showing a bare JID.
    contacts: ContactResolver,
}

impl<'a> WhatsAppDb<'a> {
    /// Open the first WhatsApp container that has data.
    pub fn open(backup: &'a Backup) -> Result<Self> {
        let domain = DOMAINS
            .iter()
            .find(|(domain, _)| {
                backup
                    .find(domain, CHAT_DB)
                    .ok()
                    .flatten()
                    .is_some_and(|entry| backup.has_data(&entry))
            })
            .map(|(domain, _)| *domain)
            .ok_or(ExtractError::SourceAbsent {
                source_id: WhatsApp::ID,
            })?;

        let conn = backup.open_db(domain, CHAT_DB)?;
        let names = Self::load_names(backup, domain);
        let contacts = ContactResolver::open(backup)?;
        Ok(Self {
            backup,
            conn,
            domain,
            names,
            contacts,
        })
    }

    /// Borrow the connection, for structure-only queries in tests.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Which container was opened, for diagnostics.
    pub fn domain(&self) -> &'static str {
        self.domain
    }

    /// Build the JID to name map from `ContactsV2.sqlite`.
    ///
    /// Best effort: a backup without it simply falls back to push names and the
    /// iOS address book.
    fn load_names(backup: &Backup, domain: &str) -> HashMap<String, String> {
        let mut names = HashMap::new();
        let Ok(conn) = backup.open_db(domain, CONTACTS_DB) else {
            return names;
        };
        let columns = table_columns(&conn, "ZWAADDRESSBOOKCONTACT");
        if columns.is_empty() {
            return names;
        }
        let has = |name: &str| columns.iter().any(|c| c == name);

        // A real backup addresses contacts by ZWHATSAPPID and, since the move to
        // linked identities, also by ZLID. Both are indexed to the same name.
        let name_expr = match (has("ZFULLNAME"), has("ZBUSINESSNAME")) {
            (true, true) => {
                "coalesce(nullif(trim(ZFULLNAME), ''), nullif(trim(ZBUSINESSNAME), ''))"
            }
            (true, false) => "nullif(trim(ZFULLNAME), '')",
            (false, true) => "nullif(trim(ZBUSINESSNAME), '')",
            (false, false) => return names,
        };
        let mut keys: Vec<&str> = Vec::new();
        if has("ZWHATSAPPID") {
            keys.push("ZWHATSAPPID");
        }
        if has("ZLID") {
            keys.push("ZLID");
        }
        if has("ZPHONENUMBER") {
            keys.push("ZPHONENUMBER");
        }

        for key in keys {
            let sql = format!(
                "SELECT {key}, {name_expr} FROM ZWAADDRESSBOOKCONTACT \
                 WHERE {key} IS NOT NULL AND {name_expr} IS NOT NULL"
            );
            let Ok(mut stmt) = conn.prepare(&sql) else {
                continue;
            };
            let Ok(rows) = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) else {
                continue;
            };
            for row in rows.flatten() {
                let (id, name) = row;
                // First writer wins, so the order of `keys` sets the priority.
                names.entry(normalize_jid(&id)).or_insert(name);
            }
        }
        names
    }

    /// Conversations, most recently active first.
    ///
    /// `include_broadcasts` adds the status, channel and broadcast sessions,
    /// which are feeds rather than conversations and are hidden by default.
    pub fn list_conversations(
        &self,
        page: Page,
        include_broadcasts: bool,
    ) -> Result<Paged<ConversationSummary>> {
        let filter = self.session_filter(include_broadcasts);

        let total: i64 = self.conn.query_one(
            &format!("SELECT count(*) FROM ZWACHATSESSION s WHERE {filter}"),
            [],
            |r| r.get(0),
        )?;

        let sql = format!(
            "SELECT s.Z_PK, s.ZCONTACTJID, s.ZPARTNERNAME, s.ZSESSIONTYPE,
                    s.ZMESSAGECOUNTER, s.ZLASTMESSAGEDATE, s.ZLASTMESSAGETEXT
             FROM ZWACHATSESSION s
             WHERE {filter}
             ORDER BY s.ZLASTMESSAGEDATE IS NULL, s.ZLASTMESSAGEDATE DESC, s.Z_PK DESC
             LIMIT ?1 OFFSET ?2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([page.sql_limit(), page.sql_offset()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?;

        let mut items = Vec::new();
        for row in rows {
            let (pk, jid, partner, session_type, counter, last_date, last_text) = row?;
            let is_group =
                session_type == Some(1) || jid.as_deref().is_some_and(|j| j.ends_with("@g.us"));

            let participants = if is_group {
                self.group_members(pk)?
            } else {
                jid.as_deref()
                    .map(|j| vec![self.participant(j, None)])
                    .unwrap_or_default()
            };

            // A group's name is ZPARTNERNAME; a 1:1 chat's is the contact's.
            let display_name = partner
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    if is_group {
                        None
                    } else {
                        participants.first().and_then(|p| p.display_name.clone())
                    }
                });

            items.push(ConversationSummary {
                id: pk.to_string(),
                display_name,
                participants,
                is_group,
                message_count: counter.unwrap_or(0).max(0) as u64,
                last_message_at: last_date.and_then(from_apple),
                last_message_preview: last_text
                    .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
                    .filter(|t| !t.is_empty())
                    .map(|t| truncate(&t, 80)),
                service: Service::WhatsApp,
            });
        }
        Ok(Paged::new(items, page, Some(total.max(0) as u64)))
    }

    /// The SQL predicate selecting which sessions to list.
    fn session_filter(&self, include_broadcasts: bool) -> String {
        let mut clauses: Vec<String> = Vec::new();
        if !include_broadcasts {
            let types = CONVERSATION_SESSION_TYPES
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            clauses.push(format!("coalesce(s.ZSESSIONTYPE, 0) IN ({types})"));
        }
        // `ZHIDDEN` and `ZREMOVED` are deliberately *not* filtered on. It is
        // tempting to treat them as "not really conversations", but in the real
        // backup the 123 hidden sessions hold 283 messages between them, none
        // of them empty. Excluding them would silently lose real conversations,
        // which is exactly what a backup explorer must not do.
        if clauses.is_empty() {
            "1 = 1".to_string()
        } else {
            clauses.join(" AND ")
        }
    }

    /// Members of a group chat.
    fn group_members(&self, session_pk: i64) -> Result<Vec<Participant>> {
        if !self.column_exists("ZWAGROUPMEMBER", "ZMEMBERJID") {
            return Ok(Vec::new());
        }
        let name_column = if self.column_exists("ZWAGROUPMEMBER", "ZCONTACTNAME") {
            "ZCONTACTNAME"
        } else {
            "NULL"
        };
        let sql = format!(
            "SELECT ZMEMBERJID, {name_column} FROM ZWAGROUPMEMBER \
             WHERE ZCHATSESSION = ?1 ORDER BY Z_PK"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([session_pk], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (jid, name) = row?;
            if let Some(jid) = jid {
                out.push(self.participant(&jid, name.as_deref()));
            }
        }
        Ok(out)
    }

    /// One page of a conversation, newest first.
    pub fn list_messages(
        &self,
        conversation_id: &str,
        cursor: MessageCursor,
    ) -> Result<MessagePage> {
        let session_pk: i64 = conversation_id
            .parse()
            .map_err(|_| ExtractError::Unsupported {
                source_id: WhatsApp::ID,
                detail: format!("conversation id {conversation_id:?} is not a session key"),
            })?;
        let limit = cursor.clamped_limit();

        // `ZMESSAGEDATE` is Apple seconds as a REAL, so the keyset compares on
        // whole milliseconds, with Z_PK breaking ties. An integer cursor cannot
        // carry a float exactly, and millisecond resolution is far finer than
        // any two messages are apart.
        let has_push_name = self.column_exists("ZWAMESSAGE", "ZPUSHNAME");
        let sql = format!(
            "SELECT m.Z_PK, m.ZSTANZAID, m.ZTEXT, m.ZMESSAGEDATE, m.ZISFROMME,
                    m.ZMESSAGETYPE, m.ZGROUPMEMBER, m.ZFROMJID,
                    {push_name},
                    p.ZSTANZAID, p.Z_PK,
                    mi.ZMEDIALOCALPATH, mi.ZTITLE, mi.ZFILESIZE, mi.ZVCARDNAME,
                    mi.ZLATITUDE, mi.ZLONGITUDE
             FROM ZWAMESSAGE m
             LEFT JOIN ZWAMESSAGE p ON p.Z_PK = m.ZPARENTMESSAGE
             LEFT JOIN ZWAMEDIAITEM mi ON mi.Z_PK = m.ZMEDIAITEM
             WHERE m.ZCHATSESSION = ?1
               AND (?2 IS NULL
                    OR (CAST(m.ZMESSAGEDATE * 1000 AS INTEGER), m.Z_PK) < (?2, ?3))
             ORDER BY m.ZMESSAGEDATE DESC, m.Z_PK DESC
             LIMIT ?4",
            push_name = if has_push_name { "m.ZPUSHNAME" } else { "NULL" },
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params![
                session_pk,
                cursor.before_date,
                cursor.before_rowid.unwrap_or(i64::MAX),
                i64::from(limit)
            ],
            |row| {
                Ok(WaRow {
                    pk: row.get(0)?,
                    stanza_id: row.get(1)?,
                    text: row.get(2)?,
                    date: row.get(3)?,
                    from_me: row.get::<_, Option<i64>>(4)?.unwrap_or(0) != 0,
                    message_type: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                    group_member: row.get(6)?,
                    from_jid: row.get(7)?,
                    push_name: row.get(8)?,
                    parent_stanza: row.get(9)?,
                    parent_pk: row.get(10)?,
                    media_path: row.get(11)?,
                    media_title: row.get(12)?,
                    media_size: row.get(13)?,
                    vcard_name: row.get(14)?,
                    latitude: row.get(15)?,
                    longitude: row.get(16)?,
                })
            },
        )?;

        let mut picked: Vec<WaRow> = Vec::new();
        for row in rows {
            picked.push(row?);
        }
        if picked.is_empty() {
            return Ok(MessagePage {
                messages: Vec::new(),
                older: None,
            });
        }

        let last = picked
            .last()
            .map(|r| ((r.date.unwrap_or(0.0) * 1000.0) as i64, r.pk));
        let full_page = picked.len() as u32 == limit;

        let messages = picked
            .iter()
            .map(|row| self.convert(row, conversation_id))
            .collect();

        Ok(MessagePage {
            messages,
            older: if full_page {
                last.map(|(date, pk)| MessageCursor {
                    before_date: Some(date),
                    before_rowid: Some(pk),
                    limit,
                })
            } else {
                None
            },
        })
    }

    /// Map one row onto the shared message type.
    fn convert(&self, row: &WaRow, conversation_id: &str) -> Message {
        let attachments = self.resolve_media(row);
        let deleted = row.message_type == TYPE_DELETED;

        let body = row
            .text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .or_else(|| {
                // A location message has no text; its coordinates are the
                // content, so they become the body rather than being lost.
                match (row.latitude, row.longitude) {
                    (Some(lat), Some(lon)) if row.message_type == TYPE_LOCATION => {
                        Some(format!("{lat:.5}, {lon:.5}"))
                    }
                    _ => None,
                }
            })
            .or_else(|| row.vcard_name.clone());

        Message {
            // ZSTANZAID is WhatsApp's own id and is stable; Z_PK is the
            // fallback for rows that lack one.
            id: row
                .stanza_id
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("pk:{}", row.pk)),
            conversation_id: conversation_id.to_string(),
            sent_at: row.date.and_then(from_apple),
            from_me: row.from_me,
            sender: if row.from_me {
                None
            } else {
                self.sender_of(row)
            },
            body: if deleted { None } else { body },
            kind: self.kind_of(row, &attachments),
            attachments,
            reply_to: row
                .parent_stanza
                .clone()
                .filter(|s| !s.is_empty())
                .or_else(|| row.parent_pk.map(|pk| format!("pk:{pk}"))),
            // WhatsApp reactions live in an undocumented protobuf blob inside
            // ZWAMESSAGEINFO; see docs/DATA_SOURCES.md. Left empty rather than
            // guessed at.
            reactions: Vec::new(),
            edited: false,
            unsent: deleted,
            service: Service::WhatsApp,
        }
    }

    /// Decide the message kind.
    ///
    /// The resolved file decides wherever it can, because `ZMESSAGETYPE` is
    /// unreliable: the real backup has 38 distinct values and most "text"
    /// messages with a media item are just link previews.
    fn kind_of(&self, row: &WaRow, attachments: &[AttachmentRef]) -> MessageKind {
        match row.message_type {
            TYPE_SYSTEM => return MessageKind::System,
            // A deleted message is *not* a system event: it sat in the
            // conversation and still occupies that place. It carries
            // `unsent: true`, which the UI renders as a dashed in-place bubble
            // — the same treatment M2 gives an unsent iMessage.
            TYPE_DELETED => return MessageKind::Text,
            TYPE_LOCATION => return MessageKind::Location,
            TYPE_CONTACT => return MessageKind::Contact,
            TYPE_STICKER => return MessageKind::Sticker,
            _ => {}
        }
        // `ZGROUPEVENTTYPE` deliberately plays no part here. It looks like a
        // "this is a group event" flag, but in the real backup value 2 covers
        // 35,467 messages — over half the database — of which 27,462 have
        // ordinary text. Treating non-zero as a group event classified most of
        // a user's conversation as grey system notices. `ZMESSAGETYPE` 6 is the
        // only reliable marker.

        // A type-0 message with a media item is a link preview, not an image
        // message: it has text, and the media is decoration.
        let has_text = row.text.as_deref().is_some_and(|t| !t.trim().is_empty());
        if row.message_type == TYPE_TEXT && has_text {
            return MessageKind::Text;
        }

        match attachments.first().and_then(|a| a.preview) {
            Some(PreviewKind::Image | PreviewKind::ImageUnsupported) => MessageKind::Image,
            Some(PreviewKind::Video) => MessageKind::Video,
            // A voice note and a sent audio file are both audio here; WhatsApp
            // distinguishes them by type code, which we cannot trust.
            Some(PreviewKind::Audio) => MessageKind::Voice,
            Some(PreviewKind::Pdf) => MessageKind::Document,
            Some(_) if !attachments.is_empty() => MessageKind::Document,
            _ if has_text => MessageKind::Text,
            // No text, no file we could resolve, and a code we do not know.
            _ if row.message_type == TYPE_TEXT => MessageKind::Text,
            _ => MessageKind::Unknown(row.message_type),
        }
    }

    /// Resolve a row's media against the backup.
    fn resolve_media(&self, row: &WaRow) -> Vec<AttachmentRef> {
        let Some(path) = row.media_path.as_deref().filter(|p| !p.is_empty()) else {
            return Vec::new();
        };
        // One deterministic prefix: measured at 97.5% on a real backup, against
        // 0% for the raw value.
        let relative = format!("{MEDIA_PREFIX}{path}");
        let entry: Option<ManifestEntry> = self
            .backup
            .find(self.domain, &relative)
            .ok()
            .flatten()
            .filter(|e| self.backup.has_data(e));

        let file_name = row
            .media_title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| path.rsplit('/').next().unwrap_or("attachment").to_string());

        // Extension first, then the real bytes when they are there.
        let by_extension = mime_guess::from_path(&file_name)
            .first_raw()
            .or_else(|| mime_guess::from_path(path).first_raw())
            .map(str::to_string);
        let sniffed = entry.as_ref().and_then(|e| self.backup.sniff(e).ok());
        let mime = sniffed
            .as_ref()
            .map(|s| s.mime.clone())
            .or(by_extension.clone());
        let preview = sniffed
            .as_ref()
            .map(|s| s.preview)
            .or_else(|| by_extension.as_deref().map(mime::preview_for));

        vec![AttachmentRef {
            file_id: entry.as_ref().map(|e| e.file_id.clone()),
            file_name,
            mime,
            size: row
                .media_size
                .and_then(|s| u64::try_from(s).ok())
                .filter(|s| *s > 0)
                .or_else(|| entry.as_ref().and_then(|e| e.meta.as_ref()).map(|m| m.size)),
            preview,
            is_sticker: row.message_type == TYPE_STICKER,
        }]
    }

    /// Who sent an incoming message.
    fn sender_of(&self, row: &WaRow) -> Option<Participant> {
        // In a group the sender is a member row; otherwise it is the chat's
        // partner, carried on the message as ZFROMJID.
        if let Some(member_pk) = row.group_member
            && let Ok((Some(jid), name)) = self.conn.query_one(
                "SELECT ZMEMBERJID, ZCONTACTNAME FROM ZWAGROUPMEMBER WHERE Z_PK = ?1",
                [member_pk],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                    ))
                },
            )
        {
            return Some(self.participant(&jid, name.as_deref()));
        }
        let jid = row.from_jid.as_deref()?;
        Some(self.participant(jid, row.push_name.as_deref()))
    }

    /// Build a participant, resolving its name through every source in turn.
    fn participant(&self, jid: &str, fallback_name: Option<&str>) -> Participant {
        let key = normalize_jid(jid);

        // 1. WhatsApp's own address book.
        if let Some(name) = self.names.get(&key) {
            return Participant {
                id: jid.to_string(),
                display_name: Some(name.clone()),
                from_contacts: true,
            };
        }
        // 2. The iOS address book, reusing M2's resolver.
        if let Some(name) = self.contacts.display_name(&key) {
            return Participant {
                id: jid.to_string(),
                display_name: Some(name.to_string()),
                from_contacts: true,
            };
        }
        // 3. A group member's stored name, or the push name the sender chose.
        if let Some(name) = fallback_name.map(str::trim).filter(|n| !n.is_empty()) {
            return Participant {
                id: jid.to_string(),
                display_name: Some(name.to_string()),
                // Not from an address book: a self-chosen or group-local name.
                from_contacts: false,
            };
        }
        Participant::unresolved(jid)
    }

    /// Whether a column exists, so optional ones degrade (hard rule 6).
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
}

/// One row of the message query.
struct WaRow {
    pk: i64,
    stanza_id: Option<String>,
    text: Option<String>,
    date: Option<f64>,
    from_me: bool,
    message_type: i64,
    group_member: Option<i64>,
    from_jid: Option<String>,
    push_name: Option<String>,
    parent_stanza: Option<String>,
    parent_pk: Option<i64>,
    media_path: Option<String>,
    media_title: Option<String>,
    media_size: Option<i64>,
    vcard_name: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
}

/// Reduce a JID to its identifier, for name lookups.
///
/// `447700900123@s.whatsapp.net`, `447700900123@lid` and the bare number all
/// key to the same contact; a group JID keeps its full identifier.
fn normalize_jid(jid: &str) -> String {
    let local = jid.split('@').next().unwrap_or(jid);
    local.trim().to_string()
}

/// Column names of a table, empty when it does not exist.
fn table_columns(conn: &Connection, table: &str) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare("SELECT name FROM pragma_table_info(?1)") else {
        return Vec::new();
    };
    stmt.query_map([table], |row| row.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Truncate on a character boundary.
fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{FixtureBackup, whatsapp as fixture};
    use crate::{Backup, CacheDir};

    fn open_fixture(tmp: &std::path::Path) -> Backup {
        let root = FixtureBackup::new()
            .with_whatsapp()
            .with_address_book()
            .build(tmp)
            .unwrap();
        Backup::open_with_cache(&root, None, CacheDir::at(tmp.join("cache"))).unwrap()
    }

    #[test]
    fn detection_finds_the_consumer_container() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        match WhatsApp::detect(&backup) {
            Availability::Present { version_hint } => {
                assert_eq!(version_hint.as_deref(), Some("WhatsApp"));
            }
            other => panic!("expected Present, got {other:?}"),
        }

        let tmp2 = tempfile::tempdir().unwrap();
        let root = FixtureBackup::new()
            .with_file("HomeDomain", "a.txt", b"x")
            .build(tmp2.path())
            .unwrap();
        let bare =
            Backup::open_with_cache(&root, None, CacheDir::at(tmp2.path().join("cache"))).unwrap();
        assert_eq!(WhatsApp::detect(&bare), Availability::Absent);
        assert!(WhatsAppDb::open(&bare).is_err());
    }

    #[test]
    fn conversations_exclude_status_feeds_unless_asked_for() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = WhatsAppDb::open(&backup).unwrap();

        let page = db.list_conversations(Page::default(), false).unwrap();
        assert_eq!(page.total, Some(3), "status feeds are hidden by default");
        assert!(
            !page
                .items
                .iter()
                .any(|c| c.id == fixture::STATUS_SESSION.to_string()),
            "the status session must not be listed"
        );

        let with_all = db.list_conversations(Page::default(), true).unwrap();
        assert_eq!(
            with_all.total,
            Some(4),
            "the toggle reveals the status feed"
        );
        assert!(
            with_all
                .items
                .iter()
                .any(|c| c.id == fixture::STATUS_SESSION.to_string())
        );
    }

    #[test]
    fn hidden_sessions_are_still_listed() {
        // Real data settled this: the backup's 123 hidden sessions hold 283
        // messages between them, so filtering them out would lose real
        // conversations.
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = WhatsAppDb::open(&backup).unwrap();

        let page = db.list_conversations(Page::default(), false).unwrap();
        let hidden = page
            .items
            .iter()
            .find(|c| c.id == fixture::HIDDEN_SESSION.to_string());
        assert!(
            hidden.is_some(),
            "a hidden session is still a conversation; got {:?}",
            page.items.iter().map(|c| &c.id).collect::<Vec<_>>()
        );
        assert_eq!(hidden.unwrap().message_count, 2);

        // And its messages read.
        let thread = db
            .list_messages(
                &fixture::HIDDEN_SESSION.to_string(),
                MessageCursor::first(10),
            )
            .unwrap();
        assert_eq!(thread.messages.len(), 2);
    }

    #[test]
    fn conversations_carry_names_counts_previews_and_members() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = WhatsAppDb::open(&backup).unwrap();
        let page = db.list_conversations(Page::default(), false).unwrap();

        let direct = page
            .items
            .iter()
            .find(|c| c.id == fixture::DIRECT_SESSION.to_string())
            .expect("the 1:1 chat should be listed");
        assert!(!direct.is_group);
        assert_eq!(direct.display_name.as_deref(), Some("Ada Lovelace"));
        assert_eq!(direct.message_count, 10);
        assert_eq!(
            direct.last_message_preview.as_deref(),
            Some("Fixture last message"),
            "ZLASTMESSAGETEXT gives the preview with no extra query"
        );
        assert_eq!(direct.service, Service::WhatsApp);
        assert!(direct.last_message_at.is_some());

        let group = page
            .items
            .iter()
            .find(|c| c.id == fixture::GROUP_SESSION.to_string())
            .expect("the group should be listed");
        assert!(group.is_group);
        assert_eq!(group.display_name.as_deref(), Some("Fixture Group"));
        assert_eq!(group.participants.len(), 2);
        // A member resolved through ContactsV2's ZLID column.
        assert!(
            group
                .participants
                .iter()
                .any(|p| p.display_name.as_deref() == Some("Grace Hopper")),
            "got {:?}",
            group.participants
        );
    }

    /// The finding that matters most: a type-0 message carrying a media item is
    /// a link preview, not an image. Real data has 27,813 of these.
    #[test]
    fn a_text_message_with_a_link_preview_stays_text() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = WhatsAppDb::open(&backup).unwrap();
        let page = db
            .list_messages(
                &fixture::DIRECT_SESSION.to_string(),
                MessageCursor::first(100),
            )
            .unwrap();

        let preview = page
            .messages
            .iter()
            .find(|m| m.id == fixture::LINK_PREVIEW_ID)
            .expect("the link-preview message should be listed");
        assert_eq!(
            preview.kind,
            MessageKind::Text,
            "a type-0 message with media is a link preview, not an image"
        );
        assert!(preview.body.is_some());
        // The preview image is still attached, so the UI can show it.
        assert_eq!(preview.attachments.len(), 1);
        assert!(preview.attachments[0].is_present());
    }

    /// Found by real data: `ZGROUPEVENTTYPE` 2 covers over half the database,
    /// including ordinary text, so it cannot mark a system message.
    #[test]
    fn a_message_with_group_event_type_two_is_not_a_system_message() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = WhatsAppDb::open(&backup).unwrap();
        let page = db
            .list_messages(
                &fixture::DIRECT_SESSION.to_string(),
                MessageCursor::first(100),
            )
            .unwrap();

        let message = page
            .messages
            .iter()
            .find(|m| m.id == fixture::GROUP_EVENT_TYPE_TWO_ID)
            .expect("the message should be listed");
        assert_eq!(
            message.kind,
            MessageKind::Text,
            "ZGROUPEVENTTYPE must not turn an ordinary message into a system notice"
        );
        assert_eq!(message.body.as_deref(), Some("Fixture ordinary message"));
    }

    #[test]
    fn message_kinds_come_from_the_file_not_the_type_code() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = WhatsAppDb::open(&backup).unwrap();
        let page = db
            .list_messages(
                &fixture::DIRECT_SESSION.to_string(),
                MessageCursor::first(100),
            )
            .unwrap();
        let by_id = |id: &str| {
            page.messages
                .iter()
                .find(|m| m.id == id)
                .unwrap_or_else(|| panic!("{id} missing"))
        };

        assert_eq!(by_id(fixture::TEXT_ID).kind, MessageKind::Text);
        assert_eq!(by_id(fixture::IMAGE_ID).kind, MessageKind::Image);
        // An Ogg/Opus file is a voice note.
        assert_eq!(by_id(fixture::VOICE_ID).kind, MessageKind::Voice);
        assert_eq!(by_id(fixture::DOCUMENT_ID).kind, MessageKind::Document);
        assert_eq!(by_id(fixture::LOCATION_ID).kind, MessageKind::Location);
        // Type 10 is a deleted message: no text, flagged unsent, and kept in
        // the conversation flow rather than shown as a system event.
        let deleted = by_id(fixture::DELETED_ID);
        assert!(deleted.unsent);
        assert_eq!(deleted.body, None, "a deleted message keeps no text");
        assert_eq!(
            deleted.kind,
            MessageKind::Text,
            "a deleted message is not a system event"
        );
        // An undocumented code with no resolvable file is kept, not dropped.
        assert_eq!(
            by_id(fixture::UNKNOWN_TYPE_ID).kind,
            MessageKind::Unknown(42)
        );
    }

    #[test]
    fn media_resolves_through_the_message_prefix_and_missing_media_is_marked() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = WhatsAppDb::open(&backup).unwrap();
        let page = db
            .list_messages(
                &fixture::DIRECT_SESSION.to_string(),
                MessageCursor::first(100),
            )
            .unwrap();
        let by_id = |id: &str| page.messages.iter().find(|m| m.id == id).unwrap();

        // Present: the id must really address the file, which is what ibex://
        // needs to serve it.
        let image = &by_id(fixture::IMAGE_ID).attachments[0];
        assert!(image.is_present());
        assert_eq!(image.mime.as_deref(), Some("image/png"));
        assert!(
            backup
                .get(image.file_id.as_ref().unwrap())
                .unwrap()
                .is_some(),
            "the file id should resolve in the manifest"
        );

        // A document's name comes from ZTITLE rather than the hashed path.
        let doc = &by_id(fixture::DOCUMENT_ID).attachments[0];
        assert_eq!(doc.file_name, "Fixture Document.pdf");

        // Absent: reported as missing, with the size still known.
        let missing = &by_id(fixture::MISSING_MEDIA_ID).attachments[0];
        assert!(!missing.is_present());
        assert_eq!(missing.size, Some(98765));
    }

    #[test]
    fn replies_point_at_their_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = WhatsAppDb::open(&backup).unwrap();
        let page = db
            .list_messages(
                &fixture::DIRECT_SESSION.to_string(),
                MessageCursor::first(100),
            )
            .unwrap();

        let reply = page
            .messages
            .iter()
            .find(|m| m.id == fixture::REPLY_ID)
            .unwrap();
        assert_eq!(reply.reply_to.as_deref(), Some(fixture::TEXT_ID));
    }

    #[test]
    fn group_messages_name_their_sender() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = WhatsAppDb::open(&backup).unwrap();
        let page = db
            .list_messages(
                &fixture::GROUP_SESSION.to_string(),
                MessageCursor::first(100),
            )
            .unwrap();

        assert_eq!(page.messages.len(), 1);
        let message = &page.messages[0];
        let sender = message
            .sender
            .as_ref()
            .expect("a group message has a sender");
        assert_eq!(
            sender.display_name.as_deref(),
            Some("Grace Hopper"),
            "the address book should win over the push name"
        );
        assert!(sender.from_contacts);
    }

    #[test]
    fn outgoing_messages_have_no_sender() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = WhatsAppDb::open(&backup).unwrap();
        let page = db
            .list_messages(
                &fixture::DIRECT_SESSION.to_string(),
                MessageCursor::first(100),
            )
            .unwrap();
        let voice = page
            .messages
            .iter()
            .find(|m| m.id == fixture::VOICE_ID)
            .unwrap();
        assert!(voice.from_me);
        assert!(voice.sender.is_none());
    }

    #[test]
    fn threads_come_back_newest_first_and_page_exactly_once() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = WhatsAppDb::open(&backup).unwrap();
        let id = fixture::DIRECT_SESSION.to_string();

        let all = db.list_messages(&id, MessageCursor::first(500)).unwrap();
        assert!(all.older.is_none());
        let expected = all.messages.len();
        assert_eq!(expected, 11, "the fixture's 1:1 chat has eleven messages");

        let dates: Vec<_> = all.messages.iter().filter_map(|m| m.sent_at).collect();
        assert!(dates.windows(2).all(|w| w[0] >= w[1]), "newest first");

        let mut seen: Vec<String> = Vec::new();
        let mut cursor = Some(MessageCursor::first(3));
        let mut pages = 0;
        while let Some(c) = cursor {
            let page = db.list_messages(&id, c).unwrap();
            seen.extend(page.messages.iter().map(|m| m.id.clone()));
            cursor = page.older;
            pages += 1;
            assert!(pages < 50, "paging should terminate");
        }
        assert_eq!(seen.len(), expected, "paging lost or duplicated messages");
        let unique: std::collections::HashSet<&String> = seen.iter().collect();
        assert_eq!(unique.len(), expected, "a message came back twice");
    }

    #[test]
    fn an_unknown_conversation_is_empty_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = WhatsAppDb::open(&backup).unwrap();

        let page = db.list_messages("9999", MessageCursor::default()).unwrap();
        assert!(page.messages.is_empty());
        assert!(page.older.is_none());
        assert!(
            db.list_messages("nonsense", MessageCursor::default())
                .is_err()
        );
    }

    #[test]
    fn reactions_are_left_empty_rather_than_guessed() {
        // WhatsApp keeps them in an undocumented protobuf inside
        // ZWAMESSAGEINFO. Showing a wrong reaction would be worse than none.
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = WhatsAppDb::open(&backup).unwrap();
        let page = db
            .list_messages(
                &fixture::DIRECT_SESSION.to_string(),
                MessageCursor::first(100),
            )
            .unwrap();
        assert!(page.messages.iter().all(|m| m.reactions.is_empty()));
    }

    #[test]
    fn jids_normalize_to_their_identifier() {
        assert_eq!(normalize_jid("447700900123@s.whatsapp.net"), "447700900123");
        assert_eq!(normalize_jid("447700900123@lid"), "447700900123");
        assert_eq!(normalize_jid("447700900123"), "447700900123");
        // A group JID keeps its compound identifier.
        assert_eq!(
            normalize_jid("15550001234-1545900921@g.us"),
            "15550001234-1545900921"
        );
        assert_eq!(normalize_jid(""), "");
    }

    #[test]
    fn truncation_respects_character_boundaries() {
        assert_eq!(truncate("short", 10), "short");
        let out = truncate("ααααααααααα", 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('…'));
    }
}
