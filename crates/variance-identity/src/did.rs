use crate::error::*;
use chrono::Utc;
use ed25519_dalek::SigningKey;
use libp2p::PeerId;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use variance_proto::identity_proto;

/// W3C Decentralized Identifier
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Did {
    pub id: String,
    pub document: DidDocument,
    #[serde(skip)]
    pub signing_key: Option<SigningKey>,
}

/// DID Document following W3C spec
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DidDocument {
    pub id: String,
    pub authentication: Vec<VerificationMethod>,
    pub key_agreement: Vec<VerificationMethod>,
    pub service: Vec<Service>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_cid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationMethod {
    pub id: String,
    #[serde(rename = "type")]
    pub key_type: String,
    pub controller: String,
    pub public_key_multibase: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub id: String,
    #[serde(rename = "type")]
    pub service_type: String,
    pub service_endpoint: String,
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
}

impl Did {
    /// Create a new DID with a fresh signing key
    pub fn new(peer_id: &PeerId) -> Result<Self> {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        Self::from_signing_key(signing_key, peer_id)
    }

    /// Create a DID from an existing signing key
    pub fn from_signing_key(signing_key: SigningKey, peer_id: &PeerId) -> Result<Self> {
        let verifying_key = signing_key.verifying_key();
        let did_id = format!("did:peer:{}", peer_id);

        let now = Utc::now().timestamp();

        let auth_method = VerificationMethod {
            id: format!("{}#key-1", did_id),
            key_type: "Ed25519VerificationKey2020".to_string(),
            controller: did_id.clone(),
            public_key_multibase: verifying_key.to_bytes().to_vec(),
        };

        let key_agreement = VerificationMethod {
            id: format!("{}#key-2", did_id),
            key_type: "X25519KeyAgreementKey2020".to_string(),
            controller: did_id.clone(),
            public_key_multibase: vec![], // Derived from ed25519 key
        };

        let service = Service {
            id: format!("{}#libp2p", did_id),
            service_type: "Libp2pPeer".to_string(),
            service_endpoint: peer_id.to_string(),
            metadata: Default::default(),
        };

        let document = DidDocument {
            id: did_id.clone(),
            authentication: vec![auth_method],
            key_agreement: vec![key_agreement],
            service: vec![service],
            created_at: now,
            updated_at: now,
            display_name: None,
            avatar_cid: None,
            bio: None,
        };

        Ok(Did {
            id: did_id,
            document,
            signing_key: Some(signing_key),
        })
    }

    /// Update display metadata
    pub fn update_profile(
        &mut self,
        display_name: Option<String>,
        avatar_cid: Option<String>,
        bio: Option<String>,
    ) {
        self.document.display_name = display_name;
        self.document.avatar_cid = avatar_cid;
        self.document.bio = bio;
        self.document.updated_at = Utc::now().timestamp();
    }

    /// Convert to protobuf DIDDocument
    pub fn to_proto(&self) -> identity_proto::DidDocument {
        identity_proto::DidDocument {
            id: self.document.id.clone(),
            authentication: self
                .document
                .authentication
                .iter()
                .map(|vm| identity_proto::VerificationMethod {
                    id: vm.id.clone(),
                    r#type: vm.key_type.clone(),
                    controller: vm.controller.clone(),
                    public_key_multibase: vm.public_key_multibase.clone(),
                })
                .collect(),
            key_agreement: self
                .document
                .key_agreement
                .iter()
                .map(|vm| identity_proto::VerificationMethod {
                    id: vm.id.clone(),
                    r#type: vm.key_type.clone(),
                    controller: vm.controller.clone(),
                    public_key_multibase: vm.public_key_multibase.clone(),
                })
                .collect(),
            service: self
                .document
                .service
                .iter()
                .map(|s| identity_proto::Service {
                    id: s.id.clone(),
                    r#type: s.service_type.clone(),
                    service_endpoint: s.service_endpoint.clone(),
                    metadata: s.metadata.clone(),
                })
                .collect(),
            created_at: self.document.created_at,
            updated_at: self.document.updated_at,
            display_name: self.document.display_name.clone(),
            avatar_cid: self.document.avatar_cid.clone(),
            bio: self.document.bio.clone(),
        }
    }

    /// Create from protobuf DIDDocument
    pub fn from_proto(proto: identity_proto::DidDocument) -> Result<Self> {
        let document = DidDocument {
            id: proto.id.clone(),
            authentication: proto
                .authentication
                .into_iter()
                .map(|vm| VerificationMethod {
                    id: vm.id,
                    key_type: vm.r#type,
                    controller: vm.controller,
                    public_key_multibase: vm.public_key_multibase,
                })
                .collect(),
            key_agreement: proto
                .key_agreement
                .into_iter()
                .map(|vm| VerificationMethod {
                    id: vm.id,
                    key_type: vm.r#type,
                    controller: vm.controller,
                    public_key_multibase: vm.public_key_multibase,
                })
                .collect(),
            service: proto
                .service
                .into_iter()
                .map(|s| Service {
                    id: s.id,
                    service_type: s.r#type,
                    service_endpoint: s.service_endpoint,
                    metadata: s.metadata,
                })
                .collect(),
            created_at: proto.created_at,
            updated_at: proto.updated_at,
            display_name: proto.display_name,
            avatar_cid: proto.avatar_cid,
            bio: proto.bio,
        };

        Ok(Did {
            id: proto.id,
            document,
            signing_key: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_did_creation() {
        let peer_id = PeerId::random();
        let did = Did::new(&peer_id).unwrap();
        assert!(did.id.starts_with("did:peer:"));
        assert_eq!(did.document.authentication.len(), 1);
        assert_eq!(did.document.key_agreement.len(), 1);
        assert_eq!(did.document.service.len(), 1);
    }

    #[test]
    fn test_did_profile_update() {
        let peer_id = PeerId::random();
        let mut did = Did::new(&peer_id).unwrap();

        did.update_profile(
            Some("Alice".to_string()),
            Some("Qm...".to_string()),
            Some("Bio".to_string()),
        );

        assert_eq!(did.document.display_name, Some("Alice".to_string()));
        assert_eq!(did.document.avatar_cid, Some("Qm...".to_string()));
        assert_eq!(did.document.bio, Some("Bio".to_string()));
    }

    #[test]
    fn test_did_proto_conversion() {
        let peer_id = PeerId::random();
        let did = Did::new(&peer_id).unwrap();

        let proto = did.to_proto();
        let recovered = Did::from_proto(proto).unwrap();

        assert_eq!(did.id, recovered.id);
        assert_eq!(did.document.id, recovered.document.id);
    }
}
