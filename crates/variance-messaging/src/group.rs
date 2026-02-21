use crate::error::*;
use crate::storage::MessageStorage;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use dashmap::DashMap;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use prost::Message;
use rand::{rngs::OsRng, RngCore};
use sha2::Sha256;
use std::sync::Arc;
use ulid::Ulid;
use variance_proto::messaging_proto::{
    Group, GroupInvitation, GroupKey, GroupMember, GroupMessage, GroupRole, MessageContent,
    MessageType,
};

/// Group message handler
///
/// Manages encrypted group conversations using AES-256-GCM with GossipSub.
/// Each group has a symmetric key shared among members for forward secrecy.
pub struct GroupMessageHandler {
    /// Local DID
    local_did: String,

    /// Signing key for message authentication
    signing_key: SigningKey,

    /// Groups indexed by group_id
    groups: DashMap<String, Group>,

    /// Group keys (decrypted) indexed by group_id
    /// Only contains keys for groups where local user is a member
    group_keys: DashMap<String, Vec<u8>>,

    /// Message storage backend
    storage: Arc<dyn MessageStorage>,

    /// AES-256-GCM key for encrypting group keys and plaintext at rest.
    ///
    /// Derived from the signing key via HKDF-SHA256 so it can be rederived on
    /// restart without storing it separately. A stolen DB cannot yield group keys
    /// without also having the identity file.
    storage_key: [u8; 32],

    /// X25519 static secret for decrypting group key invitations.
    ///
    /// Derived deterministically from the signing key via HKDF-SHA256.
    /// The corresponding public key is what gets published in the DID document
    /// so other members can encrypt group keys to us.
    x25519_secret: x25519_dalek::StaticSecret,
}

impl GroupMessageHandler {
    /// Create a new group message handler
    pub fn new(
        local_did: String,
        signing_key: SigningKey,
        storage: Arc<dyn MessageStorage>,
    ) -> Self {
        let hk = Hkdf::<Sha256>::new(None, signing_key.as_bytes());

        let mut storage_key = [0u8; 32];
        hk.expand(b"variance-group-storage-v1", &mut storage_key)
            .expect("HKDF expand with 32-byte output always succeeds");

        let mut x25519_seed = [0u8; 32];
        hk.expand(b"variance-group-x25519-v1", &mut x25519_seed)
            .expect("HKDF expand with 32-byte output always succeeds");
        let x25519_secret = x25519_dalek::StaticSecret::from(x25519_seed);

        Self {
            local_did,
            signing_key,
            groups: DashMap::new(),
            group_keys: DashMap::new(),
            storage,
            storage_key,
            x25519_secret,
        }
    }

    /// Return the X25519 public key for this handler.
    ///
    /// This key should be published in the DID document so group admins can
    /// encrypt group keys to us when sending invitations.
    pub fn x25519_public_key(&self) -> [u8; 32] {
        x25519_dalek::PublicKey::from(&self.x25519_secret).to_bytes()
    }

    /// Create a new group as admin
    ///
    /// Returns the group ID and the initial group key.
    /// The caller should publish this to IPFS/identity system.
    pub async fn create_group(
        &self,
        name: String,
        description: Option<String>,
    ) -> Result<(String, Group)> {
        let group_id = Ulid::new().to_string();

        // Generate group key (256-bit for AES-256)
        let mut key_bytes = vec![0u8; 32];
        rand::thread_rng().fill_bytes(&mut key_bytes);

        let group_key = GroupKey {
            version: 1,
            key: key_bytes.clone(),
            created_at: chrono::Utc::now().timestamp_millis(),
        };

        // Create admin member
        let admin_member = GroupMember {
            did: self.local_did.clone(),
            role: GroupRole::Admin.into(),
            joined_at: chrono::Utc::now().timestamp_millis(),
            nickname: None,
        };

        let group = Group {
            id: group_id.clone(),
            name,
            admin_did: self.local_did.clone(),
            members: vec![admin_member],
            current_key: Some(group_key),
            created_at: chrono::Utc::now().timestamp_millis(),
            avatar_cid: None,
            description,
        };

        // Store group and key in memory
        self.groups.insert(group_id.clone(), group.clone());
        self.group_keys.insert(group_id.clone(), key_bytes);

        // Persist to disk
        self.persist_group(&group_id).await?;
        self.persist_group_key(&group_id).await?;

        Ok((group_id, group))
    }

