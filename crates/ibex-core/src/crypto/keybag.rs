//! The backup keybag: a TLV blob in `Manifest.plist` holding every class key,
//! each wrapped with a key derived from the backup password.
//!
//! Layout (verified against a real iOS 27.2 backup, see `docs/DATA_SOURCES.md`):
//!
//! ```text
//! VERS 5 | TYPE 1 | UUID | HMCK | WRAP 0 | SALT | ITER | DPWT | DPIC | DPSL
//! then, repeated per class: UUID | CLAS | WRAP | KTYP | WPKY
//! ```
//!
//! Every item is a 4-byte ASCII tag, a 4-byte big-endian length, then that many
//! bytes. A `CLAS` tag starts a new class entry.

use std::collections::HashMap;

use zeroize::{Zeroize, Zeroizing};

use crate::error::CryptoError;

/// Keybag types we understand. `1` is the backup keybag.
const KEYBAG_TYPE_BACKUP: u32 = 1;

/// `WRAP` bit meaning "wrapped with the passcode-derived key".
const WRAP_PASSCODE: u32 = 2;

/// An unwrapped AES-256 class key.
///
/// Zeroized on drop, and its `Debug` shows only a length, so a key cannot reach
/// a log through a `{:?}` on some enclosing struct.
pub struct ClassKey {
    key: Zeroizing<Vec<u8>>,
}

impl ClassKey {
    pub(crate) fn new(key: Vec<u8>) -> Self {
        Self {
            key: Zeroizing::new(key),
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.key
    }
}

impl std::fmt::Debug for ClassKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ClassKey({} bytes, redacted)", self.key.len())
    }
}

/// One class entry as it appears in the keybag, still wrapped.
#[derive(Debug, Clone)]
pub(crate) struct WrappedClassKey {
    pub class: u32,
    pub wrap: u32,
    /// Key type; 0 is AES, which is all we have ever seen.
    pub ktyp: u32,
    pub wpky: Vec<u8>,
}

/// The key-derivation parameters from the keybag header.
#[derive(Debug, Clone)]
pub struct DerivationParams {
    /// PBKDF2-HMAC-SHA1 salt.
    pub salt: Vec<u8>,
    /// PBKDF2-HMAC-SHA1 iteration count.
    pub iter: u32,
    /// Double-protection salt, for the SHA-256 stage.
    pub dpsl: Option<Vec<u8>>,
    /// Double-protection iteration count, for the SHA-256 stage.
    pub dpic: Option<u32>,
}

/// A parsed keybag, before any key is unwrapped.
#[derive(Debug)]
pub(crate) struct ParsedKeybag {
    pub version: u32,
    pub params: DerivationParams,
    pub classes: Vec<WrappedClassKey>,
}

/// Read a big-endian `u32` from exactly four bytes.
fn be_u32(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.try_into().ok()?))
}

