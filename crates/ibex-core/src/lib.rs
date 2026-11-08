//! Core of ibex: reading iPhone backups.
//!
//! This crate has no UI dependencies — everything here works from `ibex-cli`
//! and from tests. See `docs/ARCHITECTURE.md`.
//!
//! The backup folder is treated as strictly read-only. Everything derived from
//! it (SQLite working copies, thumbnails, decrypted files) lives in the
//! [`cache`].
//!
//! ```no_run
//! use ibex_core::{Backup, Page};
//!
//! let backup = Backup::open(std::path::Path::new("/backups/UDID"), None)?;
//! println!("{}", backup.info().display_line());
//! for domain in backup.domains()? {
//!     println!("{} ({} files)", domain.domain, domain.file_count);
//! }
//! let listing = backup.list_dir("HomeDomain", "Library/SMS", Page::default())?;
//! println!("{} entries", listing.len());
//! # Ok::<(), ibex_core::Error>(())
//! ```

pub mod backup;
pub mod cache;
pub mod crypto;
pub mod error;
pub mod export;
pub mod extractors;
pub mod page;
pub mod search;
pub mod thumbs;
pub mod util;
pub mod workcopy;

#[cfg(feature = "testutil")]
pub mod testutil;

pub use backup::{
    Backup, BackupInfo, DomainSummary, EntryKind, FileMeta, Manifest, ManifestEntry, ReadSeek,
};
pub use cache::CacheDir;
pub use crypto::KeyBag;
pub use error::{BackupError, CryptoError, Error, ExtractError, Result};
pub use export::{ExportFormat, ExportOptions, ExportReport, Selection};
pub use extractors::{
    Availability, ContactResolver, ConversationSummary, Message, MessageCursor, MessagePage,
    Service,
};
pub use page::{Page, Paged};
pub use search::{IndexStatus, SearchFilters, SearchHit, SearchIndexReport};
pub use thumbs::Render;
