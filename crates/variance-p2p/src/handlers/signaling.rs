use crate::error::*;
use crate::handlers::identity::IdentityHandler;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, warn};
use variance_media::call::CallManager;
use variance_media::signaling::SignalingHandler as MediaSignalingHandler;
use variance_proto::media_proto::{signaling_message, CallControlType, SignalingMessage};

/// WebRTC signaling protocol handler
///
/// Wraps the SignalingHandler from variance-media and manages call state.
/// Routes signaling messages to the appropriate call manager and handles
/// WebRTC peer connection lifecycle.
pub struct SignalingHandler {
    /// Signaling handler for creating and verifying messages
    media_handler: Arc<RwLock<Option<MediaSignalingHandler>>>,

    /// Call manager for WebRTC peer connections
    call_manager: Arc<RwLock<Option<Arc<CallManager>>>>,

    /// Active calls (call_id -> peer_did)
    active_calls: Arc<RwLock<HashMap<String, String>>>,

    /// Identity handler for resolving DIDs and public keys
    identity_handler: Arc<IdentityHandler>,
}

impl SignalingHandler {
    /// Create a new signaling handler
    ///
    /// Note: The media handler and call manager are optional at creation
    /// and should be set when the local DID and signing key are available.
    pub fn new(identity_handler: Arc<IdentityHandler>) -> Self {
        Self {
            media_handler: Arc::new(RwLock::new(None)),
            call_manager: Arc::new(RwLock::new(None)),
            active_calls: Arc::new(RwLock::new(HashMap::new())),
            identity_handler,
        }
    }

    /// Set the media signaling handler
    ///
    /// Should be called once the local DID and signing key are available.
    pub async fn set_media_handler(&self, handler: MediaSignalingHandler) {
        let mut media_handler = self.media_handler.write().await;
        *media_handler = Some(handler);
    }

    /// Set the call manager
    ///
    /// Should be called once the local DID is available.
    pub async fn set_call_manager(&self, manager: Arc<CallManager>) {
        let mut call_manager = self.call_manager.write().await;
        *call_manager = Some(manager);
    }

    /// Handle an incoming signaling message
    pub async fn handle_message(
        &self,
        peer_did: String,
        message: SignalingMessage,
    ) -> Result<SignalingMessage> {
        debug!(
            "Handling signaling message for call {} from {}",
            message.call_id, peer_did
        );

        // Verify signature before processing
        self.verify_signature(&message).await?;

        match message.message {
            Some(signaling_message::Message::Offer(_)) => {
                self.handle_offer(peer_did, message).await
            }
            Some(signaling_message::Message::Answer(_)) => {
                self.handle_answer(peer_did, message).await
            }
            Some(signaling_message::Message::IceCandidate(_)) => {
                self.handle_ice_candidate(peer_did, message).await
            }
            Some(signaling_message::Message::Control(_)) => {
                self.handle_control(peer_did, message).await
            }
            None => {
                error!("Received signaling message with no content");
                Err(Error::InvalidMessage {
                    message: "Signaling message has no content".to_string(),
                })
            }
        }
    }

    /// Handle an offer
    async fn handle_offer(
        &self,
        peer_did: String,
        message: SignalingMessage,
    ) -> Result<SignalingMessage> {
        debug!("Handling offer for call {}", message.call_id);

        // Extract offer SDP
        let offer_sdp = match &message.message {
            Some(signaling_message::Message::Offer(offer)) => offer.sdp.clone(),
            _ => {
                return Err(Error::InvalidMessage {
                    message: "Expected offer message".to_string(),
                })
            }
        };

        // Track the call
        let mut calls = self.active_calls.write().await;
        calls.insert(message.call_id.clone(), peer_did.clone());
        drop(calls);

        // Get call manager
        let call_manager_guard = self.call_manager.read().await;
        let call_manager = call_manager_guard
            .as_ref()
            .ok_or_else(|| Error::InvalidMessage {
                message: "Call manager not initialized".to_string(),
            })?;

        // Handle offer and create answer using WebRTC
        let answer_sdp = call_manager
            .handle_offer(&message.call_id, offer_sdp)
            .await
            .map_err(|e| Error::InvalidMessage {
                message: format!("Failed to handle offer: {}", e),
            })?;

        // Get media handler to create answer message
        let media_handler = self.media_handler.read().await;
        if let Some(ref handler) = *media_handler {
            // Send answer with the generated SDP
            handler
                .send_answer(message.call_id.clone(), peer_did, answer_sdp)
                .map_err(|e| Error::InvalidMessage {
                    message: e.to_string(),
                })
        } else {
            warn!("Media handler not initialized, cannot respond to offer");
            Err(Error::InvalidMessage {
                message: "Media handler not initialized".to_string(),
            })
        }
    }

