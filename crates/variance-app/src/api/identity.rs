//! Identity HTTP handlers: local identity, resolve, username registration.

use crate::{state::AppState, Error, Result};
use axum::{
    extract::{Path, State},
    response::Json,
};

use super::types::{IdentityStatusResponse, RegisterUsernameRequest};

// ===== Health Check =====

pub(super) async fn health_check() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "variance-app"
    }))
}

// ===== Identity Handlers =====

pub(super) async fn get_identity(State(state): State<AppState>) -> Json<IdentityStatusResponse> {
    let olm_identity_key = hex::encode(state.direct_messaging.identity_key().to_bytes());
    let one_time_keys = state
        .direct_messaging
        .one_time_keys()
        .await
        .values()
        .map(|k| hex::encode(k.to_bytes()))
        .collect();

    let (username, discriminator, display_name) =
        match state.username_registry.get_username(&state.local_did) {
            Some((name, disc)) => (
                Some(name.clone()),
                Some(disc),
                Some(variance_identity::username::UsernameRegistry::format_username(&name, disc)),
            ),
            None => (None, None, None),
        };

    Json(IdentityStatusResponse {
        did: state.local_did.clone(),
        verifying_key: state.verifying_key.clone(),
        created_at: state.created_at.clone(),
        olm_identity_key,
        one_time_keys,
        username,
        discriminator,
        display_name,
    })
}

/// Resolve a DID to its identity document.
///
/// Full resolution requires the peer to be reachable via P2P. Currently returns
/// the DID as-is since DHT-to-PeerId lookup is not yet wired to the API layer.
pub(super) async fn resolve_identity(
    State(state): State<AppState>,
    Path(did): Path<String>,
) -> Result<Json<serde_json::Value>> {
    // Self-resolution: return our own identity without network round-trip
    if did == state.local_did {
        return Ok(Json(serde_json::json!({
            "did": did,
            "verifying_key": state.verifying_key,
            "created_at": state.created_at,
            "resolved": true,
        })));
    }

    // Remote resolution requires mapping DID → PeerId which needs DHT integration.
    Ok(Json(serde_json::json!({
        "did": did,
        "resolved": false,
    })))
}

pub(super) async fn register_username(
    State(state): State<AppState>,
    Json(req): Json<RegisterUsernameRequest>,
) -> Result<Json<serde_json::Value>> {
    variance_identity::username::UsernameRegistry::validate_username(&req.username).map_err(
        |e| Error::BadRequest {
            message: format!("Invalid username: {}", e),
        },
    )?;

    // Register locally with auto-assigned discriminator
    let (display_name, discriminator) = state
        .username_registry
        .register_local(req.username.clone(), state.local_did.clone())
        .map_err(|e| Error::App {
            message: format!("Failed to register username: {}", e),
        })?;

    // Persist username + discriminator to the identity file so it survives restarts
    if let Err(e) = persist_username_to_identity(&state.identity_path, &req.username, discriminator)
    {
        tracing::warn!("Failed to persist username to identity file: {}", e);
    }

    // Publish to DHT so other peers can find us
    state
        .node_handle
        .provide_username(&req.username)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to publish username to DHT: {}", e),
        })?;

    // Tell the P2P identity handler so responses to remote peers include our discriminator
    if let Err(e) = state
        .node_handle
        .set_local_username(req.username.clone(), discriminator)
        .await
    {
        tracing::warn!("Failed to update P2P handler with new username: {}", e);
    }

    // Notify connected peers of the rename so they update their cached display names
    if let Err(e) = state
        .node_handle
        .broadcast_username_change(state.local_did.clone(), req.username.clone(), discriminator)
        .await
    {
        tracing::warn!("Failed to broadcast username change: {}", e);
    }

    Ok(Json(serde_json::json!({
        "username": req.username,
        "discriminator": discriminator,
        "display_name": display_name,
        "did": state.local_did,
    })))
}

