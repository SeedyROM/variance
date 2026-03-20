//! HTTP API module — split into focused sub-modules.
//!
//! - [`types`]: Request/response structs shared across handlers.
//! - [`helpers`]: Olm session init, P2P send+queue, and other shared utilities.
//! - [`identity`]: DID identity and username handlers.
//! - [`conversations`]: Direct messages, conversations, and reactions.
//! - [`groups`]: MLS group management and group messages.
//! - [`calls`]: Call lifecycle and WebRTC signaling.
//! - [`social`]: Receipts, typing indicators, and presence.

pub mod helpers;
pub mod types;

mod calls;
mod config;
mod conversations;
mod groups;
mod identity;
mod invitations;
mod rate_limit;
mod social;

use crate::{state::AppState, Error};
use axum::http::{header, HeaderValue, Method};
use axum::{
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;

/// Create the HTTP API router
pub fn create_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list([
            HeaderValue::from_static("tauri://localhost"), // macOS/Windows Tauri 2.x
            HeaderValue::from_static("http://tauri.localhost"), // Linux Tauri 2.x
            HeaderValue::from_static("http://localhost"),  // dev / CLI
        ]))
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::PUT])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    Router::new()
        // Health check
        .route("/health", get(identity::health_check))
        // WebSocket endpoint
        .route("/ws", get(crate::websocket::websocket_handler))
        // Identity endpoints
        .route("/identity", get(identity::get_identity))
        .route("/identity/resolve/{did}", get(identity::resolve_identity))
        .route("/identity/username", post(identity::register_username))
        .route(
            "/identity/username/resolve/{username}",
            get(identity::resolve_username),
        )
        .route("/identity/passphrase", post(identity::change_passphrase))
        // Conversation endpoints
        .route("/conversations", get(conversations::list_conversations))
        .route("/conversations", post(conversations::start_conversation))
        .route(
            "/conversations/{peer_did}",
            axum::routing::delete(conversations::delete_conversation),
        )
        // Message endpoints — direct
        .route("/messages/direct", post(conversations::send_direct_message))
        .route(
            "/messages/direct/{did}",
            get(conversations::get_direct_messages),
        )
        .route(
            "/messages/direct/{message_id}/reactions",
            post(conversations::add_reaction),
        )
        .route(
            "/messages/direct/{message_id}/reactions/{emoji}",
            axum::routing::delete(conversations::remove_reaction),
        )
        // Message endpoints — group
        .route("/messages/group", post(groups::mls_send_group_message))
        .route("/messages/group/{id}", get(groups::get_group_messages))
        .route(
            "/messages/group/{message_id}/reactions",
            post(groups::add_group_reaction),
        )
        .route(
            "/messages/group/{message_id}/reactions/{emoji}",
            axum::routing::delete(groups::remove_group_reaction),
        )
        // Call endpoints
        .route("/calls/create", post(calls::create_call))
        .route("/calls/active", get(calls::list_active_calls))
        .route("/calls/{id}/accept", post(calls::accept_call))
        .route("/calls/{id}/reject", post(calls::reject_call))
        .route("/calls/{id}/end", post(calls::end_call))
        // Signaling endpoints
        .route("/signaling/offer", post(calls::send_offer))
        .route("/signaling/answer", post(calls::send_answer))
        .route("/signaling/ice", post(calls::send_ice_candidate))
        .route("/signaling/control", post(calls::send_control))
        // MLS group lifecycle (RFC 9420)
        .route(
            "/mls/groups",
            get(groups::mls_list_groups).post(groups::mls_create_group),
        )
        .route("/mls/groups/{id}/invite", post(groups::mls_invite_to_group))
        .route("/mls/groups/{id}/leave", post(groups::mls_leave_group))
        .route("/mls/groups/{id}/abandon", post(groups::mls_abandon_group))
        .route(
            "/mls/groups/{id}",
            axum::routing::delete(groups::mls_delete_group),
        )
        .route("/mls/groups/{id}/members", get(groups::mls_list_members))
        .route(
            "/mls/groups/{id}/members/{member_did}",
            axum::routing::delete(groups::mls_remove_member),
        )
        .route(
            "/mls/groups/{id}/members/{member_did}/role",
            axum::routing::put(groups::mls_change_member_role),
        )
        .route(
            "/mls/groups/{id}/reinitialize",
            post(groups::mls_reinitialize_group),
        )
        .route(
            "/mls/groups/{id}/invitations",
            get(groups::mls_list_outbound_invitations),
        )
        .route("/mls/welcome/accept", post(groups::mls_accept_welcome))
        // Group invitation endpoints
        .route("/invitations", get(invitations::list_invitations))
        .route(
            "/invitations/{group_id}/accept",
            post(invitations::accept_invitation),
        )
        .route(
            "/invitations/{group_id}/decline",
            post(invitations::decline_invitation),
        )
        // Receipt endpoints
        .route("/receipts/delivered", post(social::send_delivered_receipt))
        .route("/receipts/read", post(social::send_read_receipt))
        .route("/receipts/{message_id}", get(social::get_receipts))
        // Group receipt endpoints
        .route(
            "/groups/{group_id}/receipts/read",
            post(groups::send_group_read_receipts),
        )
        .route(
            "/groups/{group_id}/messages/{message_id}/receipts",
            get(groups::get_group_message_receipts),
        )
        // Typing endpoints
        .route("/typing/start", post(social::start_typing))
        .route("/typing/stop", post(social::stop_typing))
        .route("/typing/{recipient}", get(social::get_typing_users))
        // Presence endpoint
        .route("/presence", get(social::get_presence))
        // Config endpoints (relay management — changes take effect after restart)
        .route("/config/relays", get(config::get_relays))
        .route("/config/relays", post(config::add_relay))
        .route(
            "/config/relays/{peer_id}",
            axum::routing::delete(config::remove_relay),
        )
        // Retention config
        .route(
            "/config/retention",
            get(config::get_retention).put(config::set_retention),
        )
        .layer(cors)
        .layer(rate_limit::LocalRateLimitLayer::new(500, 10))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

