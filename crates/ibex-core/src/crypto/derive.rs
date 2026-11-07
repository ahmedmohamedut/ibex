//! Deriving the passcode key from the backup password.
//!
//! Two stages, as iOS 10.2+ specifies:
//!
//! 1. `PBKDF2-HMAC-SHA256(password, DPSL, DPIC, 32)`
//! 2. `PBKDF2-HMAC-SHA1(that, SALT, ITER, 32)`
//!
//! The first stage is deliberately expensive — a real backup asks for ten
//! million rounds, about a second with hardware SHA-256 — so this must not run
//! on a UI thread. The password is borrowed, never stored, and the derived key
//! is zeroized on drop.

use pbkdf2::pbkdf2_hmac;
use sha1::Sha1;
use sha2::Sha256;
use zeroize::Zeroizing;

use super::keybag::DerivationParams;
use crate::error::CryptoError;

/// Length of every key in this scheme.
pub(crate) const KEY_LEN: usize = 32;

/// Derive the key that the class keys are wrapped with.
pub(crate) fn passcode_key(
    password: &str,
    params: &DerivationParams,
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if params.iter == 0 {
        return Err(CryptoError::UnsupportedKeybag {
            detail: "ITER is zero".to_string(),
        });
    }

    // Stage 1 exists only when the keybag carries DPSL and DPIC. Older backups
    // have neither and go straight to the SHA-1 stage with the password itself.
    let stage1 = match (&params.dpsl, params.dpic) {
        (Some(dpsl), Some(dpic)) if dpic > 0 => {
            let mut out = Zeroizing::new(vec![0u8; KEY_LEN]);
            pbkdf2_hmac::<Sha256>(password.as_bytes(), dpsl, dpic, &mut out);
            out
        }
        _ => Zeroizing::new(password.as_bytes().to_vec()),
    };

    let mut key = Zeroizing::new(vec![0u8; KEY_LEN]);
    pbkdf2_hmac::<Sha1>(&stage1, &params.salt, params.iter, &mut key);
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(iter: u32, dpic: Option<u32>) -> DerivationParams {
        DerivationParams {
            salt: b"salt-twenty-bytes-ok".to_vec(),
            iter,
            dpsl: dpic.map(|_| b"dpsl-twenty-bytes-ok".to_vec()),
            dpic,
        }
    }

    /// RFC 6070 test vectors, so a refactor cannot quietly change the
    /// primitive underneath the derivation.
    #[test]
    fn pbkdf2_hmac_sha1_matches_rfc_6070() {
        let mut out = [0u8; 20];
        pbkdf2_hmac::<Sha1>(b"password", b"salt", 1, &mut out);
        assert_eq!(hex::encode(out), "0c60c80f961f0e71f3a9b524af6012062fe037a6");

        let mut out = [0u8; 20];
        pbkdf2_hmac::<Sha1>(b"password", b"salt", 4096, &mut out);
        assert_eq!(hex::encode(out), "4b007901b765489abead49d926f721d065a429c1");
    }

    /// A published PBKDF2-HMAC-SHA256 vector, for the first stage.
    #[test]
    fn pbkdf2_hmac_sha256_matches_a_known_vector() {
        let mut out = [0u8; 32];
        pbkdf2_hmac::<Sha256>(b"password", b"salt", 1, &mut out);
        assert_eq!(
            hex::encode(out),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );

        let mut out = [0u8; 32];
        pbkdf2_hmac::<Sha256>(b"password", b"salt", 4096, &mut out);
        assert_eq!(
            hex::encode(out),
            "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"
        );
    }

    #[test]
    fn derivation_is_deterministic_and_the_right_length() {
        let p = params(1_000, Some(1_000));
        let a = passcode_key("hunter2", &p).unwrap();
        let b = passcode_key("hunter2", &p).unwrap();
        assert_eq!(a.as_slice(), b.as_slice());
        assert_eq!(a.len(), KEY_LEN);
    }

    #[test]
    fn different_passwords_give_different_keys() {
        let p = params(1_000, Some(1_000));
        let a = passcode_key("hunter2", &p).unwrap();
        let b = passcode_key("hunter3", &p).unwrap();
        assert_ne!(a.as_slice(), b.as_slice());

        // An empty password is a legitimate input, just a bad one.
        let empty = passcode_key("", &p).unwrap();
        assert_ne!(empty.as_slice(), a.as_slice());
        assert_eq!(empty.len(), KEY_LEN);
    }

    #[test]
    fn both_stages_actually_contribute() {
        // Changing only the second-stage salt must change the result.
        let mut p = params(1_000, Some(1_000));
        let first = passcode_key("pw", &p).unwrap();
        p.salt = b"a completely other s".to_vec();
        assert_ne!(passcode_key("pw", &p).unwrap().as_slice(), first.as_slice());

        // Changing only the first-stage salt must too.
        let mut p = params(1_000, Some(1_000));
        p.dpsl = Some(b"a completely other d".to_vec());
        assert_ne!(passcode_key("pw", &p).unwrap().as_slice(), first.as_slice());

        // And changing only the iteration counts.
        let a = passcode_key("pw", &params(1_000, Some(1_000))).unwrap();
        let b = passcode_key("pw", &params(1_001, Some(1_000))).unwrap();
        let c = passcode_key("pw", &params(1_000, Some(1_001))).unwrap();
        assert_ne!(a.as_slice(), b.as_slice());
        assert_ne!(a.as_slice(), c.as_slice());
    }

    #[test]
    fn an_old_keybag_without_the_sha256_stage_still_derives() {
        // Pre-10.2 backups have no DPSL/DPIC: the password feeds the SHA-1
        // stage directly.
        let key = passcode_key("hunter2", &params(10_000, None)).unwrap();
        assert_eq!(key.len(), KEY_LEN);
        // And it differs from the two-stage result for the same password.
        let two_stage = passcode_key("hunter2", &params(10_000, Some(1_000))).unwrap();
        assert_ne!(key.as_slice(), two_stage.as_slice());
    }

    #[test]
    fn a_zero_iteration_count_is_refused() {
        // PBKDF2 with zero rounds is not a key derivation at all.
        let err = passcode_key("pw", &params(0, Some(1_000))).unwrap_err();
        assert_eq!(err.code(), "unsupported_keybag");
    }

    #[test]
    fn a_zero_dpic_falls_back_rather_than_producing_nothing() {
        let key = passcode_key("pw", &params(1_000, Some(0))).unwrap();
        assert_eq!(key.len(), KEY_LEN);
    }
}
