//! Argon2id + AES-256-GCM encryption for the identity file at rest.
//!
//! ## Format
//!
//! Encrypted files start with a 4-byte magic header (`VEID`) followed by a
//! version byte, then a CBOR-like binary layout:
//!
//! ```text
//! [ magic: 4 ][ version: 1 ][ salt: 32 ][ nonce: 12 ][ ciphertext: N ]
//! ```
//!
//! Plaintext files are detected by the absence of the magic header.
//! `load_identity` handles both transparently so old files keep working.

use aes_gcm::{
    aead::{Aead, OsRng},
    AeadCore, Aes256Gcm, Key, KeyInit,
};
use argon2::{password_hash::SaltString, Argon2};

const MAGIC: &[u8; 4] = b"VEID";
const VERSION: u8 = 1;

/// Returns `true` if `data` starts with the encrypted-identity magic header.
pub fn is_encrypted(data: &[u8]) -> bool {
    data.starts_with(MAGIC)
}

/// Encrypt `plaintext` with Argon2id KDF + AES-256-GCM.
///
/// Returns the binary-encoded ciphertext blob.
pub fn encrypt(plaintext: &str, passphrase: &str) -> anyhow::Result<Vec<u8>> {
    // Derive a 32-byte key with Argon2id.
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let mut key_bytes = [0u8; 32];
    argon2
        .hash_password_into(
            passphrase.as_bytes(),
            salt.as_str().as_bytes(),
            &mut key_bytes,
        )
        .map_err(|e| anyhow::anyhow!("Argon2 KDF failed: {}", e))?;

    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| anyhow::anyhow!("AES-GCM encryption failed: {}", e))?;

    // Encode salt as raw bytes (32 bytes from the base64-decoded salt).
    // We store the salt string bytes directly (≤ 44 ASCII chars), preceded by length.
    let salt_bytes = salt.as_str().as_bytes();
    let salt_len = salt_bytes.len() as u8;

    let mut out = Vec::with_capacity(5 + 1 + salt_len as usize + 12 + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(salt_len);
    out.extend_from_slice(salt_bytes);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);

    Ok(out)
}

/// Decrypt a blob produced by [`encrypt`].
///
/// Returns the plaintext JSON string on success.
pub fn decrypt(data: &[u8], passphrase: &str) -> anyhow::Result<String> {
    if !is_encrypted(data) {
        anyhow::bail!("Data is not an encrypted identity file");
    }

    let mut cursor = MAGIC.len();

    let version = *data
        .get(cursor)
        .ok_or_else(|| anyhow::anyhow!("Truncated encrypted file (version byte)"))?;
    cursor += 1;

    if version != VERSION {
        anyhow::bail!("Unsupported encrypted identity version: {}", version);
    }

    let salt_len = *data
        .get(cursor)
        .ok_or_else(|| anyhow::anyhow!("Truncated encrypted file (salt_len)"))?
        as usize;
    cursor += 1;

    let salt_bytes = data
        .get(cursor..cursor + salt_len)
        .ok_or_else(|| anyhow::anyhow!("Truncated encrypted file (salt)"))?;
    cursor += salt_len;

    let nonce_bytes = data
        .get(cursor..cursor + 12)
        .ok_or_else(|| anyhow::anyhow!("Truncated encrypted file (nonce)"))?;
    cursor += 12;

    let ciphertext = data
        .get(cursor..)
        .ok_or_else(|| anyhow::anyhow!("Truncated encrypted file (ciphertext)"))?;

    // Derive key with Argon2id using the stored salt.
    let salt_str = std::str::from_utf8(salt_bytes)
        .map_err(|_| anyhow::anyhow!("Invalid salt encoding in encrypted file"))?;
    let argon2 = Argon2::default();
    let mut key_bytes = [0u8; 32];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt_str.as_bytes(), &mut key_bytes)
        .map_err(|e| anyhow::anyhow!("Argon2 KDF failed: {}", e))?;

    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = aes_gcm::Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("Decryption failed (wrong passphrase?)"))?;

    String::from_utf8(plaintext)
        .map_err(|e| anyhow::anyhow!("Decrypted data is not valid UTF-8: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let plaintext = r#"{"did":"did:variance:test","signing_key":"aabbcc"}"#;
        let passphrase = "correct horse battery staple";

        let blob = encrypt(plaintext, passphrase).unwrap();
        assert!(is_encrypted(&blob));

        let recovered = decrypt(&blob, passphrase).unwrap();
        assert_eq!(recovered, plaintext);
    }

    #[test]
    fn test_wrong_passphrase_rejected() {
        let plaintext = r#"{"did":"did:variance:test"}"#;
        let blob = encrypt(plaintext, "right").unwrap();

        let result = decrypt(&blob, "wrong");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Decryption failed"));
    }

    #[test]
    fn test_plaintext_not_detected_as_encrypted() {
        let json = r#"{"did":"did:variance:test"}"#;
        assert!(!is_encrypted(json.as_bytes()));
    }

    #[test]
    fn test_is_encrypted() {
        let blob = encrypt("hello", "pass").unwrap();
        assert!(is_encrypted(&blob));
    }
}
