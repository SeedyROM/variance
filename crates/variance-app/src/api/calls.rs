//! Call management and WebRTC signaling HTTP handlers.

use crate::{state::AppState, Error, Result};
use axum::{
    extract::{Path, State},
    response::Json,
};

use super::helpers::{call_to_response, parse_call_type, parse_control_type};
use super::types::{
    CallResponse, SendAnswerRequest, SendControlRequest, SendIceCandidateRequest, SendOfferRequest,
    SignalingResponse,
};

// ===== Call Handlers =====

pub(super) async fn create_call(
    State(state): State<AppState>,
    Json(req): Json<super::types::CreateCallRequest>,
) -> Result<Json<CallResponse>> {
    let call_type = parse_call_type(&req.call_type)?;
    let call = state.calls.create_call(req.recipient_did, call_type);
    Ok(Json(call_to_response(&call)))
}

pub(super) async fn list_active_calls(State(state): State<AppState>) -> Json<Vec<CallResponse>> {
    let calls = state.calls.list_active_calls();
    Json(calls.iter().map(call_to_response).collect())
}

pub(super) async fn accept_call(
    State(state): State<AppState>,
    Path(call_id): Path<String>,
) -> Result<Json<CallResponse>> {
    let call = state.calls.accept_call(&call_id).map_err(|e| Error::App {
        message: e.to_string(),
    })?;
    Ok(Json(call_to_response(&call)))
}

pub(super) async fn reject_call(
    State(state): State<AppState>,
    Path(call_id): Path<String>,
) -> Result<Json<CallResponse>> {
    let call = state
        .calls
        .reject_call(&call_id)
        .await
        .map_err(|e| Error::App {
            message: e.to_string(),
        })?;
    Ok(Json(call_to_response(&call)))
}

pub(super) async fn end_call(
    State(state): State<AppState>,
    Path(call_id): Path<String>,
) -> Result<Json<CallResponse>> {
    let call = state
        .calls
        .end_call(&call_id)
        .await
        .map_err(|e| Error::App {
            message: e.to_string(),
        })?;
    Ok(Json(call_to_response(&call)))
}

// ===== Signaling Handlers =====

pub(super) async fn send_offer(
    State(state): State<AppState>,
    Json(req): Json<SendOfferRequest>,
) -> Result<Json<SignalingResponse>> {
    let call_type = parse_call_type(&req.call_type)?;

    let message = state
        .signaling
        .send_offer(
            req.call_id.clone(),
            req.recipient_did.clone(),
            req.sdp,
            call_type,
        )
        .map_err(|e| Error::App {
            message: e.to_string(),
        })?;

    state
        .node_handle
        .send_signaling_message(req.recipient_did, message)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to send signaling message: {}", e),
        })?;

    Ok(Json(SignalingResponse {
        success: true,
        message: "Offer sent successfully".to_string(),
    }))
}

pub(super) async fn send_answer(
    State(state): State<AppState>,
    Json(req): Json<SendAnswerRequest>,
) -> Result<Json<SignalingResponse>> {
    let message = state
        .signaling
        .send_answer(req.call_id.clone(), req.recipient_did.clone(), req.sdp)
        .map_err(|e| Error::App {
            message: e.to_string(),
        })?;

    state
        .node_handle
        .send_signaling_message(req.recipient_did, message)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to send signaling message: {}", e),
        })?;

    Ok(Json(SignalingResponse {
        success: true,
        message: "Answer sent successfully".to_string(),
    }))
}

pub(super) async fn send_ice_candidate(
    State(state): State<AppState>,
    Json(req): Json<SendIceCandidateRequest>,
) -> Result<Json<SignalingResponse>> {
    let message = state
        .signaling
        .send_ice_candidate(
            req.call_id.clone(),
            req.recipient_did.clone(),
            req.candidate,
            req.sdp_mid,
            req.sdp_m_line_index,
        )
        .map_err(|e| Error::App {
            message: e.to_string(),
        })?;

    state
        .node_handle
        .send_signaling_message(req.recipient_did, message)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to send signaling message: {}", e),
        })?;

    Ok(Json(SignalingResponse {
        success: true,
        message: "ICE candidate sent successfully".to_string(),
    }))
}

pub(super) async fn send_control(
    State(state): State<AppState>,
    Json(req): Json<SendControlRequest>,
) -> Result<Json<SignalingResponse>> {
    let control_type = parse_control_type(&req.control_type)?;

    let message = state
        .signaling
        .send_control(
            req.call_id.clone(),
            req.recipient_did.clone(),
            control_type,
            req.reason,
        )
        .map_err(|e| Error::App {
            message: e.to_string(),
        })?;

    state
        .node_handle
        .send_signaling_message(req.recipient_did, message)
        .await
        .map_err(|e| Error::App {
            message: format!("Failed to send signaling message: {}", e),
        })?;

    Ok(Json(SignalingResponse {
        success: true,
        message: "Control message sent successfully".to_string(),
    }))
}
