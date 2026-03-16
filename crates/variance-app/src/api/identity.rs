//! Identity HTTP handlers: local identity, resolve, username registration.

use crate::{state::AppState, Error, Result};
use axum::{
    extract::{Path, State},
    response::Json,
};

use super::types::{ChangePassphraseRequest, IdentityStatusResponse, RegisterUsernameRequest};

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
/// Tries P2P resolution first (not yet wired), then falls back to IPFS/IPNS
/// via the `ipfs_storage` backend.
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

    // Try IPFS/IPNS fallback — resolve the IPNS name, fetch the DID document.
    let did_hex = hex::encode(did.as_bytes());
    let key_name = format!("variance-{}", &did_hex[..16.min(did_hex.len())]);
    if let Ok(Some(cid)) = state.ipfs_storage.resolve(&key_name).await {
        if let Ok(Some(did_doc)) = state.ipfs_storage.fetch(&cid).await {
            let _ = state.identity_cache.insert(&did, did_doc.clone());
            return Ok(Json(serde_json::json!({
                "did": did,
                "display_name": did_doc.document.display_name,
                "resolved": true,
                "source": "ipfs",
            })));
        }
    }

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
    if let Err(e) = persist_username_to_identity(&state, &req.username, discriminator) {
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
        .broadcast_username_change(
            state.local_did.clone(),
            req.username.clone(),
            discriminator,
            state.signing_key.to_bytes().to_vec(),
        )
        .await
    {
        tracing::warn!("Failed to broadcast username change: {}", e);
    }

    // Update IPFS record with the new display name (best-effort).
    {
        use variance_identity::did::{Did, DidDocument};

        let did_str = &state.local_did;
        let now = chrono::Utc::now().timestamp();
        let did_doc = Did {
            id: did_str.clone(),
            document: DidDocument {
                id: did_str.clone(),
                authentication: vec![],
                key_agreement: vec![],
                service: vec![],
                created_at: now,
                updated_at: now,
                display_name: Some(display_name.clone()),
                avatar_cid: None,
                bio: None,
            },
            signing_key: None,
            x25519_secret: None,
            document_signature: None,
        };
        if let Ok(cid) = state.ipfs_storage.store(&did_doc).await {
            let did_hex = hex::encode(did_str.as_bytes());
            let key_name = format!("variance-{}", &did_hex[..16.min(did_hex.len())]);
            if let Err(e) = state.ipfs_storage.publish(&key_name, &cid).await {
                tracing::warn!("Failed to republish updated DID to IPFS: {}", e);
            } else {
                tracing::info!("Republished DID with username {} to IPFS", req.username);
            }
        }
    }

    Ok(Json(serde_json::json!({
        "username": req.username,
        "discriminator": discriminator,
        "display_name": display_name,
        "did": state.local_did,
    })))
}

/// Re-encrypt the identity file with a new passphrase (or remove encryption).
///
/// Validates the current passphrase matches the one the node was started with,
/// then rewrites the file. The caller should restart the node for the in-memory
/// `AppState.identity_passphrase` to reflect the new value.
pub(super) async fn change_passphrase(
    State(state): State<AppState>,
    Json(req): Json<ChangePassphraseRequest>,
) -> Result<Json<serde_json::Value>> {
    // Verify the supplied current passphrase matches the one used at startup.
    // This prevents an attacker with local access from silently changing the passphrase.
    let stored = state.identity_passphrase.as_ref().map(|s| s.as_str());
    if req.current_passphrase.as_deref() != stored {
        return Err(Error::BadRequest {
            message: "Current passphrase is incorrect".to_string(),
        });
    }

    // Load with current, save with new — this is the re-encryption.
    let identity =
        AppState::load_identity_with_passphrase(&state.identity_path, stored).map_err(|e| {
            Error::App {
                message: format!("Failed to load identity: {}", e),
            }
        })?;
    AppState::save_identity(
        &state.identity_path,
        &identity,
        req.new_passphrase.as_deref(),
    )
    .map_err(|e| Error::App {
        message: format!("Failed to save identity with new passphrase: {}", e),
    })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Passphrase changed. Restart the app for the change to take full effect."
    })))
}

/// Write username + discriminator into the identity JSON file.
fn persist_username_to_identity(
    state: &AppState,
    username: &str,
    discriminator: u32,
) -> anyhow::Result<()> {
    let passphrase = state.identity_passphrase.as_ref().map(|s| s.as_str());
    let mut identity = AppState::load_identity_with_passphrase(&state.identity_path, passphrase)?;
    identity.username = Some(username.to_string());
    identity.discriminator = Some(discriminator);
    AppState::save_identity(&state.identity_path, &identity, passphrase)
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
