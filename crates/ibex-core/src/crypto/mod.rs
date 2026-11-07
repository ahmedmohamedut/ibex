//! Encrypted-backup support.
//!
//! An encrypted backup stores every file, and `Manifest.db` itself, under
//! AES-256-CBC with a zero IV. The keys come from a chain rooted in the backup
//! password:
//!
//! ```text
//! password ──PBKDF2-SHA256(DPSL, DPIC)──┐
//!                                       ├─PBKDF2-SHA1(SALT, ITER)──> passcode key
//!                                       ┘                               │
//!                                                      RFC 3394 unwrap   ▼
//!   keybag WPKY per protection class ─────────────────────────────> class keys
//!                                                                        │
//!   ManifestKey / MBFile EncryptionKey ──────RFC 3394 unwrap─────────────▼
//!                                                              per-file keys
//! ```
//!
//! Rules this module keeps:
//!
//! - the password is borrowed, used once and never stored;
//! - keys live in `Zeroizing` buffers and are wiped on drop;
//! - `Debug` never prints key bytes;
//! - a wrong password surfaces as [`crate::CryptoError::BadPassword`], which the
//!   AES key wrap's integrity check makes reliable.
//!
//! See `docs/DATA_SOURCES.md` for the on-disk format, verified against a real
//! iOS 27.2 backup.

// `derive` and `unwrap` are internal, but the test fixture builder needs them
// so that an encrypted fixture is produced by the very code that reads it,
// rather than by a parallel implementation that could drift.
pub mod decrypt;
#[cfg(feature = "testutil")]
pub(crate) mod derive;
#[cfg(not(feature = "testutil"))]
mod derive;
pub mod keybag;
#[cfg(feature = "testutil")]
pub(crate) mod unwrap;
#[cfg(not(feature = "testutil"))]
mod unwrap;

use std::collections::HashMap;

pub use decrypt::{decrypt_buffer, decrypt_to_file};
pub use keybag::{ClassKey, KeyBag, split_class_and_key};

use crate::error::CryptoError;

impl KeyBag {
    /// Unlock a backup's keybag with its password.
    ///
    /// Expensive by design: a current backup asks for ten million SHA-256
    /// rounds, around a second with hardware acceleration. Call it from a
    /// blocking context, never on a UI thread.
    pub fn unlock(keybag_blob: &[u8], password: &str) -> Result<Self, CryptoError> {
        let parsed = keybag::parse(keybag_blob)?;
        let passcode_key = derive::passcode_key(password, &parsed.params)?;

        let mut classes: HashMap<u32, ClassKey> = HashMap::new();
        let mut attempted = 0usize;

        for entry in &parsed.classes {
            // Classes wrapped with a device-bound key cannot be unwrapped from
            // a backup, and are simply absent rather than an error.
            if !keybag::needs_passcode_key(entry.wrap) {
                continue;
            }
            // Key types other than AES have never been observed; skip rather
            // than guess at them.
            if entry.ktyp != 0 {
                continue;
            }
            attempted += 1;
            if let Ok(key) = unwrap::unwrap_key(&passcode_key, &entry.wpky) {
                classes.insert(entry.class, ClassKey::new(key.to_vec()));
            }
        }

        // Every single unwrap failing its integrity check means the derived key
        // was wrong, which means the password was. One class failing on its own
        // is tolerated: that class is just unavailable.
        if attempted > 0 && classes.is_empty() {
            return Err(CryptoError::BadPassword);
        }
        if classes.is_empty() {
            return Err(CryptoError::UnsupportedKeybag {
                detail: "no class key in this keybag is wrapped with the backup password"
                    .to_string(),
            });
        }
        Ok(KeyBag::from_classes(classes, parsed.version))
    }

    /// Unwrap one file's key, given its `EncryptionKey` blob from the manifest.
    ///
    /// The blob's first four bytes name the protection class; the rest is the
    /// wrapped key.
    pub fn unwrap_file_key(&self, blob: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let (class, wrapped) = split_class_and_key(blob)?;
        let class_key = self
            .class_key(class)
            .ok_or(CryptoError::NoClassKey { class })?;
        let key = unwrap::unwrap_key(class_key.bytes(), wrapped).map_err(|_| {
            // At this point the password is already known good, so a failure
            // here is a damaged manifest entry, not a wrong password.
            CryptoError::Decrypt {
                detail: format!("could not unwrap the file key for class {class}"),
            }
        })?;
        Ok(key.to_vec())
    }
}
