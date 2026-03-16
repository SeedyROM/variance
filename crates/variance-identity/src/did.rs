use crate::error::*;
use chrono::Utc;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier};
use libp2p::PeerId;
use prost::Message;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use variance_proto::identity_proto;
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret as X25519Secret};

/// Wrapper to allow `X25519Secret` in a `#[derive(Debug)]` struct.
/// The secret bytes are intentionally omitted from debug output.
pub struct X25519SecretWrap(Arc<X25519Secret>);

impl std::fmt::Debug for X25519SecretWrap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<x25519 secret>")
    }
}

impl Clone for X25519SecretWrap {
    fn clone(&self) -> Self {
        X25519SecretWrap(Arc::clone(&self.0))
    }
}

/// W3C Decentralized Identifier
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Did {
    pub id: String,
    pub document: DidDocument,
    #[serde(skip)]
    pub signing_key: Option<SigningKey>,
    /// Long-term X25519 secret; wrapped in Arc because StaticSecret is not Clone.
    /// None when the DID is loaded from the network (only the owner holds the secret).
    #[serde(skip)]
    pub x25519_secret: Option<X25519SecretWrap>,
    /// Ed25519 signature over the canonical protobuf-encoded DIDDocument.
    /// Produced by `sign_document()` and verified by `verify_document()`.
    /// None only for self-owned DIDs that haven't been signed yet (transient state).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_signature: Option<Vec<u8>>,
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
    pub metadata: HashMap<String, String>,
}

impl Did {
    /// Create a new DID with a fresh signing key
    pub fn new(peer_id: &PeerId) -> Result<Self> {
        let signing_key = SigningKey::generate(&mut OsRng);
        Self::from_signing_key(signing_key, peer_id)
    }

    /// Create a DID from an existing signing key
    pub fn from_signing_key(signing_key: SigningKey, peer_id: &PeerId) -> Result<Self> {
        let verifying_key = signing_key.verifying_key();
        let did_id = format!("did:peer:{}", peer_id);

        let now = Utc::now().timestamp();

        let x25519_secret = X25519Secret::random_from_rng(OsRng);
        let x25519_public = X25519PublicKey::from(&x25519_secret);

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
            public_key_multibase: x25519_public.as_bytes().to_vec(),
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

        Did {
            id: did_id,
            document,
            signing_key: Some(signing_key),
            x25519_secret: Some(X25519SecretWrap(Arc::new(x25519_secret))),
            document_signature: None,
        }
        .signed()
    }

