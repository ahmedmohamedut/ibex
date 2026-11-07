//! The wrap-and-encrypt side of the backup crypto, for building encrypted
//! fixtures.
//!
//! Production code only ever *decrypts*, so this lives in `testutil` rather
//! than in `crypto`. Having both halves is what makes the decrypt side
//! trustworthy: a test can encrypt with a known password and prove the bytes
//! come back identical.
//!
//! Keys are derived deterministically from a fixed seed with SHA-256, so
//! fixtures are reproducible and no random-number dependency is needed.

use std::collections::HashMap;

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockModeEncrypt, KeyInit, KeyIvInit};
use aes_kw::KwAes256;
use sha2::{Digest, Sha256};

use crate::error::{CryptoError, Result};

type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;

/// Iteration counts for fixtures. Real backups use 10 000 and 10 000 000; a
/// test suite cannot afford the latter, and the code path is identical.
const FIXTURE_ITER: u32 = 1_000;
const FIXTURE_DPIC: u32 = 1_000;

/// Protection classes a fixture provides keys for, matching the classes a real
/// backup carries.
const FIXTURE_CLASSES: &[u32] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 17, 18, 19];

/// The class every fixture file is protected with. 3 is the usual one for
/// ordinary files in a real backup.
const DEFAULT_CLASS: u32 = 3;

/// Deterministic 32 bytes from a label.
fn derived_key(label: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(b"ibex fixture key v1\0");
    hasher.update(label.as_bytes());
    hasher.finalize().to_vec()
}

/// Build one TLV item: 4-byte tag, 4-byte big-endian length, value.
fn tlv(tag: &[u8; 4], value: &[u8]) -> Vec<u8> {
    let mut out = tag.to_vec();
    out.extend_from_slice(&(value.len() as u32).to_be_bytes());
    out.extend_from_slice(value);
    out
}

fn tlv_u32(tag: &[u8; 4], value: u32) -> Vec<u8> {
    tlv(tag, &value.to_be_bytes())
}

/// Wrap a key with AES-KW.
fn wrap(kek: &[u8], key: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let wrapper = KwAes256::new_from_slice(kek).map_err(|_| CryptoError::Decrypt {
        detail: "fixture: bad key-encryption key".to_string(),
    })?;
    let mut out = vec![0u8; key.len() + 8];
    wrapper
        .wrap_key(key, &mut out)
        .map_err(|_| CryptoError::Decrypt {
            detail: "fixture: wrap failed".to_string(),
        })?;
    Ok(out)
}

/// Encrypt with AES-256-CBC, zero IV, PKCS#7 — exactly what a device writes.
fn encrypt(key: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let iv = [0u8; 16];
    let enc = Aes256CbcEnc::new_from_slices(key, &iv).map_err(|_| CryptoError::Decrypt {
        detail: "fixture: bad file key".to_string(),
    })?;
    let mut out = vec![0u8; plaintext.len() + 16];
    let len = enc
        .encrypt_padded_b2b::<Pkcs7>(plaintext, &mut out)
        .map_err(|_| CryptoError::Decrypt {
            detail: "fixture: encrypt failed".to_string(),
        })?
        .len();
    out.truncate(len);
    Ok(out)
}

/// The keys and blobs for one encrypted fixture.
pub(crate) struct Encryption {
    /// Unwrapped class keys, by protection class.
    class_keys: HashMap<u32, Vec<u8>>,
    /// The serialized keybag, for `Manifest.plist`.
    keybag: Vec<u8>,
    /// Class + wrapped key for `Manifest.db`.
    manifest_key_blob: Vec<u8>,
    /// The unwrapped manifest key.
    manifest_key: Vec<u8>,
}

impl Encryption {
    /// Derive everything from a password, deterministically.
    pub(crate) fn new(password: &str) -> Result<Self, CryptoError> {
        let salt = derived_key("salt")[..20].to_vec();
        let dpsl = derived_key("dpsl")[..20].to_vec();

        // Derive the passcode key exactly as the production path will, so the
        // fixture proves that path rather than a parallel implementation.
        let params = crate::crypto::keybag::DerivationParams {
            salt: salt.clone(),
            iter: FIXTURE_ITER,
            dpsl: Some(dpsl.clone()),
            dpic: Some(FIXTURE_DPIC),
        };
        let passcode_key = crate::crypto::derive::passcode_key(password, &params)?;

        let mut class_keys = HashMap::new();
        let mut keybag = Vec::new();
        keybag.extend(tlv_u32(b"VERS", 5));
        keybag.extend(tlv_u32(b"TYPE", 1));
        keybag.extend(tlv(b"UUID", &derived_key("uuid")[..16]));
        keybag.extend(tlv(b"HMCK", &derived_key("hmck")[..32]));
        keybag.extend(tlv_u32(b"WRAP", 0));
        keybag.extend(tlv(b"SALT", &salt));
        keybag.extend(tlv_u32(b"ITER", FIXTURE_ITER));
        keybag.extend(tlv_u32(b"DPWT", 1));
        keybag.extend(tlv_u32(b"DPIC", FIXTURE_DPIC));
        keybag.extend(tlv(b"DPSL", &dpsl));

        for &class in FIXTURE_CLASSES {
            let key = derived_key(&format!("class-{class}"));
            let wpky = wrap(&passcode_key, &key)?;

            keybag.extend(tlv(b"UUID", &derived_key(&format!("uuid-{class}"))[..16]));
            keybag.extend(tlv_u32(b"CLAS", class));
            keybag.extend(tlv_u32(b"WRAP", 2));
            keybag.extend(tlv_u32(b"KTYP", 0));
            keybag.extend(tlv(b"WPKY", &wpky));

            class_keys.insert(class, key);
        }

        // The manifest key, wrapped under a class key, with the class in the
        // first four bytes little-endian.
        let manifest_key = derived_key("manifest");
        let class_key = class_keys
            .get(&DEFAULT_CLASS)
            .expect("the default class is always present");
        let mut manifest_key_blob = DEFAULT_CLASS.to_le_bytes().to_vec();
        manifest_key_blob.extend(wrap(class_key, &manifest_key)?);

        Ok(Self {
            class_keys,
            keybag,
            manifest_key_blob,
            manifest_key,
        })
    }

