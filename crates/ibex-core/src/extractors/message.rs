//! The normalized message types every chat source maps onto.
//!
//! SMS/iMessage (M2) and WhatsApp (M3) produce these, and the exporters (M4)
//! and the UI consume them, so a bubble or an HTML export does not need to know
//! which app a message came from. See `docs/ARCHITECTURE.md`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::util::mime::PreviewKind;

/// Which messaging service a conversation or message belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Service {
    Sms,
    IMessage,
    /// RCS, which iOS 18 writes into the same database.
    Rcs,
    WhatsApp,
    Other,
}

impl Service {
    /// Map the `message.service` text, which is `SMS`, `iMessage` or `RCS`.
    pub fn from_sms_db(value: Option<&str>) -> Self {
        match value.map(str::to_ascii_lowercase).as_deref() {
            Some("imessage") => Service::IMessage,
            Some("sms") => Service::Sms,
            Some("rcs") => Service::Rcs,
            _ => Service::Other,
        }
    }
}

/// What a message carries, after normalizing each source's own type codes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MessageKind {
    Text,
    Image,
    Video,
    Audio,
    /// A recorded voice message, as distinct from an audio file.
    Voice,
    Document,
    Location,
    Contact,
    Sticker,
    /// A group rename, a join or leave, a call record: shown centred and muted.
    System,
    /// A code the source used that we do not recognise. Kept so the UI can say
    /// so rather than silently dropping the message.
    Unknown(i64),
}

/// A person in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Participant {
    /// Phone number or email as the database stores it.
    pub id: String,
    /// Resolved from the address book, when it could be.
    pub display_name: Option<String>,
    /// False when `display_name` is just a formatted version of `id`.
    pub from_contacts: bool,
}

impl Participant {
    /// An unresolved participant, identified only by number or email.
    pub fn unresolved(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            display_name: None,
            from_contacts: false,
        }
    }

    /// The best label available.
    pub fn label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.id)
    }
}

/// A tapback or reaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ReactionKind {
    Loved,
    Liked,
    Disliked,
    Laughed,
    Emphasized,
    Questioned,
    /// iOS 18 lets any emoji be a tapback.
    Emoji(String),
    Unknown(i64),
}

/// One reaction on a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Reaction {
    pub kind: ReactionKind,
    pub from: Option<Participant>,
    /// True when this records the *removal* of a reaction. Kept rather than
    /// dropped so a thread can be reconstructed faithfully; the UI hides them.
    pub removed: bool,
    pub at: Option<DateTime<Utc>>,
}

/// An attachment, resolved against the backup where possible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AttachmentRef {
    /// `None` when the file is not in the backup — common and normal, not an
    /// error. The UI shows "Not in backup".
    pub file_id: Option<String>,
    pub file_name: String,
    pub mime: Option<String>,
    pub size: Option<u64>,
    /// How to show it, when we could look at the bytes.
    pub preview: Option<PreviewKind>,
    /// True for a sticker rather than a sent file.
    pub is_sticker: bool,
}

impl AttachmentRef {
    /// Whether the bytes are available to display or export.
    pub fn is_present(&self) -> bool {
        self.file_id.is_some()
    }
}

/// One message, normalized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Message {
    /// Stable within a backup. For `sms.db` this is the message GUID.
    pub id: String,
    pub conversation_id: String,
    /// `None` when the row records no date, which does happen.
    pub sent_at: Option<DateTime<Utc>>,
    pub from_me: bool,
    /// `None` for our own messages and for system events.
    pub sender: Option<Participant>,
    pub body: Option<String>,
    pub kind: MessageKind,
    pub attachments: Vec<AttachmentRef>,
    /// The `id` of the message this replies to.
    pub reply_to: Option<String>,
    pub reactions: Vec<Reaction>,
    pub edited: bool,
    /// Sent then retracted: iOS keeps the row with its text removed.
    pub unsent: bool,
    pub service: Service,
}

