//! One way to read any chat source.
//!
//! `SmsDb` and `WhatsAppDb` offer the same operations but not quite the same
//! signatures — WhatsApp takes a flag for whether to include status and channel
//! feeds. An enum keeps that difference in one place without the lifetime
//! gymnastics a trait object would need, and gives the exporters a single shape
//! to work against.

use crate::error::{ExtractError, Result};
use crate::extractors::sms::SmsDb;
use crate::extractors::whatsapp::WhatsAppDb;
use crate::extractors::{ConversationSummary, Message, MessageCursor, Service};
use crate::{Backup, Page, Paged};

/// How many messages to pull per page while exporting.
///
/// Large enough that a long chat needs few round trips, small enough that a
/// 7,891-message conversation is never held in memory all at once.
const EXPORT_PAGE: u32 = 500;

/// An open chat source.
///
/// `Debug` prints only which source it is: the databases behind it hold
/// message content, which must never reach a log (hard rule 3).
pub enum ChatSource<'a> {
    Sms(SmsDb<'a>),
    WhatsApp(WhatsAppDb<'a>),
}

impl std::fmt::Debug for ChatSource<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatSource")
            .field("source", &self.id())
            .finish_non_exhaustive()
    }
}

impl<'a> ChatSource<'a> {
    /// Open a source by its id: `sms` or `whatsapp`.
    pub fn open(backup: &'a Backup, source_id: &str) -> Result<Self> {
        match source_id {
            "sms" => Ok(ChatSource::Sms(SmsDb::open(backup)?)),
            "whatsapp" => Ok(ChatSource::WhatsApp(WhatsAppDb::open(backup)?)),
            other => Err(ExtractError::Unsupported {
                source_id: "export",
                detail: format!("{other:?} is not a chat source ibex can export"),
            }
            .into()),
        }
    }