    /// Add a member to a group
    ///
    /// Only admin/moderators can add members.
    /// `invitee_x25519_key` is the invitee's X25519 public key (from their DID document,
    /// via `GroupMessageHandler::x25519_public_key()`). The group key is encrypted with
    /// this key so only the invitee can decrypt it.
    ///
    /// Returns a GroupInvitation that should be sent to the invitee.
    pub async fn add_member(
        &self,
        group_id: &str,
        invitee_did: String,
        invitee_x25519_key: [u8; 32],
    ) -> Result<GroupInvitation> {
        let mut group_ref = self
            .groups
            .get_mut(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        // Check authorization
        if !self.is_admin_or_moderator(&group_ref, &self.local_did) {
            return Err(Error::Unauthorized {
                message: "Only admins/moderators can add members".to_string(),
            });
        }

        // Check if already a member
        if group_ref.members.iter().any(|m| m.did == invitee_did) {
            return Err(Error::InvalidFormat {
                message: "User is already a member".to_string(),
            });
        }

        // Add member
        let new_member = GroupMember {
            did: invitee_did.clone(),
            role: GroupRole::Member.into(),
            joined_at: chrono::Utc::now().timestamp_millis(),
            nickname: None,
        };
        group_ref.members.push(new_member);
        drop(group_ref);

        // Get raw group key
        let raw_group_key = self
            .group_keys
            .get(group_id)
            .ok_or_else(|| Error::Encryption {
                message: "Group key not found".to_string(),
            })?
            .clone();

        // Encrypt group key for invitee using ECDH + HKDF + AES-256-GCM.
        //
        // Wire format: ephemeral_pub (32 bytes) || nonce (12 bytes) || ciphertext.
        // The recipient recovers the shared secret using their own X25519 secret key
        // + the ephemeral public key, then applies the same HKDF to get the cipher key.
        let ephemeral_secret = x25519_dalek::EphemeralSecret::random_from_rng(OsRng);
        let ephemeral_public = x25519_dalek::PublicKey::from(&ephemeral_secret);
        let invitee_public = x25519_dalek::PublicKey::from(invitee_x25519_key);
        let shared_secret = ephemeral_secret.diffie_hellman(&invitee_public);

        let hk = Hkdf::<Sha256>::new(None, shared_secret.as_bytes());
        let mut cipher_key_bytes = [0u8; 32];
        hk.expand(b"variance-group-key-v1", &mut cipher_key_bytes)
            .expect("HKDF expand with 32-byte output always succeeds");

        let cipher = Aes256Gcm::new_from_slice(&cipher_key_bytes).map_err(|_| Error::Crypto {
            message: "Failed to build AES-256-GCM cipher for group key encryption".to_string(),
        })?;

        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext =
            cipher
                .encrypt(nonce, raw_group_key.as_ref())
                .map_err(|_| Error::Encryption {
                    message: "AES-256-GCM encryption of group key failed".to_string(),
                })?;

        let mut encrypted_group_key = Vec::with_capacity(32 + 12 + ciphertext.len());
        encrypted_group_key.extend_from_slice(ephemeral_public.as_bytes());
        encrypted_group_key.extend_from_slice(&nonce_bytes);
        encrypted_group_key.extend_from_slice(&ciphertext);

        let group_ref = self
            .groups
            .get(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;
        let group_name = group_ref.name.clone();
        let members = group_ref.members.clone();
        drop(group_ref);

        // Create invitation with full member list so the acceptor
        // starts with correct group membership state.
        let invitation = GroupInvitation {
            group_id: group_id.to_string(),
            group_name,
            inviter_did: self.local_did.clone(),
            invitee_did: invitee_did.clone(),
            encrypted_group_key,
            timestamp: chrono::Utc::now().timestamp_millis(),
            signature: vec![],
            members,
        };

        // Sign invitation
        let mut invitation_with_sig = invitation.clone();
        invitation_with_sig.signature = self.sign_invitation(&invitation)?;

        // Persist updated group membership
        if let Err(e) = self.persist_group(group_id).await {
            tracing::warn!("Failed to persist group after add_member: {}", e);
        }

        Ok(invitation_with_sig)
    }

    /// Remove a member from a group
    ///
    /// Only admin can remove members. After removal, rotates the group key.
    pub async fn remove_member(&self, group_id: &str, member_did: &str) -> Result<()> {
        let mut group_ref = self
            .groups
            .get_mut(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        // Only admin can remove
        if group_ref.admin_did != self.local_did {
            return Err(Error::Unauthorized {
                message: "Only admin can remove members".to_string(),
            });
        }

        // Remove member
        group_ref.members.retain(|m| m.did != member_did);
        drop(group_ref);

        // Rotate key for forward secrecy (also persists the new key)
        self.rotate_key(group_id).await?;

        // Persist updated membership
        if let Err(e) = self.persist_group(group_id).await {
            tracing::warn!("Failed to persist group after remove_member: {}", e);
        }

        Ok(())
    }

    /// Rotate group key
    ///
    /// Generates a new key and increments version.
    /// Should be called after member removal or periodically.
    pub async fn rotate_key(&self, group_id: &str) -> Result<()> {
        let mut group_ref = self
            .groups
            .get_mut(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        // Generate new key
        let mut key_bytes = vec![0u8; 32];
        rand::thread_rng().fill_bytes(&mut key_bytes);

        let old_version = group_ref
            .current_key
            .as_ref()
            .map(|k| k.version)
            .unwrap_or(0);

        let new_key = GroupKey {
            version: old_version + 1,
            key: key_bytes.clone(),
            created_at: chrono::Utc::now().timestamp_millis(),
        };

        group_ref.current_key = Some(new_key);
        drop(group_ref);

        // Update local key
        self.group_keys.insert(group_id.to_string(), key_bytes);

        // Persist encrypted key to disk
        if let Err(e) = self.persist_group_key(group_id).await {
            tracing::warn!("Failed to persist group key after rotation: {}", e);
        }

        Ok(())
    }

    /// Send a group message
    ///
    /// Encrypts with group key (AES-256-GCM), signs, and returns the message.
    /// Caller should publish to GossipSub topic: /variance/group/{group_id}
    pub async fn send_message(
        &self,
        group_id: String,
        content: MessageContent,
    ) -> Result<GroupMessage> {
        // Check membership
        let group = self
            .groups
            .get(&group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.clone(),
            })?;

        if !group.members.iter().any(|m| m.did == self.local_did) {
            return Err(Error::Unauthorized {
                message: "Not a member of this group".to_string(),
            });
        }
        drop(group);

        // Get group key
        let group_key = self
            .group_keys
            .get(&group_id)
            .ok_or_else(|| Error::Encryption {
                message: "Group key not found".to_string(),
            })?;

        // Serialize content using protobuf
        let plaintext = prost::Message::encode_to_vec(&content);

        // Encrypt with AES-256-GCM
        let cipher = Aes256Gcm::new_from_slice(&group_key).map_err(|e| Error::Crypto {
            message: format!("Invalid key length: {}", e),
        })?;

        // Generate random nonce (96 bits for GCM)
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext =
            cipher
                .encrypt(nonce, plaintext.as_ref())
                .map_err(|e| Error::Encryption {
                    message: format!("AES-GCM encryption failed: {}", e),
                })?;

        drop(group_key);

        // Generate ULID for message ID
        let id = Ulid::new().to_string();
        let timestamp = chrono::Utc::now().timestamp_millis();

        // Create message
        let mut message = GroupMessage {
            id: id.clone(),
            sender_did: self.local_did.clone(),
            group_id: group_id.clone(),
            ciphertext,
            nonce: nonce_bytes.to_vec(),
            signature: vec![],
            timestamp,
            r#type: Self::infer_message_type(&content),
            reply_to: content.reply_to.clone(),
        };

        // Sign message
        message.signature = self.sign_message(&message)?;

        // Store message
        self.storage.store_group(&message).await?;

        Ok(message)
    }

    /// Receive and decrypt a group message
    ///
    /// NOTE: Caller must verify message signature using verify_message_with_key()
    /// before calling this, passing the sender's public key from their DID document.
    pub async fn receive_message(&self, message: GroupMessage) -> Result<MessageContent> {
        // Check membership
        let group = self
            .groups
            .get(&message.group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: message.group_id.clone(),
            })?;

        if !group.members.iter().any(|m| m.did == self.local_did) {
            return Err(Error::Unauthorized {
                message: "Not a member of this group".to_string(),
            });
        }
        drop(group);

        // Get group key
        let group_key =
            self.group_keys
                .get(&message.group_id)
                .ok_or_else(|| Error::Decryption {
                    message: "Group key not found".to_string(),
                })?;

        // Decrypt with AES-256-GCM
        let cipher = Aes256Gcm::new_from_slice(&group_key).map_err(|e| Error::Crypto {
            message: format!("Invalid key length: {}", e),
        })?;

        if message.nonce.len() != 12 {
            return Err(Error::InvalidFormat {
                message: "Invalid nonce size".to_string(),
            });
        }

        let nonce = Nonce::from_slice(&message.nonce);

        let plaintext = cipher
            .decrypt(nonce, message.ciphertext.as_ref())
            .map_err(|e| Error::Decryption {
                message: format!("AES-GCM decryption failed: {}", e),
            })?;

        drop(group_key);

        // Deserialize content using protobuf
        let content = MessageContent::decode(plaintext.as_slice())
            .map_err(|e| Error::Protocol { source: e })?;

        // Persist decrypted plaintext for future reads (avoids re-decryption)
        if let Err(e) = self.persist_plaintext(&message.id, &content).await {
            tracing::warn!("Failed to persist group message plaintext: {}", e);
        }

        // Store the ciphertext message
        self.storage.store_group(&message).await?;

        Ok(content)
    }

    /// Encrypt `content` with AES-256-GCM and persist under `message_id`.
    ///
    /// Format: random 12-byte nonce || GCM ciphertext (plaintext + 16-byte tag).
    /// Called after successful decryption so history is readable across restarts
    /// without re-doing AES-GCM group decryption.
    async fn persist_plaintext(&self, message_id: &str, content: &MessageContent) -> Result<()> {
        let key = aes_gcm::Key::<Aes256Gcm>::from_slice(&self.storage_key);
        let cipher = Aes256Gcm::new(key);

        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let plaintext = content.encode_to_vec();
        let ciphertext =
            cipher
                .encrypt(nonce, plaintext.as_slice())
                .map_err(|_| Error::Crypto {
                    message: "At-rest encryption failed".to_string(),
                })?;

        let mut blob = nonce_bytes.to_vec();
        blob.extend_from_slice(&ciphertext);

        self.storage.store_plaintext(message_id, &blob).await
    }

    /// Decrypt a blob previously written by `persist_plaintext`.
    async fn load_plaintext(&self, message_id: &str) -> Result<Option<MessageContent>> {
        let Some(blob) = self.storage.fetch_plaintext(message_id).await? else {
            return Ok(None);
        };

        if blob.len() < 12 {
            return Err(Error::Crypto {
                message: "Stored plaintext blob is too short".to_string(),
            });
        }

        let (nonce_bytes, ciphertext) = blob.split_at(12);
        let key = aes_gcm::Key::<Aes256Gcm>::from_slice(&self.storage_key);
        let cipher = Aes256Gcm::new(key);
        let nonce = Nonce::from_slice(nonce_bytes);

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| Error::Crypto {
                message: "At-rest decryption failed (wrong key or corrupted data)".to_string(),
            })?;

        let content = MessageContent::decode(plaintext.as_slice())
            .map_err(|e| Error::Protocol { source: e })?;

        Ok(Some(content))
    }

    /// Get message content for display.
    ///
    /// Checks the encrypted persistent plaintext store first (survives restarts).
    /// Falls through to AES-GCM group decryption only for messages not yet in
    /// the store, which also writes the result for future reads.
    pub async fn get_message_content(&self, message: &GroupMessage) -> Result<MessageContent> {
        if let Some(content) = self.load_plaintext(&message.id).await? {
            return Ok(content);
        }

        // Not in the persistent store yet — decrypt (also persists).
        self.receive_message(message.clone()).await
    }

    /// Get group by ID
    pub async fn get_group(&self, group_id: &str) -> Result<Option<Group>> {
        Ok(self.groups.get(group_id).map(|r| r.clone()))
    }

    /// List all groups the local user is a member of
    pub async fn list_groups(&self) -> Result<Vec<Group>> {
        Ok(self
            .groups
            .iter()
            .filter(|entry| {
                entry
                    .value()
                    .members
                    .iter()
                    .any(|m| m.did == self.local_did)
            })
            .map(|entry| entry.value().clone())
            .collect())
    }

    /// Fetch conversation history for a group
    pub async fn get_conversation(
        &self,
        group_id: &str,
        limit: usize,
        before: Option<String>,
    ) -> Result<Vec<GroupMessage>> {
        self.storage.fetch_group(group_id, limit, before).await
    }

    /// Accept a group invitation
    ///
    /// Decrypts the group key using our X25519 secret key, inserts the group into
    /// local state, and persists both to disk.
    ///
    /// NOTE: The caller must verify `invitation.signature` using the inviter's Ed25519
    /// verifying key before calling this, to ensure the invitation is authentic.
    pub async fn accept_invitation(&self, invitation: GroupInvitation) -> Result<()> {
        // Decrypt group key: ephemeral_pub (32) || nonce (12) || ciphertext
        let enc = &invitation.encrypted_group_key;
        if enc.len() < 44 {
            return Err(Error::Crypto {
                message: "encrypted_group_key is too short to be valid".to_string(),
            });
        }

        let ephemeral_pub_bytes: [u8; 32] = enc[..32].try_into().map_err(|_| Error::Crypto {
            message: "Failed to parse ephemeral public key from invitation".to_string(),
        })?;
        let nonce_bytes: [u8; 12] = enc[32..44].try_into().map_err(|_| Error::Crypto {
            message: "Failed to parse nonce from invitation".to_string(),
        })?;
        let ciphertext = &enc[44..];

        let ephemeral_public = x25519_dalek::PublicKey::from(ephemeral_pub_bytes);
        let shared_secret = self.x25519_secret.diffie_hellman(&ephemeral_public);

        let hk = Hkdf::<Sha256>::new(None, shared_secret.as_bytes());
        let mut cipher_key_bytes = [0u8; 32];
        hk.expand(b"variance-group-key-v1", &mut cipher_key_bytes)
            .expect("HKDF expand with 32-byte output always succeeds");

        let cipher = Aes256Gcm::new_from_slice(&cipher_key_bytes).map_err(|_| Error::Crypto {
            message: "Failed to build AES-256-GCM cipher for group key decryption".to_string(),
        })?;
        let nonce = Nonce::from_slice(&nonce_bytes);
        let group_key = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| Error::Decryption {
                message: "Failed to decrypt group key — wrong key or corrupted invitation"
                    .to_string(),
            })?;

        let group_id = invitation.group_id.clone();

        // Use the full member list from the invitation if provided,
        // falling back to inviter + self for backward compatibility.
        let members = if invitation.members.is_empty() {
            let now = chrono::Utc::now().timestamp_millis();
            vec![
                GroupMember {
                    did: invitation.inviter_did.clone(),
                    role: GroupRole::Admin.into(),
                    joined_at: invitation.timestamp,
                    nickname: None,
                },
                GroupMember {
                    did: self.local_did.clone(),
                    role: GroupRole::Member.into(),
                    joined_at: now,
                    nickname: None,
                },
            ]
        } else {
            invitation.members.clone()
        };

        let group = Group {
            id: group_id.clone(),
            name: invitation.group_name.clone(),
            admin_did: invitation.inviter_did.clone(),
            members,
            current_key: None, // stored separately in group_keys
            created_at: invitation.timestamp,
            avatar_cid: None,
            description: None,
        };

        // Insert into in-memory state
        self.groups.insert(group_id.clone(), group);
        self.group_keys.insert(group_id.clone(), group_key);

        // Persist both to disk
        self.persist_group(&group_id).await?;
        self.persist_group_key(&group_id).await?;

        Ok(())
    }