impl Message {
    /// A one-line preview for a conversation list, with newlines flattened.
    ///
    /// Falls back to describing the attachment when there is no text, so a
    /// photo-only message does not look blank.
    pub fn preview(&self, max_chars: usize) -> Option<String> {
        if self.unsent {
            return Some("Message unsent".to_string());
        }
        let text = self
            .body
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let text = match text {
            Some(text) => text.split_whitespace().collect::<Vec<_>>().join(" "),
            None => {
                // No text: describe the attachment instead, so a photo-only
                // message does not look blank in the list.
                let first = self.attachments.first()?;
                match self.kind {
                    MessageKind::Image => "Photo".to_string(),
                    MessageKind::Video => "Video".to_string(),
                    MessageKind::Voice => "Voice message".to_string(),
                    MessageKind::Audio => "Audio".to_string(),
                    MessageKind::Sticker => "Sticker".to_string(),
                    _ => first.file_name.clone(),
                }
            }
        };
        Some(truncate_chars(&text, max_chars))
    }
}

/// Truncate on a character boundary, appending an ellipsis.
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// A conversation, for the list pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConversationSummary {
    pub id: String,
    /// A group's name, or a contact's name when it resolved.
    pub display_name: Option<String>,
    pub participants: Vec<Participant>,
    pub is_group: bool,
    pub message_count: u64,
    pub last_message_at: Option<DateTime<Utc>>,
    pub last_message_preview: Option<String>,
    pub service: Service,
}

impl ConversationSummary {
    /// The best label for this conversation.
    pub fn label(&self) -> String {
        if let Some(name) = self
            .display_name
            .as_deref()
            .filter(|n| !n.trim().is_empty())
        {
            return name.to_string();
        }
        if self.participants.is_empty() {
            return "Unknown".to_string();
        }
        // A group with no name is listed by its members, as Messages does.
        let names: Vec<&str> = self.participants.iter().map(Participant::label).collect();
        names.join(", ")
    }

    /// Initials for an avatar placeholder.
    pub fn initials(&self) -> String {
        let label = self.label();
        let mut initials = String::new();
        for word in label.split_whitespace().take(2) {
            if let Some(c) = word.chars().find(|c| c.is_alphanumeric()) {
                initials.extend(c.to_uppercase());
            }
        }
        if initials.is_empty() {
            initials.push('?');
        }
        initials
    }
}

/// A keyset cursor into a conversation, newest-first.
///
/// Not an offset: a thread can hold tens of thousands of messages, where
/// `OFFSET` gets slower the deeper you page and can skip or repeat rows if
/// anything shifts. `(date, rowid)` is unique and ordered, so paging is exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MessageCursor {
    /// Return messages strictly older than this point. `None` starts at the
    /// newest message.
    pub before_date: Option<i64>,
    pub before_rowid: Option<i64>,
    pub limit: u32,
}

impl Default for MessageCursor {
    fn default() -> Self {
        Self {
            before_date: None,
            before_rowid: None,
            limit: Self::DEFAULT_LIMIT,
        }
    }
}

impl MessageCursor {
    pub const DEFAULT_LIMIT: u32 = 100;
    pub const MAX_LIMIT: u32 = 1_000;

    pub fn first(limit: u32) -> Self {
        Self {
            before_date: None,
            before_rowid: None,
            limit: limit.clamp(1, Self::MAX_LIMIT),
        }
    }

    pub(crate) fn clamped_limit(&self) -> u32 {
        self.limit.clamp(1, Self::MAX_LIMIT)
    }
}

