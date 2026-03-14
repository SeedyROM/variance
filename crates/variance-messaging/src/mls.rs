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

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use dashmap::DashMap;
use ed25519_dalek::SigningKey;
use hkdf::Hkdf;
use openmls::messages::group_info::GroupInfo;
use openmls::prelude::tls_codec::{Deserialize, Serialize};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use prost::Message as ProstMessage;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize as SerdeDeserialize, Serialize as SerdeSerialize};
use sha2::Sha256;

use crate::error::{Error, Result};
use crate::storage::LocalMessageStorage;

/// Helper to convert a `PoisonError` into our `Error::LockPoisoned` variant.
fn lock_poisoned<T>(err: std::sync::PoisonError<T>) -> Error {
    Error::LockPoisoned {
        message: err.to_string(),
    }
}

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

    /// AES-256-GCM key for at-rest encryption of decrypted group message plaintext.
    ///
    /// Derived from the Ed25519 signing key via HKDF-SHA256 with a label distinct
    /// from the DM storage key so the two keys never alias. This ensures a stolen
    /// sled database is unreadable without the identity file.
    storage_key: [u8; 32],
}

/// The output of an `add_member` operation.
///
/// Contains the commit (broadcast to existing members via GossipSub)
/// and the welcome (sent directly to the new member).
#[derive(Debug)]
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

        // Derive AES-256-GCM key for at-rest plaintext encryption.
        // Label is distinct from the DM key ("variance-plaintext-storage-v1")
        // so the two keys never alias even with the same signing key.
        let hk = Hkdf::<Sha256>::new(None, signing_key.as_bytes());
        let mut storage_key = [0u8; 32];
        hk.expand(b"variance-group-plaintext-v1", &mut storage_key)
            .expect("HKDF expand with 32-byte output always succeeds");

        Ok(Self {
            local_did,
            provider,
            signature_keypair,
            credential_with_key,
            groups: DashMap::new(),
            storage_key,
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

        let mut group = group_lock.write().map_err(lock_poisoned)?;

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

    /// Add a member without immediately merging the commit.
    ///
    /// Unlike `add_member()`, this leaves the group in `PendingCommit` state.
    /// The caller must serialize the commit+welcome and send a `GroupInvitation`
    /// to the invitee. When the invitee responds:
    /// - **Accept**: call `confirm_add_member()` to merge the commit
    /// - **Decline / timeout**: call `cancel_add_member()` to roll back
    ///
    /// While a commit is pending, the group is blocked from other MLS operations
    /// (no new invites, no encrypted messages, no removes).
    pub fn add_member_deferred(
        &self,
        group_id: &str,
        key_package: KeyPackage,
    ) -> Result<AddMemberResult> {
        let group_lock = self
            .groups
            .get(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        let mut group = group_lock.write().map_err(lock_poisoned)?;

        // Fail fast if there's already a pending commit (another invite in flight).
        if group.pending_commit().is_some() {
            return Err(Error::MlsPendingCommit);
        }

        let (commit, welcome, group_info) = group
            .add_members(&self.provider, &self.signature_keypair, &[key_package])
            .map_err(|e| Error::MlsCommit {
                message: format!("Failed to add member (deferred): {e:?}"),
            })?;

        // Do NOT merge — leave group in PendingCommit state.
        Ok(AddMemberResult {
            commit,
            welcome,
            group_info,
        })
    }

    /// Merge the pending commit after the invitee accepts.
    ///
    /// This finalizes the add-member operation, advancing the group epoch.
    /// After this call, the commit should be broadcast to existing members
    /// via GossipSub so they process the epoch change.
    pub fn confirm_add_member(&self, group_id: &str) -> Result<()> {
        let group_lock = self
            .groups
            .get(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        let mut group = group_lock.write().map_err(lock_poisoned)?;

        if group.pending_commit().is_none() {
            return Err(Error::MlsNoPendingCommit {
                group_id: group_id.to_string(),
            });
        }

        group
            .merge_pending_commit(&self.provider)
            .map_err(|e| Error::MlsCommit {
                message: format!("Failed to merge pending commit: {e:?}"),
            })?;

        Ok(())
    }

    /// Roll back the pending commit after the invitee declines or the invite times out.
    ///
    /// This restores the group to its pre-invite state — same epoch, same membership.
    /// No message needs to be broadcast; other members never saw the commit.
    pub fn cancel_add_member(&self, group_id: &str) -> Result<()> {
        let group_lock = self
            .groups
            .get(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        let mut group = group_lock.write().map_err(lock_poisoned)?;

        if group.pending_commit().is_none() {
            return Err(Error::MlsNoPendingCommit {
                group_id: group_id.to_string(),
            });
        }

        group
            .clear_pending_commit(self.provider.storage())
            .map_err(|e| Error::MlsCommit {
                message: format!("Failed to clear pending commit: {e:?}"),
            })?;

        Ok(())
    }

    /// Check whether a group has an in-flight pending commit.
    ///
    /// Returns `true` while the group is blocked awaiting an invite response.
    pub fn has_pending_commit(&self, group_id: &str) -> Result<bool> {
        let group_lock = self
            .groups
            .get(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        let group = group_lock.read().map_err(lock_poisoned)?;
        Ok(group.pending_commit().is_some())
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

        let mut group = group_lock.write().map_err(lock_poisoned)?;

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

        let group = group_lock.read().map_err(lock_poisoned)?;
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

        let mut group = group_lock.write().map_err(lock_poisoned)?;

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

        let mut group = group_lock.write().map_err(lock_poisoned)?;

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

        let group = group_lock.read().map_err(lock_poisoned)?;
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

        let group = group_lock.read().map_err(lock_poisoned)?;
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

        let mut group = group_lock.write().map_err(lock_poisoned)?;

        let msg = group
            .leave_group(&self.provider, &self.signature_keypair)
            .map_err(|e| Error::MlsGroup {
                message: format!("Failed to leave group: {e:?}"),
            })?;

        Ok(msg)
    }

    /// Commit any pending proposals in the group's proposal store.
    ///
    /// After a leave proposal is received, a remaining member must commit it
    /// to actually remove the departing member from the MLS tree.
    /// Returns the commit message to broadcast, or `None` if there are no
    /// pending proposals.
    pub fn commit_pending_proposals(&self, group_id: &str) -> Result<Option<MlsMessageOut>> {
        let group_lock = self
            .groups
            .get(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        let mut group = group_lock.write().map_err(lock_poisoned)?;

        if group.pending_proposals().next().is_none() {
            return Ok(None);
        }

        let (commit, _welcome, _group_info) = group
            .commit_to_pending_proposals(&self.provider, &self.signature_keypair)
            .map_err(|e| Error::MlsCommit {
                message: format!("Failed to commit pending proposals: {e:?}"),
            })?;

        group
            .merge_pending_commit(&self.provider)
            .map_err(|e| Error::MlsCommit {
                message: format!("Failed to merge pending commit after proposal commit: {e:?}"),
            })?;

        Ok(Some(commit))
    }

    /// Remove a group from local state (after leaving or being removed).
    ///
    /// This deletes the group's persisted state from the OpenMLS provider
    /// storage so that a future `join_group_from_welcome` for the same
    /// group ID does not fail with `GroupAlreadyExists`.
    pub fn remove_group(&self, group_id: &str) {
        if let Some((_, group_arc)) = self.groups.remove(group_id) {
            if let Ok(mut group) = group_arc.write() {
                if let Err(e) = group.delete(self.provider.storage()) {
                    tracing::warn!(
                        "Failed to delete MLS group {} from provider storage: {:?}",
                        group_id,
                        e,
                    );
                }
            }
        }
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

    /// Encrypt and persist the plaintext of a group message for later retrieval.
    ///
    /// Uses the same at-rest encryption pattern as `DirectMessageHandler::persist_plaintext`:
    /// `random 12-byte nonce || AES-256-GCM ciphertext`. The storage key is derived from
    /// the identity signing key via HKDF so the DB is unreadable without the identity file.
    ///
    /// Call this immediately after encrypting a sent message and after decrypting a
    /// received one. Because MLS provides forward secrecy at the wire level, we
    /// cannot re-decrypt historical messages — this cache is the only source for history.
    pub async fn persist_group_plaintext(
        &self,
        storage: &LocalMessageStorage,
        message_id: &str,
        content: &variance_proto::messaging_proto::MessageContent,
    ) -> Result<()> {
        let key = aes_gcm::Key::<Aes256Gcm>::from_slice(&self.storage_key);
        let cipher = Aes256Gcm::new(key);

        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let plaintext = content.encode_to_vec();
        let ciphertext =
            cipher
                .encrypt(nonce, plaintext.as_slice())
                .map_err(|_| Error::Encryption {
                    message: "At-rest encryption of group plaintext failed".to_string(),
                })?;

        let mut blob = nonce_bytes.to_vec();
        blob.extend_from_slice(&ciphertext);

        storage.store_group_plaintext(message_id, &blob).await
    }

    /// Decrypt and return the cached plaintext for a group message.
    ///
    /// Returns `None` if no plaintext was persisted (e.g. message predates this feature).
    pub async fn load_group_plaintext(
        &self,
        storage: &LocalMessageStorage,
        message_id: &str,
    ) -> Result<Option<variance_proto::messaging_proto::MessageContent>> {
        let Some(blob) = storage.fetch_group_plaintext(message_id).await? else {
            return Ok(None);
        };

        if blob.len() < 12 {
            return Err(Error::Decryption {
                message: "Stored group plaintext blob is too short".to_string(),
            });
        }

        let (nonce_bytes, ciphertext) = blob.split_at(12);
        let key = aes_gcm::Key::<Aes256Gcm>::from_slice(&self.storage_key);
        let cipher = Aes256Gcm::new(key);
        let nonce = Nonce::from_slice(nonce_bytes);

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| Error::Decryption {
                message: "At-rest decryption of group plaintext failed".to_string(),
            })?;

        let content = variance_proto::messaging_proto::MessageContent::decode(plaintext.as_slice())
            .map_err(|e| Error::Protocol { source: e })?;

        Ok(Some(content))
    }

    /// Serialize the full MLS provider state to bytes for persistent storage.
    ///
    /// The snapshot contains the complete openmls `MemoryStorage` key-value map —
    /// ratchet trees, epoch secrets, leaf nodes, the local signature keypair — plus
    /// the list of active group IDs. Passing this to `restore_in_place` on a fresh
    /// handler reconstructs all groups exactly as they were.
    ///
    /// Call this after every mutating operation (`create_group`, `add_member`,
    /// `remove_member`, `join_group_from_welcome`, `encrypt_message`,
    /// `process_message`) and persist the result via `MessageStorage::store_mls_state`.
    pub fn export_state(&self) -> Result<Vec<u8>> {
        let values = self
            .provider
            .storage()
            .values
            .read()
            .map_err(lock_poisoned)?;

        let storage_entries: Vec<[String; 2]> = values
            .iter()
            .map(|(k, v)| [hex::encode(k), hex::encode(v)])
            .collect();

        let group_ids: Vec<String> = self.groups.iter().map(|e| e.key().clone()).collect();

        let snapshot = MlsStateSnapshot {
            group_ids,
            storage_entries,
        };

        serde_json::to_vec(&snapshot).map_err(|e| Error::MlsGroup {
            message: format!("Failed to serialize MLS state snapshot: {e}"),
        })
    }

    /// Restore MLS group state in-place from a snapshot produced by `export_state`.
    ///
    /// Replaces the provider's in-memory key store with the persisted state, then
    /// reloads each `MlsGroup` object from the restored store. Safe to call on a
    /// handler that was just constructed with `new()` — any state written by `new()`
    /// (the initial keypair store) is replaced by the snapshot, which already
    /// contains the keypair.
    ///
    /// Returns the number of groups successfully restored.
    pub fn restore_in_place(&self, state_bytes: &[u8]) -> Result<usize> {
        let snapshot: MlsStateSnapshot =
            serde_json::from_slice(state_bytes).map_err(|e| Error::MlsGroup {
                message: format!("Failed to deserialize MLS state snapshot: {e}"),
            })?;

        // Decode hex pairs back to binary and build the restored map.
        let mut restored_map: HashMap<Vec<u8>, Vec<u8>> =
            HashMap::with_capacity(snapshot.storage_entries.len());

        for [k_hex, v_hex] in &snapshot.storage_entries {
            let k = hex::decode(k_hex).map_err(|e| Error::MlsGroup {
                message: format!("Corrupted MLS snapshot: bad hex key: {e}"),
            })?;
            let v = hex::decode(v_hex).map_err(|e| Error::MlsGroup {
                message: format!("Corrupted MLS snapshot: bad hex value: {e}"),
            })?;
            restored_map.insert(k, v);
        }

        // Atomically replace the provider's storage with the restored map.
        // The snapshot includes the keypair that was written during new(), so
        // we don't lose it. We re-store the keypair afterwards as a safety net
        // for snapshots taken before any group was created (edge case on first run).
        {
            let mut values = self
                .provider
                .storage()
                .values
                .write()
                .map_err(lock_poisoned)?;
            *values = restored_map;
        }

        self.signature_keypair
            .store(self.provider.storage())
            .map_err(|e| Error::MlsKeyPackage {
                message: format!("Failed to re-store signature keypair after restore: {e:?}"),
            })?;

        // Reload each MlsGroup from the restored provider storage.
        let mut restored = 0;
        for group_id in &snapshot.group_ids {
            let gid = GroupId::from_slice(group_id.as_bytes());
            match MlsGroup::load(self.provider.storage(), &gid) {
                Ok(Some(group)) => {
                    self.groups
                        .insert(group_id.clone(), Arc::new(RwLock::new(group)));
                    restored += 1;
                    tracing::debug!("Restored MLS group '{}'", group_id);
                }
                Ok(None) => {
                    tracing::warn!(
                        "MLS group '{}' listed in snapshot but missing from storage — skipping",
                        group_id
                    );
                }
                Err(e) => {
                    tracing::warn!("Failed to restore MLS group '{}': {:?}", group_id, e);
                }
            }
        }

        Ok(restored)
    }
}

/// Serializable snapshot of `MlsGroupHandler` state.
///
/// `storage_entries` holds the raw `MemoryStorage` key-value pairs as hex strings.
/// This captures the complete openmls key store: ratchet trees, epoch secrets,
/// leaf nodes, and the local signature keypair. `group_ids` drives the `MlsGroup::load`
/// calls needed to reconstruct the live group objects on restore.
#[derive(SerdeSerialize, SerdeDeserialize)]
struct MlsStateSnapshot {
    group_ids: Vec<String>,
    /// Hex-encoded `[key, value]` pairs from `MemoryStorage.values`.
    storage_entries: Vec<[String; 2]>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_signing_key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
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
    fn export_and_restore_roundtrip() {
        let sk = test_signing_key();
        let alice = MlsGroupHandler::new("did:key:alice".to_string(), &sk).unwrap();
        alice.create_group("persist-test").unwrap();

        // Export state from the live handler.
        let snapshot = alice.export_state().unwrap();
        assert!(!snapshot.is_empty());

        // Restore into a fresh handler — it should know about the group.
        let alice2 = MlsGroupHandler::new("did:key:alice".to_string(), &sk).unwrap();
        assert!(
            !alice2.is_member("persist-test"),
            "fresh handler should have no groups"
        );

        let n = alice2.restore_in_place(&snapshot).unwrap();
        assert_eq!(n, 1);
        assert!(alice2.is_member("persist-test"));
        assert_eq!(
            alice2.list_members("persist-test").unwrap(),
            vec!["did:key:alice"]
        );
    }

    #[test]
    fn restored_handler_can_encrypt_and_decrypt() {
        // Alice creates a group and adds Bob.
        let alice_sk = test_signing_key();
        let alice = MlsGroupHandler::new("did:key:alice".to_string(), &alice_sk).unwrap();
        alice.create_group("restart-group").unwrap();

        let bob_sk = test_signing_key();
        let bob = MlsGroupHandler::new("did:key:bob".to_string(), &bob_sk).unwrap();
        let bob_kp = bob.generate_key_package().unwrap();

        let add = alice.add_member("restart-group", bob_kp).unwrap();
        let welcome_bytes = MlsGroupHandler::serialize_message(&add.welcome).unwrap();
        bob.join_group_from_welcome(MlsGroupHandler::deserialize_message(&welcome_bytes).unwrap())
            .unwrap();

        // Send a message before "restart".
        let enc = alice
            .encrypt_message("restart-group", b"pre-restart")
            .unwrap();
        let enc_bytes = MlsGroupHandler::serialize_message(&enc).unwrap();
        let dec = bob
            .process_message(
                "restart-group",
                MlsGroupHandler::deserialize_message(&enc_bytes).unwrap(),
            )
            .unwrap()
            .expect("should be application message");
        assert_eq!(dec.plaintext, b"pre-restart");

        // Export Alice's state and restore into a new handler (simulated restart).
        let snapshot = alice.export_state().unwrap();
        let alice_r = MlsGroupHandler::new("did:key:alice".to_string(), &alice_sk).unwrap();
        assert_eq!(alice_r.restore_in_place(&snapshot).unwrap(), 1);

        // The restored handler can still encrypt messages that Bob decrypts.
        let enc2 = alice_r
            .encrypt_message("restart-group", b"post-restart")
            .unwrap();
        let enc2_bytes = MlsGroupHandler::serialize_message(&enc2).unwrap();
        let dec2 = bob
            .process_message(
                "restart-group",
                MlsGroupHandler::deserialize_message(&enc2_bytes).unwrap(),
            )
            .unwrap()
            .expect("should be application message");
        assert_eq!(dec2.plaintext, b"post-restart");
    }

    #[test]
    fn restore_empty_snapshot_has_no_groups() {
        let sk = test_signing_key();
        // Export state with no groups created yet.
        let handler = MlsGroupHandler::new("did:key:test".to_string(), &sk).unwrap();
        let snapshot = handler.export_state().unwrap();

        // Restore into a fresh handler — should succeed with 0 groups.
        let handler2 = MlsGroupHandler::new("did:key:test".to_string(), &sk).unwrap();
        let n = handler2.restore_in_place(&snapshot).unwrap();
        assert_eq!(n, 0);
        assert!(handler2.group_ids().is_empty());
    }

    #[test]
    fn test_mls_group_survives_restart() {
        // Set up Alice and Bob in a group.
        let alice_sk = test_signing_key();
        let alice = MlsGroupHandler::new("did:key:alice".to_string(), &alice_sk).unwrap();
        alice.create_group("restart-both").unwrap();

        let bob_sk = test_signing_key();
        let bob = MlsGroupHandler::new("did:key:bob".to_string(), &bob_sk).unwrap();
        let bob_kp = bob.generate_key_package().unwrap();

        let add = alice.add_member("restart-both", bob_kp).unwrap();
        bob.join_group_from_welcome(
            MlsGroupHandler::deserialize_message(
                &MlsGroupHandler::serialize_message(&add.welcome).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();

        // Verify pre-restart messaging.
        let pre_enc = alice
            .encrypt_message("restart-both", b"pre-restart")
            .unwrap();
        let pre_dec = bob
            .process_message(
                "restart-both",
                MlsGroupHandler::deserialize_message(
                    &MlsGroupHandler::serialize_message(&pre_enc).unwrap(),
                )
                .unwrap(),
            )
            .unwrap()
            .expect("application message");
        assert_eq!(pre_dec.plaintext, b"pre-restart");

        // Simulate restart: export both states, build fresh handlers, restore.
        let alice_snapshot = alice.export_state().unwrap();
        let bob_snapshot = bob.export_state().unwrap();

        let alice_r = MlsGroupHandler::new("did:key:alice".to_string(), &alice_sk).unwrap();
        assert_eq!(alice_r.restore_in_place(&alice_snapshot).unwrap(), 1);

        let bob_r = MlsGroupHandler::new("did:key:bob".to_string(), &bob_sk).unwrap();
        assert_eq!(bob_r.restore_in_place(&bob_snapshot).unwrap(), 1);

        // Alice (restored) → Bob (restored)
        let enc1 = alice_r
            .encrypt_message("restart-both", b"alice-post-restart")
            .unwrap();
        let dec1 = bob_r
            .process_message(
                "restart-both",
                MlsGroupHandler::deserialize_message(
                    &MlsGroupHandler::serialize_message(&enc1).unwrap(),
                )
                .unwrap(),
            )
            .unwrap()
            .expect("application message");
        assert_eq!(dec1.plaintext, b"alice-post-restart");

        // Bob (restored) → Alice (restored)
        let enc2 = bob_r
            .encrypt_message("restart-both", b"bob-post-restart")
            .unwrap();
        let dec2 = alice_r
            .process_message(
                "restart-both",
                MlsGroupHandler::deserialize_message(
                    &MlsGroupHandler::serialize_message(&enc2).unwrap(),
                )
                .unwrap(),
            )
            .unwrap()
            .expect("application message");
        assert_eq!(dec2.plaintext, b"bob-post-restart");
    }

    #[test]
    fn test_mls_group_survives_multiple_messages_then_restart() {
        // Set up Alice and Bob in a group.
        let alice_sk = test_signing_key();
        let alice = MlsGroupHandler::new("did:key:alice".to_string(), &alice_sk).unwrap();
        alice.create_group("ratchet-restart").unwrap();

        let bob_sk = test_signing_key();
        let bob = MlsGroupHandler::new("did:key:bob".to_string(), &bob_sk).unwrap();
        let bob_kp = bob.generate_key_package().unwrap();

        let add = alice.add_member("ratchet-restart", bob_kp).unwrap();
        bob.join_group_from_welcome(
            MlsGroupHandler::deserialize_message(
                &MlsGroupHandler::serialize_message(&add.welcome).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();

        // Advance the ratchet with 5 messages in each direction before restart.
        for i in 0u8..5 {
            let msg = format!("alice-msg-{i}");
            let enc = alice
                .encrypt_message("ratchet-restart", msg.as_bytes())
                .unwrap();
            let dec = bob
                .process_message(
                    "ratchet-restart",
                    MlsGroupHandler::deserialize_message(
                        &MlsGroupHandler::serialize_message(&enc).unwrap(),
                    )
                    .unwrap(),
                )
                .unwrap()
                .expect("application message");
            assert_eq!(dec.plaintext, msg.as_bytes());

            let reply = format!("bob-msg-{i}");
            let enc_r = bob
                .encrypt_message("ratchet-restart", reply.as_bytes())
                .unwrap();
            let dec_r = alice
                .process_message(
                    "ratchet-restart",
                    MlsGroupHandler::deserialize_message(
                        &MlsGroupHandler::serialize_message(&enc_r).unwrap(),
                    )
                    .unwrap(),
                )
                .unwrap()
                .expect("application message");
            assert_eq!(dec_r.plaintext, reply.as_bytes());
        }

        // Simulate restart.
        let alice_snapshot = alice.export_state().unwrap();
        let bob_snapshot = bob.export_state().unwrap();

        let alice_r = MlsGroupHandler::new("did:key:alice".to_string(), &alice_sk).unwrap();
        assert_eq!(alice_r.restore_in_place(&alice_snapshot).unwrap(), 1);

        let bob_r = MlsGroupHandler::new("did:key:bob".to_string(), &bob_sk).unwrap();
        assert_eq!(bob_r.restore_in_place(&bob_snapshot).unwrap(), 1);

        // Post-restart messages still decrypt correctly.
        let enc = alice_r
            .encrypt_message("ratchet-restart", b"after-5-rounds")
            .unwrap();
        let dec = bob_r
            .process_message(
                "ratchet-restart",
                MlsGroupHandler::deserialize_message(
                    &MlsGroupHandler::serialize_message(&enc).unwrap(),
                )
                .unwrap(),
            )
            .unwrap()
            .expect("application message");
        assert_eq!(dec.plaintext, b"after-5-rounds");

        let enc_b = bob_r
            .encrypt_message("ratchet-restart", b"bob-after-5-rounds")
            .unwrap();
        let dec_b = alice_r
            .process_message(
                "ratchet-restart",
                MlsGroupHandler::deserialize_message(
                    &MlsGroupHandler::serialize_message(&enc_b).unwrap(),
                )
                .unwrap(),
            )
            .unwrap()
            .expect("application message");
        assert_eq!(dec_b.plaintext, b"bob-after-5-rounds");
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

    #[test]
    fn deferred_add_then_confirm() {
        // Alice creates a group and adds Bob with the deferred flow.
        let alice_sk = test_signing_key();
        let alice = MlsGroupHandler::new("did:key:alice".to_string(), &alice_sk).unwrap();
        alice.create_group("deferred-1").unwrap();

        let bob_sk = test_signing_key();
        let bob = MlsGroupHandler::new("did:key:bob".to_string(), &bob_sk).unwrap();
        let bob_kp = bob.generate_key_package().unwrap();

        let epoch_before = alice.epoch("deferred-1").unwrap();

        // Deferred add — group should now have a pending commit.
        let result = alice.add_member_deferred("deferred-1", bob_kp).unwrap();
        assert!(alice.has_pending_commit("deferred-1").unwrap());

        // Epoch should NOT have advanced yet (commit not merged).
        assert_eq!(alice.epoch("deferred-1").unwrap(), epoch_before);

        // Bob joins via the Welcome.
        let welcome_bytes = MlsGroupHandler::serialize_message(&result.welcome).unwrap();
        let welcome_in = MlsGroupHandler::deserialize_message(&welcome_bytes).unwrap();
        let joined_id = bob.join_group_from_welcome(welcome_in).unwrap();
        assert_eq!(joined_id, "deferred-1");

        // Alice confirms (simulating invitee accepted).
        alice.confirm_add_member("deferred-1").unwrap();
        assert!(!alice.has_pending_commit("deferred-1").unwrap());

        // Epoch should have advanced now.
        assert!(alice.epoch("deferred-1").unwrap() > epoch_before);

        // Both see each other as members.
        let alice_members = alice.list_members("deferred-1").unwrap();
        assert_eq!(alice_members.len(), 2);
        let bob_members = bob.list_members("deferred-1").unwrap();
        assert_eq!(bob_members.len(), 2);

        // Messaging works after confirm.
        let enc = alice
            .encrypt_message("deferred-1", b"hello deferred")
            .unwrap();
        let enc_bytes = MlsGroupHandler::serialize_message(&enc).unwrap();
        let dec = bob
            .process_message(
                "deferred-1",
                MlsGroupHandler::deserialize_message(&enc_bytes).unwrap(),
            )
            .unwrap()
            .expect("application message");
        assert_eq!(dec.plaintext, b"hello deferred");
    }

    #[test]
    fn deferred_add_then_cancel() {
        // Alice creates a group and starts adding Bob, then cancels.
        let alice_sk = test_signing_key();
        let alice = MlsGroupHandler::new("did:key:alice".to_string(), &alice_sk).unwrap();
        alice.create_group("deferred-cancel").unwrap();

        let bob_sk = test_signing_key();
        let bob = MlsGroupHandler::new("did:key:bob".to_string(), &bob_sk).unwrap();
        let bob_kp = bob.generate_key_package().unwrap();

        let epoch_before = alice.epoch("deferred-cancel").unwrap();

        // Deferred add.
        let _result = alice
            .add_member_deferred("deferred-cancel", bob_kp)
            .unwrap();
        assert!(alice.has_pending_commit("deferred-cancel").unwrap());

        // Cancel (simulating invitee declined or timeout).
        alice.cancel_add_member("deferred-cancel").unwrap();
        assert!(!alice.has_pending_commit("deferred-cancel").unwrap());

        // Epoch should NOT have advanced — group is back to original state.
        assert_eq!(alice.epoch("deferred-cancel").unwrap(), epoch_before);

        // Alice is still the only member.
        let members = alice.list_members("deferred-cancel").unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0], "did:key:alice");

        // Alice can still encrypt messages (group is unblocked).
        // We need a second member to decrypt, so add Charlie immediately.
        let charlie_sk = test_signing_key();
        let charlie = MlsGroupHandler::new("did:key:charlie".to_string(), &charlie_sk).unwrap();
        let charlie_kp = charlie.generate_key_package().unwrap();
        let add = alice.add_member("deferred-cancel", charlie_kp).unwrap();

        let welcome_bytes = MlsGroupHandler::serialize_message(&add.welcome).unwrap();
        charlie
            .join_group_from_welcome(MlsGroupHandler::deserialize_message(&welcome_bytes).unwrap())
            .unwrap();

        let enc = alice
            .encrypt_message("deferred-cancel", b"after cancel")
            .unwrap();
        let enc_bytes = MlsGroupHandler::serialize_message(&enc).unwrap();
        let dec = charlie
            .process_message(
                "deferred-cancel",
                MlsGroupHandler::deserialize_message(&enc_bytes).unwrap(),
            )
            .unwrap()
            .expect("application message");
        assert_eq!(dec.plaintext, b"after cancel");
    }

    #[test]
    fn deferred_add_blocks_second_invite() {
        // A second deferred add should fail while one is pending.
        let alice_sk = test_signing_key();
        let alice = MlsGroupHandler::new("did:key:alice".to_string(), &alice_sk).unwrap();
        alice.create_group("deferred-block").unwrap();

        let bob_sk = test_signing_key();
        let bob = MlsGroupHandler::new("did:key:bob".to_string(), &bob_sk).unwrap();
        let bob_kp = bob.generate_key_package().unwrap();

        let charlie_sk = test_signing_key();
        let charlie = MlsGroupHandler::new("did:key:charlie".to_string(), &charlie_sk).unwrap();
        let charlie_kp = charlie.generate_key_package().unwrap();

        // First invite — should succeed.
        let _result = alice.add_member_deferred("deferred-block", bob_kp).unwrap();

        // Second invite — should fail with MlsPendingCommit.
        let err = alice
            .add_member_deferred("deferred-block", charlie_kp)
            .unwrap_err();
        assert!(
            matches!(err, Error::MlsPendingCommit),
            "expected MlsPendingCommit, got: {err:?}"
        );
    }

    #[test]
    fn confirm_without_pending_commit_errors() {
        let sk = test_signing_key();
        let handler = MlsGroupHandler::new("did:key:test".to_string(), &sk).unwrap();
        handler.create_group("no-pending").unwrap();

        let err = handler.confirm_add_member("no-pending").unwrap_err();
        assert!(
            matches!(err, Error::MlsNoPendingCommit { .. }),
            "expected MlsNoPendingCommit, got: {err:?}"
        );
    }

    #[test]
    fn cancel_without_pending_commit_errors() {
        let sk = test_signing_key();
        let handler = MlsGroupHandler::new("did:key:test".to_string(), &sk).unwrap();
        handler.create_group("no-pending-cancel").unwrap();

        let err = handler.cancel_add_member("no-pending-cancel").unwrap_err();
        assert!(
            matches!(err, Error::MlsNoPendingCommit { .. }),
            "expected MlsNoPendingCommit, got: {err:?}"
        );
    }

    #[test]
    fn reinvite_after_kick_with_fresh_key_package() {
        // Regression test: after being removed from a group, generating a
        // fresh KeyPackage must allow the kicked user to rejoin via a new
        // Welcome.  Previously the stale (consumed) key package caused
        // `NoMatchingKeyPackage`.
        let alice_sk = test_signing_key();
        let alice = MlsGroupHandler::new("did:key:alice".to_string(), &alice_sk).unwrap();
        alice.create_group("kick-reinvite").unwrap();

        let bob_sk = test_signing_key();
        let bob = MlsGroupHandler::new("did:key:bob".to_string(), &bob_sk).unwrap();
        let bob_kp = bob.generate_key_package().unwrap();

        // Alice adds Bob.
        let add_result = alice.add_member("kick-reinvite", bob_kp).unwrap();
        let welcome_bytes = MlsGroupHandler::serialize_message(&add_result.welcome).unwrap();
        bob.join_group_from_welcome(MlsGroupHandler::deserialize_message(&welcome_bytes).unwrap())
            .unwrap();

        // Verify Bob is a member.
        let members = alice.list_members("kick-reinvite").unwrap();
        assert!(members.contains(&"did:key:bob".to_string()));

        // Alice kicks Bob.
        let bob_idx = alice
            .find_member_index("kick-reinvite", "did:key:bob")
            .unwrap()
            .expect("Bob should be in group");
        alice.remove_member("kick-reinvite", bob_idx).unwrap();

        // Simulate the kicked user's cleanup: remove local group state.
        bob.remove_group("kick-reinvite");

        // Generate a fresh KeyPackage (the fix).
        let bob_kp2 = bob.generate_key_package().unwrap();

        // Alice re-invites Bob with the fresh key package.
        let readd = alice.add_member("kick-reinvite", bob_kp2).unwrap();
        let welcome2_bytes = MlsGroupHandler::serialize_message(&readd.welcome).unwrap();
        let joined_id = bob
            .join_group_from_welcome(MlsGroupHandler::deserialize_message(&welcome2_bytes).unwrap())
            .unwrap();
        assert_eq!(joined_id, "kick-reinvite");

        // Both see each other as members again.
        let alice_members = alice.list_members("kick-reinvite").unwrap();
        assert_eq!(alice_members.len(), 2);
        let bob_members = bob.list_members("kick-reinvite").unwrap();
        assert_eq!(bob_members.len(), 2);

        // Messaging works after rejoin.
        let enc = alice
            .encrypt_message("kick-reinvite", b"welcome back")
            .unwrap();
        let enc_bytes = MlsGroupHandler::serialize_message(&enc).unwrap();
        let dec = bob
            .process_message(
                "kick-reinvite",
                MlsGroupHandler::deserialize_message(&enc_bytes).unwrap(),
            )
            .unwrap()
            .expect("application message");
        assert_eq!(dec.plaintext, b"welcome back");
    }

    #[test]
    fn leave_proposal_committed_by_remaining_member() {
        // When Bob leaves via a proposal, Alice (a remaining member) must
        // commit the proposal for the removal to take effect.
        let alice_sk = test_signing_key();
        let alice = MlsGroupHandler::new("did:key:alice".to_string(), &alice_sk).unwrap();
        alice.create_group("leave-commit").unwrap();

        let bob_sk = test_signing_key();
        let bob = MlsGroupHandler::new("did:key:bob".to_string(), &bob_sk).unwrap();
        let bob_kp = bob.generate_key_package().unwrap();

        // Alice adds Bob.
        let add_result = alice.add_member("leave-commit", bob_kp).unwrap();
        let welcome_bytes = MlsGroupHandler::serialize_message(&add_result.welcome).unwrap();
        bob.join_group_from_welcome(MlsGroupHandler::deserialize_message(&welcome_bytes).unwrap())
            .unwrap();
        assert_eq!(alice.list_members("leave-commit").unwrap().len(), 2);

        // Bob leaves — this produces a proposal, not a commit.
        let leave_msg = bob.leave_group("leave-commit").unwrap();
        let leave_bytes = MlsGroupHandler::serialize_message(&leave_msg).unwrap();

        // Alice receives and processes the leave proposal.
        let result = alice
            .process_message(
                "leave-commit",
                MlsGroupHandler::deserialize_message(&leave_bytes).unwrap(),
            )
            .unwrap();
        assert!(result.is_none(), "proposal should not produce plaintext");

        // Bob is still in Alice's member list — proposal is pending, not committed.
        assert_eq!(alice.list_members("leave-commit").unwrap().len(), 2);

        // Alice auto-commits the pending proposal.
        let commit = alice
            .commit_pending_proposals("leave-commit")
            .unwrap()
            .expect("should produce a commit");
        assert!(commit.tls_serialize_detached().is_ok());

        // Now Bob is removed from Alice's member list.
        let members = alice.list_members("leave-commit").unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0], "did:key:alice");
    }

    #[test]
    fn commit_pending_proposals_returns_none_when_empty() {
        let sk = test_signing_key();
        let handler = MlsGroupHandler::new("did:key:test".to_string(), &sk).unwrap();
        handler.create_group("no-proposals").unwrap();

        let result = handler.commit_pending_proposals("no-proposals").unwrap();
        assert!(result.is_none());
    }
}
