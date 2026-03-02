//! Shared helper functions used across multiple API handler modules.
//!
//! The two most important functions here eliminate code duplication that
//! previously existed across `start_conversation`, `send_direct_message`,
//! the WebSocket `SendDirectMessage` handler, and the reaction handlers:
//!
//! - [`ensure_olm_session`]: Resolve a peer's Olm keys via P2P and establish
//!   an outbound Double Ratchet session if one doesn't already exist.
//! - [`send_dm_to_peer`]: Encrypt, transmit over P2P, and queue for offline
//!   delivery if the peer isn't connected.

use crate::state::AppState;
use crate::Error;
use variance_media::Call;
use variance_proto::media_proto::{CallControlType, CallType};
use variance_proto::messaging_proto::ReceiptStatus;
use vodozemac::Curve25519PublicKey;

use super::types::CallResponse;

/// Deterministic conversation ID from two DIDs (sorted, colon-separated).
pub fn conversation_id(did1: &str, did2: &str) -> String {
    let mut dids = [did1, did2];
    dids.sort();
    format!("{}:{}", dids[0], dids[1])
}

/// Ensure an outbound Olm session exists with `recipient_did`.
///
/// If a session already exists, this is a no-op. Otherwise, resolves the
/// peer's identity via the P2P broadcast protocol, extracts their Olm
/// identity key and one-time pre-keys, and tries each OTK until one
/// succeeds (handling stale/consumed keys gracefully).
///
/// Skipped for self-messages (same DID).
pub async fn ensure_olm_session(state: &AppState, recipient_did: &str) -> Result<(), Error> {
    // Self-messages bypass Olm entirely
    if recipient_did == state.local_did {
        return Ok(());
    }

    // Already have a session — nothing to do
    if state.direct_messaging.has_session(recipient_did).await {
        return Ok(());
    }

    tracing::debug!(
        "No session exists with {}, auto-initializing via P2P...",
        recipient_did
    );

    // Ask connected peers for the recipient's Olm keys
    let found = state
        .node_handle
        .resolve_identity_by_did(recipient_did.to_string())
        .await
        .map_err(|e| Error::SessionRequired {
            message: format!(
                "Cannot reach peer via P2P. \
                 Make sure both nodes are running and connected. ({})",
                e
            ),
        })?;

    let ik_bytes: [u8; 32] =
        found
            .olm_identity_key
            .try_into()
            .map_err(|_| Error::SessionRequired {
                message: "Peer did not provide a valid Olm identity key".to_string(),
            })?;

    if found.one_time_keys.is_empty() {
        return Err(Error::SessionRequired {
            message: "Peer has no one-time pre-keys available".to_string(),
        });
    }

    let identity_key = Curve25519PublicKey::from_bytes(ik_bytes);

    // Try each OTK until one succeeds (handles stale/consumed keys)
    let mut last_error = None;
    for otk in found.one_time_keys {
        let otk_bytes: [u8; 32] = match otk.try_into() {
            Ok(b) => b,
            Err(_) => continue,
        };
        let one_time_key = Curve25519PublicKey::from_bytes(otk_bytes);

        match state
            .direct_messaging
            .init_session_if_needed(recipient_did, identity_key, one_time_key)
            .await
        {
            Ok(_) => {
                tracing::debug!("Session initialized successfully with {}", recipient_did);
                return Ok(());
            }
            Err(e) => {
                let err_msg = e.to_string();
                if err_msg.contains("unknown one-time key")
                    || err_msg.contains("BAD_MESSAGE_KEY_ID")
                {
                    tracing::debug!(
                        "OTK failed (likely already consumed), trying next: {}",
                        err_msg
                    );
                    last_error = Some(e);
                    continue;
                }
                // Non-OTK errors are fatal
                return Err(Error::App {
                    message: format!("Failed to initialize Olm session: {}", e),
                });
            }
        }
    }

    // Exhausted all OTKs without success
    if let Some(e) = last_error {
        if !state.direct_messaging.has_session(recipient_did).await {
            return Err(Error::SessionRequired {
                message: format!(
                    "Failed to establish session: all provided OTKs were invalid ({}). \
                     Peer may need to refresh their keys.",
                    e
                ),
            });
        }
    }

    Ok(())
}