    /// Handle an answer
    async fn handle_answer(
        &self,
        peer_did: String,
        message: SignalingMessage,
    ) -> Result<SignalingMessage> {
        debug!("Handling answer for call {}", message.call_id);

        // Extract answer SDP
        let answer_sdp = match &message.message {
            Some(signaling_message::Message::Answer(answer)) => answer.sdp.clone(),
            _ => {
                return Err(Error::InvalidMessage {
                    message: "Expected answer message".to_string(),
                })
            }
        };

        // Get call manager
        let call_manager_guard = self.call_manager.read().await;
        let call_manager = call_manager_guard
            .as_ref()
            .ok_or_else(|| Error::InvalidMessage {
                message: "Call manager not initialized".to_string(),
            })?;

        // Process answer using WebRTC
        call_manager
            .handle_answer(&message.call_id, answer_sdp)
            .await
            .map_err(|e| Error::InvalidMessage {
                message: format!("Failed to handle answer: {}", e),
            })?;

        // Send acknowledgment
        let media_handler = self.media_handler.read().await;
        if let Some(ref handler) = *media_handler {
            handler
                .send_control(
                    message.call_id.clone(),
                    peer_did,
                    CallControlType::Accept,
                    None,
                )
                .map_err(|e| Error::InvalidMessage {
                    message: e.to_string(),
                })
        } else {
            warn!("Media handler not initialized, cannot respond to answer");
            Err(Error::InvalidMessage {
                message: "Media handler not initialized".to_string(),
            })
        }
    }

    /// Handle an ICE candidate
    async fn handle_ice_candidate(
        &self,
        _peer_did: String,
        message: SignalingMessage,
    ) -> Result<SignalingMessage> {
        debug!("Handling ICE candidate for call {}", message.call_id);

        // Extract ICE candidate data
        let (candidate, sdp_mid, sdp_mline_index) = match &message.message {
            Some(signaling_message::Message::IceCandidate(ice)) => (
                ice.candidate.clone(),
                ice.sdp_mid.clone(),
                ice.sdp_m_line_index.map(|i| i as u16),
            ),
            _ => {
                return Err(Error::InvalidMessage {
                    message: "Expected ICE candidate message".to_string(),
                })
            }
        };

        // Get call manager
        let call_manager_guard = self.call_manager.read().await;
        let call_manager = call_manager_guard
            .as_ref()
            .ok_or_else(|| Error::InvalidMessage {
                message: "Call manager not initialized".to_string(),
            })?;

        // Add ICE candidate to peer connection
        call_manager
            .handle_ice_candidate(&message.call_id, candidate, Some(sdp_mid), sdp_mline_index)
            .await
            .map_err(|e| Error::InvalidMessage {
                message: format!("Failed to handle ICE candidate: {}", e),
            })?;

        // ICE candidates don't require a response in the protocol
        Ok(message)
    }