    /// Restore all groups and their keys from disk (called on startup).
    ///
    /// Should be called immediately after creating the handler to hydrate the
    /// in-memory DashMaps with any groups that existed before the last restart.
    pub async fn restore_groups(&self) -> Result<()> {
        let groups = self.storage.fetch_all_group_metadata().await?;
        let count = groups.len();

        for group in groups {
            let group_id = group.id.clone();

            // Decrypt and restore the group key
            if let Some(blob) = self.storage.fetch_group_key_encrypted(&group_id).await? {
                if blob.len() >= 28 {
                    // 12-byte nonce + at least 16-byte AES-GCM tag
                    let (nonce_bytes, ciphertext) = blob.split_at(12);
                    let key = aes_gcm::Key::<Aes256Gcm>::from_slice(&self.storage_key);
                    let cipher = Aes256Gcm::new(key);
                    let nonce = Nonce::from_slice(nonce_bytes);
                    match cipher.decrypt(nonce, ciphertext) {
                        Ok(key_bytes) => {
                            self.group_keys.insert(group_id.clone(), key_bytes);
                        }
                        Err(_) => {
                            tracing::warn!(
                                "Failed to decrypt stored group key for {} (corrupted or key mismatch)",
                                group_id
                            );
                        }
                    }
                }
            }

            self.groups.insert(group_id, group);
        }

        tracing::info!("Restored {} groups from storage", count);
        Ok(())
    }

