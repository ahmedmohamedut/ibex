//! AES key unwrapping (RFC 3394).
//!
//! Class keys in the keybag, and each file's own key, are wrapped. The wrap
//! carries an integrity check, which is what tells a wrong password apart from
//! a corrupt backup: with the wrong derived key, every unwrap fails that check.

use aes::cipher::KeyInit;
use aes_kw::KwAes256;
use zeroize::Zeroizing;

use crate::error::CryptoError;

/// Unwrap a key with a 256-bit key-encryption key.
///
/// `wrapped` is 8 bytes longer than the key inside it.
pub(crate) fn unwrap_key(kek: &[u8], wrapped: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if kek.len() != 32 {
        return Err(CryptoError::Decrypt {
            detail: format!("key-encryption key is {} bytes, expected 32", kek.len()),
        });
    }
    // RFC 3394 works in 64-bit blocks and needs at least three of them.
    if wrapped.len() < 24 || !wrapped.len().is_multiple_of(8) {
        return Err(CryptoError::Decrypt {
            detail: format!("wrapped key length {} is not usable", wrapped.len()),
        });
    }

    let kek = KwAes256::new_from_slice(kek).map_err(|_| CryptoError::Decrypt {
        detail: "could not build the key-encryption key".to_string(),
    })?;

    let mut out = Zeroizing::new(vec![0u8; wrapped.len() - 8]);
    let len = kek
        .unwrap_key(wrapped, &mut out)
        // The integrity check failing is overwhelmingly a wrong password, which
        // is what the UI needs to say. A corrupt keybag is indistinguishable
        // here, and telling the user to check the password is still the most
        // useful thing to do.
        .map_err(|_| CryptoError::BadPassword)?
        .len();

    out.truncate(len);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 3394 section 4.6: wrapping a 256-bit key with a 256-bit KEK.
    /// A known-answer test, so the primitive cannot drift under a refactor.
    #[test]
    fn matches_rfc_3394_section_4_6() {
        let kek = hex::decode("000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F")
            .unwrap();
        let wrapped = hex::decode(
            "28C9F404C4B810F4CBCCB35CFB87F8263F5786E2D80ED326CBC7F0E71A99F43BFB988B9B7A02DD21",
        )
        .unwrap();
        let expected = "00112233445566778899AABBCCDDEEFF000102030405060708090A0B0C0D0E0F";

        let out = unwrap_key(&kek, &wrapped).unwrap();
        assert_eq!(hex::encode_upper(out.as_slice()), expected);
    }

    #[test]
    fn a_wrong_kek_is_reported_as_a_bad_password() {
        let kek = [0x11u8; 32];
        let key = [0x42u8; 32];

        let wrapper = KwAes256::new_from_slice(&kek).unwrap();
        let mut wrapped = vec![0u8; key.len() + 8];
        wrapper.wrap_key(&key, &mut wrapped).unwrap();

        // Right key unwraps.
        assert_eq!(unwrap_key(&kek, &wrapped).unwrap().as_slice(), &key);

        // One bit different and the integrity check catches it.
        let mut wrong = kek;
        wrong[0] ^= 1;
        let err = unwrap_key(&wrong, &wrapped).unwrap_err();
        assert_eq!(err.code(), "bad_password");
    }

    #[test]
    fn tampered_ciphertext_is_caught() {
        let kek = [0x11u8; 32];
        let wrapper = KwAes256::new_from_slice(&kek).unwrap();
        let mut wrapped = vec![0u8; 40];
        wrapper.wrap_key(&[0x42u8; 32], &mut wrapped).unwrap();

        wrapped[20] ^= 0x80;
        assert!(unwrap_key(&kek, &wrapped).is_err());
    }

    #[test]
    fn malformed_lengths_are_refused_without_panicking() {
        let kek = [0u8; 32];
        // Not a multiple of 8.
        assert!(unwrap_key(&kek, &[0u8; 41]).is_err());
        // Too few blocks.
        assert!(unwrap_key(&kek, &[0u8; 16]).is_err());
        assert!(unwrap_key(&kek, &[]).is_err());
        // Wrong KEK size.
        assert!(unwrap_key(&[0u8; 16], &[0u8; 40]).is_err());
    }

    #[test]
    fn round_trips_a_forty_byte_wrapped_key_like_a_real_backup() {
        // Every class entry in the real backup has a 40-byte WPKY holding a
        // 32-byte key.
        let kek = [0x5au8; 32];
        let key: Vec<u8> = (0..32u8).collect();

        let wrapper = KwAes256::new_from_slice(&kek).unwrap();
        let mut wrapped = vec![0u8; 40];
        wrapper.wrap_key(&key, &mut wrapped).unwrap();
        assert_eq!(wrapped.len(), 40);

        let out = unwrap_key(&kek, &wrapped).unwrap();
        assert_eq!(out.as_slice(), key.as_slice());
        assert_eq!(out.len(), 32);
    }
}
