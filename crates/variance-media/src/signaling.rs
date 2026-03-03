use crate::error::*;
use dashmap::DashMap;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::RngCore;
use std::collections::HashSet;
use std::sync::Arc;
use variance_proto::media_proto::{
    signaling_message, Answer, CallControl, CallControlType, IceCandidate, Offer, SignalingMessage,
};

/// WebRTC signaling handler
///
/// Manages WebRTC signaling messages (Offer/Answer/ICE) with signature verification
/// and per-call nonce tracking to prevent replay attacks.
pub struct SignalingHandler {
    /// Local DID
    local_did: String,

    /// Signing key for message authentication
    signing_key: SigningKey,

    /// Per-call seen nonces; rejects replayed messages within the same call.
    /// Keyed by call_id; call `purge_call_nonces` when a call ends.
    seen_nonces: Arc<DashMap<String, HashSet<[u8; 16]>>>,
}

impl SignalingHandler {
    /// Create a new signaling handler
    pub fn new(local_did: String, signing_key: SigningKey) -> Self {
        Self {
            local_did,
            signing_key,
            seen_nonces: Arc::new(DashMap::new()),
        }
    }

    /// Discard recorded nonces for a call that has ended.
    ///
    /// Call this after ending, rejecting, or failing a call to free memory.
    pub fn purge_call_nonces(&self, call_id: &str) {
        self.seen_nonces.remove(call_id);
    }

    /// Send an offer
    pub fn send_offer(
        &self,
        call_id: String,
        recipient_did: String,
        sdp: String,
        call_type: variance_proto::media_proto::CallType,
    ) -> Result<SignalingMessage> {
        let offer = Offer {
            sdp,
            call_type: call_type.into(),
        };

        self.create_signaling_message(
            call_id,
            recipient_did,
            signaling_message::Message::Offer(offer),
        )
    }

    /// Send an answer
    pub fn send_answer(
        &self,
        call_id: String,
        recipient_did: String,
        sdp: String,
    ) -> Result<SignalingMessage> {
        let answer = Answer { sdp };

        self.create_signaling_message(
            call_id,
            recipient_did,
            signaling_message::Message::Answer(answer),
        )
    }

    /// Send an ICE candidate
    pub fn send_ice_candidate(
        &self,
        call_id: String,
        recipient_did: String,
        candidate: String,
        sdp_mid: String,
        sdp_m_line_index: Option<u32>,
    ) -> Result<SignalingMessage> {
        let ice = IceCandidate {
            candidate,
            sdp_mid,
            sdp_m_line_index,
        };

        self.create_signaling_message(
            call_id,
            recipient_did,
            signaling_message::Message::IceCandidate(ice),
        )
    }

    /// Send call control message (ring, accept, reject, hangup, mute, etc.)
    pub fn send_control(
        &self,
        call_id: String,
        recipient_did: String,
        control_type: CallControlType,
        reason: Option<String>,
    ) -> Result<SignalingMessage> {
        let control = CallControl {
            r#type: control_type.into(),
            reason,
        };

        self.create_signaling_message(
            call_id,
            recipient_did,
            signaling_message::Message::Control(control),
        )
    }

    /// Create and sign a signaling message
    fn create_signaling_message(
        &self,
        call_id: String,
        recipient_did: String,
        message: signaling_message::Message,
    ) -> Result<SignalingMessage> {
        let timestamp = chrono::Utc::now().timestamp_millis();

        let mut nonce = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut nonce);

        let mut signaling_msg = SignalingMessage {
            call_id,
            sender_did: self.local_did.clone(),
            recipient_did,
            message: Some(message),
            timestamp,
            signature: vec![],
            nonce: nonce.to_vec(),
        };

        // Sign message
        signaling_msg.signature = self.sign_message(&signaling_msg)?;

