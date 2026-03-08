use sha2::{Digest, Sha256};

/// Derive the relay mailbox token from an Ed25519 signing key (raw 32 bytes).
///
/// Token = SHA-256(signing_key_bytes || "variance-mailbox-v1").
///
/// The relay indexes messages by this opaque 32-byte value rather than the
/// recipient's DID, so a relay operator cannot link stored messages to
/// human-readable identities without independently resolving every DID on
/// the network.
///
/// The token is stable and deterministic — the same signing key always
/// produces the same token, so no extra storage is needed.
pub fn mailbox_token(signing_key_bytes: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(signing_key_bytes);
    h.update(b"variance-mailbox-v1");
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic() {
        let key = [1u8; 32];
        assert_eq!(mailbox_token(&key), mailbox_token(&key));
    }

    #[test]
    fn test_different_keys_produce_different_tokens() {
        let token_a = mailbox_token(&[1u8; 32]);
        let token_b = mailbox_token(&[2u8; 32]);
        assert_ne!(token_a, token_b);
    }

    #[test]
    fn test_output_length() {
        let token = mailbox_token(&[0u8; 32]);
        assert_eq!(token.len(), 32);
    }
}
