//! SMS, MMS and iMessage, from `HomeDomain` / `Library/SMS/sms.db`.
//!
//! Table parsing goes through the `imessage-database` crate, which brings the
//! two things hardest to get right: decoding `attributedBody` (on iOS 16+
//! `message.text` is often NULL and the real text lives in Apple's typedstream
//! archive) and probing which columns a given iOS version actually has. See
//! `docs/DATA_SOURCES.md` for the decision and its licence consequence.
//!
//! Pagination is ours, because the library's query is hardcoded to
//! `ORDER BY m.date` ascending with no limit. We pick each page's row IDs with
//! our own keyset query, narrow the library's stream to that date window, and
//! keep the rows we asked for.

use std::collections::{BTreeSet, HashMap};

use imessage_database::tables::capabilities::Capabilities;
use imessage_database::tables::messages::Message as ImMessage;
use imessage_database::util::query_context::QueryContext;
use rusqlite::Connection;

use crate::error::{ExtractError, Result};
use crate::extractors::{
    Availability, ContactResolver, ConversationSummary, Extractor, Message, MessageCursor,
    MessageKind, MessagePage, Participant, Reaction, ReactionKind, Service,
};
use crate::page::{Page, Paged};
use crate::util::time::from_apple;
use crate::{Backup, ManifestEntry};

/// Where the database lives.
const DOMAIN: &str = "HomeDomain";
const PATH: &str = "Library/SMS/sms.db";
/// Attachments live in a different domain, under this prefix.
const ATTACHMENT_DOMAIN: &str = "MediaDomain";

/// Characters Apple embeds in body text that should not be displayed: one
/// `U+FFFC` per attachment and `U+FFFD` for app messages.
const PLACEHOLDER_CHARS: [char; 2] = ['\u{FFFC}', '\u{FFFD}'];

/// SQL predicate: this row is a tapback, so it belongs on another message
/// rather than in the thread.
///
/// The ranges matter and are easy to get wrong. `associated_message_type` is
/// **not** simply "non-zero means tapback": a real iOS 27 backup carries
/// ordinary messages with codes 2 and 3, and the library classifies 0, 2 and 3
/// alike as normal. Only 1000 (sticker tapback) and the 2000-2007 / 3000-3007
/// ranges are tapbacks; 4000 is a poll vote, which stays in the thread.
const IS_TAPBACK_SQL: &str = "(coalesce(m.associated_message_type, 0) = 1000 \
     OR coalesce(m.associated_message_type, 0) BETWEEN 2000 AND 2007 \
     OR coalesce(m.associated_message_type, 0) BETWEEN 3000 AND 3007)";

/// Whether an `associated_message_type` marks a tapback. Mirrors
/// [`IS_TAPBACK_SQL`].
fn is_tapback_code(code: i64) -> bool {
    code == 1000 || (2000..=2007).contains(&code) || (3000..=3007).contains(&code)
}

/// The SMS/iMessage extractor.
pub struct Sms;

impl Extractor for Sms {
    const ID: &'static str = "sms";

    fn detect(backup: &Backup) -> Availability {
        match backup.find(DOMAIN, PATH) {
            Ok(Some(entry)) if backup.has_data(&entry) => {
                Availability::Present { version_hint: None }
            }
            Ok(Some(_)) => Availability::Unsupported(
                "sms.db is listed in the manifest but its contents are not in the backup"
                    .to_string(),
            ),
            Ok(None) => Availability::Absent,
            Err(e) => Availability::Unsupported(e.to_string()),
        }
    }
}

/// An open `sms.db`, with its schema probed once.
///
/// Opening is not free — it copies the database out of the backup — so callers
/// that make several queries should hold one of these rather than reopening.
pub struct SmsDb<'a> {
    backup: &'a Backup,
    conn: Connection,
    capabilities: Capabilities,
    contacts: ContactResolver,
}

impl<'a> SmsDb<'a> {
    /// Open the database through a cached working copy.
    pub fn open(backup: &'a Backup) -> Result<Self> {
        if !Sms::detect(backup).is_present() {
            return Err(ExtractError::SourceAbsent { source_id: Sms::ID }.into());
        }
        // Our working copy brings the -wal and -shm siblings along, so recent
        // messages are not missed.
        let conn = backup.open_db(DOMAIN, PATH)?;
        let capabilities =
            Capabilities::determine(&conn).map_err(|e| ExtractError::Unsupported {
                source_id: Sms::ID,
                detail: format!("could not read the sms.db schema: {e}"),
            })?;
        let contacts = ContactResolver::open(backup)?;
        Ok(Self {
            backup,
            conn,
            capabilities,
            contacts,
        })
    }