    pub(crate) fn keybag_blob(&self) -> Vec<u8> {
        self.keybag.clone()
    }

    pub(crate) fn manifest_key_blob(&self) -> Vec<u8> {
        self.manifest_key_blob.clone()
    }

    /// Encrypt the finished `Manifest.db`.
    pub(crate) fn encrypt_manifest(&self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        encrypt(&self.manifest_key, plaintext)
    }

    /// Make a per-file key for `label`, returning the blob that goes into the
    /// MBFile's `EncryptionKey`: class, little-endian, then the wrapped key.
    pub(crate) fn wrap_new_file_key(&self, label: &str) -> Result<Vec<u8>, CryptoError> {
        let file_key = derived_key(&format!("file-{label}"));
        let class_key = self
            .class_keys
            .get(&DEFAULT_CLASS)
            .ok_or(CryptoError::NoClassKey {
                class: DEFAULT_CLASS,
            })?;
        let mut blob = DEFAULT_CLASS.to_le_bytes().to_vec();
        blob.extend(wrap(class_key, &file_key)?);
        Ok(blob)
    }

    /// Encrypt a payload with the key inside `blob`.
    pub(crate) fn encrypt_for(
        &self,
        blob: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        let (class, wrapped) = crate::crypto::split_class_and_key(blob)?;
        let class_key = self
            .class_keys
            .get(&class)
            .ok_or(CryptoError::NoClassKey { class })?;
        // Unwrap through the production path, so the fixture cannot drift from
        // what the decryptor expects.
        let file_key = crate::crypto::unwrap::unwrap_key(class_key, wrapped)?;
        encrypt(&file_key, plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keybag_it_builds_is_one_the_parser_accepts() {
        let crypt = Encryption::new("hunter2").unwrap();
        let bag = crate::crypto::KeyBag::unlock(&crypt.keybag_blob(), "hunter2").unwrap();

        assert_eq!(bag.classes(), FIXTURE_CLASSES.to_vec());
        // And the keys it unwraps are the ones the fixture wrapped.
        for &class in FIXTURE_CLASSES {
            assert_eq!(
                bag.class_key(class).unwrap().bytes(),
                crypt.class_keys[&class].as_slice(),
                "class {class}"
            );
        }
    }

    #[test]
    fn a_wrong_password_cannot_unlock_it() {
        let crypt = Encryption::new("hunter2").unwrap();
        let err = crate::crypto::KeyBag::unlock(&crypt.keybag_blob(), "hunter3").unwrap_err();
        assert_eq!(err.code(), "bad_password");
    }

    #[test]
    fn manifest_encryption_round_trips_through_the_production_decryptor() {
        let crypt = Encryption::new("pw").unwrap();
        let bag = crate::crypto::KeyBag::unlock(&crypt.keybag_blob(), "pw").unwrap();

        let plain = b"SQLite format 3\0 pretend this is a database".to_vec();
        let cipher = crypt.encrypt_manifest(&plain).unwrap();
        assert_ne!(cipher, plain);

        let key = bag.unwrap_file_key(&crypt.manifest_key_blob()).unwrap();
        let out = crate::crypto::decrypt_buffer(&key, &cipher, None).unwrap();
        assert_eq!(out, plain);
    }

    #[test]
    fn file_encryption_round_trips_at_awkward_lengths() {
        let crypt = Encryption::new("pw").unwrap();
        let bag = crate::crypto::KeyBag::unlock(&crypt.keybag_blob(), "pw").unwrap();

        for len in [0usize, 1, 15, 16, 17, 4096] {
            let plain: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let blob = crypt.wrap_new_file_key(&format!("f{len}")).unwrap();
            let cipher = crypt.encrypt_for(&blob, &plain).unwrap();

            let key = bag.unwrap_file_key(&blob).unwrap();
            let out = crate::crypto::decrypt_buffer(&key, &cipher, Some(len as u64)).unwrap();
            assert_eq!(out, plain, "length {len}");
        }
    }

    #[test]
    fn every_file_gets_a_distinct_key() {
        let crypt = Encryption::new("pw").unwrap();
        let a = crypt.wrap_new_file_key("a.txt").unwrap();
        let b = crypt.wrap_new_file_key("b.txt").unwrap();
        assert_ne!(a, b);

        // Same label twice gives the same key, which is what keeps fixtures
        // reproducible.
        assert_eq!(a, crypt.wrap_new_file_key("a.txt").unwrap());
    }

    #[test]
    fn fixtures_are_reproducible_across_runs() {
        let a = Encryption::new("pw").unwrap();
        let b = Encryption::new("pw").unwrap();
        assert_eq!(a.keybag_blob(), b.keybag_blob());
        assert_eq!(a.manifest_key_blob(), b.manifest_key_blob());
    }
}