/// Resolve an invitee identifier to a DID.
///
/// If `invitee` starts with `did:` it's returned as-is. Otherwise it's treated
/// as a username (with optional `#discriminator`) and resolved via the local
/// registry first, then DHT + P2P identity queries.
pub async fn resolve_invitee_did(state: &AppState, invitee: &str) -> Result<String, Error> {
    // Already a DID — use directly
    if invitee.starts_with("did:") {
        return Ok(invitee.to_string());
    }

    use variance_identity::username::UsernameRegistry;

    let (base_name, requested_disc) = match UsernameRegistry::parse_username(invitee) {
        Some((name, disc)) => (name, Some(disc)),
        None => (invitee.to_string(), None),
    };

    // 1. Check local registry
    if let Some(disc) = requested_disc {
        if let Some(did) = state.username_registry.lookup_exact(&base_name, disc) {
            return Ok(did);
        }
    } else {
        let matches = state.username_registry.lookup_all(&base_name);
        if matches.len() == 1 {
            return Ok(matches[0].1.clone());
        } else if matches.len() > 1 {
            return Err(Error::BadRequest {
                message: format!(
                    "Ambiguous username '{}' — {} users share this name. \
                     Add a discriminator, e.g. '{}#0001'.",
                    base_name,
                    matches.len(),
                    base_name,
                ),
            });
        }
    }

    // 2. Query DHT for username providers
    let providers = state
        .node_handle
        .find_username_providers(&base_name)
        .await
        .map_err(|e| Error::App {
            message: format!("DHT lookup failed for '{}': {}", base_name, e),
        })?;

    if providers.is_empty() {
        return Err(Error::NotFound {
            message: format!("No user found with username '{}'", invitee),
        });
    }

    // 3. Query each provider via P2P identity protocol
    for peer_id in &providers {
        let request = variance_proto::identity_proto::IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::PeerId(
                    peer_id.to_string(),
                ),
            ),
            timestamp: chrono::Utc::now().timestamp(),
            requester_did: Some(state.local_did.clone()),
        };

        match state
            .node_handle
            .send_identity_request(*peer_id, request)
            .await
        {
            Ok(response) => {
                if let Some(variance_proto::identity_proto::identity_response::Result::Found(
                    found,
                )) = response.result
                {
                    if let Some(ref doc) = found.did_document {
                        let disc = found.discriminator.unwrap_or(1);

                        // If a specific discriminator was requested, check match
                        if let Some(req_disc) = requested_disc {
                            if disc != req_disc {
                                continue;
                            }
                        }

                        // Cache for future lookups
                        let _ = state.username_registry.register_with_discriminator(
                            base_name.clone(),
                            disc,
                            doc.id.clone(),
                        );

                        return Ok(doc.id.clone());
                    }
                }
            }
            Err(e) => {
                tracing::debug!(
                    "Failed to query provider {} for '{}': {}",
                    peer_id,
                    base_name,
                    e
                );
            }
        }
    }

    Err(Error::NotFound {
        message: format!(
            "Found {} provider(s) for '{}' but none responded with a valid identity",
            providers.len(),
            invitee,
        ),
    })
}

/// Ensure an Olm session exists, optionally using caller-supplied keys.
///
/// This variant is used by `start_conversation` which allows tests and
/// manual callers to supply explicit Olm keys instead of auto-resolving.
/// If explicit keys are `None`, falls through to [`ensure_olm_session`].
pub async fn ensure_olm_session_with_keys(
    state: &AppState,
    recipient_did: &str,
    identity_key_hex: Option<&str>,
    one_time_key_hex: Option<&str>,
) -> Result<(), Error> {
    if recipient_did == state.local_did {
        return Ok(());
    }
    if state.direct_messaging.has_session(recipient_did).await {
        return Ok(());
    }

    if let (Some(ik_hex), Some(otk_hex)) = (identity_key_hex, one_time_key_hex) {
        let ik_bytes: [u8; 32] = hex::decode(ik_hex)
            .map_err(|_| Error::BadRequest {
                message: "recipient_identity_key must be hex-encoded".to_string(),
            })?
            .try_into()
            .map_err(|_| Error::BadRequest {
                message: "recipient_identity_key must be exactly 32 bytes".to_string(),
            })?;
        let otk_bytes: [u8; 32] = hex::decode(otk_hex)
            .map_err(|_| Error::BadRequest {
                message: "recipient_one_time_key must be hex-encoded".to_string(),
            })?
            .try_into()
            .map_err(|_| Error::BadRequest {
                message: "recipient_one_time_key must be exactly 32 bytes".to_string(),
            })?;

        state
            .direct_messaging
            .init_session_if_needed(
                recipient_did,
                Curve25519PublicKey::from_bytes(ik_bytes),
                Curve25519PublicKey::from_bytes(otk_bytes),
            )
            .await
            .map_err(|e| Error::App {
                message: format!("Failed to initialize Olm session: {}", e),
            })?;
        Ok(())
    } else {
        // No explicit keys — fall through to P2P auto-resolve
        ensure_olm_session(state, recipient_did).await
    }
}