    /// Persist group metadata (without the raw key) to sled.
    ///
    /// The `current_key` field is cleared before writing — raw key bytes are
    /// stored separately via `persist_group_key`, encrypted at rest.
    async fn persist_group(&self, group_id: &str) -> Result<()> {
        if let Some(group) = self.groups.get(group_id) {
            let mut for_storage = group.clone();
            for_storage.current_key = None; // raw key stored separately, encrypted
            self.storage.store_group_metadata(&for_storage).await?;
        }
        Ok(())
    }

    /// Encrypt the current group key with AES-256-GCM and persist it.
    ///
    /// Format: random 12-byte nonce || GCM ciphertext. The encryption key is
    /// derived from the signing key so a stolen DB cannot yield group keys
    /// without the identity file.
    async fn persist_group_key(&self, group_id: &str) -> Result<()> {
        let raw_key = match self.group_keys.get(group_id) {
            Some(k) => k.clone(),
            None => return Ok(()),
        };

        let cipher_key = aes_gcm::Key::<Aes256Gcm>::from_slice(&self.storage_key);
        let cipher = Aes256Gcm::new(cipher_key);

        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, raw_key.as_ref())
            .map_err(|_| Error::Crypto {
                message: "Failed to encrypt group key for storage".to_string(),
            })?;

