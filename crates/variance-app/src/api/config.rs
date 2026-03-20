use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{config::RelayPeerConfig, state::AppState, Error, Result};

#[derive(Debug, Serialize, Deserialize)]
pub struct RetentionConfig {
    pub group_message_max_age_days: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AddRelayRequest {
    pub peer_id: String,
    pub multiaddr: String,
}

pub async fn get_relays(State(state): State<AppState>) -> Result<Json<Vec<RelayPeerConfig>>> {
    let config = crate::config::AppConfig::load_or_default(&state.config_dir);
    Ok(Json(config.p2p.relay_peers))
}

pub async fn add_relay(
    State(state): State<AppState>,
    Json(body): Json<AddRelayRequest>,
) -> Result<Json<serde_json::Value>> {
    let mut config = crate::config::AppConfig::load_or_default(&state.config_dir);
    config.p2p.relay_peers.push(RelayPeerConfig {
        peer_id: body.peer_id,
        multiaddr: body.multiaddr,
    });
    config.save(&state.config_dir).map_err(|e| Error::App {
        message: e.to_string(),
    })?;
    Ok(Json(serde_json::json!({ "success": true })))
}

pub async fn get_retention(State(state): State<AppState>) -> Result<Json<RetentionConfig>> {
    let config = crate::config::AppConfig::load_or_default(&state.config_dir);
    Ok(Json(RetentionConfig {
        group_message_max_age_days: config.storage.group_message_max_age_days,
    }))
}

pub async fn set_retention(
    State(state): State<AppState>,
    Json(body): Json<RetentionConfig>,
) -> Result<Json<Value>> {
    let mut config = crate::config::AppConfig::load_or_default(&state.config_dir);
    config.storage.group_message_max_age_days = body.group_message_max_age_days;
    config.save(&state.config_dir).map_err(|e| Error::App {
        message: e.to_string(),
    })?;
    Ok(Json(serde_json::json!({ "success": true })))
}

pub async fn remove_relay(
    State(state): State<AppState>,
    Path(peer_id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let mut config = crate::config::AppConfig::load_or_default(&state.config_dir);
    config.p2p.relay_peers.retain(|r| r.peer_id != peer_id);
    config.save(&state.config_dir).map_err(|e| Error::App {
        message: e.to_string(),
    })?;
    Ok(Json(serde_json::json!({ "success": true })))
}