    /// Update display metadata. Re-signs the document if a signing key is present.
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
        // Re-sign if we own the signing key; ignore error (callers can check signature)
        if self.signing_key.is_some() {
            let _ = self.sign_document();
        }
    }

    /// Extract Ed25519 verifying key from DID document's authentication method
    pub fn get_verifying_key(&self) -> Result<ed25519_dalek::VerifyingKey> {
        let auth_method =
            self.document
                .authentication
                .first()
                .ok_or_else(|| Error::InvalidDid {
                    did: format!("{}: No authentication method in DID document", self.id),
                })?;

        if auth_method.key_type != "Ed25519VerificationKey2020" {
            return Err(Error::InvalidDid {
                did: format!(
                    "{}: Unsupported key type: {}. Expected Ed25519VerificationKey2020",
                    self.id, auth_method.key_type
                ),
            });
        }

        // Extract 32-byte public key
        let pub_key_bytes: &[u8; 32] = auth_method
            .public_key_multibase
            .as_slice()
            .try_into()
            .map_err(|_| Error::InvalidDid {
                did: format!(
                    "{}: Public key must be 32 bytes, got {}",
                    self.id,
                    auth_method.public_key_multibase.len()
                ),
            })?;

        ed25519_dalek::VerifyingKey::from_bytes(pub_key_bytes).map_err(|e| Error::InvalidDid {
            did: format!("{}: Invalid Ed25519 public key: {}", self.id, e),
        })
    }

    /// Extract X25519 public key from DID document's key agreement method
    pub fn get_x25519_public_key(&self) -> Result<X25519PublicKey> {
        let ka_method = self
            .document
            .key_agreement
            .first()
            .ok_or_else(|| Error::InvalidDid {
                did: format!("{}: No key agreement method in DID document", self.id),
            })?;

        let key_bytes: &[u8; 32] = ka_method
            .public_key_multibase
            .as_slice()
            .try_into()
            .map_err(|_| Error::InvalidDid {
                did: format!(
                    "{}: X25519 key must be 32 bytes, got {}",
                    self.id,
                    ka_method.public_key_multibase.len()
                ),
            })?;

        Ok(X25519PublicKey::from(*key_bytes))
    }

    /// Canonical bytes used as the signing payload for document signatures.
    /// This is the deterministic protobuf encoding of the DIDDocument.
    pub fn document_signing_bytes(&self) -> Vec<u8> {
        self.to_proto_document().encode_to_vec()
    }

    /// Sign the DID document in-place using the owner's signing key.
    /// Returns an error if no signing key is available.
    pub fn sign_document(&mut self) -> Result<()> {
        let signing_key = self.signing_key.as_ref().ok_or_else(|| Error::Crypto {
            message: format!("{}: cannot sign without signing key", self.id),
        })?;
        let payload = self.document_signing_bytes();
        let signature = signing_key.sign(&payload);
        self.document_signature = Some(signature.to_bytes().to_vec());
        Ok(())
    }

    /// Consume self and return a signed copy (convenience for chaining after construction).
    fn signed(mut self) -> Result<Self> {
        self.sign_document()?;
        Ok(self)
    }

    /// Verify the document signature against the embedded Ed25519 public key.
    /// Returns `Ok(())` if the signature is present and valid.
    pub fn verify_document(&self) -> Result<()> {
        let sig_bytes =
            self.document_signature
                .as_ref()
                .ok_or_else(|| Error::MissingSignature {
                    did: self.id.clone(),
                })?;

        let signature =
            Signature::from_slice(sig_bytes).map_err(|e| Error::SignatureVerification {
                did: self.id.clone(),
                reason: format!("malformed signature: {e}"),
            })?;

        let verifying_key = self.get_verifying_key()?;

        let payload = self.document_signing_bytes();
        verifying_key
            .verify(&payload, &signature)
            .map_err(|e| Error::SignatureVerification {
                did: self.id.clone(),
                reason: format!("Ed25519 verification failed: {e}"),
            })
    }

    /// Verify that the PeerId embedded in the DID string (`did:peer:<PeerId>`)
    /// matches the expected PeerId. Prevents DID-to-PeerId mapping forgery.
    pub fn verify_peer_id(&self, expected_peer_id: &PeerId) -> Result<()> {
        let embedded = self
            .id
            .strip_prefix("did:peer:")
            .ok_or_else(|| Error::InvalidDid {
                did: format!("{}: not a did:peer: DID", self.id),
            })?;

        let parsed: PeerId = embedded.parse().map_err(|e| Error::InvalidDid {
            did: format!("{}: invalid PeerId in DID: {e}", self.id),
        })?;

        if parsed != *expected_peer_id {
            return Err(Error::PeerIdMismatch {
                did: self.id.clone(),
                expected: expected_peer_id.to_string(),
                actual: parsed.to_string(),
            });
        }
        Ok(())
    }

    /// Convert the DidDocument (only) to protobuf. Used for signing payload
    /// and by `to_proto()`.
    fn to_proto_document(&self) -> identity_proto::DidDocument {
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

    /// Convert to protobuf DIDDocument (public API).
    pub fn to_proto(&self) -> identity_proto::DidDocument {
        self.to_proto_document()
    }

    /// Create from protobuf DIDDocument with an optional document signature.
    /// When `signature` is `Some`, it is stored but **not** automatically verified
    /// — call `verify_document()` explicitly when you need to enforce trust.
    pub fn from_proto_with_signature(
        proto: identity_proto::DidDocument,
        signature: Option<Vec<u8>>,
    ) -> Result<Self> {
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

        // Filter out empty signature bytes (protobuf default)
        let sig = signature.filter(|s| !s.is_empty());

        Ok(Did {
            id: proto.id,
            document,
            signing_key: None,
            x25519_secret: None,
            document_signature: sig,
        })
    }

    /// Create from protobuf DIDDocument without a signature.
    /// The resulting `Did` will have `document_signature: None`.
    pub fn from_proto(proto: identity_proto::DidDocument) -> Result<Self> {
        Self::from_proto_with_signature(proto, None)
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
        assert_eq!(did.document.key_agreement[0].public_key_multibase.len(), 32);
    }

    #[test]
    fn test_get_x25519_public_key() {
        let peer_id = PeerId::random();
        let did = Did::new(&peer_id).unwrap();

        let public_key = did.get_x25519_public_key().unwrap();

        // The extracted public key should match what was stored in the DID document
        assert_eq!(
            public_key.as_bytes(),
            did.document.key_agreement[0]
                .public_key_multibase
                .as_slice()
        );
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

    #[test]
    fn test_get_verifying_key() {
        let peer_id = PeerId::random();
        let did = Did::new(&peer_id).unwrap();

        // Should successfully extract key
        let verifying_key = did.get_verifying_key().unwrap();

        // Verify it matches the original key
        if let Some(signing_key) = &did.signing_key {
            assert_eq!(
                verifying_key.as_bytes(),
                signing_key.verifying_key().as_bytes()
            );
        }
    }
}
