//! AES-256-CBC decryption with a zero IV, as iOS backups use.
//!
//! Two paths:
//!
//! - [`decrypt_buffer`] for things that must be whole in memory anyway, such as
//!   `Manifest.db`'s own bytes before SQLite sees them.
//! - [`decrypt_to_file`] streams in chunks. A real backup holds multi-gigabyte
//!   videos, and buffering one to preview it would be unacceptable.
//!
//! Trailing bytes are handled by **truncating to the size the manifest
//! records** rather than by trusting PKCS#7 padding, which costs nothing and
//! cannot be fooled by a corrupt final block.

use std::io::{Read, Write};
use std::path::Path;

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockModeDecrypt, KeyIvInit};

use crate::error::{CryptoError, IoResultExt, Result};

type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

/// AES block size, and the alignment every ciphertext must have.
const BLOCK: usize = 16;

/// Chunk size for streaming. A multiple of the block size.
const CHUNK: usize = 64 * 1024;

/// Build a decryptor with the all-zero IV that iOS backups use.
fn decryptor(key: &[u8]) -> Result<Aes256CbcDec, CryptoError> {
    if key.len() != 32 {
        return Err(CryptoError::Decrypt {
            detail: format!("file key is {} bytes, expected 32", key.len()),
        });
    }
    let iv = [0u8; BLOCK];
    Aes256CbcDec::new_from_slices(key, &iv).map_err(|_| CryptoError::Decrypt {
        detail: "could not build the AES-256-CBC decryptor".to_string(),
    })
}

/// Decrypt a whole buffer.
///
/// `expected_len` is the plaintext length the manifest records. When given, the
/// result is truncated to it; otherwise PKCS#7 padding is stripped, falling
/// back to the unpadded bytes when the padding does not validate — a backup
/// with an odd final block is still worth showing.
pub fn decrypt_buffer(
    key: &[u8],
    ciphertext: &[u8],
    expected_len: Option<u64>,
) -> Result<Vec<u8>, CryptoError> {
    if ciphertext.is_empty() {
        return Ok(Vec::new());
    }
    if !ciphertext.len().is_multiple_of(BLOCK) {
        return Err(CryptoError::Decrypt {
            detail: format!(
                "ciphertext is {} bytes, not a multiple of {BLOCK}",
                ciphertext.len()
            ),
        });
    }

    let mut buf = ciphertext.to_vec();
    let decryptor = decryptor(key)?;

    match expected_len {
        // The manifest knows the real length: decrypt every block and cut.
        Some(len) => {
            decryptor
                .decrypt_padded::<aes::cipher::block_padding::NoPadding>(&mut buf)
                .map_err(|_| CryptoError::Decrypt {
                    detail: "block decryption failed".to_string(),
                })?;
            let len = usize::try_from(len).unwrap_or(usize::MAX).min(buf.len());
            buf.truncate(len);
            Ok(buf)
        }
        // No recorded length, as for Manifest.db: trust the padding, but do not
        // fail if it is wrong.
        None => {
            let mut padded = buf.clone();
            match decryptor.decrypt_padded::<Pkcs7>(&mut padded) {
                Ok(plain) => {
                    let len = plain.len();
                    padded.truncate(len);
                    Ok(padded)
                }
                Err(_) => {
                    let decryptor = self::decryptor(key)?;
                    decryptor
                        .decrypt_padded::<aes::cipher::block_padding::NoPadding>(&mut buf)
                        .map_err(|_| CryptoError::Decrypt {
                            detail: "block decryption failed".to_string(),
                        })?;
                    Ok(buf)
                }
            }
        }
    }
}