/// Transmit a DM over P2P. If the peer is offline, queue for later delivery.
///
/// Also emits a `DirectMessageSent` event on the event channels so the
/// WebSocket bridge can notify the frontend.
///
/// Returns `"sent"` if delivered immediately, or `"pending"` if queued.
pub async fn send_dm_to_peer(
    state: &AppState,
    recipient_did: &str,
    message: &variance_proto::messaging_proto::DirectMessage,
) -> &'static str {
    match state
        .node_handle
        .send_direct_message(recipient_did.to_string(), message.clone())
        .await
    {
        Ok(_) => {
            tracing::debug!("P2P direct message delivered to {}", recipient_did);
            "sent"
        }
        Err(e) => {
            tracing::debug!(
                "Peer {} not reachable, queuing message for later delivery: {}",
                recipient_did,
                e
            );
            if let Err(queue_err) = state
                .direct_messaging
                .queue_pending_message(recipient_did, message.clone())
                .await
            {
                tracing::warn!(
                    "Failed to queue pending message for {}: {}",
                    recipient_did,
                    queue_err
                );
            }
            "pending"
        }
    }
}

/// Emit a `DirectMessageSent` event if event channels are available.
pub fn emit_dm_sent_event(state: &AppState, message_id: &str, recipient_did: &str) {
    if let Some(ref channels) = state.event_channels {
        channels.send_direct_message(variance_p2p::events::DirectMessageEvent::MessageSent {
            message_id: message_id.to_string(),
            recipient: recipient_did.to_string(),
        });
    }
}

// ===== Call helper functions =====

pub fn call_to_response(call: &Call) -> CallResponse {
    CallResponse {
        call_id: call.id.clone(),
        participants: call.participants.clone(),
        call_type: call_type_to_string(call.call_type),
        status: call_status_to_string(call.status),
        started_at: call.started_at,
        ended_at: call.ended_at,
    }
}

pub fn call_type_to_string(call_type: CallType) -> String {
    match call_type {
        CallType::Unspecified => "unspecified".to_string(),
        CallType::Audio => "audio".to_string(),
        CallType::Video => "video".to_string(),
        CallType::ScreenShare => "screen".to_string(),
    }
}

pub fn call_status_to_string(status: variance_proto::media_proto::CallStatus) -> String {
    match status {
        variance_proto::media_proto::CallStatus::Unspecified => "unspecified".to_string(),
        variance_proto::media_proto::CallStatus::Ringing => "ringing".to_string(),
        variance_proto::media_proto::CallStatus::Connecting => "connecting".to_string(),
        variance_proto::media_proto::CallStatus::Active => "active".to_string(),
        variance_proto::media_proto::CallStatus::Ended => "ended".to_string(),
        variance_proto::media_proto::CallStatus::Failed => "failed".to_string(),
    }
}

pub fn receipt_status_to_string(status: i32) -> String {
    if status == ReceiptStatus::Delivered as i32 {
        "delivered".to_string()
    } else if status == ReceiptStatus::Read as i32 {
        "read".to_string()
    } else {
        "unknown".to_string()
    }
}

/// Parse a string call type into the protobuf enum.
pub fn parse_call_type(s: &str) -> Result<CallType, Error> {
    match s {
        "audio" => Ok(CallType::Audio),
        "video" => Ok(CallType::Video),
        "screen" => Ok(CallType::ScreenShare),
        _ => Err(Error::BadRequest {
            message: format!("Invalid call type '{}'. Expected: audio, video, screen", s),
        }),
    }
}

/// Parse a string control type into the protobuf enum.
pub fn parse_control_type(s: &str) -> Result<CallControlType, Error> {
    match s {
        "ring" => Ok(CallControlType::Ring),
        "accept" => Ok(CallControlType::Accept),
        "reject" => Ok(CallControlType::Reject),
        "hangup" => Ok(CallControlType::Hangup),
        _ => Err(Error::App {
            message: format!("Invalid control type: {}", s),
        }),
    }
}