/// One page of a conversation, plus the cursor for the page before it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MessagePage {
    /// Newest first, which is the order the thread view renders from.
    pub messages: Vec<Message>,
    /// Pass this back to load older messages. `None` at the start of the
    /// conversation.
    pub older: Option<MessageCursor>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(body: Option<&str>, kind: MessageKind) -> Message {
        Message {
            id: "guid".into(),
            conversation_id: "1".into(),
            sent_at: None,
            from_me: false,
            sender: None,
            body: body.map(str::to_string),
            kind,
            attachments: Vec::new(),
            reply_to: None,
            reactions: Vec::new(),
            edited: false,
            unsent: false,
            service: Service::IMessage,
        }
    }

    #[test]
    fn service_mapping_is_case_insensitive() {
        assert_eq!(Service::from_sms_db(Some("iMessage")), Service::IMessage);
        assert_eq!(Service::from_sms_db(Some("IMESSAGE")), Service::IMessage);
        assert_eq!(Service::from_sms_db(Some("SMS")), Service::Sms);
        assert_eq!(Service::from_sms_db(Some("RCS")), Service::Rcs);
        assert_eq!(Service::from_sms_db(None), Service::Other);
        assert_eq!(Service::from_sms_db(Some("carrier-thing")), Service::Other);
    }

    #[test]
    fn previews_flatten_whitespace_and_truncate_on_char_boundaries() {
        let m = msg(Some("  hello\n\n  world  "), MessageKind::Text);
        assert_eq!(m.preview(40).unwrap(), "hello world");

        // Multi-byte characters must not be cut mid-character.
        let m = msg(Some("ααααααααααααα"), MessageKind::Text);
        let preview = m.preview(5).unwrap();
        assert_eq!(preview.chars().count(), 5);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn an_attachment_only_message_previews_as_its_kind() {
        let mut m = msg(None, MessageKind::Image);
        m.attachments.push(AttachmentRef {
            file_id: Some("abc".into()),
            file_name: "IMG_0001.HEIC".into(),
            mime: Some("image/heic".into()),
            size: Some(10),
            preview: None,
            is_sticker: false,
        });
        assert_eq!(m.preview(40).unwrap(), "Photo");

        // An unrecognised kind falls back to the file name rather than blank.
        let mut m = msg(None, MessageKind::Unknown(99));
        m.attachments.push(AttachmentRef {
            file_id: None,
            file_name: "thing.bin".into(),
            mime: None,
            size: None,
            preview: None,
            is_sticker: false,
        });
        assert_eq!(m.preview(40).unwrap(), "thing.bin");
    }

    #[test]
    fn an_empty_message_has_no_preview_and_unsent_says_so() {
        assert_eq!(msg(None, MessageKind::Text).preview(40), None);
        assert_eq!(msg(Some("   "), MessageKind::Text).preview(40), None);

        let mut m = msg(Some("was here"), MessageKind::Text);
        m.unsent = true;
        assert_eq!(m.preview(40).unwrap(), "Message unsent");
    }

    #[test]
    fn conversation_labels_prefer_a_name_then_participants() {
        let mut c = ConversationSummary {
            id: "1".into(),
            display_name: Some("Book Club".into()),
            participants: vec![Participant::unresolved("+15550001")],
            is_group: true,
            message_count: 3,
            last_message_at: None,
            last_message_preview: None,
            service: Service::IMessage,
        };
        assert_eq!(c.label(), "Book Club");
        assert_eq!(c.initials(), "BC");

        // A blank name falls through to the members.
        c.display_name = Some("   ".into());
        c.participants = vec![
            Participant {
                id: "+15550001".into(),
                display_name: Some("Ada Lovelace".into()),
                from_contacts: true,
            },
            Participant::unresolved("+15550002"),
        ];
        assert_eq!(c.label(), "Ada Lovelace, +15550002");
        assert_eq!(c.initials(), "AL");

        // Nothing at all is still labelled, not blank.
        c.display_name = None;
        c.participants.clear();
        assert_eq!(c.label(), "Unknown");
        assert_eq!(c.initials(), "U");
    }

    #[test]
    fn initials_cope_with_leading_punctuation_and_numbers() {
        let c = ConversationSummary {
            id: "1".into(),
            display_name: Some("+1 555 0001".into()),
            participants: vec![],
            is_group: false,
            message_count: 0,
            last_message_at: None,
            last_message_preview: None,
            service: Service::Sms,
        };
        assert_eq!(c.initials(), "15");
    }

    #[test]
    fn cursor_limits_are_clamped() {
        assert_eq!(MessageCursor::first(0).limit, 1);
        assert_eq!(MessageCursor::first(99_999).limit, MessageCursor::MAX_LIMIT);
        assert_eq!(
            MessageCursor::default().clamped_limit(),
            MessageCursor::DEFAULT_LIMIT
        );
        // A hand-built cursor from IPC is clamped at use, not just at build.
        let hostile = MessageCursor {
            before_date: None,
            before_rowid: None,
            limit: u32::MAX,
        };
        assert_eq!(hostile.clamped_limit(), MessageCursor::MAX_LIMIT);
    }
}
