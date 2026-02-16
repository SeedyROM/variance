use crate::error::*;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, warn};
use variance_media::signaling::SignalingHandler as MediaSignalingHandler;
use variance_proto::media_proto::{signaling_message, CallControlType, SignalingMessage};

/// WebRTC signaling protocol handler
///
/// Wraps the SignalingHandler from variance-media and manages call state.
/// Routes signaling messages to the appropriate call manager.
pub struct SignalingHandler {
    /// Signaling handler for creating and verifying messages
    media_handler: Arc<RwLock<Option<MediaSignalingHandler>>>,

    /// Active calls (call_id -> peer_did)
    active_calls: Arc<RwLock<std::collections::HashMap<String, String>>>,
}

impl SignalingHandler {
    /// Create a new signaling handler
    ///
    /// Note: The media handler is optional at creation and should be set
    /// when the local DID and signing key are available.
    pub fn new() -> Self {
        Self {
            media_handler: Arc::new(RwLock::new(None)),
            active_calls: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// Set the media signaling handler
    ///
    /// Should be called once the local DID and signing key are available.
    pub async fn set_media_handler(&self, handler: MediaSignalingHandler) {
        let mut media_handler = self.media_handler.write().await;
        *media_handler = Some(handler);
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

        // Verify signature if we have the sender's public key
        // TODO: Fetch sender's public key from identity system
        // For now, skip verification

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

        // Track the call
        let mut calls = self.active_calls.write().await;
        calls.insert(message.call_id.clone(), peer_did.clone());
        drop(calls);

        // TODO: Delegate to call manager to create answer
        // For now, return a placeholder response
        let media_handler = self.media_handler.read().await;
        if let Some(ref handler) = *media_handler {
            // Send a ringing control message as acknowledgment
            handler
                .send_control(
                    message.call_id.clone(),
                    peer_did,
                    CallControlType::Ring,
                    None,
                )
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

        // TODO: Delegate to call manager to process answer
        // For now, send acknowledgment
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

        // TODO: Delegate to call manager to process ICE candidate
        // For now, just acknowledge receipt
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

    /// Get active calls
    pub async fn active_calls(&self) -> Vec<(String, String)> {
        let calls = self.active_calls.read().await;
        calls.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

impl Default for SignalingHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use variance_proto::media_proto::{CallType, Offer};

    #[tokio::test]
    async fn test_create_handler() {
        let handler = SignalingHandler::new();
        let calls = handler.active_calls().await;
        assert_eq!(calls.len(), 0);
    }

    #[tokio::test]
    async fn test_set_media_handler() {
        let handler = SignalingHandler::new();

        let signing_key = SigningKey::generate(&mut OsRng);
        let media_handler =
            MediaSignalingHandler::new("did:variance:alice".to_string(), signing_key);

        handler.set_media_handler(media_handler).await;

        let guard = handler.media_handler.read().await;
        assert!(guard.is_some());
    }

    #[tokio::test]
    async fn test_handle_offer() {
        let handler = SignalingHandler::new();

        let signing_key = SigningKey::generate(&mut OsRng);
        let media_handler = MediaSignalingHandler::new("did:variance:bob".to_string(), signing_key);
        handler.set_media_handler(media_handler).await;

        let offer = SignalingMessage {
            call_id: "call123".to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            message: Some(signaling_message::Message::Offer(Offer {
                sdp: "sdp_data".to_string(),
                call_type: CallType::Audio.into(),
            })),
            timestamp: 0,
            signature: vec![],
        };

        let response = handler
            .handle_message("did:variance:alice".to_string(), offer)
            .await
            .unwrap();

        // Should respond with a ringing control message
        assert!(matches!(
            response.message,
            Some(signaling_message::Message::Control(_))
        ));

        // Call should be tracked
        let calls = handler.active_calls().await;
        assert_eq!(calls.len(), 1);
    }

    #[tokio::test]
    async fn test_handle_hangup() {
        let handler = SignalingHandler::new();

        let signing_key = SigningKey::generate(&mut OsRng);
        let media_handler = MediaSignalingHandler::new("did:variance:bob".to_string(), signing_key);
        handler.set_media_handler(media_handler).await;

        // First, simulate an active call
        {
            let mut calls = handler.active_calls.write().await;
            calls.insert("call123".to_string(), "did:variance:alice".to_string());
        }

        // Send hangup
        let hangup = SignalingMessage {
            call_id: "call123".to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            message: Some(signaling_message::Message::Control(
                variance_proto::media_proto::CallControl {
                    r#type: CallControlType::Hangup.into(),
                    reason: Some("User ended call".to_string()),
                },
            )),
            timestamp: 0,
            signature: vec![],
        };

        handler
            .handle_message("did:variance:alice".to_string(), hangup)
            .await
            .unwrap();

        // Call should be removed
        let calls = handler.active_calls().await;
        assert_eq!(calls.len(), 0);
    }
}
