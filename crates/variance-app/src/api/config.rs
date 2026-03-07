use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::{config::RelayPeerConfig, state::AppState, Error, Result};

#[derive(Debug, Serialize, Deserialize)]
pub struct AddRelayRequest {
    pub peer_id: String,
    pub multiaddr: String,
}

pub async fn get_relays(State(state): State<AppState>) -> Result<Json<Vec<RelayPeerConfig>>> {
    let base_dir = state
        .config_path
        .parent()
        .unwrap_or(&state.config_path)
        .to_path_buf();
    let config = crate::config::AppConfig::load_or_default(&base_dir);
    Ok(Json(config.p2p.relay_peers))
}

pub async fn add_relay(
    State(state): State<AppState>,
    Json(body): Json<AddRelayRequest>,
) -> Result<Json<serde_json::Value>> {
    let base_dir = state
        .config_path
        .parent()
        .unwrap_or(&state.config_path)
        .to_path_buf();
    let mut config = crate::config::AppConfig::load_or_default(&base_dir);
    config.p2p.relay_peers.push(RelayPeerConfig {
        peer_id: body.peer_id,
        multiaddr: body.multiaddr,
    });
    config.save(&base_dir).map_err(|e| Error::App {
        message: e.to_string(),
    })?;
    Ok(Json(serde_json::json!({ "success": true })))
}

pub async fn remove_relay(
    State(state): State<AppState>,
    Path(peer_id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let base_dir = state
        .config_path
        .parent()
        .unwrap_or(&state.config_path)
        .to_path_buf();
    let mut config = crate::config::AppConfig::load_or_default(&base_dir);
    config.p2p.relay_peers.retain(|r| r.peer_id != peer_id);
    config.save(&base_dir).map_err(|e| Error::App {
        message: e.to_string(),
    })?;
    Ok(Json(serde_json::json!({ "success": true })))
}
