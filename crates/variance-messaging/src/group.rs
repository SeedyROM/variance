use crate::error::*;
use crate::storage::MessageStorage;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use dashmap::DashMap;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use prost::Message;
use rand::RngCore;
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
}

impl GroupMessageHandler {
    /// Create a new group message handler
    pub fn new(
        local_did: String,
        signing_key: SigningKey,
        storage: Arc<dyn MessageStorage>,
    ) -> Self {
        Self {
            local_did,
            signing_key,
            groups: DashMap::new(),
            group_keys: DashMap::new(),
            storage,
        }
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

        // Store group and key
        self.groups.insert(group_id.clone(), group.clone());
        self.group_keys.insert(group_id.clone(), key_bytes);

        Ok((group_id, group))
    }

    /// Add a member to a group
    ///
    /// Only admin/moderators can add members.
    /// Returns a GroupInvitation that should be sent to the invitee.
    pub async fn add_member(
        &self,
        group_id: &str,
        invitee_did: String,
    ) -> Result<GroupInvitation> {
        let mut group_ref = self.groups.get_mut(group_id).ok_or_else(|| {
            Error::GroupNotFound {
                group_id: group_id.to_string(),
            }
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

        // Get group key
        let group_key = self
            .group_keys
            .get(group_id)
            .ok_or_else(|| Error::DoubleRatchet {
                message: "Group key not found".to_string(),
            })?;

        // Create invitation
        let invitation = GroupInvitation {
            group_id: group_id.to_string(),
            group_name: self.groups.get(group_id).unwrap().name.clone(),
            inviter_did: self.local_did.clone(),
            invitee_did: invitee_did.clone(),
            encrypted_group_key: group_key.clone(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            signature: vec![],
        };

        // Sign invitation
        let mut invitation_with_sig = invitation.clone();
        invitation_with_sig.signature = self.sign_invitation(&invitation)?;

        Ok(invitation_with_sig)
    }

    /// Remove a member from a group
    ///
    /// Only admin can remove members. After removal, rotates the group key.
    pub async fn remove_member(&self, group_id: &str, member_did: &str) -> Result<()> {
        let mut group_ref = self.groups.get_mut(group_id).ok_or_else(|| {
            Error::GroupNotFound {
                group_id: group_id.to_string(),
            }
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

        // Rotate key for forward secrecy
        self.rotate_key(group_id).await?;

        Ok(())
    }

    /// Rotate group key
    ///
    /// Generates a new key and increments version.
    /// Should be called after member removal or periodically.
    pub async fn rotate_key(&self, group_id: &str) -> Result<()> {
        let mut group_ref = self.groups.get_mut(group_id).ok_or_else(|| {
            Error::GroupNotFound {
                group_id: group_id.to_string(),
            }
        })?;

        // Generate new key
        let mut key_bytes = vec![0u8; 32];
        rand::thread_rng().fill_bytes(&mut key_bytes);

        let old_version = group_ref.current_key.as_ref().map(|k| k.version).unwrap_or(0);

        let new_key = GroupKey {
            version: old_version + 1,
            key: key_bytes.clone(),
            created_at: chrono::Utc::now().timestamp_millis(),
        };

        group_ref.current_key = Some(new_key);
        drop(group_ref);

        // Update local key
        self.group_keys.insert(group_id.to_string(), key_bytes);

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
        let group = self.groups.get(&group_id).ok_or_else(|| Error::GroupNotFound {
            group_id: group_id.clone(),
        })?;

        if !group.members.iter().any(|m| m.did == self.local_did) {
            return Err(Error::Unauthorized {
                message: "Not a member of this group".to_string(),
            });
        }
        drop(group);

        // Get group key
        let group_key = self.group_keys.get(&group_id).ok_or_else(|| Error::Encryption {
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

        let ciphertext = cipher
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
        let group_key = self
            .group_keys
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
        let content =
            MessageContent::decode(plaintext.as_slice()).map_err(|e| Error::Protocol { source: e })?;

        // Store message
        self.storage.store_group(&message).await?;

        Ok(content)
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
            .filter(|entry| entry.value().members.iter().any(|m| m.did == self.local_did))
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
    /// Adds the group and key to local state.
    pub async fn accept_invitation(&self, invitation: GroupInvitation) -> Result<()> {
        self.group_keys.insert(
            invitation.group_id.clone(),
            invitation.encrypted_group_key.clone(),
        );

        Ok(())
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

        let signature = Signature::from_bytes(
            message
                .signature
                .as_slice()
                .try_into()
                .map_err(|_| Error::InvalidSignature {
                    message_id: message.id.clone(),
                })?,
        );

        sender_public_key
            .verify(&data, &signature)
            .map_err(|_| Error::InvalidSignature {
                message_id: message.id.clone(),
            })?;

        Ok(())
    }

    /// Sign a group invitation
    fn sign_invitation(&self, invitation: &GroupInvitation) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        data.extend_from_slice(invitation.group_id.as_bytes());
        data.extend_from_slice(invitation.inviter_did.as_bytes());
        data.extend_from_slice(invitation.invitee_did.as_bytes());
        data.extend_from_slice(&invitation.encrypted_group_key);
        data.extend_from_slice(&invitation.timestamp.to_le_bytes());

        let signature = self.signing_key.sign(&data);
        Ok(signature.to_bytes().to_vec())
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
        let handler = GroupMessageHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

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
        let handler = GroupMessageHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

        let (group_id, _) = handler
            .create_group("Test Group".to_string(), None)
            .await
            .unwrap();

        let invitation = handler
            .add_member(&group_id, "did:variance:bob".to_string())
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
        let handler = GroupMessageHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

        let (group_id, _) = handler
            .create_group("Test Group".to_string(), None)
            .await
            .unwrap();

        handler
            .add_member(&group_id, "did:variance:bob".to_string())
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

        let handler = GroupMessageHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

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

        let handler = GroupMessageHandler::new(
            "did:variance:alice".to_string(),
            signing_key,
            storage,
        );

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
        assert!(matches!(result.unwrap_err(), Error::InvalidSignature { .. }));
    }
}