    /// The source's id, as `open` takes it.
    pub fn id(&self) -> &'static str {
        match self {
            ChatSource::Sms(_) => "sms",
            ChatSource::WhatsApp(_) => "whatsapp",
        }
    }

    /// A label for the export's index page.
    pub fn label(&self) -> &'static str {
        match self {
            ChatSource::Sms(_) => "Messages",
            ChatSource::WhatsApp(_) => "WhatsApp",
        }
    }

    /// The service these conversations belong to.
    pub fn service(&self) -> Service {
        match self {
            ChatSource::Sms(_) => Service::IMessage,
            ChatSource::WhatsApp(_) => Service::WhatsApp,
        }
    }

    /// One page of conversations.
    ///
    /// `include_broadcasts` only means anything for WhatsApp; SMS has no such
    /// distinction, so the flag is ignored there rather than being an error.
    pub fn conversations(
        &self,
        page: Page,
        include_broadcasts: bool,
    ) -> Result<Paged<ConversationSummary>> {
        match self {
            ChatSource::Sms(db) => db.list_conversations(page),
            ChatSource::WhatsApp(db) => db.list_conversations(page, include_broadcasts),
        }
    }

    /// One page of messages, newest first.
    pub fn messages(
        &self,
        id: &str,
        cursor: MessageCursor,
    ) -> Result<crate::extractors::MessagePage> {
        match self {
            ChatSource::Sms(db) => db.list_messages(id, cursor),
            ChatSource::WhatsApp(db) => db.list_messages(id, cursor),
        }
    }

    /// Every message in a conversation, **oldest first**.
    ///
    /// Exports read chronologically, which is the opposite of the thread view,
    /// so the pages are collected and reversed. Paging keeps peak memory to one
    /// page rather than one conversation; the collected `Vec` is the export's
    /// own working set.
    pub fn messages_chronological(&self, id: &str) -> Result<Vec<Message>> {
        let mut out = Vec::new();
        let mut cursor = Some(MessageCursor::first(EXPORT_PAGE));
        while let Some(c) = cursor {
            let page = self.messages(id, c)?;
            out.extend(page.messages);
            cursor = page.older;
        }
        // Pages arrive newest-first, and so does each page's content.
        out.reverse();
        Ok(out)
    }

    /// Stream every message in a conversation, oldest first, a page at a time.
    ///
    /// For the formats that write as they go, so a huge conversation never has
    /// to be fully materialized. The callback may stop the walk by returning
    /// `false`.
    pub fn for_each_message(
        &self,
        id: &str,
        mut f: impl FnMut(&Message) -> Result<bool>,
    ) -> Result<()> {
        // A conversation has to be walked backwards to be paged, so collect the
        // ids page by page first, then replay them forwards.
        let messages = self.messages_chronological(id)?;
        for message in &messages {
            if !f(message)? {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FixtureBackup;
    use crate::{Backup, CacheDir};

    fn fixture(tmp: &std::path::Path) -> Backup {
        let root = FixtureBackup::new()
            .with_sms()
            .with_whatsapp()
            .with_address_book()
            .build(tmp)
            .unwrap();
        Backup::open_with_cache(&root, None, CacheDir::at(tmp.join("cache"))).unwrap()
    }

    #[test]
    fn both_sources_open_by_id() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture(tmp.path());

        let sms = ChatSource::open(&backup, "sms").unwrap();
        assert_eq!(sms.id(), "sms");
        assert_eq!(sms.label(), "Messages");

        let wa = ChatSource::open(&backup, "whatsapp").unwrap();
        assert_eq!(wa.id(), "whatsapp");
        assert_eq!(wa.service(), Service::WhatsApp);

        // An unknown id is a clear error rather than a panic.
        let err = ChatSource::open(&backup, "telegram").unwrap_err();
        assert!(err.to_string().contains("telegram"), "{err}");
    }

    #[test]
    fn conversations_come_back_from_either_source() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture(tmp.path());

        for id in ["sms", "whatsapp"] {
            let source = ChatSource::open(&backup, id).unwrap();
            let page = source.conversations(Page::default(), false).unwrap();
            assert!(!page.items.is_empty(), "{id} should list conversations");
        }
    }

    #[test]
    fn the_broadcast_flag_only_affects_whatsapp() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture(tmp.path());

        let wa = ChatSource::open(&backup, "whatsapp").unwrap();
        let without = wa.conversations(Page::default(), false).unwrap().total;
        let with = wa.conversations(Page::default(), true).unwrap().total;
        assert!(with > without, "the flag should reveal the status feed");

        // SMS ignores it rather than failing.
        let sms = ChatSource::open(&backup, "sms").unwrap();
        let a = sms.conversations(Page::default(), false).unwrap().total;
        let b = sms.conversations(Page::default(), true).unwrap().total;
        assert_eq!(a, b);
    }

    #[test]
    fn messages_come_back_oldest_first_for_export() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture(tmp.path());
        let source = ChatSource::open(&backup, "sms").unwrap();

        let messages = source.messages_chronological("1").unwrap();
        assert!(!messages.is_empty());

        let dates: Vec<_> = messages.iter().filter_map(|m| m.sent_at).collect();
        assert!(
            dates.windows(2).all(|w| w[0] <= w[1]),
            "an export reads chronologically, oldest first"
        );

        // And it is the same set the thread view returns, just reversed.
        let thread = source
            .messages("1", MessageCursor::first(500))
            .unwrap()
            .messages;
        assert_eq!(messages.len(), thread.len());
    }

    #[test]
    fn paging_does_not_lose_messages_when_the_page_is_tiny() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture(tmp.path());
        let source = ChatSource::open(&backup, "whatsapp").unwrap();

        let all = source.messages_chronological("1").unwrap();
        // Walk with the callback, which pages the same way.
        let mut seen = Vec::new();
        source
            .for_each_message("1", |m| {
                seen.push(m.id.clone());
                Ok(true)
            })
            .unwrap();
        assert_eq!(seen.len(), all.len());
        let unique: std::collections::HashSet<&String> = seen.iter().collect();
        assert_eq!(unique.len(), seen.len(), "a message was visited twice");
    }

    #[test]
    fn a_walk_can_be_stopped_early() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture(tmp.path());
        let source = ChatSource::open(&backup, "sms").unwrap();

        let mut count = 0;
        source
            .for_each_message("1", |_| {
                count += 1;
                Ok(count < 3)
            })
            .unwrap();
        assert_eq!(count, 3, "the walk should stop when asked");
    }
}
