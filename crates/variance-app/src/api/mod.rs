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
}
