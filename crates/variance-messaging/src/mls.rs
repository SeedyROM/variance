//! MLS (RFC 9420) group encryption via openmls.
//!
//! Replaces the hand-rolled AES-256-GCM + X25519 group crypto in `group.rs`.
//! This module provides `MlsGroupHandler` which manages MLS groups with per-message
//! forward secrecy and post-compromise security — properties the legacy scheme lacks.
//!
//! # Architecture
//!
//! - **Ciphersuite**: `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519` —
//!   matches our existing Ed25519 signing keys and AES-128-GCM for message encryption.
//! - **Credential**: `BasicCredential` containing the DID string bytes.
//! - **Provider**: `OpenMlsRustCrypto` bundles crypto, in-memory storage, and RNG.
//! - **Key bridge**: Our existing `ed25519_dalek::SigningKey` is imported into openmls
//!   via `SignatureKeyPair::from_raw()`.
//!
//! # Phase 1 (current)
//!
//! Standalone module behind `pub mod mls;`. No callers yet — the existing `group.rs`
//! path is untouched. Phase 2 will wire this into `AppState` and replace `group.rs`
//! encrypt/decrypt calls.

use std::sync::Arc;
use std::sync::RwLock;

use dashmap::DashMap;
use ed25519_dalek::SigningKey;
use openmls::messages::group_info::GroupInfo;
use openmls::prelude::tls_codec::{Deserialize, Serialize};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;

use crate::error::{Error, Result};

/// The ciphersuite used for all Variance MLS groups.
///
/// X25519 for DHKEM, AES-128-GCM for AEAD, SHA-256 for hashing, Ed25519 for signatures.
/// This is the closest match to our existing crypto primitives.
const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

/// MLS-based group encryption handler.
///
/// Each instance is bound to a single identity (DID + signing key). It manages
/// creation, membership changes, and encrypt/decrypt for all groups the local
/// user participates in.
pub struct MlsGroupHandler {
    /// Local DID (used as the BasicCredential identity).
    local_did: String,

    /// openmls crypto provider — bundles RustCrypto primitives, an in-memory
    /// key store, and a system RNG.
    provider: OpenMlsRustCrypto,

    /// The MLS `SignatureKeyPair` imported from our Ed25519 signing key.
    /// Stored in the provider's key store on construction.
    signature_keypair: SignatureKeyPair,

    /// The `CredentialWithKey` derived from the local DID + public key.
    credential_with_key: CredentialWithKey,

    /// Active MLS groups keyed by group ID string.
    ///
    /// Each `MlsGroup` contains the full ratchet tree and epoch state.
    /// `RwLock` is used because `MlsGroup` mutating methods take `&mut self`
    /// (e.g. `create_message`, `process_message`, `add_members`).
    groups: DashMap<String, Arc<RwLock<MlsGroup>>>,
}

/// The output of an `add_member` operation.
///
/// Contains the commit (broadcast to existing members via GossipSub)
/// and the welcome (sent directly to the new member).
pub struct AddMemberResult {
    /// The commit message — broadcast to existing group members.
    pub commit: MlsMessageOut,
    /// The welcome message — sent to the newly added member.
    pub welcome: MlsMessageOut,
    /// Optional group info (present when ratchet_tree_extension is enabled).
    pub group_info: Option<GroupInfo>,
}

/// The output of a `remove_member` operation.
pub struct RemoveMemberResult {
    /// The commit message — broadcast to remaining group members.
    pub commit: MlsMessageOut,
    /// Optional welcome (if the commit re-adds someone).
    pub welcome: Option<MlsMessageOut>,
    /// Optional group info.
    pub group_info: Option<GroupInfo>,
}

/// A decrypted application message from `process_message`.
pub struct DecryptedMessage {
    /// The plaintext bytes.
    pub plaintext: Vec<u8>,
    /// The credential of the sender.
    pub sender_credential: Credential,
}