    /// Handle a control message
    async fn handle_control(
        &self,
        peer_did: String,
        message: SignalingMessage,
    ) -> Result<SignalingMessage> {
        if let Some(signaling_message::Message::Control(ref control)) = message.message {
            let control_type =
                CallControlType::try_from(control.r#type).unwrap_or(CallControlType::Unspecified);

            debug!(
                "Handling control message {:?} for call {} from {}",
                control_type, message.call_id, peer_did
            );

            match control_type {
                CallControlType::Hangup | CallControlType::Reject => {
                    // Remove call from active calls
                    let mut calls = self.active_calls.write().await;
                    calls.remove(&message.call_id);
                }
                CallControlType::Accept => {
                    // Call accepted, keep tracking it
                }
                _ => {
                    // Other control messages (mute, unmute, etc.)
                }
            }
        }

        // Control messages don't require a response
        Ok(message)
    }

    /// Verify signaling message signature
    async fn verify_signature(&self, message: &SignalingMessage) -> Result<()> {
        // Get sender's DID from cache
        let did = self
            .identity_handler
            .get_cached_did(&message.sender_did)
            .await
            .ok_or_else(|| Error::Protocol {
                message: format!(
                    "Sender DID not found in cache: {}. Cannot verify signature.",
                    message.sender_did
                ),
            })?;

        // Extract verifying key from DID document
        let verifying_key = did.get_verifying_key().map_err(|e| Error::Protocol {
            message: format!("Failed to extract public key from DID: {}", e),
        })?;

        // Get media handler to verify signature
        let media_handler_guard = self.media_handler.read().await;
        let media_handler = media_handler_guard
            .as_ref()
            .ok_or_else(|| Error::Protocol {
                message: "Media handler not initialized".to_string(),
            })?;

        // Verify signature
        media_handler
            .verify_message(message, &verifying_key)
            .map_err(|e| Error::Protocol {
                message: format!("Signature verification failed: {}", e),
            })?;

        debug!(
            "Successfully verified signature for call {} from {}",
            message.call_id, message.sender_did
        );

        Ok(())
    }

    /// Get active calls
    pub async fn active_calls(&self) -> Vec<(String, String)> {
        let calls = self.active_calls.read().await;
        calls.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand_core::OsRng;

    #[tokio::test]
    async fn test_create_handler() {
        let peer_id = libp2p::PeerId::random();
        let identity_handler = Arc::new(IdentityHandler::new(peer_id));
        let handler = SignalingHandler::new(identity_handler);
        let calls = handler.active_calls().await;
        assert_eq!(calls.len(), 0);
    }

    #[tokio::test]
    async fn test_set_media_handler() {
        let peer_id = libp2p::PeerId::random();
        let identity_handler = Arc::new(IdentityHandler::new(peer_id));
        let handler = SignalingHandler::new(identity_handler);

        let signing_key = SigningKey::generate(&mut OsRng);
        let media_handler =
            MediaSignalingHandler::new("did:variance:alice".to_string(), signing_key);

        handler.set_media_handler(media_handler).await;

        let guard = handler.media_handler.read().await;
        assert!(guard.is_some());
    }

    #[tokio::test]
    async fn test_handle_offer() {
        let peer_id = libp2p::PeerId::random();
        let identity_handler = Arc::new(IdentityHandler::new(peer_id));
        let handler = SignalingHandler::new(identity_handler.clone());

        // Create sender's DID and media handler
        let sender_peer_id = libp2p::PeerId::random();
        let sender_did = variance_identity::did::Did::new(&sender_peer_id).unwrap();
        let sender_signing_key = sender_did.signing_key.clone().unwrap();
        identity_handler
            .cache_did(sender_did.clone())
            .await
            .unwrap();

        let sender_handler = MediaSignalingHandler::new(sender_did.id.clone(), sender_signing_key);

        // Set up call manager
        let call_manager = Arc::new(
            CallManager::new(
                "did:variance:bob".to_string(),
                vec!["stun:stun.l.google.com:19302".to_string()],
            )
            .unwrap(),
        );
        handler.set_call_manager(call_manager.clone()).await;

        // Register incoming call in call manager
        call_manager.register_incoming_call(
            "call123".to_string(),
            sender_did.id.clone(),
            variance_proto::media_proto::CallType::Audio,
        );

        let signing_key = SigningKey::generate(&mut OsRng);
        let media_handler = MediaSignalingHandler::new("did:variance:bob".to_string(), signing_key);
        handler.set_media_handler(media_handler).await;

        // Create and sign the offer with valid SDP
        let valid_sdp = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n\
                        a=group:BUNDLE 0\r\na=ice-options:trickle\r\n\
                        m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n\
                        c=IN IP4 0.0.0.0\r\na=ice-ufrag:test\r\na=ice-pwd:testpassword\r\n\
                        a=fingerprint:sha-256 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00\r\n\
                        a=setup:actpass\r\na=mid:0\r\na=sctp-port:5000\r\n"
            .to_string();

        let offer = sender_handler
            .send_offer(
                "call123".to_string(),
                "did:variance:bob".to_string(),
                valid_sdp,
                variance_proto::media_proto::CallType::Audio,
            )
            .unwrap();

        let response = handler
            .handle_message(sender_did.id.clone(), offer)
            .await
            .unwrap();

        // Should respond with an answer message containing SDP
        assert!(matches!(
            response.message,
            Some(signaling_message::Message::Answer(_))
        ));

        // Call should be tracked
        let calls = handler.active_calls().await;
        assert_eq!(calls.len(), 1);
    }

    #[tokio::test]
    async fn test_handle_hangup() {
        let peer_id = libp2p::PeerId::random();
        let identity_handler = Arc::new(IdentityHandler::new(peer_id));
        let handler = SignalingHandler::new(identity_handler.clone());

        // Create sender's DID and media handler
        let sender_peer_id = libp2p::PeerId::random();
        let sender_did = variance_identity::did::Did::new(&sender_peer_id).unwrap();
        let sender_signing_key = sender_did.signing_key.clone().unwrap();
        identity_handler
            .cache_did(sender_did.clone())
            .await
            .unwrap();

        let sender_handler = MediaSignalingHandler::new(sender_did.id.clone(), sender_signing_key);

        let signing_key = SigningKey::generate(&mut OsRng);
        let media_handler = MediaSignalingHandler::new("did:variance:bob".to_string(), signing_key);
        handler.set_media_handler(media_handler).await;

        // First, simulate an active call
        {
            let mut calls = handler.active_calls.write().await;
            calls.insert("call123".to_string(), sender_did.id.clone());
        }

        // Send hangup (create and sign properly)
        let hangup = sender_handler
            .send_control(
                "call123".to_string(),
                "did:variance:bob".to_string(),
                CallControlType::Hangup,
                Some("User ended call".to_string()),
            )
            .unwrap();

        handler
            .handle_message(sender_did.id.clone(), hangup)
            .await
            .unwrap();

        // Call should be removed
        let calls = handler.active_calls().await;
        assert_eq!(calls.len(), 0);
    }
}