/// Parse the keybag TLV.
///
/// Unknown tags are skipped rather than rejected: Apple has added header items
/// over time, and a reader that insists on knowing every tag would break on the
/// next iOS release.
pub(crate) fn parse(blob: &[u8]) -> Result<ParsedKeybag, CryptoError> {
    let mut at = 0usize;
    let mut version = None;
    let mut keybag_type = None;
    let mut salt = None;
    let mut iter = None;
    let mut dpsl = None;
    let mut dpic = None;

    let mut classes: Vec<WrappedClassKey> = Vec::new();
    // Fields of the class entry currently being read.
    let mut current: Option<WrappedClassKey> = None;

    while at + 8 <= blob.len() {
        let tag = &blob[at..at + 4];
        let len = be_u32(&blob[at + 4..at + 8]).ok_or_else(|| CryptoError::UnsupportedKeybag {
            detail: "truncated item length".to_string(),
        })? as usize;
        at += 8;

        if at + len > blob.len() {
            return Err(CryptoError::UnsupportedKeybag {
                detail: format!(
                    "item {} claims {len} bytes but only {} remain",
                    String::from_utf8_lossy(tag),
                    blob.len() - at
                ),
            });
        }
        let value = &blob[at..at + len];
        at += len;

        match tag {
            b"VERS" => version = be_u32(value),
            b"TYPE" => keybag_type = be_u32(value),
            b"SALT" => salt = Some(value.to_vec()),
            b"ITER" => iter = be_u32(value),
            b"DPSL" => dpsl = Some(value.to_vec()),
            b"DPIC" => dpic = be_u32(value),
            b"CLAS" => {
                // A new class entry begins; bank the previous one.
                if let Some(entry) = current.take() {
                    classes.push(entry);
                }
                current = Some(WrappedClassKey {
                    class: be_u32(value).unwrap_or(0),
                    wrap: 0,
                    ktyp: 0,
                    wpky: Vec::new(),
                });
            }
            b"WRAP" => {
                // Before the first CLAS this is the header's own WRAP.
                if let Some(entry) = current.as_mut() {
                    entry.wrap = be_u32(value).unwrap_or(0);
                }
            }
            b"KTYP" => {
                if let Some(entry) = current.as_mut() {
                    entry.ktyp = be_u32(value).unwrap_or(0);
                }
            }
            b"WPKY" => {
                if let Some(entry) = current.as_mut() {
                    entry.wpky = value.to_vec();
                }
            }
            // UUID, HMCK, DPWT, PBKY and anything Apple adds later.
            _ => {}
        }
    }
    if let Some(entry) = current.take() {
        classes.push(entry);
    }

    let version = version.ok_or_else(|| CryptoError::UnsupportedKeybag {
        detail: "no VERS item".to_string(),
    })?;
    // Versions 2..=5 share this layout. A newer one may not, so say so rather
    // than silently producing wrong keys.
    if !(2..=5).contains(&version) {
        return Err(CryptoError::UnsupportedKeybag {
            detail: format!("keybag version {version} is not supported"),
        });
    }
    let keybag_type = keybag_type.unwrap_or(KEYBAG_TYPE_BACKUP);
    if keybag_type != KEYBAG_TYPE_BACKUP {
        return Err(CryptoError::UnsupportedKeybag {
            detail: format!("keybag type {keybag_type} is not a backup keybag"),
        });
    }
    let salt = salt.ok_or_else(|| CryptoError::UnsupportedKeybag {
        detail: "no SALT item".to_string(),
    })?;
    let iter = iter.ok_or_else(|| CryptoError::UnsupportedKeybag {
        detail: "no ITER item".to_string(),
    })?;
    if classes.is_empty() {
        return Err(CryptoError::UnsupportedKeybag {
            detail: "no class keys".to_string(),
        });
    }

    Ok(ParsedKeybag {
        version,
        params: DerivationParams {
            salt,
            iter,
            dpsl,
            dpic,
        },
        classes,
    })
}

/// The unwrapped class keys for one backup.
///
/// Holds no password: it is dropped as soon as the keys are derived. Keys are
/// zeroized when this is dropped.
#[derive(Debug, Default)]
pub struct KeyBag {
    classes: HashMap<u32, ClassKey>,
    version: u32,
}

impl KeyBag {
    pub(crate) fn from_classes(classes: HashMap<u32, ClassKey>, version: u32) -> Self {
        Self { classes, version }
    }

    /// The keybag's format version, worth reporting when a backup will not
    /// open: an unexpected version is the first thing to suspect.
    pub fn keybag_version(&self) -> u32 {
        self.version
    }

    /// The key for one protection class.
    pub fn class_key(&self, class: u32) -> Option<&ClassKey> {
        self.classes.get(&class)
    }

    /// Protection classes we hold a key for, ascending.
    pub fn classes(&self) -> Vec<u32> {
        let mut classes: Vec<u32> = self.classes.keys().copied().collect();
        classes.sort_unstable();
        classes
    }

    pub fn len(&self) -> usize {
        self.classes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }
}

impl Drop for KeyBag {
    fn drop(&mut self) {
        // ClassKey zeroizes itself, but clear the map eagerly so the keys go as
        // soon as the backup is closed rather than whenever the map is freed.
        self.classes.clear();
    }
}

impl Zeroize for KeyBag {
    fn zeroize(&mut self) {
        self.classes.clear();
    }
}

/// Split an `EncryptionKey` or `ManifestKey` blob into its protection class and
/// wrapped key.
///
/// The first four bytes are the class, little-endian; the rest is the wrapped
/// key. A real backup's `ManifestKey` is 44 bytes: 4 + a 40-byte wrapped key.
pub fn split_class_and_key(blob: &[u8]) -> Result<(u32, &[u8]), CryptoError> {
    if blob.len() < 4 + 8 {
        return Err(CryptoError::Decrypt {
            detail: format!("key blob is only {} bytes", blob.len()),
        });
    }
    let class = u32::from_le_bytes(blob[..4].try_into().expect("checked length above"));
    Ok((class, &blob[4..]))
}