impl MlsGroupHandler {
    /// Create a new MLS group handler from an existing Ed25519 signing key.
    ///
    /// The signing key is imported into openmls and stored in the provider's key store.
    /// A `BasicCredential` is created from the DID string.
    pub fn new(local_did: String, signing_key: &SigningKey) -> Result<Self> {
        let provider = OpenMlsRustCrypto::default();

        // Import our Ed25519 key into openmls's SignatureKeyPair.
        // from_raw takes (scheme, private_key_bytes, public_key_bytes).
        let private_bytes = signing_key.to_bytes();
        let public_bytes = signing_key.verifying_key().to_bytes();

        let signature_keypair = SignatureKeyPair::from_raw(
            SignatureScheme::ED25519,
            private_bytes.to_vec(),
            public_bytes.to_vec(),
        );

        // Store the keypair in the provider's key store so openmls can find it.
        signature_keypair
            .store(provider.storage())
            .map_err(|e| Error::MlsKeyPackage {
                message: format!("Failed to store signature keypair: {e:?}"),
            })?;

        // Build BasicCredential from DID bytes.
        let credential = BasicCredential::new(local_did.as_bytes().to_vec());
        let credential_with_key = CredentialWithKey {
            credential: credential.into(),
            signature_key: signature_keypair.to_public_vec().into(),
        };

        Ok(Self {
            local_did,
            provider,
            signature_keypair,
            credential_with_key,
            groups: DashMap::new(),
        })
    }

    /// Generate a fresh `KeyPackage` for distributing to peers.
    ///
    /// Other users need our KeyPackage to add us to their MLS groups.
    /// KeyPackages are single-use; generate a new one for each group join.
    pub fn generate_key_package(&self) -> Result<KeyPackage> {
        let bundle = KeyPackage::builder()
            .build(
                CIPHERSUITE,
                &self.provider,
                &self.signature_keypair,
                self.credential_with_key.clone(),
            )
            .map_err(|e| Error::MlsKeyPackage {
                message: format!("Failed to build KeyPackage: {e:?}"),
            })?;
        Ok(bundle.key_package().clone())
    }

    /// Create a new MLS group.
    ///
    /// The local user becomes the sole member (and implicit admin).
    /// Returns the group ID (opaque bytes chosen by openmls) as a hex string.
    pub fn create_group(&self, group_id: &str) -> Result<()> {
        let mls_group_create_config = MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .build();

        let group = MlsGroup::new_with_group_id(
            &self.provider,
            &self.signature_keypair,
            &mls_group_create_config,
            GroupId::from_slice(group_id.as_bytes()),
            self.credential_with_key.clone(),
        )
        .map_err(|e| Error::MlsGroup {
            message: format!("Failed to create MLS group: {e:?}"),
        })?;

        self.groups
            .insert(group_id.to_string(), Arc::new(RwLock::new(group)));

        Ok(())
    }

