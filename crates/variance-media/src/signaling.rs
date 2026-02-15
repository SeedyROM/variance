use crate::error::*;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use variance_proto::media_proto::{
    signaling_message, Answer, CallControl, CallControlType, IceCandidate, Offer,
    SignalingMessage,
};

/// WebRTC signaling handler
///
/// Manages WebRTC signaling messages (Offer/Answer/ICE) with signature verification.
pub struct SignalingHandler {
    /// Local DID
    local_did: String,

    /// Signing key for message authentication
    signing_key: SigningKey,
}

impl SignalingHandler {
    /// Create a new signaling handler
    pub fn new(local_did: String, signing_key: SigningKey) -> Self {
        Self {
            local_did,
            signing_key,
        }
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

        let mut signaling_msg = SignalingMessage {
            call_id,
            sender_did: self.local_did.clone(),
            recipient_did,
            message: Some(message),
            timestamp,
            signature: vec![],
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

    /// Verify a signaling message signature
    ///
    /// NOTE: This requires the sender's public key which must be fetched from their
    /// DID document via the identity system.
    pub fn verify_message(
        &self,
        message: &SignalingMessage,
        sender_public_key: &VerifyingKey,
    ) -> Result<()> {
        let mut data = Vec::new();
        data.extend_from_slice(message.call_id.as_bytes());
        data.extend_from_slice(message.sender_did.as_bytes());
        data.extend_from_slice(message.recipient_did.as_bytes());
        data.extend_from_slice(&message.timestamp.to_le_bytes());

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

        let signature = Signature::from_bytes(
            message
                .signature
                .as_slice()
                .try_into()
                .map_err(|_| Error::InvalidSignature {
                    call_id: message.call_id.clone(),
                })?,
        );

        sender_public_key
            .verify(&data, &signature)
            .map_err(|_| Error::InvalidSignature {
                call_id: message.call_id.clone(),
            })?;

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
        assert!(matches!(result.unwrap_err(), Error::InvalidSignature { .. }));
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
}
