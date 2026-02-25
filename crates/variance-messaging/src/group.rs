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

    /// Versioned group keys (decrypted), indexed by group_id → version → raw key bytes.
    ///
    /// Old versions are kept so historical messages can still be decrypted after rotation.
    /// Only contains keys for groups where local user is a member.
    group_keys: DashMap<String, DashMap<u32, Vec<u8>>>,

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

        // Create admin member (no X25519 key stored for the creator — they already have the key)
        let admin_member = GroupMember {
            did: self.local_did.clone(),
            role: GroupRole::Admin.into(),
            joined_at: chrono::Utc::now().timestamp_millis(),
            nickname: None,
            x25519_key: None,
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

        // Store group and key in memory (version 1 for the initial key)
        self.groups.insert(group_id.clone(), group.clone());
        let inner: DashMap<u32, Vec<u8>> = DashMap::new();
        inner.insert(1, key_bytes);
        self.group_keys.insert(group_id.clone(), inner);

        // Persist to disk
        self.persist_group(&group_id).await?;
        self.persist_group_key(&group_id, 1).await?;

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

        // Add member (store their X25519 key so rotate_key can re-encrypt for them)
        let new_member = GroupMember {
            did: invitee_did.clone(),
            role: GroupRole::Member.into(),
            joined_at: chrono::Utc::now().timestamp_millis(),
            nickname: None,
            x25519_key: Some(invitee_x25519_key.to_vec()),
        };
        group_ref.members.push(new_member);
        drop(group_ref);

        // Get current key version and raw key
        let current_version = self.current_key_version(group_id);
        let raw_group_key = self
            .group_keys
            .get(group_id)
            .and_then(|inner| inner.get(&current_version).map(|k| k.clone()))
            .ok_or_else(|| Error::Encryption {
                message: "Group key not found".to_string(),
            })?;

        let group_ref = self
            .groups
            .get(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;
        let group_name = group_ref.name.clone();
        let members = group_ref.members.clone();
        drop(group_ref);

        let invitation_with_sig = self
            .encrypt_key_for_member(
                group_id,
                &group_name,
                &invitee_did,
                invitee_x25519_key,
                &raw_group_key,
                current_version,
                &members,
            )
            .await?;

        // Persist updated group membership
        if let Err(e) = self.persist_group(group_id).await {
            tracing::warn!("Failed to persist group after add_member: {}", e);
        }

        Ok(invitation_with_sig)
    }

    /// Remove a member from a group
    ///
    /// Any member may remove themselves (leave). Admins may also remove other members.
    /// After removal, rotates the group key and returns re-key invitations for remaining
    /// members who have a stored X25519 key.
    pub async fn remove_member(
        &self,
        group_id: &str,
        member_did: &str,
    ) -> Result<Vec<GroupInvitation>> {
        let mut group_ref = self
            .groups
            .get_mut(group_id)
            .ok_or_else(|| Error::GroupNotFound {
                group_id: group_id.to_string(),
            })?;

        // Self-removal is always allowed; admin check applies only to removing others.
        if member_did != self.local_did && group_ref.admin_did != self.local_did {
            return Err(Error::Unauthorized {
                message: "Only admin can remove other members".to_string(),
            });
        }

        // Remove member
        group_ref.members.retain(|m| m.did != member_did);
        drop(group_ref);

        // Rotate key for forward secrecy; collect re-key invitations for remaining members
        let invitations = self.rotate_key(group_id).await?;

        // Persist updated membership
        if let Err(e) = self.persist_group(group_id).await {
            tracing::warn!("Failed to persist group after remove_member: {}", e);
        }

        Ok(invitations)
    }

    /// Rotate group key
    ///
    /// Generates a new key and increments version. Old key versions are kept in the
    /// in-memory map so historical messages can still be decrypted.
    ///
    /// Returns re-key `GroupInvitation`s for every remaining member that has a stored
    /// X25519 key. The caller should distribute these to the respective members.
    pub async fn rotate_key(&self, group_id: &str) -> Result<Vec<GroupInvitation>> {
        let new_version = {
            let mut group_ref = self
                .groups
                .get_mut(group_id)
                .ok_or_else(|| Error::GroupNotFound {
                    group_id: group_id.to_string(),
                })?;

            let old_version = group_ref
                .current_key
                .as_ref()
                .map(|k| k.version)
                .unwrap_or(0);
            let new_version = old_version + 1;

            let new_key_meta = GroupKey {
                version: new_version,
                key: vec![], // raw key stored in group_keys map, not here
                created_at: chrono::Utc::now().timestamp_millis(),
            };
            group_ref.current_key = Some(new_key_meta);
            new_version
        };

        // Generate new key
        let mut key_bytes = vec![0u8; 32];
        rand::thread_rng().fill_bytes(&mut key_bytes);

        // Insert into versioned map, keeping old versions for historical decryption
        {
            let inner = self
                .group_keys
                .entry(group_id.to_string())
                .or_insert_with(DashMap::new);
            inner.insert(new_version, key_bytes.clone());
        }

        // Persist encrypted new key
        if let Err(e) = self.persist_group_key(group_id, new_version).await {
            tracing::warn!("Failed to persist group key after rotation: {}", e);
        }

        // Build re-key invitations for every remaining member that has an X25519 key.
        // Members without a stored key (e.g. the admin who created the group, or
        // pre-migration members) are skipped with a warning.
        let (group_name, members) = {
            let g = self
                .groups
                .get(group_id)
                .ok_or_else(|| Error::GroupNotFound {
                    group_id: group_id.to_string(),
                })?;
            (g.name.clone(), g.members.clone())
        };

        let mut invitations = Vec::new();
        for member in &members {
            if member.did == self.local_did {
                continue; // no need to send an invitation to ourselves
            }
            let Some(ref x25519_bytes) = member.x25519_key else {
                tracing::warn!(
                    "Skipping re-key invitation for {} (no X25519 key stored)",
                    member.did
                );
                continue;
            };
            let x25519_key: [u8; 32] = match x25519_bytes.as_slice().try_into() {
                Ok(k) => k,
                Err(_) => {
                    tracing::warn!(
                        "Skipping re-key invitation for {} (malformed X25519 key)",
                        member.did
                    );
                    continue;
                }
            };

            match self
                .encrypt_key_for_member(
                    group_id,
                    &group_name,
                    &member.did,
                    x25519_key,
                    &key_bytes,
                    new_version,
                    &members,
                )
                .await
            {
                Ok(inv) => invitations.push(inv),
                Err(e) => {
                    tracing::warn!(
                        "Failed to create re-key invitation for {}: {}",
                        member.did,
                        e
                    );
                }
            }
        }

        Ok(invitations)
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

        // Get current key version and raw key bytes
        let current_version = self.current_key_version(&group_id);
        let group_key = self
            .group_keys
            .get(&group_id)
            .and_then(|inner| inner.get(&current_version).map(|k| k.clone()))
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
            key_version: current_version,
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

        // Look up the key for this message's specific version
        let group_key = self
            .group_keys
            .get(&message.group_id)
            .and_then(|inner| {
                // key_version 0 means pre-versioning: use version 1 for compat
                let ver = if message.key_version == 0 {
                    1
                } else {
                    message.key_version
                };
                inner.get(&ver).map(|k| k.clone())
            })
            .ok_or_else(|| Error::Decryption {
                message: format!(
                    "Group key version {} not found",
                    message.key_version
                ),
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

        // key_version 0 means pre-versioning field; treat as version 1 for backward compat
        let key_version = if invitation.key_version == 0 {
            1
        } else {
            invitation.key_version
        };

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
                    x25519_key: None,
                },
                GroupMember {
                    did: self.local_did.clone(),
                    role: GroupRole::Member.into(),
                    joined_at: now,
                    nickname: None,
                    x25519_key: None,
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
            current_key: Some(GroupKey {
                version: key_version,
                key: vec![],
                created_at: invitation.timestamp,
            }),
            created_at: invitation.timestamp,
            avatar_cid: None,
            description: None,
        };

        // Insert into in-memory versioned key map
        {
            let inner = self
                .group_keys
                .entry(group_id.clone())
                .or_insert_with(DashMap::new);
            inner.insert(key_version, group_key);
        }
        self.groups.insert(group_id.clone(), group);

        // Persist both to disk
        self.persist_group(&group_id).await?;
        self.persist_group_key(&group_id, key_version).await?;

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

            // Decrypt and restore all versioned group keys
            let versioned_blobs = self.storage.fetch_all_group_keys(&group_id).await?;
            if !versioned_blobs.is_empty() {
                let key = aes_gcm::Key::<Aes256Gcm>::from_slice(&self.storage_key);
                let cipher = Aes256Gcm::new(key);
                let inner: DashMap<u32, Vec<u8>> = DashMap::new();

                for (version, blob) in versioned_blobs {
                    if blob.len() < 28 {
                        // 12-byte nonce + at least 16-byte AES-GCM tag
                        tracing::warn!(
                            "Versioned key blob for {}/{} is too short, skipping",
                            group_id,
                            version
                        );
                        continue;
                    }
                    let (nonce_bytes, ciphertext) = blob.split_at(12);
                    let nonce = Nonce::from_slice(nonce_bytes);
                    match cipher.decrypt(nonce, ciphertext) {
                        Ok(key_bytes) => {
                            inner.insert(version, key_bytes);
                        }
                        Err(_) => {
                            tracing::warn!(
                                "Failed to decrypt stored group key v{} for {} (corrupted or key mismatch)",
                                version,
                                group_id
                            );
                        }
                    }
                }

                if !inner.is_empty() {
                    self.group_keys.insert(group_id.clone(), inner);
                }
            } else {
                // Fallback: try the legacy single-key tree for groups stored before migration
                if let Ok(Some(blob)) =
                    self.storage.fetch_group_key_encrypted(&group_id).await
                {
                    if blob.len() >= 28 {
                        let (nonce_bytes, ciphertext) = blob.split_at(12);
                        let key = aes_gcm::Key::<Aes256Gcm>::from_slice(&self.storage_key);
                        let cipher = Aes256Gcm::new(key);
                        let nonce = Nonce::from_slice(nonce_bytes);
                        if let Ok(key_bytes) = cipher.decrypt(nonce, ciphertext) {
                            let inner: DashMap<u32, Vec<u8>> = DashMap::new();
                            inner.insert(1, key_bytes);
                            self.group_keys.insert(group_id.clone(), inner);
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

    /// Encrypt a specific version of the group key with AES-256-GCM and persist it.
    ///
    /// Format: random 12-byte nonce || GCM ciphertext. The encryption key is
    /// derived from the signing key so a stolen DB cannot yield group keys
    /// without the identity file.
    async fn persist_group_key(&self, group_id: &str, version: u32) -> Result<()> {
        let raw_key = match self
            .group_keys
            .get(group_id)
            .and_then(|inner| inner.get(&version).map(|k| k.clone()))
        {
            Some(k) => k,
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
            .store_versioned_group_key(group_id, version, &blob)
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
        data.extend_from_slice(&message.key_version.to_le_bytes());

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
        data.extend_from_slice(&message.key_version.to_le_bytes());

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
        // Bind key version to prevent downgrade attacks
        data.extend_from_slice(&invitation.key_version.to_le_bytes());
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

    /// Return the highest key version stored for a group, or 0 if none.
    fn current_key_version(&self, group_id: &str) -> u32 {
        self.group_keys
            .get(group_id)
            .and_then(|inner| inner.iter().map(|e| *e.key()).max())
            .unwrap_or(0)
    }

    /// Encrypt `raw_key` for a single member and return a signed `GroupInvitation`.
    ///
    /// Uses ECDH (X25519 ephemeral) + HKDF-SHA256 + AES-256-GCM.
    /// Wire format for `encrypted_group_key`: ephemeral_pub (32 B) || nonce (12 B) || ciphertext.
    async fn encrypt_key_for_member(
        &self,
        group_id: &str,
        group_name: &str,
        invitee_did: &str,
        invitee_x25519_key: [u8; 32],
        raw_key: &[u8],
        key_version: u32,
        members: &[GroupMember],
    ) -> Result<GroupInvitation> {
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

        let ciphertext = cipher
            .encrypt(nonce, raw_key)
            .map_err(|_| Error::Encryption {
                message: "AES-256-GCM encryption of group key failed".to_string(),
            })?;

        let mut encrypted_group_key = Vec::with_capacity(32 + 12 + ciphertext.len());
        encrypted_group_key.extend_from_slice(ephemeral_public.as_bytes());
        encrypted_group_key.extend_from_slice(&nonce_bytes);
        encrypted_group_key.extend_from_slice(&ciphertext);

        let invitation = GroupInvitation {
            group_id: group_id.to_string(),
            group_name: group_name.to_string(),
            inviter_did: self.local_did.clone(),
            invitee_did: invitee_did.to_string(),
            encrypted_group_key,
            timestamp: chrono::Utc::now().timestamp_millis(),
            signature: vec![],
            members: members.to_vec(),
            key_version,
        };

        let mut invitation_with_sig = invitation.clone();
        invitation_with_sig.signature = self.sign_invitation(&invitation)?;
        Ok(invitation_with_sig)
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
        let old_version = group.current_key.as_ref().unwrap().version;

        // Remove member; admin has no stored X25519 key so invitations will be empty
        let invitations = handler
            .remove_member(&group_id, "did:variance:bob")
            .await
            .unwrap();
        assert!(invitations.is_empty(), "admin has no x25519_key stored; no invitations expected");

        // Check member removed
        let group = handler.get_group(&group_id).await.unwrap().unwrap();
        assert_eq!(group.members.len(), 1);
        assert!(!group.members.iter().any(|m| m.did == "did:variance:bob"));

        // Check key rotated
        let new_version = group.current_key.as_ref().unwrap().version;
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
        assert_eq!(message.key_version, 1);

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
                x25519_key: None,
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

        // Group and key should be restored with versioned inner map
        assert!(handler2.get_group(&group_id).await.unwrap().is_some());
        assert!(handler2.group_keys.contains_key(&group_id));
        assert!(
            handler2
                .group_keys
                .get(&group_id)
                .unwrap()
                .contains_key(&1u32),
            "version 1 key should be present after restore"
        );

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

    /// Bob can decrypt a v1 message sent before Carol joined, and a v2 message
    /// sent after Carol was removed (key rotation).
    #[tokio::test]
    async fn test_receive_old_message_after_key_rotation() {
        let alice_dir = tempdir().unwrap();
        let bob_dir = tempdir().unwrap();
        let carol_dir = tempdir().unwrap();

        let alice = GroupMessageHandler::new(
            "did:variance:alice".to_string(),
            SigningKey::generate(&mut OsRng),
            Arc::new(LocalMessageStorage::new(alice_dir.path()).unwrap()),
        );
        let bob = GroupMessageHandler::new(
            "did:variance:bob".to_string(),
            SigningKey::generate(&mut OsRng),
            Arc::new(LocalMessageStorage::new(bob_dir.path()).unwrap()),
        );
        let carol = GroupMessageHandler::new(
            "did:variance:carol".to_string(),
            SigningKey::generate(&mut OsRng),
            Arc::new(LocalMessageStorage::new(carol_dir.path()).unwrap()),
        );

        // Alice creates group and invites Bob
        let (group_id, _) = alice.create_group("Test Group".to_string(), None).await.unwrap();
        let bob_invite = alice
            .add_member(&group_id, "did:variance:bob".to_string(), bob.x25519_public_key())
            .await
            .unwrap();
        bob.accept_invitation(bob_invite).await.unwrap();

        // Alice sends a v1 message
        let v1_msg = alice
            .send_message(
                group_id.clone(),
                MessageContent {
                    text: "v1 message".to_string(),
                    attachments: vec![],
                    mentions: vec![],
                    reply_to: None,
                    metadata: HashMap::new(),
                },
            )
            .await
            .unwrap();
        assert_eq!(v1_msg.key_version, 1);

        // Alice invites Carol, then removes Carol → key rotates to v2
        alice
            .add_member(&group_id, "did:variance:carol".to_string(), carol.x25519_public_key())
            .await
            .unwrap();
        let rekey_invites = alice
            .remove_member(&group_id, "did:variance:carol")
            .await
            .unwrap();

        // Bob accepts his re-key invitation
        let bob_rekey = rekey_invites
            .into_iter()
            .find(|inv| inv.invitee_did == "did:variance:bob")
            .expect("bob should receive a re-key invitation");
        bob.accept_invitation(bob_rekey).await.unwrap();

        // Alice sends a v2 message
        let v2_msg = alice
            .send_message(
                group_id.clone(),
                MessageContent {
                    text: "v2 message".to_string(),
                    attachments: vec![],
                    mentions: vec![],
                    reply_to: None,
                    metadata: HashMap::new(),
                },
            )
            .await
            .unwrap();
        assert_eq!(v2_msg.key_version, 2);

        // Bob can decrypt both v1 and v2 messages
        let dec_v1 = bob.receive_message(v1_msg).await.unwrap();
        assert_eq!(dec_v1.text, "v1 message");

        let dec_v2 = bob.receive_message(v2_msg).await.unwrap();
        assert_eq!(dec_v2.text, "v2 message");
    }

    #[tokio::test]
    async fn test_rotate_key_generates_invitations_for_remaining_members() {
        let alice_dir = tempdir().unwrap();
        let bob_dir = tempdir().unwrap();

        let alice = GroupMessageHandler::new(
            "did:variance:alice".to_string(),
            SigningKey::generate(&mut OsRng),
            Arc::new(LocalMessageStorage::new(alice_dir.path()).unwrap()),
        );
        let bob = GroupMessageHandler::new(
            "did:variance:bob".to_string(),
            SigningKey::generate(&mut OsRng),
            Arc::new(LocalMessageStorage::new(bob_dir.path()).unwrap()),
        );

        let (group_id, _) = alice.create_group("Test".to_string(), None).await.unwrap();
        alice
            .add_member(&group_id, "did:variance:bob".to_string(), bob.x25519_public_key())
            .await
            .unwrap();

        // Rotate key — Bob has an x25519_key stored so he gets an invitation;
        // Alice (creator) has no stored x25519_key so she's skipped.
        let invitations = alice.rotate_key(&group_id).await.unwrap();
        assert_eq!(invitations.len(), 1);
        assert_eq!(invitations[0].invitee_did, "did:variance:bob");
        assert_eq!(invitations[0].key_version, 2);
    }

    #[tokio::test]
    async fn test_non_admin_can_leave_group() {
        let alice_dir = tempdir().unwrap();
        let bob_dir = tempdir().unwrap();

        let alice = GroupMessageHandler::new(
            "did:variance:alice".to_string(),
            SigningKey::generate(&mut OsRng),
            Arc::new(LocalMessageStorage::new(alice_dir.path()).unwrap()),
        );
        let bob = GroupMessageHandler::new(
            "did:variance:bob".to_string(),
            SigningKey::generate(&mut OsRng),
            Arc::new(LocalMessageStorage::new(bob_dir.path()).unwrap()),
        );

        let (group_id, _) = alice.create_group("Test".to_string(), None).await.unwrap();
        let invite = alice
            .add_member(&group_id, "did:variance:bob".to_string(), bob.x25519_public_key())
            .await
            .unwrap();
        bob.accept_invitation(invite).await.unwrap();

        // Bob (non-admin) leaves — should succeed
        let result = bob.remove_member(&group_id, "did:variance:bob").await;
        assert!(result.is_ok(), "non-admin should be able to leave: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_removed_member_cannot_decrypt_after_rotation() {
        let alice_dir = tempdir().unwrap();
        let bob_dir = tempdir().unwrap();
        let carol_dir = tempdir().unwrap();

        let alice = GroupMessageHandler::new(
            "did:variance:alice".to_string(),
            SigningKey::generate(&mut OsRng),
            Arc::new(LocalMessageStorage::new(alice_dir.path()).unwrap()),
        );
        let bob = GroupMessageHandler::new(
            "did:variance:bob".to_string(),
            SigningKey::generate(&mut OsRng),
            Arc::new(LocalMessageStorage::new(bob_dir.path()).unwrap()),
        );
        let carol = GroupMessageHandler::new(
            "did:variance:carol".to_string(),
            SigningKey::generate(&mut OsRng),
            Arc::new(LocalMessageStorage::new(carol_dir.path()).unwrap()),
        );

        let (group_id, _) = alice.create_group("Test".to_string(), None).await.unwrap();
        let bob_invite = alice
            .add_member(&group_id, "did:variance:bob".to_string(), bob.x25519_public_key())
            .await
            .unwrap();
        bob.accept_invitation(bob_invite).await.unwrap();
        let carol_invite = alice
            .add_member(&group_id, "did:variance:carol".to_string(), carol.x25519_public_key())
            .await
            .unwrap();
        carol.accept_invitation(carol_invite).await.unwrap();

        // Remove Carol; key rotates to v2
        alice.remove_member(&group_id, "did:variance:carol").await.unwrap();

        // Alice sends a v2 message
        let v2_msg = alice
            .send_message(
                group_id.clone(),
                MessageContent {
                    text: "post-rotation secret".to_string(),
                    attachments: vec![],
                    mentions: vec![],
                    reply_to: None,
                    metadata: HashMap::new(),
                },
            )
            .await
            .unwrap();
        assert_eq!(v2_msg.key_version, 2);

        // Carol only has v1 key — she cannot decrypt the v2 message
        let carol_result = carol.receive_message(v2_msg).await;
        assert!(
            carol_result.is_err(),
            "Carol should not be able to decrypt after removal"
        );
    }
}