/// Write username + discriminator into the identity JSON file.
fn persist_username_to_identity(
    identity_path: &std::path::Path,
    username: &str,
    discriminator: u32,
) -> anyhow::Result<()> {
    let mut identity = AppState::load_identity(identity_path)?;
    identity.username = Some(username.to_string());
    identity.discriminator = Some(discriminator);
    let json = serde_json::to_string_pretty(&identity)
        .map_err(|e| anyhow::anyhow!("Failed to serialize identity: {}", e))?;
    std::fs::write(identity_path, json)
        .map_err(|e| anyhow::anyhow!("Failed to write identity file: {}", e))?;
    Ok(())
}

/// Resolve a username (with or without discriminator) to a DID.
///
/// Accepts formats: `name%230001` (URL-encoded `name#0001`) or just `name`.
/// Checks local registry first, then queries DHT for providers and asks them
/// for their identity via the P2P identity protocol.
pub(super) async fn resolve_username(
    State(state): State<AppState>,
    Path(username): Path<String>,
) -> Result<Json<serde_json::Value>> {
    use variance_identity::username::UsernameRegistry;

    // Extract base name (strip discriminator for DHT lookup)
    let (base_name, requested_disc) = match UsernameRegistry::parse_username(&username) {
        Some((name, disc)) => (name, Some(disc)),
        None => (username.clone(), None),
    };

    // --- 1. Check local registry first ---
    if let Some(disc) = requested_disc {
        if let Some(did) = state.username_registry.lookup_exact(&base_name, disc) {
            return Ok(Json(serde_json::json!({
                "did": did,
                "username": base_name,
                "discriminator": disc,
                "display_name": UsernameRegistry::format_username(&base_name, disc),
            })));
        }
    } else {
        let local_matches = state.username_registry.lookup_all(&base_name);
        if local_matches.len() == 1 {
            let (disc, did) = &local_matches[0];
            return Ok(Json(serde_json::json!({
                "did": did,
                "username": base_name,
                "discriminator": disc,
                "display_name": UsernameRegistry::format_username(&base_name, *disc),
            })));
        } else if local_matches.len() > 1 {
            let results: Vec<serde_json::Value> = local_matches
                .iter()
                .map(|(disc, did)| {
                    serde_json::json!({
                        "did": did,
                        "username": base_name,
                        "discriminator": disc,
                        "display_name": UsernameRegistry::format_username(&base_name, *disc),
                    })
                })
                .collect();
            return Ok(Json(serde_json::json!({ "matches": results })));
        }
    }

    // --- 2. Not found locally — query DHT for username providers ---
    let providers = state
        .node_handle
        .find_username_providers(&base_name)
        .await
        .map_err(|e| Error::App {
            message: format!("DHT lookup failed: {}", e),
        })?;

    if providers.is_empty() {
        return Err(Error::NotFound {
            message: format!("No user found with username '{}'", username),
        });
    }

    // --- 3. Query each provider for their identity via P2P protocol ---
    for peer_id in &providers {
        tracing::debug!(
            "Querying DHT provider {} for username '{}'",
            peer_id,
            base_name
        );

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
                        let did = doc.id.clone();

                        // Cache in the local registry so future lookups are instant.
                        let disc = found.discriminator.unwrap_or(1);
                        let _ = state.username_registry.register_with_discriminator(
                            base_name.clone(),
                            disc,
                            did.clone(),
                        );

                        // If a specific discriminator was requested, check it matches
                        if let Some(req_disc) = requested_disc {
                            if disc != req_disc {
                                continue; // Try next provider
                            }
                        }

                        return Ok(Json(serde_json::json!({
                            "did": did,
                            "username": base_name,
                            "discriminator": disc,
                            "display_name": UsernameRegistry::format_username(&base_name, disc),
                        })));
                    }
                }
            }
            Err(e) => {
                tracing::debug!(
                    "Failed to query provider {} for username '{}': {}",
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
            username
        ),
    })
}