        let mut blob = nonce_bytes.to_vec();
        blob.extend_from_slice(&ciphertext);

        self.storage
            .store_group_key_encrypted(group_id, &blob)
            .await
    }

    /// Sign a group message
    fn sign_message(&self, message: &GroupMessage) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        data.extend_from_slice(message.id.as_bytes());
        data.extend_from_slice(message.sender_did.as_bytes());
        data.extend_from_slice(message.group_id.as_bytes());
        data.extend_from_slice(&message.ciphertext);
        data.extend_from_slice(&message.nonce);
        data.extend_from_slice(&message.timestamp.to_le_bytes());

        let signature = self.signing_key.sign(&data);
        Ok(signature.to_bytes().to_vec())
    }

    /// Verify a group message signature
    pub fn verify_message_with_key(
        &self,
        message: &GroupMessage,
        sender_public_key: &VerifyingKey,
    ) -> Result<()> {
        let mut data = Vec::new();
        data.extend_from_slice(message.id.as_bytes());
        data.extend_from_slice(message.sender_did.as_bytes());
        data.extend_from_slice(message.group_id.as_bytes());
        data.extend_from_slice(&message.ciphertext);
        data.extend_from_slice(&message.nonce);
        data.extend_from_slice(&message.timestamp.to_le_bytes());

        let signature =
            Signature::from_bytes(message.signature.as_slice().try_into().map_err(|_| {
                Error::InvalidSignature {
                    message_id: message.id.clone(),
                }
            })?);

        sender_public_key
            .verify(&data, &signature)
            .map_err(|_| Error::InvalidSignature {
                message_id: message.id.clone(),
            })?;

        Ok(())
    }

    /// Sign a group invitation
    fn sign_invitation(&self, invitation: &GroupInvitation) -> Result<Vec<u8>> {
        let data = Self::invitation_signable_bytes(invitation);
        let signature = self.signing_key.sign(&data);
        Ok(signature.to_bytes().to_vec())
    }

    /// Verify a group invitation signature against the inviter's Ed25519 key.
    ///
    /// Must be called before `accept_invitation` to ensure the invitation is
    /// authentic and hasn't been tampered with.
    pub fn verify_invitation_with_key(
        invitation: &GroupInvitation,
        inviter_verifying_key: &VerifyingKey,
    ) -> Result<()> {
        let data = Self::invitation_signable_bytes(invitation);

        let signature =
            Signature::from_bytes(invitation.signature.as_slice().try_into().map_err(|_| {
                Error::InvalidSignature {
                    message_id: invitation.group_id.clone(),
                }
            })?);

        inviter_verifying_key
            .verify(&data, &signature)
            .map_err(|_| Error::InvalidSignature {
                message_id: format!(
                    "invitation:{}/{}",
                    invitation.group_id, invitation.inviter_did
                ),
            })?;

        Ok(())
    }

    /// Produce the canonical bytes that are signed/verified for an invitation.
    fn invitation_signable_bytes(invitation: &GroupInvitation) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(invitation.group_id.as_bytes());
        data.extend_from_slice(invitation.inviter_did.as_bytes());
        data.extend_from_slice(invitation.invitee_did.as_bytes());
        data.extend_from_slice(&invitation.encrypted_group_key);
        data.extend_from_slice(&invitation.timestamp.to_le_bytes());
        // Include member DIDs so the member list can't be tampered with
        for member in &invitation.members {
            data.extend_from_slice(member.did.as_bytes());
        }
        data
    }

    /// Infer message type from content
    fn infer_message_type(content: &MessageContent) -> i32 {
        if !content.attachments.is_empty() {
            let first = &content.attachments[0];
            match first.r#type {
                1 => MessageType::Image.into(),
                2 => MessageType::File.into(),
                3 => MessageType::Audio.into(),
                4 => MessageType::Video.into(),
                _ => MessageType::Text.into(),
            }
        } else {
            MessageType::Text.into()
        }
    }

    /// Check if user is admin or moderator
    fn is_admin_or_moderator(&self, group: &Group, did: &str) -> bool {
        group.members.iter().any(|m| {
            m.did == did
                && (m.role == GroupRole::Admin as i32 || m.role == GroupRole::Moderator as i32)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::LocalMessageStorage;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use std::collections::HashMap;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_create_group() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler =
            GroupMessageHandler::new("did:variance:alice".to_string(), signing_key, storage);

        let (group_id, group) = handler
            .create_group("Test Group".to_string(), Some("A test group".to_string()))
            .await
            .unwrap();

        assert_eq!(group.name, "Test Group");
        assert_eq!(group.admin_did, "did:variance:alice");
        assert_eq!(group.members.len(), 1);
        assert_eq!(group.members[0].did, "did:variance:alice");
        assert_eq!(group.members[0].role, GroupRole::Admin as i32);

        // Check key is stored
        assert!(handler.group_keys.contains_key(&group_id));
    }

    #[tokio::test]
    async fn test_add_member() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler =
            GroupMessageHandler::new("did:variance:alice".to_string(), signing_key, storage);

        // Bob's handler provides his X25519 public key for the invitation encryption
        let bob_dir = tempdir().unwrap();
        let bob_storage = Arc::new(LocalMessageStorage::new(bob_dir.path()).unwrap());
        let bob_signing = SigningKey::generate(&mut OsRng);
        let bob =
            GroupMessageHandler::new("did:variance:bob".to_string(), bob_signing, bob_storage);

        let (group_id, _) = handler
            .create_group("Test Group".to_string(), None)
            .await
            .unwrap();

        let invitation = handler
            .add_member(
                &group_id,
                "did:variance:bob".to_string(),
                bob.x25519_public_key(),
            )
            .await
            .unwrap();

        assert_eq!(invitation.group_id, group_id);
        assert_eq!(invitation.invitee_did, "did:variance:bob");
        assert_eq!(invitation.inviter_did, "did:variance:alice");
        assert!(!invitation.signature.is_empty());

        // Check member added
        let group = handler.get_group(&group_id).await.unwrap().unwrap();
        assert_eq!(group.members.len(), 2);
        assert!(group.members.iter().any(|m| m.did == "did:variance:bob"));
    }

    #[tokio::test]
    async fn test_remove_member_rotates_key() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler =
            GroupMessageHandler::new("did:variance:alice".to_string(), signing_key, storage);

        let bob_dir = tempdir().unwrap();
        let bob_storage = Arc::new(LocalMessageStorage::new(bob_dir.path()).unwrap());
        let bob_signing = SigningKey::generate(&mut OsRng);
        let bob =
            GroupMessageHandler::new("did:variance:bob".to_string(), bob_signing, bob_storage);

        let (group_id, _) = handler
            .create_group("Test Group".to_string(), None)
            .await
            .unwrap();

        handler
            .add_member(
                &group_id,
                "did:variance:bob".to_string(),
                bob.x25519_public_key(),
            )
            .await
            .unwrap();

        // Get original key version
        let group = handler.get_group(&group_id).await.unwrap().unwrap();
        let old_version = group.current_key.unwrap().version;

        // Remove member
        handler
            .remove_member(&group_id, "did:variance:bob")
            .await
            .unwrap();

        // Check member removed
        let group = handler.get_group(&group_id).await.unwrap().unwrap();
        assert_eq!(group.members.len(), 1);
        assert!(!group.members.iter().any(|m| m.did == "did:variance:bob"));

        // Check key rotated
        let new_version = group.current_key.unwrap().version;
        assert_eq!(new_version, old_version + 1);
    }

    #[tokio::test]
    async fn test_send_and_receive_message() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let handler =
            GroupMessageHandler::new("did:variance:alice".to_string(), signing_key, storage);

        let (group_id, _) = handler
            .create_group("Test Group".to_string(), None)
            .await
            .unwrap();

        let content = MessageContent {
            text: "Hello group!".to_string(),
            attachments: vec![],
            mentions: vec![],
            reply_to: None,
            metadata: HashMap::new(),
        };

        // Send message
        let message = handler
            .send_message(group_id.clone(), content.clone())
            .await
            .unwrap();

        assert_eq!(message.sender_did, "did:variance:alice");
        assert_eq!(message.group_id, group_id);
        assert!(!message.ciphertext.is_empty());
        assert_eq!(message.nonce.len(), 12);
        assert!(!message.signature.is_empty());

        // Verify signature
        assert!(handler
            .verify_message_with_key(&message, &verifying_key)
            .is_ok());

        // Receive message
        let decrypted = handler.receive_message(message).await.unwrap();
        assert_eq!(decrypted.text, "Hello group!");
    }

    #[tokio::test]
    async fn test_unauthorized_send() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = GroupMessageHandler::new(
            "did:variance:bob".to_string(), // Not a member
            signing_key,
            storage,
        );

        // Create a fake group (simulating received group info)
        let group = Group {
            id: "group123".to_string(),
            name: "Test".to_string(),
            admin_did: "did:variance:alice".to_string(),
            members: vec![GroupMember {
                did: "did:variance:alice".to_string(),
                role: GroupRole::Admin.into(),
                joined_at: 0,
                nickname: None,
            }],
            current_key: None,
            created_at: 0,
            avatar_cid: None,
            description: None,
        };

        handler.groups.insert("group123".to_string(), group);

        let content = MessageContent {
            text: "Hello".to_string(),
            attachments: vec![],
            mentions: vec![],
            reply_to: None,
            metadata: HashMap::new(),
        };

        // Should fail - not a member
        let result = handler.send_message("group123".to_string(), content).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::Unauthorized { .. }));
    }

    /// Alice invites Bob, Bob accepts, Bob can decrypt a message Alice sends.
    #[tokio::test]
    async fn test_invitation_encryption_round_trip() {
        let alice_dir = tempdir().unwrap();
        let bob_dir = tempdir().unwrap();
        let alice_storage = Arc::new(LocalMessageStorage::new(alice_dir.path()).unwrap());
        let bob_storage = Arc::new(LocalMessageStorage::new(bob_dir.path()).unwrap());

        let alice = GroupMessageHandler::new(
            "did:variance:alice".to_string(),
            SigningKey::generate(&mut OsRng),
            alice_storage,
        );
        let bob = GroupMessageHandler::new(
            "did:variance:bob".to_string(),
            SigningKey::generate(&mut OsRng),
            bob_storage,
        );

        let (group_id, _) = alice
            .create_group("Secret Group".to_string(), None)
            .await
            .unwrap();

        // Alice invites Bob using his X25519 public key
        let invitation = alice
            .add_member(
                &group_id,
                "did:variance:bob".to_string(),
                bob.x25519_public_key(),
            )
            .await
            .unwrap();

        // Bob accepts: key is decrypted and group inserted
        bob.accept_invitation(invitation).await.unwrap();

        // Bob is now a member and can send a message
        let content = MessageContent {
            text: "Hello from Bob!".to_string(),
            attachments: vec![],
            mentions: vec![],
            reply_to: None,
            metadata: HashMap::new(),
        };
        let message = bob.send_message(group_id.clone(), content).await.unwrap();

        // Alice can decrypt it with the same group key
        let decrypted = alice.receive_message(message).await.unwrap();
        assert_eq!(decrypted.text, "Hello from Bob!");
    }

    /// Group state survives a simulated restart via restore_groups().
    #[tokio::test]
    async fn test_group_state_restored_after_restart() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());
        let signing_key = SigningKey::generate(&mut OsRng);

        let group_id = {
            let handler = GroupMessageHandler::new(
                "did:variance:alice".to_string(),
                signing_key.clone(),
                storage.clone(),
            );
            let (gid, _) = handler
                .create_group("Persistent Group".to_string(), None)
                .await
                .unwrap();
            gid
        };

        // Simulate restart: new handler, same storage + signing key
        let handler2 =
            GroupMessageHandler::new("did:variance:alice".to_string(), signing_key, storage);
        handler2.restore_groups().await.unwrap();

        // Group and key should be restored
        assert!(handler2.get_group(&group_id).await.unwrap().is_some());
        assert!(handler2.group_keys.contains_key(&group_id));

        // Should be able to send and receive a message
        let content = MessageContent {
            text: "still here!".to_string(),
            attachments: vec![],
            mentions: vec![],
            reply_to: None,
            metadata: HashMap::new(),
        };
        let message = handler2.send_message(group_id, content).await.unwrap();
        let decrypted = handler2.receive_message(message).await.unwrap();
        assert_eq!(decrypted.text, "still here!");
    }

    #[tokio::test]
    async fn test_message_type_inference() {
        let content = MessageContent {
            text: "Hello".to_string(),
            attachments: vec![],
            mentions: vec![],
            reply_to: None,
            metadata: HashMap::new(),
        };

        let msg_type = GroupMessageHandler::infer_message_type(&content);
        assert_eq!(msg_type, MessageType::Text as i32);
    }

    #[tokio::test]
    async fn test_signature_verification_failure() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(LocalMessageStorage::new(dir.path()).unwrap());

        let signing_key = SigningKey::generate(&mut OsRng);
        let wrong_key = SigningKey::generate(&mut OsRng).verifying_key();

        let handler =
            GroupMessageHandler::new("did:variance:alice".to_string(), signing_key, storage);

        let (group_id, _) = handler
            .create_group("Test Group".to_string(), None)
            .await
            .unwrap();

        let content = MessageContent {
            text: "Hello".to_string(),
            attachments: vec![],
            mentions: vec![],
            reply_to: None,
            metadata: HashMap::new(),
        };

        let message = handler.send_message(group_id, content).await.unwrap();

        // Verify with wrong key should fail
        let result = handler.verify_message_with_key(&message, &wrong_key);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            Error::InvalidSignature { .. }
        ));
    }

    #[tokio::test]
    async fn test_verify_invitation_signature() {
        let alice_dir = tempdir().unwrap();
        let bob_dir = tempdir().unwrap();
        let alice_storage = Arc::new(LocalMessageStorage::new(alice_dir.path()).unwrap());
        let bob_storage = Arc::new(LocalMessageStorage::new(bob_dir.path()).unwrap());

        let alice_key = SigningKey::generate(&mut OsRng);
        let alice_verifying_key = alice_key.verifying_key();

        let alice =
            GroupMessageHandler::new("did:variance:alice".to_string(), alice_key, alice_storage);
        let bob = GroupMessageHandler::new(
            "did:variance:bob".to_string(),
            SigningKey::generate(&mut OsRng),
            bob_storage,
        );

        let (group_id, _) = alice
            .create_group("Signed Group".to_string(), None)
            .await
            .unwrap();

        let invitation = alice
            .add_member(
                &group_id,
                "did:variance:bob".to_string(),
                bob.x25519_public_key(),
            )
            .await
            .unwrap();

        // Verify with correct key succeeds
        GroupMessageHandler::verify_invitation_with_key(&invitation, &alice_verifying_key).unwrap();

        // Tampered invitation fails verification
        let mut tampered = invitation.clone();
        tampered.group_name = "Tampered Group".to_string();
        // group_name isn't in signable bytes, so this won't fail — but mutating
        // a signable field will:
        tampered.invitee_did = "did:variance:mallory".to_string();
        let result =
            GroupMessageHandler::verify_invitation_with_key(&tampered, &alice_verifying_key);
        assert!(result.is_err());

        // Wrong key fails verification
        let wrong_key = SigningKey::generate(&mut OsRng).verifying_key();
        let result = GroupMessageHandler::verify_invitation_with_key(&invitation, &wrong_key);
        assert!(result.is_err());
    }
}