        Ok(signaling_msg)
    }

    /// Sign a signaling message
    fn sign_message(&self, message: &SignalingMessage) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        data.extend_from_slice(message.call_id.as_bytes());
        data.extend_from_slice(message.sender_did.as_bytes());
        data.extend_from_slice(message.recipient_did.as_bytes());
        data.extend_from_slice(&message.timestamp.to_le_bytes());
        data.extend_from_slice(&message.nonce);

        // Include message type discriminator
        if let Some(ref msg) = message.message {
            match msg {
                signaling_message::Message::Offer(offer) => {
                    data.push(1); // Offer type
                    data.extend_from_slice(offer.sdp.as_bytes());
                    data.extend_from_slice(&offer.call_type.to_le_bytes());
                }
                signaling_message::Message::Answer(answer) => {
                    data.push(2); // Answer type
                    data.extend_from_slice(answer.sdp.as_bytes());
                }
                signaling_message::Message::IceCandidate(ice) => {
                    data.push(3); // ICE type
                    data.extend_from_slice(ice.candidate.as_bytes());
                    data.extend_from_slice(ice.sdp_mid.as_bytes());
                }
                signaling_message::Message::Control(control) => {
                    data.push(4); // Control type
                    data.extend_from_slice(&control.r#type.to_le_bytes());
                }
            }
        }

        let signature = self.signing_key.sign(&data);
        Ok(signature.to_bytes().to_vec())
    }

    /// Verify a signaling message signature and reject replayed nonces.
    ///
    /// NOTE: This requires the sender's public key which must be fetched from their
    /// DID document via the identity system.
    pub fn verify_message(
        &self,
        message: &SignalingMessage,
        sender_public_key: &VerifyingKey,
    ) -> Result<()> {
        // Validate nonce length before doing anything else.
        let nonce: [u8; 16] =
            message
                .nonce
                .as_slice()
                .try_into()
                .map_err(|_| Error::Signaling {
                    message: "invalid nonce length".to_string(),
                })?;

        let mut data = Vec::new();
        data.extend_from_slice(message.call_id.as_bytes());
        data.extend_from_slice(message.sender_did.as_bytes());
        data.extend_from_slice(message.recipient_did.as_bytes());
        data.extend_from_slice(&message.timestamp.to_le_bytes());
        data.extend_from_slice(&message.nonce);

        // Include message type discriminator
        if let Some(ref msg) = message.message {
            match msg {
                signaling_message::Message::Offer(offer) => {
                    data.push(1);
                    data.extend_from_slice(offer.sdp.as_bytes());
                    data.extend_from_slice(&offer.call_type.to_le_bytes());
                }
                signaling_message::Message::Answer(answer) => {
                    data.push(2);
                    data.extend_from_slice(answer.sdp.as_bytes());
                }
                signaling_message::Message::IceCandidate(ice) => {
                    data.push(3);
                    data.extend_from_slice(ice.candidate.as_bytes());
                    data.extend_from_slice(ice.sdp_mid.as_bytes());
                }
                signaling_message::Message::Control(control) => {
                    data.push(4);
                    data.extend_from_slice(&control.r#type.to_le_bytes());
                }
            }
        }

        let signature =
            Signature::from_bytes(message.signature.as_slice().try_into().map_err(|_| {
                Error::InvalidSignature {
                    call_id: message.call_id.clone(),
                }
            })?);

        sender_public_key
            .verify(&data, &signature)
            .map_err(|_| Error::InvalidSignature {
                call_id: message.call_id.clone(),
            })?;

        // Reject replayed nonces after the signature passes (avoids DoS via bad sigs).
        let mut nonce_set = self.seen_nonces.entry(message.call_id.clone()).or_default();
        if !nonce_set.insert(nonce) {
            return Err(Error::Signaling {
                message: "replayed nonce".to_string(),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;
    use variance_proto::media_proto::CallType;

    #[test]
    fn test_create_handler() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = SignalingHandler::new("did:variance:alice".to_string(), signing_key);

        assert_eq!(handler.local_did, "did:variance:alice");
    }

    #[test]
    fn test_send_offer() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = SignalingHandler::new("did:variance:alice".to_string(), signing_key);

        let message = handler
            .send_offer(
                "call123".to_string(),
                "did:variance:bob".to_string(),
                "sdp_offer_data".to_string(),
                CallType::Video,
            )
            .unwrap();

        assert_eq!(message.call_id, "call123");
        assert_eq!(message.sender_did, "did:variance:alice");
        assert_eq!(message.recipient_did, "did:variance:bob");
        assert!(!message.signature.is_empty());
        assert_eq!(message.nonce.len(), 16);

        match message.message {
            Some(signaling_message::Message::Offer(offer)) => {
                assert_eq!(offer.sdp, "sdp_offer_data");
                assert_eq!(offer.call_type, CallType::Video as i32);
            }
            _ => panic!("Expected offer message"),
        }
    }

    #[test]
    fn test_send_answer() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = SignalingHandler::new("did:variance:bob".to_string(), signing_key);

        let message = handler
            .send_answer(
                "call123".to_string(),
                "did:variance:alice".to_string(),
                "sdp_answer_data".to_string(),
            )
            .unwrap();

        assert_eq!(message.call_id, "call123");

        match message.message {
            Some(signaling_message::Message::Answer(answer)) => {
                assert_eq!(answer.sdp, "sdp_answer_data");
            }
            _ => panic!("Expected answer message"),
        }
    }

    #[test]
    fn test_send_ice_candidate() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = SignalingHandler::new("did:variance:alice".to_string(), signing_key);

        let message = handler
            .send_ice_candidate(
                "call123".to_string(),
                "did:variance:bob".to_string(),
                "candidate_data".to_string(),
                "audio".to_string(),
                Some(0),
            )
            .unwrap();

        match message.message {
            Some(signaling_message::Message::IceCandidate(ice)) => {
                assert_eq!(ice.candidate, "candidate_data");
                assert_eq!(ice.sdp_mid, "audio");
                assert_eq!(ice.sdp_m_line_index, Some(0));
            }
            _ => panic!("Expected ICE candidate message"),
        }
    }

    #[test]
    fn test_send_control() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let handler = SignalingHandler::new("did:variance:alice".to_string(), signing_key);

        let message = handler
            .send_control(
                "call123".to_string(),
                "did:variance:bob".to_string(),
                CallControlType::Hangup,
                Some("User ended call".to_string()),
            )
            .unwrap();

        match message.message {
            Some(signaling_message::Message::Control(control)) => {
                assert_eq!(control.r#type, CallControlType::Hangup as i32);
                assert_eq!(control.reason, Some("User ended call".to_string()));
            }
            _ => panic!("Expected control message"),
        }
    }

    #[test]
    fn test_verify_message_success() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let handler = SignalingHandler::new("did:variance:alice".to_string(), signing_key);

        let message = handler
            .send_offer(
                "call123".to_string(),
                "did:variance:bob".to_string(),
                "sdp_data".to_string(),
                CallType::Audio,
            )
            .unwrap();

        // Verify with correct key
        assert!(handler.verify_message(&message, &verifying_key).is_ok());
    }

    #[test]
    fn test_verify_message_failure() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let wrong_key = SigningKey::generate(&mut OsRng).verifying_key();

        let handler = SignalingHandler::new("did:variance:alice".to_string(), signing_key);

        let message = handler
            .send_offer(
                "call123".to_string(),
                "did:variance:bob".to_string(),
                "sdp_data".to_string(),
                CallType::Audio,
            )
            .unwrap();

        // Verify with wrong key should fail
        let result = handler.verify_message(&message, &wrong_key);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            Error::InvalidSignature { .. }
        ));
    }

    #[test]
    fn test_verify_different_message_types() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let handler = SignalingHandler::new("did:variance:alice".to_string(), signing_key);

        // Verify offer
        let offer = handler
            .send_offer(
                "call1".to_string(),
                "did:variance:bob".to_string(),
                "sdp".to_string(),
                CallType::Video,
            )
            .unwrap();
        assert!(handler.verify_message(&offer, &verifying_key).is_ok());

        // Verify answer
        let answer = handler
            .send_answer(
                "call2".to_string(),
                "did:variance:bob".to_string(),
                "sdp".to_string(),
            )
            .unwrap();
        assert!(handler.verify_message(&answer, &verifying_key).is_ok());

        // Verify ICE
        let ice = handler
            .send_ice_candidate(
                "call3".to_string(),
                "did:variance:bob".to_string(),
                "candidate".to_string(),
                "audio".to_string(),
                None,
            )
            .unwrap();
        assert!(handler.verify_message(&ice, &verifying_key).is_ok());

        // Verify control
        let control = handler
            .send_control(
                "call4".to_string(),
                "did:variance:bob".to_string(),
                CallControlType::Ring,
                None,
            )
            .unwrap();
        assert!(handler.verify_message(&control, &verifying_key).is_ok());
    }

    #[test]
    fn test_replay_rejected() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let handler = SignalingHandler::new("did:variance:alice".to_string(), signing_key);

        let message = handler
            .send_offer(
                "call123".to_string(),
                "did:variance:bob".to_string(),
                "sdp_data".to_string(),
                CallType::Audio,
            )
            .unwrap();

        // First verify succeeds
        assert!(handler.verify_message(&message, &verifying_key).is_ok());

        // Replaying the same message fails
        let result = handler.verify_message(&message, &verifying_key);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::Signaling { .. }));
    }

    #[test]
    fn test_purge_call_nonces() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let handler = SignalingHandler::new("did:variance:alice".to_string(), signing_key);

        let message = handler
            .send_offer(
                "call123".to_string(),
                "did:variance:bob".to_string(),
                "sdp_data".to_string(),
                CallType::Audio,
            )
            .unwrap();

        assert!(handler.verify_message(&message, &verifying_key).is_ok());

        // After purging, the nonce set is gone so replaying no longer detected —
        // this is acceptable: purge is called on call end.
        handler.purge_call_nonces("call123");
        assert!(handler.seen_nonces.get("call123").is_none());
    }
}