    /// Add a member to an existing group.
    ///
    /// The caller provides the new member's `KeyPackage` (obtained via identity resolution).
    /// Returns the commit (to broadcast) and welcome (to send to the new member).
    ///
    /// The caller must call `merge_pending_commit` after confirming the commit
    /// was delivered to the group.
    pub fn add_member(&self, group_id: &str, key_package: KeyPackage) -> Result<AddMemberResult> {
        let group_lock = self
            .groups
            .get(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        let mut group = group_lock.write().expect("group lock poisoned");

        let (commit, welcome, group_info) = group
            .add_members(&self.provider, &self.signature_keypair, &[key_package])
            .map_err(|e| Error::MlsCommit {
                message: format!("Failed to add member: {e:?}"),
            })?;

        // Immediately merge our own pending commit since we're the committer
        // and in a P2P context there's no DS to reject it.
        group
            .merge_pending_commit(&self.provider)
            .map_err(|e| Error::MlsCommit {
                message: format!("Failed to merge pending commit after add: {e:?}"),
            })?;

        Ok(AddMemberResult {
            commit,
            welcome,
            group_info,
        })
    }

    /// Remove a member from a group by their leaf index.
    ///
    /// Finding the `LeafNodeIndex` for a given DID: iterate `group.members()` and
    /// match on credential identity bytes.
    pub fn remove_member(
        &self,
        group_id: &str,
        member_index: LeafNodeIndex,
    ) -> Result<RemoveMemberResult> {
        let group_lock = self
            .groups
            .get(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        let mut group = group_lock.write().expect("group lock poisoned");

        let (commit, welcome, group_info) = group
            .remove_members(&self.provider, &self.signature_keypair, &[member_index])
            .map_err(|e| Error::MlsCommit {
                message: format!("Failed to remove member: {e:?}"),
            })?;

        group
            .merge_pending_commit(&self.provider)
            .map_err(|e| Error::MlsCommit {
                message: format!("Failed to merge pending commit after remove: {e:?}"),
            })?;

        Ok(RemoveMemberResult {
            commit,
            welcome,
            group_info,
        })
    }

    /// Find the `LeafNodeIndex` for a member by DID.
    ///
    /// Returns `None` if no member with that DID is in the group.
    pub fn find_member_index(
        &self,
        group_id: &str,
        member_did: &str,
    ) -> Result<Option<LeafNodeIndex>> {
        let group_lock = self
            .groups
            .get(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        let group = group_lock.read().expect("group lock poisoned");
        let did_bytes = member_did.as_bytes();

        for member in group.members() {
            if member.credential.serialized_content() == did_bytes {
                return Ok(Some(member.index));
            }
        }

        Ok(None)
    }

    /// Encrypt a message for a group.
    ///
    /// Returns the serialized `MlsMessageOut` ready for GossipSub broadcast.
    pub fn encrypt_message(&self, group_id: &str, plaintext: &[u8]) -> Result<MlsMessageOut> {
        let group_lock = self
            .groups
            .get(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        let mut group = group_lock.write().expect("group lock poisoned");

        group
            .create_message(&self.provider, &self.signature_keypair, plaintext)
            .map_err(|e| Error::Encryption {
                message: format!("MLS encrypt failed: {e:?}"),
            })
    }

    /// Process an incoming MLS message (application message, commit, or proposal).
    ///
    /// For application messages, returns `Some(DecryptedMessage)`.
    /// For commits and proposals, applies them to group state and returns `None`.
    pub fn process_message(
        &self,
        group_id: &str,
        message: MlsMessageIn,
    ) -> Result<Option<DecryptedMessage>> {
        let group_lock = self
            .groups
            .get(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        let mut group = group_lock.write().expect("group lock poisoned");

        let protocol_message: ProtocolMessage =
            message
                .try_into_protocol_message()
                .map_err(|e| Error::Decryption {
                    message: format!("Not a protocol message: {e:?}"),
                })?;

        let processed = group
            .process_message(&self.provider, protocol_message)
            .map_err(|e| Error::Decryption {
                message: format!("MLS process_message failed: {e:?}"),
            })?;

        let sender_credential = processed.credential().clone();

        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(app_msg) => Ok(Some(DecryptedMessage {
                plaintext: app_msg.into_bytes(),
                sender_credential,
            })),
            ProcessedMessageContent::ProposalMessage(proposal) => {
                // Store the proposal for later commit processing
                group
                    .store_pending_proposal(self.provider.storage(), *proposal)
                    .map_err(|e| Error::MlsGroup {
                        message: format!("Failed to store proposal: {e:?}"),
                    })?;
                Ok(None)
            }
            ProcessedMessageContent::StagedCommitMessage(staged_commit) => {
                // Merge the staged commit to advance the epoch
                group
                    .merge_staged_commit(&self.provider, *staged_commit)
                    .map_err(|e| Error::MlsCommit {
                        message: format!("Failed to merge staged commit: {e:?}"),
                    })?;
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Join a group from a Welcome message.
    ///
    /// Called when another member adds us and sends the Welcome directly.
    /// Returns the group ID of the joined group.
    pub fn join_group_from_welcome(&self, welcome: MlsMessageIn) -> Result<String> {
        let welcome_msg = match welcome.extract() {
            MlsMessageBodyIn::Welcome(w) => w,
            _ => {
                return Err(Error::MlsWelcome {
                    message: "Message is not a Welcome".to_string(),
                })
            }
        };

        let mls_group_config = MlsGroupJoinConfig::builder()
            .use_ratchet_tree_extension(true)
            .build();

        let staged = StagedWelcome::new_from_welcome(
            &self.provider,
            &mls_group_config,
            welcome_msg,
            None, // ratchet tree is in the extension
        )
        .map_err(|e| Error::MlsWelcome {
            message: format!("Failed to stage welcome: {e:?}"),
        })?;

        let group = staged
            .into_group(&self.provider)
            .map_err(|e| Error::MlsWelcome {
                message: format!("Failed to join group from welcome: {e:?}"),
            })?;

        let group_id = String::from_utf8_lossy(group.group_id().as_slice()).to_string();

        self.groups
            .insert(group_id.clone(), Arc::new(RwLock::new(group)));

        Ok(group_id)
    }

    /// List members of a group, returning their DID strings.
    pub fn list_members(&self, group_id: &str) -> Result<Vec<String>> {
        let group_lock = self
            .groups
            .get(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        let group = group_lock.read().expect("group lock poisoned");
        let mut dids = Vec::new();

        for member in group.members() {
            let did = String::from_utf8_lossy(member.credential.serialized_content()).to_string();
            dids.push(did);
        }

        Ok(dids)
    }

    /// Check if the local user is a member of the given group.
    pub fn is_member(&self, group_id: &str) -> bool {
        self.groups.contains_key(group_id)
    }

    /// Get the current epoch of a group.
    pub fn epoch(&self, group_id: &str) -> Result<u64> {
        let group_lock = self
            .groups
            .get(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        let group = group_lock.read().expect("group lock poisoned");
        Ok(group.epoch().as_u64())
    }

    /// Leave a group.
    ///
    /// Sends a remove proposal for self. Another member must commit it.
    /// Returns the proposal message to broadcast.
    pub fn leave_group(&self, group_id: &str) -> Result<MlsMessageOut> {
        let group_lock = self
            .groups
            .get(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        let mut group = group_lock.write().expect("group lock poisoned");

        let msg = group
            .leave_group(&self.provider, &self.signature_keypair)
            .map_err(|e| Error::MlsGroup {
                message: format!("Failed to leave group: {e:?}"),
            })?;

        Ok(msg)
    }

    /// Remove a group from local state (after leaving or being removed).
    pub fn remove_group(&self, group_id: &str) {
        self.groups.remove(group_id);
    }

    /// Serialize an `MlsMessageOut` to bytes for wire transport.
    pub fn serialize_message(message: &MlsMessageOut) -> Result<Vec<u8>> {
        message
            .tls_serialize_detached()
            .map_err(|e| Error::Encryption {
                message: format!("Failed to serialize MLS message: {e:?}"),
            })
    }

    /// Serialize any TLS-serializable type (e.g. `KeyPackage`) to bytes.
    pub fn serialize_message_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
        value
            .tls_serialize_detached()
            .map_err(|e| Error::Encryption {
                message: format!("Failed to TLS-serialize: {e:?}"),
            })
    }

    /// Deserialize bytes into an `MlsMessageIn`.
    pub fn deserialize_message(bytes: &[u8]) -> Result<MlsMessageIn> {
        MlsMessageIn::tls_deserialize_exact(bytes).map_err(|e| Error::Decryption {
            message: format!("Failed to deserialize MLS message: {e:?}"),
        })
    }

    /// Deserialize bytes into a `KeyPackage`.
    pub fn deserialize_key_package(bytes: &[u8]) -> Result<KeyPackageIn> {
        KeyPackageIn::tls_deserialize_exact(bytes).map_err(|e| Error::Decryption {
            message: format!("Failed to deserialize KeyPackage: {e:?}"),
        })
    }

    /// Validate an incoming `KeyPackageIn` into a verified `KeyPackage`.
    pub fn validate_key_package(&self, kp_in: KeyPackageIn) -> Result<KeyPackage> {
        kp_in
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|e| Error::MlsKeyPackage {
                message: format!("KeyPackage validation failed: {e:?}"),
            })
    }

    /// Get all group IDs.
    pub fn group_ids(&self) -> Vec<String> {
        self.groups.iter().map(|e| e.key().clone()).collect()
    }

    /// Access the local DID.
    pub fn local_did(&self) -> &str {
        &self.local_did
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_signing_key() -> SigningKey {
        SigningKey::generate(&mut rand::rngs::OsRng)
    }

    #[test]
    fn create_handler_and_generate_key_package() {
        let sk = test_signing_key();
        let handler = MlsGroupHandler::new("did:key:test1".to_string(), &sk).unwrap();
        let kp = handler.generate_key_package().unwrap();

        // KeyPackage should use our ciphersuite
        assert_eq!(kp.ciphersuite(), CIPHERSUITE);
    }

    #[test]
    fn create_group_and_check_membership() {
        let sk = test_signing_key();
        let handler = MlsGroupHandler::new("did:key:test1".to_string(), &sk).unwrap();

        handler.create_group("test-group-1").unwrap();

        assert!(handler.is_member("test-group-1"));
        assert!(!handler.is_member("nonexistent"));

        let members = handler.list_members("test-group-1").unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0], "did:key:test1");
    }

    #[test]
    fn add_member_and_join_via_welcome() {
        // Alice creates a group
        let alice_sk = test_signing_key();
        let alice = MlsGroupHandler::new("did:key:alice".to_string(), &alice_sk).unwrap();
        alice.create_group("group-ab").unwrap();

        // Bob generates a KeyPackage
        let bob_sk = test_signing_key();
        let bob = MlsGroupHandler::new("did:key:bob".to_string(), &bob_sk).unwrap();
        let bob_kp = bob.generate_key_package().unwrap();

        // Alice adds Bob
        let result = alice.add_member("group-ab", bob_kp).unwrap();

        // Bob joins via Welcome
        let welcome_bytes = MlsGroupHandler::serialize_message(&result.welcome).unwrap();
        let welcome_in = MlsGroupHandler::deserialize_message(&welcome_bytes).unwrap();
        let joined_id = bob.join_group_from_welcome(welcome_in).unwrap();
        assert_eq!(joined_id, "group-ab");

        // Both see each other as members
        let alice_members = alice.list_members("group-ab").unwrap();
        assert_eq!(alice_members.len(), 2);

        let bob_members = bob.list_members("group-ab").unwrap();
        assert_eq!(bob_members.len(), 2);
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        // Set up Alice and Bob in a group
        let alice_sk = test_signing_key();
        let alice = MlsGroupHandler::new("did:key:alice".to_string(), &alice_sk).unwrap();
        alice.create_group("group-msg").unwrap();

        let bob_sk = test_signing_key();
        let bob = MlsGroupHandler::new("did:key:bob".to_string(), &bob_sk).unwrap();
        let bob_kp = bob.generate_key_package().unwrap();

        let add_result = alice.add_member("group-msg", bob_kp).unwrap();

        let welcome_bytes = MlsGroupHandler::serialize_message(&add_result.welcome).unwrap();
        let welcome_in = MlsGroupHandler::deserialize_message(&welcome_bytes).unwrap();
        bob.join_group_from_welcome(welcome_in).unwrap();

        // Bob joins via Welcome which already includes the post-commit state,
        // so he does NOT need to process Alice's commit separately.

        // Alice sends a message
        let plaintext = b"Hello from Alice!";
        let encrypted = alice.encrypt_message("group-msg", plaintext).unwrap();

        // Bob decrypts
        let enc_bytes = MlsGroupHandler::serialize_message(&encrypted).unwrap();
        let enc_in = MlsGroupHandler::deserialize_message(&enc_bytes).unwrap();
        let decrypted = bob
            .process_message("group-msg", enc_in)
            .unwrap()
            .expect("should be an application message");

        assert_eq!(decrypted.plaintext, plaintext);

        // Sender credential should be Alice's DID
        let sender_did = String::from_utf8_lossy(decrypted.sender_credential.serialized_content());
        assert_eq!(sender_did, "did:key:alice");
    }

    #[test]
    fn remove_member_rotates_epoch() {
        let alice_sk = test_signing_key();
        let alice = MlsGroupHandler::new("did:key:alice".to_string(), &alice_sk).unwrap();
        alice.create_group("group-rm").unwrap();

        let bob_sk = test_signing_key();
        let bob = MlsGroupHandler::new("did:key:bob".to_string(), &bob_sk).unwrap();
        let bob_kp = bob.generate_key_package().unwrap();

        let add_result = alice.add_member("group-rm", bob_kp).unwrap();
        let welcome_bytes = MlsGroupHandler::serialize_message(&add_result.welcome).unwrap();
        bob.join_group_from_welcome(MlsGroupHandler::deserialize_message(&welcome_bytes).unwrap())
            .unwrap();

        let epoch_before = alice.epoch("group-rm").unwrap();

        // Find Bob's leaf index and remove
        let bob_idx = alice
            .find_member_index("group-rm", "did:key:bob")
            .unwrap()
            .expect("Bob should be in group");

        alice.remove_member("group-rm", bob_idx).unwrap();

        let epoch_after = alice.epoch("group-rm").unwrap();
        assert!(
            epoch_after > epoch_before,
            "epoch should advance after remove"
        );

        // Alice should now be the only member
        let members = alice.list_members("group-rm").unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0], "did:key:alice");
    }
}
