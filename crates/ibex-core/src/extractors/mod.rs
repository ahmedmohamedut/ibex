//! Data-source extractors.
//!
//! Each one owns a single source — `sms.db`, WhatsApp's `ChatStorage.sqlite`,
//! `Photos.sqlite` — and exposes typed, paginated queries over it. They all
//! normalize onto the shared types in [`message`] so the UI and the exporters
//! treat every chat source the same way. See `docs/ARCHITECTURE.md`.

pub mod contacts;
pub mod message;
pub mod photos;
pub mod sms;
pub mod whatsapp;

pub use contacts::ContactResolver;
pub use message::{
    AttachmentRef, ConversationSummary, Message, MessageCursor, MessageKind, MessagePage,
    Participant, Reaction, ReactionKind, Service,
};
pub use photos::{
    AlbumSummary, MonthCount, PhotoAsset, PhotoAvailability, PhotoCursor, PhotoFilter, PhotoKind,
    PhotoPage, Photos, PhotosDb,
};

/// Whether a source is usable in a given backup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// Present and readable.
    Present {
        /// A hint about which schema variant was found, for diagnostics.
        version_hint: Option<String>,
    },
    /// The app was not installed, or its data was not backed up. Not an error.
    Absent,
    /// Present but we cannot read it, with a reason worth showing.
    Unsupported(String),
}

impl Availability {
    pub fn is_present(&self) -> bool {
        matches!(self, Availability::Present { .. })
    }
}

/// The small contract every extractor shares. Per-source queries are not part
/// of it: their shapes differ too much to be worth forcing into one trait.
pub trait Extractor {
    /// Stable id, matching the sidebar section: `sms`, `whatsapp`, ...
    const ID: &'static str;

    /// Whether this backup holds the source's data.
    fn detect(backup: &crate::Backup) -> Availability;
}