/// `WRAP & 2`: wrapped with the passcode-derived key.
pub(crate) fn needs_passcode_key(wrap: u32) -> bool {
    wrap & WRAP_PASSCODE != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build one TLV item.
    fn item(tag: &[u8; 4], value: &[u8]) -> Vec<u8> {
        let mut out = tag.to_vec();
        out.extend_from_slice(&(value.len() as u32).to_be_bytes());
        out.extend_from_slice(value);
        out
    }

    fn u32_item(tag: &[u8; 4], value: u32) -> Vec<u8> {
        item(tag, &value.to_be_bytes())
    }

    /// A keybag shaped exactly like the real iOS 27.2 one that was inspected:
    /// VERS 5, TYPE 1, ITER 10000, DPIC 10000000, 14 classes with WRAP 2 or 3.
    fn realistic_keybag() -> Vec<u8> {
        let mut blob = Vec::new();
        blob.extend(u32_item(b"VERS", 5));
        blob.extend(u32_item(b"TYPE", 1));
        blob.extend(item(b"UUID", &[0xaa; 16]));
        blob.extend(item(b"HMCK", &[0xbb; 40]));
        blob.extend(u32_item(b"WRAP", 0));
        blob.extend(item(b"SALT", &[0xcc; 20]));
        blob.extend(u32_item(b"ITER", 10_000));
        blob.extend(u32_item(b"DPWT", 1));
        blob.extend(u32_item(b"DPIC", 10_000_000));
        blob.extend(item(b"DPSL", &[0xdd; 20]));

        for (class, wrap) in [
            (1, 2),
            (2, 2),
            (3, 2),
            (4, 2),
            (5, 3),
            (6, 2),
            (7, 2),
            (8, 2),
            (9, 3),
            (10, 3),
            (11, 3),
            (17, 2),
            (18, 2),
            (19, 3),
        ] {
            blob.extend(item(b"UUID", &[0xee; 16]));
            blob.extend(u32_item(b"CLAS", class));
            blob.extend(u32_item(b"WRAP", wrap));
            blob.extend(u32_item(b"KTYP", 0));
            blob.extend(item(b"WPKY", &[class as u8; 40]));
        }
        blob
    }

    #[test]
    fn parses_the_real_backups_shape() {
        let parsed = parse(&realistic_keybag()).unwrap();

        assert_eq!(parsed.version, 5);
        assert_eq!(parsed.params.iter, 10_000);
        assert_eq!(parsed.params.salt.len(), 20);
        assert_eq!(parsed.params.dpic, Some(10_000_000));
        assert_eq!(parsed.params.dpsl.as_ref().map(Vec::len), Some(20));

        assert_eq!(parsed.classes.len(), 14);
        let classes: Vec<u32> = parsed.classes.iter().map(|c| c.class).collect();
        assert_eq!(classes, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 17, 18, 19]);
        // Every class is AES with a 40-byte wrapped key, and all of them are
        // wrapped with the passcode key.
        for entry in &parsed.classes {
            assert_eq!(entry.ktyp, 0, "class {}", entry.class);
            assert_eq!(entry.wpky.len(), 40, "class {}", entry.class);
            assert!(needs_passcode_key(entry.wrap), "class {}", entry.class);
        }
    }

    #[test]
    fn the_header_wrap_is_not_mistaken_for_a_class_wrap() {
        // The header carries WRAP 0 before any CLAS. If that leaked into the
        // first class entry, every key would look un-wrapped and nothing would
        // decrypt.
        let parsed = parse(&realistic_keybag()).unwrap();
        assert_eq!(parsed.classes[0].class, 1);
        assert_eq!(parsed.classes[0].wrap, 2);
    }

    #[test]
    fn unknown_tags_are_skipped() {
        let mut blob = realistic_keybag();
        blob.extend(item(b"XXXX", b"something new from a future iOS"));
        let parsed = parse(&blob).unwrap();
        assert_eq!(parsed.classes.len(), 14);
    }

    #[test]
    fn truncation_is_an_error_not_a_panic() {
        let full = realistic_keybag();
        // Cut in the middle of the final WPKY value.
        let truncated = &full[..full.len() - 10];
        let err = parse(truncated).unwrap_err();
        assert!(
            matches!(err, CryptoError::UnsupportedKeybag { .. }),
            "got {err:?}"
        );

        // And every shorter prefix must also fail cleanly rather than panic.
        for cut in (0..full.len()).step_by(7) {
            let _ = parse(&full[..cut]);
        }
    }

    #[test]
    fn a_future_keybag_version_is_refused_rather_than_guessed() {
        let mut blob = Vec::new();
        blob.extend(u32_item(b"VERS", 9));
        blob.extend(u32_item(b"TYPE", 1));
        blob.extend(item(b"SALT", &[0; 20]));
        blob.extend(u32_item(b"ITER", 1));
        blob.extend(u32_item(b"CLAS", 1));
        blob.extend(item(b"WPKY", &[0; 40]));

        let err = parse(&blob).unwrap_err();
        assert_eq!(err.code(), "unsupported_keybag");
        assert!(err.to_string().contains('9'), "{err}");
    }

    #[test]
    fn a_non_backup_keybag_is_refused() {
        let mut blob = Vec::new();
        blob.extend(u32_item(b"VERS", 5));
        // Type 0 is a system keybag, not a backup one.
        blob.extend(u32_item(b"TYPE", 0));
        blob.extend(item(b"SALT", &[0; 20]));
        blob.extend(u32_item(b"ITER", 1));
        blob.extend(u32_item(b"CLAS", 1));
        blob.extend(item(b"WPKY", &[0; 40]));
        assert_eq!(parse(&blob).unwrap_err().code(), "unsupported_keybag");
    }

    #[test]
    fn missing_required_items_are_named() {
        // No SALT.
        let mut blob = Vec::new();
        blob.extend(u32_item(b"VERS", 5));
        blob.extend(u32_item(b"ITER", 10));
        blob.extend(u32_item(b"CLAS", 1));
        let err = parse(&blob).unwrap_err();
        assert!(err.to_string().contains("SALT"), "{err}");

        // No classes at all.
        let mut blob = Vec::new();
        blob.extend(u32_item(b"VERS", 5));
        blob.extend(item(b"SALT", &[0; 20]));
        blob.extend(u32_item(b"ITER", 10));
        let err = parse(&blob).unwrap_err();
        assert!(err.to_string().contains("class keys"), "{err}");
    }

    #[test]
    fn empty_input_is_an_error() {
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn key_blobs_split_into_class_and_wrapped_key() {
        // A real ManifestKey: class 3 little-endian, then 40 wrapped bytes.
        let mut blob = 3u32.to_le_bytes().to_vec();
        blob.extend_from_slice(&[0x42; 40]);
        assert_eq!(blob.len(), 44);

        let (class, wrapped) = split_class_and_key(&blob).unwrap();
        assert_eq!(class, 3);
        assert_eq!(wrapped.len(), 40);

        // Too short to contain a class and a wrapped key.
        assert!(split_class_and_key(&[1, 2, 3]).is_err());
        assert!(split_class_and_key(&[]).is_err());
    }

    #[test]
    fn wrap_flags() {
        assert!(needs_passcode_key(2));
        assert!(needs_passcode_key(3));
        assert!(!needs_passcode_key(0));
        assert!(!needs_passcode_key(1));
    }

    #[test]
    fn keys_are_never_printed_by_debug() {
        let key = ClassKey::new(vec![0xde, 0xad, 0xbe, 0xef]);
        let shown = format!("{key:?}");
        assert!(shown.contains("redacted"), "{shown}");
        assert!(!shown.contains("de"), "{shown}");
        assert!(!shown.contains("222"), "{shown}");

        let mut classes = HashMap::new();
        classes.insert(3, ClassKey::new(vec![0xaa; 32]));
        let bag = KeyBag::from_classes(classes, 5);
        let shown = format!("{bag:?}");
        assert!(shown.contains("redacted"), "{shown}");
        assert!(!shown.contains("170"), "{shown}");
    }

    #[test]
    fn a_keybag_reports_the_classes_it_holds() {
        let mut classes = HashMap::new();
        classes.insert(11, ClassKey::new(vec![1; 32]));
        classes.insert(3, ClassKey::new(vec![2; 32]));
        let bag = KeyBag::from_classes(classes, 5);

        assert_eq!(bag.keybag_version(), 5);
        assert_eq!(bag.classes(), vec![3, 11]);
        assert_eq!(bag.len(), 2);
        assert!(bag.class_key(3).is_some());
        assert!(bag.class_key(99).is_none());
        assert!(!bag.is_empty());
    }
}