// ===== Error Handling =====

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Error::BadRequest { message } => (StatusCode::BAD_REQUEST, message),
            Error::NotFound { message } => (StatusCode::NOT_FOUND, message),
            Error::Unauthorized { message } => (StatusCode::UNAUTHORIZED, message),
            Error::Forbidden { message } => (StatusCode::FORBIDDEN, message),
            Error::SessionRequired { message } => (StatusCode::UNPROCESSABLE_ENTITY, message),
            Error::App { message } => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };

        let body = Json(serde_json::json!({
            "error": message,
        }));

        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tempfile::tempdir;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap())
    }

    #[tokio::test]
    async fn test_health_check() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_create_call() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "recipient_did": "did:variance:bob",
            "call_type": "audio"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/calls/create")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_list_active_calls() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/calls/active")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_send_offer() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "call_id": "call123",
            "recipient_did": "did:variance:bob",
            "sdp": "v=0\r\no=- 1234 1234 IN IP4 0.0.0.0\r\n",
            "call_type": "video"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/signaling/offer")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_send_delivered_receipt() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "message_id": "msg123",
            "sender_did": "did:variance:bob"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/receipts/delivered")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_start_typing() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "recipient": "did:variance:bob",
            "is_group": false
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/typing/start")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_get_typing_users() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/typing/did:variance:bob")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_invalid_call_type() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "recipient_did": "did:variance:bob",
            "call_type": "invalid"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/calls/create")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_get_identity() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/identity")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["did"], "did:variance:test");
        assert!(json["olm_identity_key"].as_str().is_some());
        assert!(json["one_time_keys"].as_array().is_some());
    }

    #[tokio::test]
    async fn test_list_conversations_empty() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/conversations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_list_conversations_with_messages() {
        use variance_messaging::storage::MessageStorage;
        use variance_proto::messaging_proto::{DirectMessage, MessageType};

        let state = test_state();

        let msg = DirectMessage {
            id: "test-msg-001".to_string(),
            sender_did: "did:variance:test".to_string(),
            recipient_did: "did:variance:peer".to_string(),
            ciphertext: vec![],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 9999,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };
        state.storage.store_direct(&msg).await.unwrap();

        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/conversations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["peer_did"], "did:variance:peer");
        assert_eq!(arr[0]["last_message_timestamp"], 9999);
        assert_eq!(arr[0]["has_unread"], false);
    }

    #[tokio::test]
    async fn test_has_unread_before_and_after_fetch() {
        use variance_messaging::storage::MessageStorage;
        use variance_proto::messaging_proto::{DirectMessage, MessageType};

        let state = test_state();

        let msg = DirectMessage {
            id: "test-unread-001".to_string(),
            sender_did: "did:variance:peer".to_string(),
            recipient_did: "did:variance:test".to_string(),
            ciphertext: vec![],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 5000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };
        state.storage.store_direct(&msg).await.unwrap();

        let app = create_router(state);

        // Before opening the conversation: has_unread should be true
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/conversations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["has_unread"], true);

        // Open the conversation — GET /messages/direct/:did marks it read
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/messages/direct/did:variance:peer")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // After opening: has_unread should be false
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/conversations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["has_unread"], false);
    }

    #[tokio::test]
    async fn test_delete_conversation() {
        use variance_messaging::storage::MessageStorage;
        use variance_proto::messaging_proto::{DirectMessage, MessageType};

        let state = test_state();

        let msg = DirectMessage {
            id: "test-msg-002".to_string(),
            sender_did: "did:variance:test".to_string(),
            recipient_did: "did:variance:deleteme".to_string(),
            ciphertext: vec![],
            olm_message_type: 0,
            signature: vec![],
            timestamp: 5000,
            r#type: MessageType::Text.into(),
            reply_to: None,
            sender_identity_key: None,
        };
        state.storage.store_direct(&msg).await.unwrap();

        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/conversations/did:variance:deleteme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_start_conversation() {
        let app = create_router(test_state());

        let mut recipient_account = vodozemac::olm::Account::new();
        recipient_account.generate_one_time_keys(1);
        let identity_key_hex = hex::encode(recipient_account.curve25519_key().to_bytes());
        let otk_hex = hex::encode(
            recipient_account
                .one_time_keys()
                .values()
                .next()
                .unwrap()
                .to_bytes(),
        );

        let req_body = serde_json::json!({
            "recipient_did": "did:variance:bob",
            "text": "Hello!",
            "recipient_identity_key": identity_key_hex,
            "recipient_one_time_key": otk_hex,
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/conversations")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["conversation_id"].as_str().is_some());
        assert!(json["message_id"].as_str().is_some());
    }

    #[tokio::test]
    async fn test_register_username() {
        let app = create_router(test_state());
        let req_body = serde_json::json!({ "username": "alice" });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/identity/username")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["username"], "alice");
    }

    #[tokio::test]
    async fn test_register_invalid_username() {
        let app = create_router(test_state());
        let req_body = serde_json::json!({ "username": "alice@bad" });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/identity/username")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_start_conversation_without_key_fails() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "recipient_did": "did:variance:bob",
            "text": "Hello!",
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/conversations")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn test_get_identity_with_username() {
        let state = test_state();
        state
            .username_registry
            .register_local("alice".to_string(), "did:variance:test".to_string())
            .unwrap();

        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/identity")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["did"], "did:variance:test");
        assert_eq!(json["username"], "alice");
        assert!(json["discriminator"].as_u64().is_some());
        assert!(json["display_name"].as_str().unwrap().starts_with("alice#"));
    }

    #[tokio::test]
    async fn test_rate_limit_returns_429() {
        use axum::routing::get as get_route;

        // Minimal router with a low limit (3 req / 60s) to trigger 429 quickly
        let app = Router::new()
            .route("/ping", get_route(|| async { "pong" }))
            .layer(rate_limit::LocalRateLimitLayer::new(3, 60));

        // First 3 requests should succeed
        for _ in 0..3 {
            let resp = app
                .clone()
                .oneshot(Request::builder().uri("/ping").body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        // 4th request should be rate-limited
        let resp = app
            .oneshot(Request::builder().uri("/ping").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "Rate limit exceeded");
    }

    // ===== Helper function tests =====

    #[test]
    fn test_conversation_id_sorted() {
        let id = helpers::conversation_id("did:variance:bob", "did:variance:alice");
        assert_eq!(id, "did:variance:alice:did:variance:bob");
    }

    #[test]
    fn test_conversation_id_deterministic() {
        let id1 = helpers::conversation_id("did:variance:alice", "did:variance:bob");
        let id2 = helpers::conversation_id("did:variance:bob", "did:variance:alice");
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_conversation_id_same_did() {
        let id = helpers::conversation_id("did:variance:alice", "did:variance:alice");
        assert_eq!(id, "did:variance:alice:did:variance:alice");
    }

    #[test]
    fn test_parse_call_type_audio() {
        let ct = helpers::parse_call_type("audio").unwrap();
        assert_eq!(ct, variance_proto::media_proto::CallType::Audio);
    }

    #[test]
    fn test_parse_call_type_video() {
        let ct = helpers::parse_call_type("video").unwrap();
        assert_eq!(ct, variance_proto::media_proto::CallType::Video);
    }

    #[test]
    fn test_parse_call_type_screen() {
        let ct = helpers::parse_call_type("screen").unwrap();
        assert_eq!(ct, variance_proto::media_proto::CallType::ScreenShare);
    }

    #[test]
    fn test_parse_call_type_invalid() {
        let result = helpers::parse_call_type("hologram");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_control_type_valid() {
        assert_eq!(
            helpers::parse_control_type("ring").unwrap(),
            variance_proto::media_proto::CallControlType::Ring
        );
        assert_eq!(
            helpers::parse_control_type("accept").unwrap(),
            variance_proto::media_proto::CallControlType::Accept
        );
        assert_eq!(
            helpers::parse_control_type("reject").unwrap(),
            variance_proto::media_proto::CallControlType::Reject
        );
        assert_eq!(
            helpers::parse_control_type("hangup").unwrap(),
            variance_proto::media_proto::CallControlType::Hangup
        );
    }

    #[test]
    fn test_parse_control_type_invalid() {
        assert!(helpers::parse_control_type("mute").is_err());
    }

    #[test]
    fn test_call_type_to_string() {
        assert_eq!(
            helpers::call_type_to_string(variance_proto::media_proto::CallType::Audio),
            "audio"
        );
        assert_eq!(
            helpers::call_type_to_string(variance_proto::media_proto::CallType::Video),
            "video"
        );
        assert_eq!(
            helpers::call_type_to_string(variance_proto::media_proto::CallType::ScreenShare),
            "screen"
        );
        assert_eq!(
            helpers::call_type_to_string(variance_proto::media_proto::CallType::Unspecified),
            "unspecified"
        );
    }

    #[test]
    fn test_call_status_to_string() {
        use variance_proto::media_proto::CallStatus;
        assert_eq!(
            helpers::call_status_to_string(CallStatus::Ringing),
            "ringing"
        );
        assert_eq!(helpers::call_status_to_string(CallStatus::Active), "active");
        assert_eq!(helpers::call_status_to_string(CallStatus::Ended), "ended");
        assert_eq!(helpers::call_status_to_string(CallStatus::Failed), "failed");
        assert_eq!(
            helpers::call_status_to_string(CallStatus::Connecting),
            "connecting"
        );
        assert_eq!(
            helpers::call_status_to_string(CallStatus::Unspecified),
            "unspecified"
        );
    }

    #[test]
    fn test_receipt_status_to_string() {
        use variance_proto::messaging_proto::ReceiptStatus;
        assert_eq!(
            helpers::receipt_status_to_string(ReceiptStatus::Delivered as i32),
            "delivered"
        );
        assert_eq!(
            helpers::receipt_status_to_string(ReceiptStatus::Read as i32),
            "read"
        );
        assert_eq!(helpers::receipt_status_to_string(999), "unknown");
    }

    // ===== Error response mapping =====

    #[tokio::test]
    async fn test_error_bad_request() {
        let err = crate::Error::BadRequest {
            message: "test error".to_string(),
        };
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_error_not_found() {
        let err = crate::Error::NotFound {
            message: "not found".to_string(),
        };
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_error_unauthorized() {
        let err = crate::Error::Unauthorized {
            message: "unauthorized".to_string(),
        };
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_error_forbidden() {
        let err = crate::Error::Forbidden {
            message: "forbidden".to_string(),
        };
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_error_session_required() {
        let err = crate::Error::SessionRequired {
            message: "no session".to_string(),
        };
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn test_error_app() {
        let err = crate::Error::App {
            message: "internal".to_string(),
        };
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_error_body_contains_message() {
        let err = crate::Error::BadRequest {
            message: "field X is required".to_string(),
        };
        let response = err.into_response();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "field X is required");
    }

    // ===== Call lifecycle tests =====

    #[tokio::test]
    async fn test_accept_call_nonexistent() {
        let app = create_router(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/calls/nonexistent-call/accept")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_reject_call_nonexistent() {
        let app = create_router(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/calls/nonexistent-call/reject")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_end_call_nonexistent() {
        let app = create_router(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/calls/nonexistent-call/end")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_create_and_accept_call() {
        let state = test_state();
        let app = create_router(state);

        // Create a call
        let req_body = serde_json::json!({
            "recipient_did": "did:variance:bob",
            "call_type": "audio"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/calls/create")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let call_id = json["call_id"].as_str().unwrap().to_string();
        assert_eq!(json["status"], "ringing");

        // Accept the call
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/calls/{}/accept", call_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "connecting");
    }

    #[tokio::test]
    async fn test_create_and_end_call() {
        let state = test_state();
        let app = create_router(state);

        let req_body = serde_json::json!({
            "recipient_did": "did:variance:bob",
            "call_type": "video"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/calls/create")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let call_id = json["call_id"].as_str().unwrap().to_string();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/calls/{}/end", call_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ended");
    }

    // ===== Signaling tests =====

    #[tokio::test]
    async fn test_send_answer() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "call_id": "call123",
            "recipient_did": "did:variance:bob",
            "sdp": "v=0\r\n"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/signaling/answer")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_send_ice_candidate() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "call_id": "call123",
            "recipient_did": "did:variance:bob",
            "candidate": "candidate:1 1 UDP 2130706431 10.0.0.1 9 typ host",
            "sdp_mid": "0",
            "sdp_m_line_index": 0
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/signaling/ice")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_send_control() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "call_id": "call123",
            "recipient_did": "did:variance:bob",
            "control_type": "ring",
            "reason": null
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/signaling/control")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_send_control_invalid_type() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "call_id": "call123",
            "recipient_did": "did:variance:bob",
            "control_type": "mute"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/signaling/control")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    // ===== Social handler tests =====

    #[tokio::test]
    async fn test_send_read_receipt() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "message_id": "msg123",
            "sender_did": "did:variance:bob"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/receipts/read")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["message_id"], "msg123");
        assert_eq!(json["status"], "read");
    }

    #[tokio::test]
    async fn test_get_receipts_empty() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/receipts/msg-nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_get_receipts_after_delivered() {
        let state = test_state();
        let app = create_router(state);

        // Send a delivered receipt first
        let req_body = serde_json::json!({
            "message_id": "msg-rcpt-001",
            "sender_did": "did:variance:bob"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/receipts/delivered")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Now get receipts for that message
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/receipts/msg-rcpt-001")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["message_id"], "msg-rcpt-001");
        assert_eq!(arr[0]["status"], "delivered");
    }

    #[tokio::test]
    async fn test_stop_typing() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "recipient": "did:variance:bob",
            "is_group": false
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/typing/stop")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_stop_typing_group() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "recipient": "group-123",
            "is_group": true
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/typing/stop")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_start_typing_group() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "recipient": "group-123",
            "is_group": true
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/typing/start")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_get_typing_users_group_prefix() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/typing/group:some-group-id")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["users"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_get_presence() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/presence")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Mock node handle returns empty list
        assert!(json["online"].as_array().unwrap().is_empty());
    }

    // ===== Direct message handler tests =====

    #[tokio::test]
    async fn test_get_direct_messages_empty() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/messages/direct/did:variance:bob")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_get_direct_messages_with_pagination() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/messages/direct/did:variance:bob?before=9999999&limit=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_start_conversation_empty_did() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "recipient_did": "",
            "text": "Hello!",
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/conversations")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_start_conversation_invalid_did() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "recipient_did": "not-a-did",
            "text": "Hello!",
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/conversations")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_start_conversation_empty_text() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "recipient_did": "did:variance:bob",
            "text": "   ",
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/conversations")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_start_conversation_text_too_long() {
        let app = create_router(test_state());

        let long_text = "x".repeat(4097);
        let req_body = serde_json::json!({
            "recipient_did": "did:variance:bob",
            "text": long_text,
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/conversations")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_delete_nonexistent_conversation() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/conversations/did:variance:nobody")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // delete_conversation succeeds even if no messages exist (idempotent)
        assert_eq!(response.status(), StatusCode::OK);
    }

    // ===== MLS group handler tests =====

    #[tokio::test]
    async fn test_mls_list_groups_empty() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/mls/groups")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_mls_create_group() {
        let state = test_state();
        let app = create_router(state);

        let req_body = serde_json::json!({
            "name": "Test Group"
        });

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mls/groups")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["success"], true);
        assert!(json["group_id"].as_str().is_some());
        assert_eq!(json["name"], "Test Group");
        assert_eq!(json["mls"], true);

        // After creating a group, list should show it
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/mls/groups")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 1);
        assert_eq!(json[0]["name"], "Test Group");
        assert_eq!(json[0]["your_role"], "admin");
    }

    #[tokio::test]
    async fn test_mls_create_group_empty_name() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "name": "   "
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mls/groups")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_mls_create_group_name_too_long() {
        let app = create_router(test_state());

        let req_body = serde_json::json!({
            "name": "x".repeat(101)
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mls/groups")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_mls_list_members_nonexistent_group() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/mls/groups/nonexistent/members")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_mls_create_and_list_members() {
        let state = test_state();
        let app = create_router(state);

        // Create a group
        let req_body = serde_json::json!({ "name": "Member Test" });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mls/groups")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let group_id = json["group_id"].as_str().unwrap().to_string();

        // List members
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/mls/groups/{}/members", group_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let members = json.as_array().unwrap();
        // Creator is the sole member
        assert_eq!(members.len(), 1);
        assert_eq!(members[0]["did"], "did:variance:test");
        assert_eq!(members[0]["role"], "admin");
    }

    #[tokio::test]
    async fn test_get_group_messages_empty() {
        let state = test_state();
        let app = create_router(state.clone());

        // Create a group first so the endpoint doesn't fail on missing group
        state.mls_groups.create_group("test-group").unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/messages/group/test-group")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().is_empty());
    }

    // ===== Invitation handler tests =====

    #[tokio::test]
    async fn test_list_invitations_empty() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/invitations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_accept_invitation_nonexistent() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/invitations/nonexistent-group/accept")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_decline_invitation_nonexistent() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/invitations/nonexistent-group/decline")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ===== Config handler tests =====

    #[tokio::test]
    async fn test_get_relays_default() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let mut state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());
        // Point config_dir to the temp dir so load_or_default creates a default config
        state.config_dir = dir.path().to_path_buf();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/config/relays")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_add_and_get_relay() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let mut state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());
        state.config_dir = dir.path().to_path_buf();
        let app = create_router(state);

        // Add a relay
        let req_body = serde_json::json!({
            "peer_id": "12D3KooWTest",
            "multiaddr": "/ip4/1.2.3.4/tcp/4001"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/config/relays")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Verify relay was added
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/config/relays")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["peer_id"], "12D3KooWTest");
        assert_eq!(arr[0]["multiaddr"], "/ip4/1.2.3.4/tcp/4001");
    }

    #[tokio::test]
    async fn test_remove_relay() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let mut state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());
        state.config_dir = dir.path().to_path_buf();
        let app = create_router(state);

        // Add a relay first
        let req_body = serde_json::json!({
            "peer_id": "12D3KooWRemoveMe",
            "multiaddr": "/ip4/5.6.7.8/tcp/4001"
        });
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/config/relays")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Remove it
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/config/relays/12D3KooWRemoveMe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Verify it's gone
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/config/relays")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_get_retention_default() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let mut state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());
        state.config_dir = dir.path().to_path_buf();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/config/retention")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["group_message_max_age_days"], 30);
    }

    #[tokio::test]
    async fn test_set_and_get_retention() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let mut state =
            AppState::with_db_path("did:variance:test".to_string(), db_path.to_str().unwrap());
        state.config_dir = dir.path().to_path_buf();
        let app = create_router(state);

        // Set retention to 90 days
        let req_body = serde_json::json!({
            "group_message_max_age_days": 90
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/config/retention")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Verify it was persisted
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/config/retention")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["group_message_max_age_days"], 90);
    }

    // ===== Identity handler tests =====

    #[tokio::test]
    async fn test_resolve_identity_self() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/identity/resolve/did:variance:test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["did"], "did:variance:test");
        assert_eq!(json["resolved"], true);
    }

    #[tokio::test]
    async fn test_resolve_identity_unknown() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/identity/resolve/did:variance:unknown")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["resolved"], false);
    }

    #[tokio::test]
    async fn test_resolve_username_not_found() {
        let app = create_router(test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/identity/username/resolve/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Mock node handle returns empty providers list, so NotFound
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_resolve_username_local_hit() {
        let state = test_state();
        state
            .username_registry
            .register_local("alice".to_string(), "did:variance:alice-local".to_string())
            .unwrap();
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/identity/username/resolve/alice")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["did"], "did:variance:alice-local");
        assert_eq!(json["username"], "alice");
        assert!(json["discriminator"].as_u64().is_some());
    }

    // ===== MLS group send message validation =====

    #[tokio::test]
    async fn test_mls_send_group_message_empty_text() {
        let state = test_state();
        state.mls_groups.create_group("test-group").unwrap();
        let app = create_router(state);

        let req_body = serde_json::json!({
            "group_id": "test-group",
            "text": "   "
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/messages/group")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_mls_send_group_message_too_long() {
        let state = test_state();
        state.mls_groups.create_group("test-group").unwrap();
        let app = create_router(state);

        let req_body = serde_json::json!({
            "group_id": "test-group",
            "text": "x".repeat(4097)
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/messages/group")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // ===== MLS outbound invitations (requires admin role) =====

    #[tokio::test]
    async fn test_mls_list_outbound_invitations() {
        let state = test_state();
        let app = create_router(state.clone());

        // Create a group so the local user is admin
        let req_body = serde_json::json!({ "name": "Invite Test" });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mls/groups")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let group_id = json["group_id"].as_str().unwrap().to_string();

        // List outbound invitations (should be empty)
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/mls/groups/{}/invitations", group_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_mls_list_outbound_invitations_non_admin() {
        let app = create_router(test_state());

        // Try to list invitations for a group we're not admin of (no metadata stored)
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/mls/groups/nonexistent-group/invitations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // member_role_from_metadata returns "member" when no metadata exists, so Forbidden
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    // ===== MLS group message send + retrieve roundtrip =====

    #[tokio::test]
    async fn test_mls_send_and_get_group_messages() {
        let state = test_state();
        let app = create_router(state.clone());

        // Create a group
        let req_body = serde_json::json!({ "name": "Msg Test" });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mls/groups")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let group_id = json["group_id"].as_str().unwrap().to_string();

        // Send a message
        let req_body = serde_json::json!({
            "group_id": group_id,
            "text": "Hello group!"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/messages/group")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&req_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["success"], true);
        assert!(json["message_id"].as_str().is_some());

        // Retrieve messages
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/messages/group/{}", group_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let msgs = json.as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["text"], "Hello group!");
        assert_eq!(msgs[0]["sender_did"], "did:variance:test");
    }
}
