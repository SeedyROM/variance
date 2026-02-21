use crate::error::*;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use ulid::Ulid;
use variance_proto::media_proto::{CallState, CallStatus, CallType};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

/// Events emitted by CallManager for state changes and ICE candidates
#[derive(Debug, Clone)]
pub enum CallEvent {
    /// WebRTC connection state changed, call status updated
    StateChanged { call_id: String, status: CallStatus },
    /// Local ICE candidate gathered, needs to be sent to remote peer
    IceCandidateGathered {
        call_id: String,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
}

/// Call manager
///
/// Manages active call states and WebRTC peer connections.
/// Handles the full WebRTC lifecycle including:
/// - Peer connection creation and configuration
/// - SDP offer/answer negotiation
/// - ICE candidate gathering and exchange
/// - Connection state monitoring
pub struct CallManager {
    /// Local DID
    local_did: String,

    /// Active calls indexed by call ID
    calls: Arc<DashMap<String, Call>>,

    /// WebRTC API instance (configured with codecs and interceptors)
    webrtc_api: webrtc::api::API,

    /// STUN/TURN servers for NAT traversal
    ice_servers: Vec<String>,

    /// Broadcast channel for call events (state changes, ICE candidates)
    event_tx: broadcast::Sender<CallEvent>,
}

/// Call state
pub struct Call {
    /// Unique call ID
    pub id: String,

    /// Participant DIDs
    pub participants: Vec<String>,

    /// Call type (audio, video, screen share)
    pub call_type: CallType,

    /// Current status
    pub status: CallStatus,

    /// Start timestamp (milliseconds)
    pub started_at: i64,

    /// End timestamp (milliseconds)
    pub ended_at: Option<i64>,

    /// WebRTC peer connection (present for active calls)
    pub peer_connection: Arc<Mutex<Option<Arc<RTCPeerConnection>>>>,
}

// Manual Clone implementation since RTCPeerConnection is not Clone
impl Clone for Call {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            participants: self.participants.clone(),
            call_type: self.call_type,
            status: self.status,
            started_at: self.started_at,
            ended_at: self.ended_at,
            peer_connection: Arc::new(Mutex::new(None)),
        }
    }
}

// Manual Debug implementation to avoid exposing RTCPeerConnection internals
impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Call")
            .field("id", &self.id)
            .field("participants", &self.participants)
            .field("call_type", &self.call_type)
            .field("status", &self.status)
            .field("started_at", &self.started_at)
            .field("ended_at", &self.ended_at)
            .field("peer_connection", &"<RTCPeerConnection>")
            .finish()
    }
}

impl CallManager {
    /// Create a new call manager
    ///
    /// Configures WebRTC with default codecs and interceptors.
    /// Uses provided STUN/TURN servers for NAT traversal.
    pub fn new(local_did: String, ice_servers: Vec<String>) -> Result<Self> {
        // Configure media engine with default codecs
        let mut media_engine = MediaEngine::default();
        media_engine
            .register_default_codecs()
            .map_err(|e| Error::WebRtc {
                message: format!("Failed to register codecs: {}", e),
            })?;

        // Create interceptor registry for RTCP/TWCC
        let mut registry = interceptor::registry::Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine).map_err(|e| {
            Error::WebRtc {
                message: format!("Failed to register interceptors: {}", e),
            }
        })?;

        // Build WebRTC API
        let webrtc_api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build();

        let (event_tx, _) = broadcast::channel(64);