/// Decrypt `source` into `dest`, streaming.
///
/// Returns the number of plaintext bytes written. `expected_len` truncates the
/// output; without it the whole decrypted stream is written, padding included.
pub fn decrypt_to_file(
    key: &[u8],
    source: &Path,
    dest: &Path,
    expected_len: Option<u64>,
) -> Result<u64> {
    let mut decryptor = decryptor(key)?;

    let mut input = std::fs::File::open(source).at(source)?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).at(parent)?;
    }
    // Write to a uniquely-named temporary file and rename, so an interrupted
    // decryption cannot leave a half-written file that later looks complete
    // — and, just as important, so two callers decrypting the same
    // destination concurrently (two range requests for one video, two
    // thumbnail renders of one photo) cannot truncate each other's
    // in-progress write out from under them. A name shared between callers
    // is not safe even when their output would be identical: it is a shared
    // mutable file, and `File::create` truncates on open regardless of what
    // the previous opener was doing with it.
    let temp = crate::cache::CacheDir::unique_temp_path(dest);
    let mut output = std::fs::File::create(&temp).at(&temp)?;

    let limit = expected_len.unwrap_or(u64::MAX);
    let mut written = 0u64;
    let mut buf = vec![0u8; CHUNK];
    let mut carry: Vec<u8> = Vec::with_capacity(BLOCK);

    loop {
        let read = input.read(&mut buf).at(source)?;
        if read == 0 {
            break;
        }

        // Only whole blocks can be decrypted; anything left over joins the
        // next chunk. A well-formed backup file is block-aligned, so the carry
        // stays empty, but a truncated one must not panic.
        carry.extend_from_slice(&buf[..read]);
        let usable = carry.len() - (carry.len() % BLOCK);
        if usable == 0 {
            continue;
        }

        let mut blocks: Vec<u8> = carry.drain(..usable).collect();
        // Block at a time through a safe, length-checked conversion:
        // `Block` is a `hybrid_array::Array<u8, U16>`, which implements
        // `TryFrom<&mut [u8]>`. No unsafe and no copy. `chunks_exact_mut`
        // guarantees the length, so the conversion cannot actually fail.
        // `as_chunks_mut` hands out `&mut [u8; 16]`, which converts into the
        // `&mut Block` the cipher wants infallibly: no unsafe, no copy, and no
        // error path that cannot happen.
        for chunk in blocks.as_chunks_mut::<BLOCK>().0 {
            decryptor.decrypt_block(chunk.into());
        }

        let remaining = limit.saturating_sub(written);
        let take = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(blocks.len());
        if take > 0 {
            output.write_all(&blocks[..take]).at(&temp)?;
            written += take as u64;
        }
        if written >= limit {
            break;
        }
    }

    output.flush().at(&temp)?;
    drop(output);
    std::fs::rename(&temp, dest).at(dest)?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::{BlockModeEncrypt, block_padding::NoPadding};

    type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;

    /// Encrypt with PKCS#7 the way the device does, for round-trip tests.
    fn encrypt(key: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let iv = [0u8; BLOCK];
        let enc = Aes256CbcEnc::new_from_slices(key, &iv).unwrap();
        let mut buf = vec![0u8; plaintext.len() + BLOCK];
        let len = enc
            .encrypt_padded_b2b::<Pkcs7>(plaintext, &mut buf)
            .unwrap()
            .len();
        buf.truncate(len);
        buf
    }

    const KEY: [u8; 32] = [0x7fu8; 32];

    #[test]
    fn round_trips_with_a_recorded_length() {
        for len in [0usize, 1, 15, 16, 17, 1000, 4096] {
            let plain: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let cipher = encrypt(&KEY, &plain);
            let out = decrypt_buffer(&KEY, &cipher, Some(len as u64)).unwrap();
            assert_eq!(out, plain, "length {len}");
        }
    }

    #[test]
    fn round_trips_by_stripping_padding_when_no_length_is_known() {
        // This is the Manifest.db case.
        let plain = b"SQLite format 3\0and then some more content".to_vec();
        let cipher = encrypt(&KEY, &plain);
        let out = decrypt_buffer(&KEY, &cipher, None).unwrap();
        assert_eq!(out, plain);
    }

    #[test]
    fn a_wrong_key_does_not_error_but_gives_different_bytes() {
        // CBC has no integrity check, so a wrong key yields garbage rather
        // than a failure. The keybag's wrap is what catches a bad password;
        // this test documents that this layer cannot.
        let plain = vec![0xabu8; 64];
        let cipher = encrypt(&KEY, &plain);
        let wrong = [0x01u8; 32];
        let out = decrypt_buffer(&wrong, &cipher, Some(64)).unwrap();
        assert_ne!(out, plain);
        assert_eq!(out.len(), 64);
    }

    #[test]
    fn misaligned_ciphertext_is_refused() {
        let err = decrypt_buffer(&KEY, &[0u8; 17], Some(17)).unwrap_err();
        assert_eq!(err.code(), "decrypt_failed");
        assert!(err.to_string().contains("multiple"), "{err}");
    }

    #[test]
    fn a_wrong_key_length_is_refused() {
        assert!(decrypt_buffer(&[0u8; 16], &[0u8; 16], None).is_err());
    }

    #[test]
    fn empty_input_decrypts_to_empty() {
        assert!(decrypt_buffer(&KEY, &[], None).unwrap().is_empty());
    }

    #[test]
    fn invalid_padding_still_yields_the_bytes() {
        // Random ciphertext almost never decrypts to valid PKCS#7. Rather than
        // failing, the no-length path hands back the raw decrypted blocks.
        let cipher = [0x5au8; 48];
        let out = decrypt_buffer(&KEY, &cipher, None).unwrap();
        assert_eq!(out.len(), 48, "all blocks should come through");
    }

    #[test]
    fn streaming_matches_the_buffered_path() {
        let tmp = tempfile::tempdir().unwrap();
        // Larger than one CHUNK, so the streaming loop really iterates, and
        // not a multiple of it.
        let len = CHUNK * 2 + 777;
        let plain: Vec<u8> = (0..len).map(|i| (i % 253) as u8).collect();
        let cipher = encrypt(&KEY, &plain);

        let source = tmp.path().join("cipher.bin");
        std::fs::write(&source, &cipher).unwrap();
        let dest = tmp.path().join("nested/plain.bin");

        let written = decrypt_to_file(&KEY, &source, &dest, Some(len as u64)).unwrap();
        assert_eq!(written, len as u64);
        assert!(dest.is_file(), "parent directories should be created");
        assert_eq!(std::fs::read(&dest).unwrap(), plain);

        // And it agrees with the in-memory path byte for byte.
        let buffered = decrypt_buffer(&KEY, &cipher, Some(len as u64)).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), buffered);
    }

    #[test]
    fn streaming_leaves_no_partial_file_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let plain = vec![1u8; 64];
        let cipher = encrypt(&KEY, &plain);
        let source = tmp.path().join("c.bin");
        std::fs::write(&source, &cipher).unwrap();
        let dest = tmp.path().join("p.bin");

        decrypt_to_file(&KEY, &source, &dest, Some(64)).unwrap();
        assert!(dest.exists());
        // The temp name is unique per call (two concurrent decrypts to the
        // same destination must not share one), so check the directory for
        // any leftover rather than guessing one fixed name.
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .filter(|name| name != "c.bin" && name != "p.bin")
            .collect();
        assert!(
            leftovers.is_empty(),
            "a temporary file should have been renamed away, found {leftovers:?}"
        );
    }

    #[test]
    fn streaming_truncates_to_the_recorded_length() {
        let tmp = tempfile::tempdir().unwrap();
        // 100 bytes of content pads out to 112 bytes of ciphertext.
        let plain = vec![9u8; 100];
        let cipher = encrypt(&KEY, &plain);
        assert_eq!(cipher.len(), 112);

        let source = tmp.path().join("c.bin");
        std::fs::write(&source, &cipher).unwrap();
        let dest = tmp.path().join("p.bin");

        let written = decrypt_to_file(&KEY, &source, &dest, Some(100)).unwrap();
        assert_eq!(written, 100, "the padding must not reach the output");
        assert_eq!(std::fs::read(&dest).unwrap(), plain);
    }

    #[test]
    fn a_truncated_ciphertext_does_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let cipher = encrypt(&KEY, &[3u8; 64]);
        // Cut mid-block.
        let source = tmp.path().join("c.bin");
        std::fs::write(&source, &cipher[..cipher.len() - 5]).unwrap();
        let dest = tmp.path().join("p.bin");

        // Whatever it does, it must not panic and must not claim more than it
        // wrote.
        let written = decrypt_to_file(&KEY, &source, &dest, Some(64)).unwrap();
        assert!(written <= 64);
    }

    #[test]
    fn no_padding_decrypt_is_exact_for_aligned_input() {
        // Sanity check on the primitive the streaming path relies on.
        let plain = vec![7u8; 32];
        let iv = [0u8; BLOCK];
        let enc = Aes256CbcEnc::new_from_slices(&KEY, &iv).unwrap();
        let mut cipher = vec![0u8; 32];
        enc.encrypt_padded_b2b::<NoPadding>(&plain, &mut cipher)
            .unwrap();

        let out = decrypt_buffer(&KEY, &cipher, Some(32)).unwrap();
        assert_eq!(out, plain);
    }
}