    /// Borrow the contact resolver, which WhatsApp reuses.
    pub fn contacts(&self) -> &ContactResolver {
        &self.contacts
    }

    /// Conversations, most recently active first.
    pub fn list_conversations(&self, page: Page) -> Result<Paged<ConversationSummary>> {
        let total: i64 = self
            .conn
            .query_one("SELECT count(*) FROM chat", [], |r| r.get(0))?;

        // Counts and last-activity come from one grouped query rather than a
        // query per chat, which would be hundreds of round trips.
        let sql = &format!(
            "
            SELECT c.ROWID, c.display_name, c.chat_identifier, c.style, c.service_name,
                   -- Tapbacks are folded into the messages they target, so
                   -- they must not inflate the count either, or the list and
                   -- the thread disagree about how many messages there are.
                   count(CASE WHEN NOT {IS_TAPBACK} THEN j.message_id END),
                   max(m.date)
            FROM chat c
            LEFT JOIN chat_message_join j ON j.chat_id = c.ROWID
            LEFT JOIN message m ON m.ROWID = j.message_id
            GROUP BY c.ROWID
            ORDER BY max(m.date) IS NULL, max(m.date) DESC, c.ROWID DESC
            LIMIT ?1 OFFSET ?2",
            IS_TAPBACK = IS_TAPBACK_SQL
        );
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([page.sql_limit(), page.sql_offset()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<i64>>(6)?,
            ))
        })?;

        let mut items = Vec::new();
        for row in rows {
            let (rowid, display_name, identifier, style, service, count, last_date) = row?;
            let participants = self.participants_of(rowid)?;
            // style 43 is a group, 45 a one-to-one. Anything else: treat more
            // than one participant as a group.
            let is_group = match style {
                Some(43) => true,
                Some(45) => false,
                _ => participants.len() > 1,
            };
            // A one-to-one chat has no display_name, so fall back to the
            // contact's name, then to the raw identifier.
            let display_name = display_name.filter(|n| !n.trim().is_empty()).or_else(|| {
                if is_group {
                    None
                } else {
                    participants
                        .first()
                        .and_then(|p| p.display_name.clone())
                        .or_else(|| identifier.clone())
                }
            });

            items.push(ConversationSummary {
                id: rowid.to_string(),
                display_name,
                participants,
                is_group,
                message_count: count.max(0) as u64,
                last_message_at: last_date.and_then(from_apple),
                // Filled in below, in one pass, to avoid a query per chat.
                last_message_preview: None,
                service: Service::from_sms_db(service.as_deref()),
            });
        }

        self.fill_previews(&mut items)?;
        Ok(Paged::new(items, page, Some(total.max(0) as u64)))
    }

    /// The handles belonging to one chat.
    fn participants_of(&self, chat_rowid: i64) -> Result<Vec<Participant>> {
        let mut stmt = self.conn.prepare(
            "SELECT h.id FROM chat_handle_join j
             JOIN handle h ON h.ROWID = j.handle_id
             WHERE j.chat_id = ?1
             ORDER BY h.id",
        )?;
        let rows = stmt.query_map([chat_rowid], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        for row in rows {
            let id = row?;
            // The same person can appear twice, once per service.
            if seen.insert(id.clone()) {
                out.push(self.contacts.participant(&id));
            }
        }
        Ok(out)
    }

    /// Add a one-line preview to each conversation, newest message first.
    fn fill_previews(&self, items: &mut [ConversationSummary]) -> Result<()> {
        let sql = format!(
            "SELECT m.text, m.cache_has_attachments, m.associated_message_type
             FROM chat_message_join j
             JOIN message m ON m.ROWID = j.message_id
             WHERE j.chat_id = ?1 AND NOT {IS_TAPBACK_SQL}
             ORDER BY m.date DESC, m.ROWID DESC
             LIMIT 1"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        for item in items.iter_mut() {
            let Ok(chat_id) = item.id.parse::<i64>() else {
                continue;
            };
            let mut rows = stmt.query([chat_id])?;
            if let Some(row) = rows.next()? {
                let text: Option<String> = row.get(0)?;
                let has_attachments: Option<i64> = row.get(1)?;
                let cleaned = text
                    .map(|t| strip_placeholders(&t))
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty());
                item.last_message_preview = match cleaned {
                    Some(text) => Some(truncate(&text, 80)),
                    // No text: say what it was rather than leaving it blank.
                    None if has_attachments.unwrap_or(0) != 0 => Some("Attachment".to_string()),
                    None => None,
                };
            }
        }
        Ok(())
    }

    /// One page of a conversation, newest first.
    pub fn list_messages(
        &self,
        conversation_id: &str,
        cursor: MessageCursor,
    ) -> Result<MessagePage> {
        let chat_id: i64 = conversation_id
            .parse()
            .map_err(|_| ExtractError::Unsupported {
                source_id: Sms::ID,
                detail: format!("conversation id {conversation_id:?} is not a chat rowid"),
            })?;
        let limit = cursor.clamped_limit();

        // Pick exactly which rows this page holds, with a keyset query. Row
        // values keep (date, ROWID) ordered and unique, so paging cannot skip
        // or repeat a message.
        let picks: Vec<(i64, i64)> = {
            let sql = format!(
                "SELECT m.ROWID, m.date
                 FROM chat_message_join j
                 JOIN message m ON m.ROWID = j.message_id
                 WHERE j.chat_id = ?1 AND NOT {IS_TAPBACK_SQL}
                   AND (?2 IS NULL OR (m.date, m.ROWID) < (?2, ?3))
                 ORDER BY m.date DESC, m.ROWID DESC
                 LIMIT ?4"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(
                rusqlite::params![
                    chat_id,
                    cursor.before_date,
                    cursor.before_rowid.unwrap_or(i64::MAX),
                    i64::from(limit)
                ],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )?;
            rows.collect::<rusqlite::Result<_>>()?
        };

        if picks.is_empty() {
            return Ok(MessagePage {
                messages: Vec::new(),
                older: None,
            });
        }

        let wanted: HashMap<i64, i64> = picks.iter().map(|(id, date)| (*id, *date)).collect();

        // `date_retracted` is the column DATA_SOURCES.md documents for an
        // unsent message. The library's `is_fully_unsent` instead reads the
        // `message_summary_info` plist, which describes per-part edits; both
        // are consulted, since either can be the one that is populated.
        let retracted: BTreeSet<i64> = if self.column_exists("message", "date_retracted") {
            let mut stmt = self.conn.prepare(
                "SELECT m.ROWID FROM chat_message_join j
                 JOIN message m ON m.ROWID = j.message_id
                 WHERE j.chat_id = ?1 AND coalesce(m.date_retracted, 0) <> 0",
            )?;
            stmt.query_map([chat_id], |row| row.get::<_, i64>(0))?
                .collect::<rusqlite::Result<_>>()?
        } else {
            BTreeSet::new()
        };
        let min_date = picks.iter().map(|(_, d)| *d).min().unwrap_or(i64::MIN);
        let max_date = picks.iter().map(|(_, d)| *d).max().unwrap_or(i64::MAX);

        // Now let the library read those rows, with all its enrichment. Its
        // query is ordered and unbounded, so the date window keeps the scan
        // small and the row-id set keeps the result exact.
        let mut context = QueryContext::default();
        context.set_selected_chat_ids(BTreeSet::from([chat_id as i32]));
        context.start = Some(min_date);
        context.end = Some(max_date);

        let mut statement = ImMessage::stream_rows(&self.conn, &self.capabilities, &context)
            .map_err(|e| ExtractError::Unsupported {
                source_id: Sms::ID,
                detail: format!("could not query messages: {e}"),
            })?;
        let rows = ImMessage::rows(&mut statement, []).map_err(|e| ExtractError::Unsupported {
            source_id: Sms::ID,
            detail: format!("could not read messages: {e}"),
        })?;

        let mut messages = Vec::new();
        for result in rows {
            let Ok(im) = result else {
                // One unreadable row must not lose the whole page.
                continue;
            };
            if !wanted.contains_key(&i64::from(im.rowid)) {
                continue;
            }
            // Reading the row has already run the library's `parse_body`, so
            // `im.text` holds the decoded attributedBody when the `text`
            // column was NULL — the iOS 16+ norm.
            let mut message = self.convert(&im, conversation_id);
            if retracted.contains(&i64::from(im.rowid)) {
                message.unsent = true;
                message.body = None;
            }
            messages.push(message);
        }

        // Newest first for the thread view.
        messages.sort_by(|a, b| b.sent_at.cmp(&a.sent_at).then(b.id.cmp(&a.id)));

        self.attach_reactions(chat_id, &mut messages)?;
        self.attach_attachments(&mut messages)?;

        // Another page exists only if this one filled up.
        let older = if picks.len() as u32 == limit {
            picks.last().map(|(rowid, date)| MessageCursor {
                before_date: Some(*date),
                before_rowid: Some(*rowid),
                limit,
            })
        } else {
            None
        };

        Ok(MessagePage { messages, older })
    }

    /// Map a library message onto our normalized type.
    fn convert(&self, im: &ImMessage, conversation_id: &str) -> Message {
        // Hard rule 7: the raw Apple timestamp goes through our one helper,
        // not the library's own date conversion.
        let sent_at = from_apple(im.date);

        let sender = if im.is_from_me() {
            None
        } else {
            self.handle_participant(im.handle_id)
        };

        let body = im
            .text
            .as_deref()
            .map(strip_placeholders)
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());

        let unsent = im.is_fully_unsent();
        let kind = if im.is_announcement() || im.item_type != 0 {
            MessageKind::System
        } else {
            // Refined once the attachments are resolved; text until then.
            MessageKind::Text
        };

        Message {
            id: im.guid.clone(),
            conversation_id: conversation_id.to_string(),
            sent_at,
            from_me: im.is_from_me(),
            sender,
            body: if unsent { None } else { body },
            kind,
            attachments: Vec::new(),
            reply_to: im.thread_originator_guid.clone(),
            reactions: Vec::new(),
            edited: im.is_edited(),
            unsent,
            service: Service::from_sms_db(im.service.as_deref()),
        }
    }

    /// Resolve a `handle.ROWID` to a participant.
    fn handle_participant(&self, handle_id: Option<i32>) -> Option<Participant> {
        let handle_id = handle_id.filter(|id| *id > 0)?;
        let id: String = self
            .conn
            .query_one(
                "SELECT id FROM handle WHERE ROWID = ?1",
                [handle_id],
                |row| row.get(0),
            )
            .ok()?;
        Some(self.contacts.participant(&id))
    }

    /// Fold tapbacks into the messages they target.
    ///
    /// Queried per page rather than through the library's `tapbacks_by_target`,
    /// which loads every tapback in the database — fine for a one-shot export,
    /// too much for paging a thread.
    fn attach_reactions(&self, chat_id: i64, messages: &mut [Message]) -> Result<()> {
        if messages.is_empty() || !self.capabilities.associated_message_guids {
            return Ok(());
        }
        // Owned keys: the map outlives the immutable borrow, so reactions can
        // be written back without fighting the borrow checker.
        let by_id: HashMap<String, usize> = messages
            .iter()
            .enumerate()
            .map(|(index, message)| (message.id.clone(), index))
            .collect();

        let has_emoji = self.column_exists("message", "associated_message_emoji");
        let sql = format!(
            "SELECT m.associated_message_guid, m.associated_message_type, m.handle_id,
                    m.is_from_me, m.date, {emoji}
             FROM chat_message_join j
             JOIN message m ON m.ROWID = j.message_id
             WHERE j.chat_id = ?1
               AND m.associated_message_guid IS NOT NULL
               AND {IS_TAPBACK_SQL}",
            emoji = if has_emoji {
                "m.associated_message_emoji"
            } else {
                "NULL"
            }
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([chat_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i32>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;

        for row in rows {
            let (target, kind_code, handle_id, from_me, date, emoji) = row?;
            let Some(target) = target.as_deref().map(clean_target_guid) else {
                continue;
            };
            let Some(&index) = by_id.get(&target) else {
                continue;
            };
            let code = kind_code.unwrap_or(0);
            let reaction = Reaction {
                kind: reaction_kind(code, emoji.as_deref()),
                from: if from_me.unwrap_or(0) != 0 {
                    None
                } else {
                    self.handle_participant(handle_id)
                },
                // 3000-3007 record the removal of the matching 2000-2007.
                removed: (3000..=3007).contains(&code),
                at: date.and_then(from_apple),
            };
            messages[index].reactions.push(reaction);
        }
        Ok(())
    }

    /// Resolve each message's attachments against the backup.
    fn attach_attachments(&self, messages: &mut [Message]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let by_id: HashMap<String, usize> = messages
            .iter()
            .enumerate()
            .map(|(index, message)| (message.id.clone(), index))
            .collect();

        let has_sticker = self.column_exists("attachment", "is_sticker");
        let sql = format!(
            "SELECT m.guid, a.filename, a.mime_type, a.total_bytes, {sticker}
             FROM message m
             JOIN message_attachment_join mj ON mj.message_id = m.ROWID
             JOIN attachment a ON a.ROWID = mj.attachment_id
             WHERE m.guid IN ({placeholders})
             ORDER BY mj.attachment_id",
            sticker = if has_sticker { "a.is_sticker" } else { "0" },
            placeholders = std::iter::repeat_n("?", messages.len())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let ids: Vec<String> = messages.iter().map(|m| m.id.clone()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })?;

        let mut resolved: Vec<(usize, crate::extractors::AttachmentRef)> = Vec::new();
        for row in rows {
            let (guid, filename, mime, size, sticker) = row?;
            let Some(&index) = by_id.get(&guid) else {
                continue;
            };
            resolved.push((
                index,
                self.resolve_attachment(
                    filename.as_deref(),
                    mime.as_deref(),
                    size,
                    sticker.unwrap_or(0) != 0,
                ),
            ));
        }
        for (index, attachment) in resolved {
            messages[index].attachments.push(attachment);
        }

        // Now that the attachments are known, refine the message kind.
        for message in messages.iter_mut() {
            if message.kind == MessageKind::System {
                continue;
            }
            message.kind = kind_for(message);
        }
        Ok(())
    }

    /// Turn an `attachment.filename` into a reference into the backup.
    fn resolve_attachment(
        &self,
        filename: Option<&str>,
        mime: Option<&str>,
        size: Option<i64>,
        is_sticker: bool,
    ) -> crate::extractors::AttachmentRef {
        let raw = filename.unwrap_or_default();
        let name = raw.rsplit('/').next().unwrap_or(raw).to_string();
        // `filename` holds `~/Library/SMS/Attachments/...`; the manifest lists
        // it without the `~/`, in MediaDomain.
        let relative = raw.trim_start_matches('~').trim_start_matches('/');
        let entry: Option<ManifestEntry> = if relative.is_empty() {
            None
        } else {
            self.backup_entry(relative)
        };

        crate::extractors::AttachmentRef {
            file_id: entry.as_ref().map(|e| e.file_id.clone()),
            file_name: if name.is_empty() {
                "attachment".to_string()
            } else {
                name
            },
            mime: mime.map(str::to_string),
            size: size
                .and_then(|s| u64::try_from(s).ok())
                .or_else(|| entry.as_ref().and_then(|e| e.meta.as_ref()).map(|m| m.size)),
            preview: mime.map(crate::util::mime::preview_for),
            is_sticker,
        }
    }

    /// Look a path up in the attachment domain.
    ///
    /// `None` means the attachment is not in the backup, which is common and
    /// normal — the UI shows "Not in backup" rather than an error.
    fn backup_entry(&self, relative: &str) -> Option<ManifestEntry> {
        let entry = self
            .backup
            .find(ATTACHMENT_DOMAIN, relative)
            .ok()
            .flatten()?;
        // Listed but with no bytes captured counts as missing.
        if !self.backup.has_data(&entry) {
            return None;
        }
        Some(entry)
    }

    /// Whether a column exists, so optional ones degrade instead of failing
    /// (hard rule 6).
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

/// Strip an `associated_message_guid` down to the target GUID.
///
/// The column holds `p:<part>/<guid>` or `bp:<guid>`, and sometimes the bare
/// GUID.
fn clean_target_guid(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix("bp:") {
        return rest.to_string();
    }
    match raw.split_once('/') {
        Some((prefix, guid)) if prefix.starts_with("p:") => guid.to_string(),
        _ => raw.to_string(),
    }
}

/// Map `associated_message_type` to a reaction.
fn reaction_kind(code: i64, emoji: Option<&str>) -> ReactionKind {
    if let Some(emoji) = emoji.map(str::trim).filter(|e| !e.is_empty()) {
        return ReactionKind::Emoji(emoji.to_string());
    }
    // 2000-2007 add a tapback, 3000-3007 remove the same one; 1000 is a
    // sticker placed on a message.
    if !is_tapback_code(code) {
        return ReactionKind::Unknown(code);
    }
    match code % 1000 {
        0 if code != 1000 => ReactionKind::Loved,
        1 => ReactionKind::Liked,
        2 => ReactionKind::Disliked,
        3 => ReactionKind::Laughed,
        4 => ReactionKind::Emphasized,
        5 => ReactionKind::Questioned,
        // 2006/2007 are the custom-emoji and sticker tapbacks, whose glyph
        // comes from associated_message_emoji; without it there is no label.
        _ => ReactionKind::Unknown(code),
    }
}

/// Decide a message's kind from its attachments.
fn kind_for(message: &Message) -> MessageKind {
    use crate::util::mime::PreviewKind;
    let Some(first) = message.attachments.first() else {
        return MessageKind::Text;
    };
    if first.is_sticker {
        return MessageKind::Sticker;
    }
    match first.preview {
        Some(PreviewKind::Image | PreviewKind::ImageUnsupported) => MessageKind::Image,
        Some(PreviewKind::Video) => MessageKind::Video,
        Some(PreviewKind::Audio) => MessageKind::Audio,
        Some(PreviewKind::Pdf) => MessageKind::Document,
        _ => MessageKind::Document,
    }
}

/// Remove the attachment and app-message placeholders from body text.
fn strip_placeholders(text: &str) -> String {
    text.chars()
        .filter(|c| !PLACEHOLDER_CHARS.contains(c))
        .collect()
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
    use crate::testutil::{FixtureBackup, sms as fixture};
    use crate::{Backup, CacheDir};

    /// A fixture backup with messages and an address book.
    fn open_fixture(tmp: &std::path::Path) -> Backup {
        let root = FixtureBackup::new()
            .with_sms()
            .with_address_book()
            .build(tmp)
            .unwrap();
        Backup::open_with_cache(&root, None, CacheDir::at(tmp.join("cache"))).unwrap()
    }

    #[test]
    fn detection_reports_presence_and_absence() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        assert!(Sms::detect(&backup).is_present());

        let tmp2 = tempfile::tempdir().unwrap();
        let root = FixtureBackup::new()
            .with_file("HomeDomain", "a.txt", b"x")
            .build(tmp2.path())
            .unwrap();
        let bare =
            Backup::open_with_cache(&root, None, CacheDir::at(tmp2.path().join("cache"))).unwrap();
        assert_eq!(Sms::detect(&bare), Availability::Absent);
        assert!(SmsDb::open(&bare).is_err());
    }

    #[test]
    fn conversations_are_listed_newest_first_with_names_and_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = SmsDb::open(&backup).unwrap();

        let page = db.list_conversations(Page::default()).unwrap();
        assert_eq!(page.total, Some(2));
        assert_eq!(page.items.len(), 2);

        // Chat 1 carries the newest activity — the tapback — so it sorts
        // first. A reaction is activity, which is how Messages behaves.
        let group = page
            .items
            .iter()
            .find(|c| c.is_group)
            .expect("the group chat should be listed");
        assert_eq!(group.display_name.as_deref(), Some("Fixture Book Club"));
        assert_eq!(group.participants.len(), 2);
        assert_eq!(
            page.items[0].id, "1",
            "the chat with the most recent activity sorts first"
        );

        // The one-to-one chat is named after the resolved contact.
        let direct = page.items.iter().find(|c| !c.is_group).unwrap();
        assert!(!direct.is_group);
        assert_eq!(
            direct.display_name.as_deref(),
            Some("Ada Lovelace"),
            "a 1:1 chat should be labelled from the address book"
        );
        assert!(direct.message_count > 0);
        assert!(direct.last_message_at.is_some());
        assert!(direct.last_message_preview.is_some());
    }

    #[test]
    fn the_conversation_count_matches_the_messages_actually_listed() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = SmsDb::open(&backup).unwrap();

        let convs = db.list_conversations(Page::default()).unwrap();
        for conv in &convs.items {
            let thread = db
                .list_messages(&conv.id, MessageCursor::first(500))
                .unwrap();
            assert_eq!(
                conv.message_count as usize,
                thread.messages.len(),
                "chat {} says {} messages but lists {}",
                conv.id,
                conv.message_count,
                thread.messages.len()
            );
        }
    }

    #[test]
    fn a_thread_reads_with_text_senders_and_services() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = SmsDb::open(&backup).unwrap();

        let page = db.list_messages("1", MessageCursor::default()).unwrap();
        assert!(!page.messages.is_empty());
        // Newest first.
        let dates: Vec<_> = page.messages.iter().filter_map(|m| m.sent_at).collect();
        assert!(
            dates.windows(2).all(|w| w[0] >= w[1]),
            "the thread must come back newest first"
        );

        // Tapbacks are folded in, never listed as messages of their own.
        assert!(
            !page.messages.iter().any(|m| m.id.contains("TAPBACK")),
            "a tapback must not appear as a message"
        );

        let by_id = |id: &str| {
            page.messages
                .iter()
                .find(|m| m.id == id)
                .unwrap_or_else(|| panic!("{id} missing from the page"))
        };

        // Incoming message: sender resolved from the address book.
        let incoming = by_id(fixture::REPLY_PARENT_GUID);
        assert!(!incoming.from_me);
        assert_eq!(
            incoming.sender.as_ref().unwrap().display_name.as_deref(),
            Some("Ada Lovelace")
        );
        assert_eq!(incoming.body.as_deref(), Some("Fixture incoming message"));
        assert_eq!(incoming.service, Service::IMessage);

        // Outgoing message has no sender.
        let outgoing = by_id(fixture::TAPBACK_TARGET_GUID);
        assert!(outgoing.from_me);
        assert!(outgoing.sender.is_none());
    }

    /// The reason the library was taken on: text that exists only inside an
    /// `attributedBody` typedstream blob, as on iOS 16+.
    #[test]
    fn text_is_recovered_from_attributedbody_when_the_column_is_null() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = SmsDb::open(&backup).unwrap();

        let page = db.list_messages("1", MessageCursor::default()).unwrap();
        let message = page
            .messages
            .iter()
            .find(|m| m.id == fixture::ATTRIBUTED_ONLY_GUID)
            .expect("the attributedBody-only message should be listed");

        assert_eq!(
            message.body.as_deref(),
            Some(fixture::ATTRIBUTED_ONLY_TEXT),
            "text should have been decoded out of the attributedBody blob"
        );
    }

    #[test]
    fn tapbacks_replies_edits_and_unsent_messages_are_marked() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = SmsDb::open(&backup).unwrap();
        let page = db.list_messages("1", MessageCursor::default()).unwrap();
        let by_id = |id: &str| page.messages.iter().find(|m| m.id == id).unwrap();

        // The tapback landed on its target.
        let target = by_id(fixture::TAPBACK_TARGET_GUID);
        assert_eq!(target.reactions.len(), 1, "expected one tapback");
        assert_eq!(target.reactions[0].kind, ReactionKind::Loved);
        assert!(!target.reactions[0].removed);
        assert_eq!(
            target.reactions[0]
                .from
                .as_ref()
                .unwrap()
                .display_name
                .as_deref(),
            Some("Ada Lovelace")
        );

        // The reply points at its parent.
        let reply = by_id("FIXTURE-MSG-0004");
        assert_eq!(reply.reply_to.as_deref(), Some(fixture::REPLY_PARENT_GUID));

        // Edited and unsent are flagged.
        assert!(by_id(fixture::EDITED_GUID).edited);
        let unsent = by_id(fixture::UNSENT_GUID);
        assert!(unsent.unsent);
        assert_eq!(unsent.body, None, "an unsent message keeps no text");
    }

    #[test]
    fn attachments_resolve_into_the_backup_and_missing_ones_are_marked() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = SmsDb::open(&backup).unwrap();
        let page = db.list_messages("1", MessageCursor::default()).unwrap();

        let message = page
            .messages
            .iter()
            .find(|m| m.id == fixture::ATTACHMENTS_GUID)
            .expect("the attachment message should be listed");
        assert_eq!(message.attachments.len(), 2);

        let present = &message.attachments[0];
        assert!(present.is_present(), "the PNG is in the backup");
        assert_eq!(present.file_name, "FIXTURE_IMG.png");
        assert_eq!(present.mime.as_deref(), Some("image/png"));
        // And the id really addresses it, which is what ibex:// needs.
        let entry = backup.get(present.file_id.as_ref().unwrap()).unwrap();
        assert!(entry.is_some());

        let missing = &message.attachments[1];
        assert!(
            !missing.is_present(),
            "the PDF was deliberately left out of the backup"
        );
        assert_eq!(missing.file_name, "FIXTURE_GONE.pdf");
        assert_eq!(
            missing.size,
            Some(4242),
            "the size is still known from the database"
        );

        // An image attachment makes it an image message.
        assert_eq!(message.kind, MessageKind::Image);
    }

    #[test]
    fn paging_covers_a_thread_exactly_once() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = SmsDb::open(&backup).unwrap();

        let all = db.list_messages("1", MessageCursor::first(500)).unwrap();
        assert!(all.older.is_none(), "one big page covers the thread");
        let expected = all.messages.len();
        assert!(expected >= 7, "the fixture should have several messages");

        // Now page through two at a time and compare.
        let mut seen: Vec<String> = Vec::new();
        let mut cursor = Some(MessageCursor::first(2));
        let mut pages = 0;
        while let Some(c) = cursor {
            let page = db.list_messages("1", c).unwrap();
            seen.extend(page.messages.iter().map(|m| m.id.clone()));
            cursor = page.older;
            pages += 1;
            assert!(pages < 50, "paging should terminate");
        }

        assert_eq!(seen.len(), expected, "paging lost or duplicated messages");
        let unique: std::collections::HashSet<&String> = seen.iter().collect();
        assert_eq!(unique.len(), expected, "a message was returned twice");
    }

    #[test]
    fn an_empty_or_unknown_conversation_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = SmsDb::open(&backup).unwrap();

        let page = db.list_messages("999", MessageCursor::default()).unwrap();
        assert!(page.messages.is_empty());
        assert!(page.older.is_none());

        // A non-numeric id is a caller error, reported as such.
        assert!(
            db.list_messages("not-a-rowid", MessageCursor::default())
                .is_err()
        );
    }

    #[test]
    fn the_group_chat_mixes_services() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = SmsDb::open(&backup).unwrap();

        let page = db.list_messages("2", MessageCursor::default()).unwrap();
        assert_eq!(page.messages.len(), 1);
        assert_eq!(page.messages[0].service, Service::Sms);
        assert_eq!(
            page.messages[0].body.as_deref(),
            Some("Fixture SMS in a group")
        );
    }

    #[test]
    fn target_guids_are_stripped_of_their_part_prefix() {
        assert_eq!(clean_target_guid("p:0/ABC-123"), "ABC-123");
        assert_eq!(clean_target_guid("p:12/ABC-123"), "ABC-123");
        assert_eq!(clean_target_guid("bp:ABC-123"), "ABC-123");
        assert_eq!(clean_target_guid("ABC-123"), "ABC-123");
        // A GUID that itself contains a slash is left alone.
        assert_eq!(clean_target_guid("weird/guid"), "weird/guid");
    }

    /// Found on a real iOS 27.2 backup: ordinary messages carrying
    /// `associated_message_type` 2 and 3. Treating "non-zero" as "tapback"
    /// silently dropped them from the thread.
    #[test]
    fn codes_two_and_three_are_messages_not_tapbacks() {
        assert!(!is_tapback_code(0));
        assert!(!is_tapback_code(2));
        assert!(!is_tapback_code(3));
        // A poll vote stays in the thread too.
        assert!(!is_tapback_code(4000));

        // Only these are tapbacks.
        assert!(is_tapback_code(1000));
        for code in 2000..=2007 {
            assert!(is_tapback_code(code), "{code}");
        }
        for code in 3000..=3007 {
            assert!(is_tapback_code(code), "{code}");
        }
        assert!(!is_tapback_code(1999));
        assert!(!is_tapback_code(2008));
        assert!(!is_tapback_code(3008));
    }

    #[test]
    fn a_message_with_associated_type_two_still_appears_in_the_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = SmsDb::open(&backup).unwrap();

        let page = db.list_messages("1", MessageCursor::first(500)).unwrap();
        let found = page
            .messages
            .iter()
            .find(|m| m.id == fixture::ASSOCIATED_TYPE_TWO_GUID);
        assert!(
            found.is_some(),
            "a code-2 message must stay in the thread, not be treated as a tapback"
        );
        assert_eq!(
            found.unwrap().body.as_deref(),
            Some("Fixture associated type two")
        );

        // And it must not have been folded into its associated message as a
        // reaction either.
        let parent = page
            .messages
            .iter()
            .find(|m| m.id == fixture::REPLY_PARENT_GUID)
            .unwrap();
        assert!(
            parent.reactions.is_empty(),
            "code 2 is not a reaction; got {:?}",
            parent.reactions
        );
    }

    #[test]
    fn reaction_codes_map_to_kinds() {
        assert_eq!(reaction_kind(2000, None), ReactionKind::Loved);
        assert_eq!(reaction_kind(2001, None), ReactionKind::Liked);
        assert_eq!(reaction_kind(2002, None), ReactionKind::Disliked);
        assert_eq!(reaction_kind(2003, None), ReactionKind::Laughed);
        assert_eq!(reaction_kind(2004, None), ReactionKind::Emphasized);
        assert_eq!(reaction_kind(2005, None), ReactionKind::Questioned);
        // Removals keep their kind; `removed` is tracked separately.
        assert_eq!(reaction_kind(3000, None), ReactionKind::Loved);
        assert_eq!(reaction_kind(3005, None), ReactionKind::Questioned);
        // Unrecognised codes are kept rather than dropped.
        assert_eq!(reaction_kind(1234, None), ReactionKind::Unknown(1234));
        // Codes that are not tapbacks at all never become a named reaction.
        assert_eq!(reaction_kind(2, None), ReactionKind::Unknown(2));
        assert_eq!(reaction_kind(3, None), ReactionKind::Unknown(3));
        assert_eq!(reaction_kind(4000, None), ReactionKind::Unknown(4000));
        // iOS 18 custom emoji wins over the code.
        assert_eq!(
            reaction_kind(2006, Some("🎉")),
            ReactionKind::Emoji("🎉".to_string())
        );
        assert_eq!(reaction_kind(2000, Some("  ")), ReactionKind::Loved);
    }

    #[test]
    fn placeholders_are_stripped_from_body_text() {
        assert_eq!(
            strip_placeholders("\u{FFFC}Check out this photo!"),
            "Check out this photo!"
        );
        assert_eq!(strip_placeholders("\u{FFFD}app message"), "app message");
        assert_eq!(strip_placeholders("plain"), "plain");
    }

    #[test]
    fn truncation_respects_character_boundaries() {
        assert_eq!(truncate("short", 10), "short");
        let out = truncate("ααααααααααα", 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('…'));
    }
}