        Ok(Self {
            local_did,
            calls: Arc::new(DashMap::new()),
            webrtc_api,
            ice_servers,
            event_tx,
        })
    }

    /// Create a new outgoing call
    pub fn create_call(&self, recipient_did: String, call_type: CallType) -> Call {
        let call_id = Ulid::new().to_string();
        let now = chrono::Utc::now().timestamp_millis();

        let call = Call {
            id: call_id.clone(),
            participants: vec![self.local_did.clone(), recipient_did],
            call_type,
            status: CallStatus::Ringing,
            started_at: now,
            ended_at: None,
            peer_connection: Arc::new(Mutex::new(None)),
        };

        self.calls.insert(call_id, call.clone());
        call
    }

    /// Accept an incoming call
    pub fn accept_call(&self, call_id: &str) -> Result<Call> {
        let mut call_ref = self
            .calls
            .get_mut(call_id)
            .ok_or_else(|| Error::CallNotFound {
                call_id: call_id.to_string(),
            })?;

        if call_ref.status != CallStatus::Ringing {
            return Err(Error::InvalidState {
                message: format!("Cannot accept call in state {:?}", call_ref.status),
            });
        }

        call_ref.status = CallStatus::Connecting;
        Ok(call_ref.clone())
    }

    /// Reject an incoming call
    pub async fn reject_call(&self, call_id: &str) -> Result<Call> {
        let mut call_ref = self
            .calls
            .get_mut(call_id)
            .ok_or_else(|| Error::CallNotFound {
                call_id: call_id.to_string(),
            })?;

        if call_ref.status != CallStatus::Ringing {
            return Err(Error::InvalidState {
                message: format!("Cannot reject call in state {:?}", call_ref.status),
            });
        }

        let now = chrono::Utc::now().timestamp_millis();
        call_ref.status = CallStatus::Ended;
        call_ref.ended_at = Some(now);

        // Close peer connection if present
        let pc_guard = call_ref.peer_connection.lock().await;
        if let Some(ref peer_connection) = *pc_guard {
            let _ = peer_connection.close().await; // Ignore close errors on reject
        }
        drop(pc_guard);

        let call = call_ref.clone();
        drop(call_ref);

        // Remove from active calls
        self.calls.remove(call_id);

        Ok(call)
    }

    /// Mark call as active (connected)
    pub fn activate_call(&self, call_id: &str) -> Result<Call> {
        let mut call_ref = self
            .calls
            .get_mut(call_id)
            .ok_or_else(|| Error::CallNotFound {
                call_id: call_id.to_string(),
            })?;

        if call_ref.status != CallStatus::Connecting {
            return Err(Error::InvalidState {
                message: format!("Cannot activate call in state {:?}", call_ref.status),
            });
        }

        call_ref.status = CallStatus::Active;
        Ok(call_ref.clone())
    }

    /// End an active call
    ///
    /// Closes the WebRTC peer connection and removes call from active calls.
    pub async fn end_call(&self, call_id: &str) -> Result<Call> {
        let mut call_ref = self
            .calls
            .get_mut(call_id)
            .ok_or_else(|| Error::CallNotFound {
                call_id: call_id.to_string(),
            })?;

        let now = chrono::Utc::now().timestamp_millis();
        call_ref.status = CallStatus::Ended;
        call_ref.ended_at = Some(now);

        // Close peer connection if present
        let pc_guard = call_ref.peer_connection.lock().await;
        if let Some(ref peer_connection) = *pc_guard {
            peer_connection.close().await.map_err(|e| Error::WebRtc {
                message: format!("Failed to close peer connection: {}", e),
            })?;
        }
        drop(pc_guard);

        let call = call_ref.clone();
        drop(call_ref);

        // Remove from active calls
        self.calls.remove(call_id);

        Ok(call)
    }

    /// Mark call as failed
    pub async fn fail_call(&self, call_id: &str) -> Result<Call> {
        let mut call_ref = self
            .calls
            .get_mut(call_id)
            .ok_or_else(|| Error::CallNotFound {
                call_id: call_id.to_string(),
            })?;

        let now = chrono::Utc::now().timestamp_millis();
        call_ref.status = CallStatus::Failed;
        call_ref.ended_at = Some(now);

        // Close peer connection if present
        let pc_guard = call_ref.peer_connection.lock().await;
        if let Some(ref peer_connection) = *pc_guard {
            let _ = peer_connection.close().await; // Ignore close errors on failure
        }
        drop(pc_guard);

        let call = call_ref.clone();
        drop(call_ref);

        // Remove from active calls
        self.calls.remove(call_id);

        Ok(call)
    }

    /// Create WebRTC offer for an outgoing call
    ///
    /// Creates a peer connection, generates an SDP offer, and returns the SDP string.
    pub async fn create_offer(&self, call_id: &str) -> Result<String> {
        let call = self.calls.get(call_id).ok_or_else(|| Error::CallNotFound {
            call_id: call_id.to_string(),
        })?;

        // Create peer connection with state/ICE handlers wired to this call
        let peer_connection = self.create_peer_connection(call_id).await?;

        // TODO: Add media tracks based on call_type
        // For now, create data channel to trigger offer generation
        peer_connection
            .create_data_channel("variance", None)
            .await
            .map_err(|e| Error::WebRtc {
                message: format!("Failed to create data channel: {}", e),
            })?;

        // Create SDP offer
        let offer = peer_connection
            .create_offer(None)
            .await
            .map_err(|e| Error::WebRtc {
                message: format!("Failed to create offer: {}", e),
            })?;

        // Set local description
        peer_connection
            .set_local_description(offer.clone())
            .await
            .map_err(|e| Error::WebRtc {
                message: format!("Failed to set local description: {}", e),
            })?;

        // Store peer connection
        let mut pc_guard = call.peer_connection.lock().await;
        *pc_guard = Some(Arc::new(peer_connection));

        Ok(offer.sdp)
    }

    /// Handle incoming WebRTC offer
    ///
    /// Sets remote description from offer, creates answer, and returns SDP string.
    pub async fn handle_offer(&self, call_id: &str, offer_sdp: String) -> Result<String> {
        let call = self.calls.get(call_id).ok_or_else(|| Error::CallNotFound {
            call_id: call_id.to_string(),
        })?;

        // Create peer connection with state/ICE handlers wired to this call
        let peer_connection = self.create_peer_connection(call_id).await?;

        // Set remote description from offer
        peer_connection
            .set_remote_description(RTCSessionDescription::offer(offer_sdp).map_err(|e| {
                Error::WebRtc {
                    message: format!("Invalid SDP offer: {}", e),
                }
            })?)
            .await
            .map_err(|e| Error::WebRtc {
                message: format!("Failed to set remote description: {}", e),
            })?;

        // Create SDP answer
        let answer = peer_connection
            .create_answer(None)
            .await
            .map_err(|e| Error::WebRtc {
                message: format!("Failed to create answer: {}", e),
            })?;

        // Set local description
        peer_connection
            .set_local_description(answer.clone())
            .await
            .map_err(|e| Error::WebRtc {
                message: format!("Failed to set local description: {}", e),
            })?;

        // Store peer connection
        let mut pc_guard = call.peer_connection.lock().await;
        *pc_guard = Some(Arc::new(peer_connection));

        Ok(answer.sdp)
    }

    /// Handle incoming WebRTC answer
    ///
    /// Sets remote description from answer to complete the connection.
    pub async fn handle_answer(&self, call_id: &str, answer_sdp: String) -> Result<()> {
        let call = self.calls.get(call_id).ok_or_else(|| Error::CallNotFound {
            call_id: call_id.to_string(),
        })?;

        let pc_guard = call.peer_connection.lock().await;
        let peer_connection = pc_guard.as_ref().ok_or_else(|| Error::InvalidState {
            message: "No peer connection found for call".to_string(),
        })?;

        // Set remote description from answer
        peer_connection
            .set_remote_description(RTCSessionDescription::answer(answer_sdp).map_err(|e| {
                Error::WebRtc {
                    message: format!("Invalid SDP answer: {}", e),
                }
            })?)
            .await
            .map_err(|e| Error::WebRtc {
                message: format!("Failed to set remote description: {}", e),
            })?;

        Ok(())
    }

    /// Handle incoming ICE candidate
    ///
    /// Adds ICE candidate to the peer connection for connectivity checks.
    pub async fn handle_ice_candidate(
        &self,
        call_id: &str,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    ) -> Result<()> {
        let call = self.calls.get(call_id).ok_or_else(|| Error::CallNotFound {
            call_id: call_id.to_string(),
        })?;

        let pc_guard = call.peer_connection.lock().await;
        let peer_connection = pc_guard.as_ref().ok_or_else(|| Error::InvalidState {
            message: "No peer connection found for call".to_string(),
        })?;

        // Add ICE candidate
        peer_connection
            .add_ice_candidate(webrtc::ice_transport::ice_candidate::RTCIceCandidateInit {
                candidate,
                sdp_mid,
                sdp_mline_index,
                username_fragment: None,
            })
            .await
            .map_err(|e| Error::WebRtc {
                message: format!("Failed to add ICE candidate: {}", e),
            })?;

        Ok(())
    }

    /// Get call by ID
    pub fn get_call(&self, call_id: &str) -> Option<Call> {
        self.calls.get(call_id).map(|r| r.clone())
    }

    /// Get the remote peer DID for a call (the participant that isn't us)
    pub fn get_remote_peer(&self, call_id: &str) -> Option<String> {
        self.calls.get(call_id).and_then(|call| {
            call.participants
                .iter()
                .find(|p| *p != &self.local_did)
                .cloned()
        })
    }

    /// List all active calls
    pub fn list_active_calls(&self) -> Vec<Call> {
        self.calls
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Register an incoming call
    pub fn register_incoming_call(
        &self,
        call_id: String,
        caller_did: String,
        call_type: CallType,
    ) -> Call {
        let now = chrono::Utc::now().timestamp_millis();

        let call = Call {
            id: call_id.clone(),
            participants: vec![caller_did, self.local_did.clone()],
            call_type,
            status: CallStatus::Ringing,
            started_at: now,
            ended_at: None,
            peer_connection: Arc::new(Mutex::new(None)),
        };

        self.calls.insert(call_id, call.clone());
        call
    }

    /// Convert Call to protobuf format
    pub fn call_to_proto(&self, call: &Call) -> CallState {
        CallState {
            call_id: call.id.clone(),
            participants: call.participants.clone(),
            call_type: call.call_type.into(),
            status: call.status.into(),
            started_at: call.started_at,
            ended_at: call.ended_at,
        }
    }

    /// Subscribe to call events (state changes, ICE candidates)
    pub fn subscribe(&self) -> broadcast::Receiver<CallEvent> {
        self.event_tx.subscribe()
    }

    /// Create a configured WebRTC peer connection
    ///
    /// Configures STUN/TURN servers and sets up state change + ICE candidate handlers.
    /// The `call_id` is used to update the correct call state when WebRTC events fire.
    async fn create_peer_connection(&self, call_id: &str) -> Result<RTCPeerConnection> {
        // Configure ICE servers
        let config = RTCConfiguration {
            ice_servers: self
                .ice_servers
                .iter()
                .map(|url| RTCIceServer {
                    urls: vec![url.clone()],
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };

        // Create peer connection
        let peer_connection = self
            .webrtc_api
            .new_peer_connection(config)
            .await
            .map_err(|e| Error::WebRtc {
                message: format!("Failed to create peer connection: {}", e),
            })?;

        // Set up connection state change handler
        // Maps RTCPeerConnectionState → CallStatus and updates the call in DashMap
        let calls = Arc::clone(&self.calls);
        let event_tx = self.event_tx.clone();
        let owned_call_id = call_id.to_string();
        peer_connection.on_peer_connection_state_change(Box::new(
            move |state: RTCPeerConnectionState| {
                let calls = Arc::clone(&calls);
                let event_tx = event_tx.clone();
                let call_id = owned_call_id.clone();

                Box::pin(async move {
                    tracing::debug!(
                        "Peer connection state changed for call {}: {:?}",
                        call_id,
                        state
                    );

                    let new_status = match state {
                        RTCPeerConnectionState::Connected => Some(CallStatus::Active),
                        RTCPeerConnectionState::Failed => Some(CallStatus::Failed),
                        RTCPeerConnectionState::Disconnected => Some(CallStatus::Failed),
                        RTCPeerConnectionState::Closed => Some(CallStatus::Ended),
                        _ => None,
                    };

                    if let Some(status) = new_status {
                        if let Some(mut call_ref) = calls.get_mut(&call_id) {
                            call_ref.status = status;

                            if matches!(status, CallStatus::Failed | CallStatus::Ended) {
                                call_ref.ended_at = Some(chrono::Utc::now().timestamp_millis());
                            }
                        }

                        let _ = event_tx.send(CallEvent::StateChanged { call_id, status });
                    }
                })
            },
        ));

        // Set up ICE candidate handler
        // Sends gathered local candidates via broadcast so they can be relayed to the remote peer
        let event_tx = self.event_tx.clone();
        let owned_call_id = call_id.to_string();
        peer_connection.on_ice_candidate(Box::new(move |candidate| {
            let event_tx = event_tx.clone();
            let call_id = owned_call_id.clone();

            Box::pin(async move {
                if let Some(candidate) = candidate {
                    let json = candidate.to_json().unwrap_or_default();
                    tracing::debug!(
                        "ICE candidate gathered for call {}: {}",
                        call_id,
                        json.candidate
                    );

                    let _ = event_tx.send(CallEvent::IceCandidateGathered {
                        call_id,
                        candidate: json.candidate,
                        sdp_mid: json.sdp_mid,
                        sdp_mline_index: json.sdp_mline_index,
                    });
                }
            })
        }));

        Ok(peer_connection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_manager(did: &str) -> CallManager {
        CallManager::new(
            did.to_string(),
            vec!["stun:stun.l.google.com:19302".to_string()],
        )
        .unwrap()
    }

    #[test]
    fn test_create_manager() {
        let manager = create_test_manager("did:variance:alice");

        assert_eq!(manager.local_did, "did:variance:alice");
        assert_eq!(manager.list_active_calls().len(), 0);
    }

    #[test]
    fn test_create_call() {
        let manager = create_test_manager("did:variance:alice");

        let call = manager.create_call("did:variance:bob".to_string(), CallType::Audio);

        assert!(!call.id.is_empty());
        assert_eq!(call.participants.len(), 2);
        assert_eq!(call.participants[0], "did:variance:alice");
        assert_eq!(call.participants[1], "did:variance:bob");
        assert_eq!(call.call_type, CallType::Audio);
        assert_eq!(call.status, CallStatus::Ringing);
        assert_eq!(manager.list_active_calls().len(), 1);
    }

    #[test]
    fn test_accept_call() {
        let manager = create_test_manager("did:variance:bob");

        let call = manager.register_incoming_call(
            "call123".to_string(),
            "did:variance:alice".to_string(),
            CallType::Video,
        );

        assert_eq!(call.status, CallStatus::Ringing);

        let accepted = manager.accept_call(&call.id).unwrap();
        assert_eq!(accepted.status, CallStatus::Connecting);
    }

    #[tokio::test]
    async fn test_reject_call() {
        let manager = create_test_manager("did:variance:bob");

        let call = manager.register_incoming_call(
            "call123".to_string(),
            "did:variance:alice".to_string(),
            CallType::Audio,
        );

        let rejected = manager.reject_call(&call.id).await.unwrap();
        assert_eq!(rejected.status, CallStatus::Ended);
        assert!(rejected.ended_at.is_some());
        assert_eq!(manager.list_active_calls().len(), 0);
    }

    #[test]
    fn test_activate_call() {
        let manager = create_test_manager("did:variance:alice");

        let call = manager.create_call("did:variance:bob".to_string(), CallType::Audio);

        manager.accept_call(&call.id).unwrap();

        let active = manager.activate_call(&call.id).unwrap();
        assert_eq!(active.status, CallStatus::Active);
    }

    #[tokio::test]
    async fn test_end_call() {
        let manager = create_test_manager("did:variance:alice");

        let call = manager.create_call("did:variance:bob".to_string(), CallType::Audio);

        manager.accept_call(&call.id).unwrap();
        manager.activate_call(&call.id).unwrap();

        let ended = manager.end_call(&call.id).await.unwrap();
        assert_eq!(ended.status, CallStatus::Ended);
        assert!(ended.ended_at.is_some());
        assert_eq!(manager.list_active_calls().len(), 0);
    }

    #[tokio::test]
    async fn test_fail_call() {
        let manager = create_test_manager("did:variance:alice");

        let call = manager.create_call("did:variance:bob".to_string(), CallType::Audio);

        let failed = manager.fail_call(&call.id).await.unwrap();
        assert_eq!(failed.status, CallStatus::Failed);
        assert!(failed.ended_at.is_some());
        assert_eq!(manager.list_active_calls().len(), 0);
    }

    #[test]
    fn test_get_call() {
        let manager = create_test_manager("did:variance:alice");

        let call = manager.create_call("did:variance:bob".to_string(), CallType::Audio);

        let retrieved = manager.get_call(&call.id);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, call.id);

        let not_found = manager.get_call("nonexistent");
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_invalid_state_transitions() {
        let manager = create_test_manager("did:variance:alice");

        let call = manager.create_call("did:variance:bob".to_string(), CallType::Audio);

        // Cannot activate a ringing call
        let result = manager.activate_call(&call.id);
        assert!(result.is_err());

        manager.accept_call(&call.id).unwrap();

        // Cannot accept a connecting call
        let result = manager.accept_call(&call.id);
        assert!(result.is_err());

        // Cannot reject a connecting call
        let result = manager.reject_call(&call.id).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_call_not_found() {
        let manager = create_test_manager("did:variance:alice");

        let result = manager.accept_call("nonexistent");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::CallNotFound { .. }));
    }

    #[test]
    fn test_get_remote_peer() {
        let manager = create_test_manager("did:variance:alice");

        let call = manager.create_call("did:variance:bob".to_string(), CallType::Audio);
        assert_eq!(
            manager.get_remote_peer(&call.id),
            Some("did:variance:bob".to_string())
        );

        assert_eq!(manager.get_remote_peer("nonexistent"), None);
    }

    #[test]
    fn test_subscribe_returns_receiver() {
        let manager = create_test_manager("did:variance:alice");
        let _rx = manager.subscribe();
    }

    #[test]
    fn test_call_to_proto() {
        let manager = create_test_manager("did:variance:alice");

        let call = manager.create_call("did:variance:bob".to_string(), CallType::Video);

        let proto = manager.call_to_proto(&call);

        assert_eq!(proto.call_id, call.id);
        assert_eq!(proto.participants, call.participants);
        assert_eq!(proto.call_type, CallType::Video as i32);
        assert_eq!(proto.status, CallStatus::Ringing as i32);
        assert_eq!(proto.started_at, call.started_at);
    }

    #[tokio::test]
    async fn test_webrtc_offer_answer_flow() {
        let manager = create_test_manager("did:variance:alice");

        // Create call
        let call = manager.create_call("did:variance:bob".to_string(), CallType::Audio);

        // Create offer
        let offer_sdp = manager.create_offer(&call.id).await.unwrap();
        assert!(!offer_sdp.is_empty());
        assert!(offer_sdp.contains("v=0")); // SDP version

        // Simulate receiving answer
        let answer_sdp = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n".to_string();

        // Note: This will fail because the answer is incomplete, but tests the flow
        let result = manager.handle_answer(&call.id, answer_sdp).await;
        // We expect this to fail with incomplete SDP, but structure is tested
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_webrtc_handle_offer() {
        let manager = create_test_manager("did:variance:bob");

        // Register incoming call
        let call = manager.register_incoming_call(
            "call456".to_string(),
            "did:variance:alice".to_string(),
            CallType::Video,
        );

        // Simulate receiving an offer (minimal valid SDP)
        let offer_sdp = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n\
                        a=group:BUNDLE 0\r\na=ice-options:trickle\r\n\
                        m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n\
                        c=IN IP4 0.0.0.0\r\na=ice-ufrag:test\r\na=ice-pwd:testpassword\r\n\
                        a=fingerprint:sha-256 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00\r\n\
                        a=setup:actpass\r\na=mid:0\r\na=sctp-port:5000\r\n".to_string();

        // Handle offer and create answer
        let answer_sdp = manager.handle_offer(&call.id, offer_sdp).await.unwrap();
        assert!(!answer_sdp.is_empty());
        assert!(answer_sdp.contains("v=0")); // SDP version
    }
}
