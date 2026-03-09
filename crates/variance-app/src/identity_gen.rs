use anyhow::Result;
use bip39::{Language, Mnemonic};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand_core::{OsRng, RngCore};
use vodozemac::olm::Account;

use crate::state::IdentityFile;

pub fn derive_signing_key(mnemonic: &Mnemonic) -> SigningKey {
    let seed = mnemonic.to_seed("");
    SigningKey::from_bytes(&seed[..32].try_into().expect("seed is at least 32 bytes"))
}

pub fn did_from_verifying_key(key: &VerifyingKey) -> String {
    format!("did:variance:{}", hex::encode(&key.to_bytes()[..8]))
}

/// Generate a new identity with a random BIP39 mnemonic.
///
/// Returns the identity file and the 12-word mnemonic phrase (space-separated).
pub fn generate() -> Result<(IdentityFile, String)> {
    let mut entropy = [0u8; 16];
    OsRng.fill_bytes(&mut entropy);
    let mnemonic = Mnemonic::from_entropy_in(Language::English, &entropy)
        .map_err(|e| anyhow::anyhow!("Failed to generate mnemonic: {}", e))?;

    let signing_key = derive_signing_key(&mnemonic);
    let verifying_key = signing_key.verifying_key();
    let signaling_key = ed25519_dalek::SigningKey::generate(&mut OsRng);
    let olm_account = Account::new();
    let did = did_from_verifying_key(&verifying_key);

    let phrase = mnemonic.to_string();

    let identity = IdentityFile {
        did,
        signing_key: hex::encode(signing_key.to_bytes()),
        verifying_key: hex::encode(verifying_key.to_bytes()),
        signaling_key: hex::encode(signaling_key.to_bytes()),
        olm_account_pickle: serde_json::to_string(&olm_account.pickle())
            .map_err(|e| anyhow::anyhow!("Failed to serialize Olm account: {}", e))?,
        username: None,
        discriminator: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        ipns_key: None,
    };

    Ok((identity, phrase))
}

/// Recover an identity from a BIP39 mnemonic phrase.
///
/// The signaling key and Olm account are regenerated (they are not recoverable
/// from the mnemonic). Users must restore those from backup if needed.
pub fn recover(mnemonic_phrase: &str) -> Result<IdentityFile> {
    let mnemonic = Mnemonic::parse_in(Language::English, mnemonic_phrase)
        .map_err(|e| anyhow::anyhow!("Invalid mnemonic: {}", e))?;

    if mnemonic.word_count() != 12 {
        anyhow::bail!("Expected 12 words, got {}", mnemonic.word_count());
    }

    let signing_key = derive_signing_key(&mnemonic);
    let verifying_key = signing_key.verifying_key();
    let signaling_key = ed25519_dalek::SigningKey::generate(&mut OsRng);
    let olm_account = Account::new();
    let did = did_from_verifying_key(&verifying_key);

    Ok(IdentityFile {
        did,
        signing_key: hex::encode(signing_key.to_bytes()),
        verifying_key: hex::encode(verifying_key.to_bytes()),
        signaling_key: hex::encode(signaling_key.to_bytes()),
        olm_account_pickle: serde_json::to_string(&olm_account.pickle())
            .map_err(|e| anyhow::anyhow!("Failed to serialize Olm account: {}", e))?,
        username: None,
        discriminator: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        ipns_key: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_trip() {
        let (identity, phrase) = generate().unwrap();
        let recovered = recover(&phrase).unwrap();
        assert_eq!(identity.did, recovered.did);
        assert_eq!(identity.signing_key, recovered.signing_key);
        assert_eq!(identity.verifying_key, recovered.verifying_key);
    }

    #[test]
    fn test_derive_signing_key_deterministic() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let mnemonic = Mnemonic::parse_in(Language::English, phrase).unwrap();
        let key1 = derive_signing_key(&mnemonic);
        let key2 = derive_signing_key(&mnemonic);
        assert_eq!(key1.to_bytes(), key2.to_bytes());
    }

    #[test]
    fn test_recover_rejects_invalid_mnemonic() {
        assert!(recover("not valid words at all for this test").is_err());
    }

    #[test]
    fn test_recover_rejects_wrong_word_count() {
        // 11 valid BIP39 words — bip39 crate rejects wrong-length inputs
        let short = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon";
        assert!(recover(short).is_err());
    }

    #[test]
    fn test_generate_produces_valid_olm_account_pickle() {
        let (identity, _phrase) = generate().unwrap();
        let pickle: vodozemac::olm::AccountPickle =
            serde_json::from_str(&identity.olm_account_pickle).unwrap();
        let account = Account::from_pickle(pickle);
        // Account must have a valid Curve25519 key
        assert_ne!(account.curve25519_key().to_bytes(), [0u8; 32]);
    }
}
