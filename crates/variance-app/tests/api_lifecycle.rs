//! Integration tests exercising multi-step API workflows through the full axum
//! router stack. Each test shares one `AppState` and router across sequential
//! HTTP requests, verifying that side-effects from earlier requests are visible
//! in later ones.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tempfile::tempdir;
use tower::ServiceExt;

use variance_app::{create_router, AppState};

// ===== Helpers =====

fn fresh_state(did: &str) -> AppState {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut state = AppState::with_db_path(did.to_string(), db_path.to_str().unwrap());
    state.config_path = dir.path().join("config.toml");
    std::mem::forget(dir);
    state
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

fn post_json(uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(body).unwrap()))
        .unwrap()
}

fn delete(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

fn put_json(uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(body).unwrap()))
        .unwrap()
}

// ===== Tests =====

#[tokio::test]
async fn test_username_registration_reflected_in_identity() {
    let app = create_router(fresh_state("did:variance:alice"));

    let resp = app
        .clone()
        .oneshot(post_json(
            "/identity/username",
            &serde_json::json!({ "username": "alice" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["username"], "alice");
    let disc = json["discriminator"].as_u64().unwrap();
    assert!(disc > 0);

    let resp = app.oneshot(get("/identity")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["did"], "did:variance:alice");
    assert_eq!(json["username"], "alice");
    assert_eq!(json["discriminator"], disc);
    let display = json["display_name"].as_str().unwrap();
    assert!(display.starts_with("alice#"), "got {display}");
}

#[tokio::test]
async fn test_conversation_start_and_message_lifecycle() {
    let app = create_router(fresh_state("did:variance:alice"));

    let mut recipient = vodozemac::olm::Account::new();
    recipient.generate_one_time_keys(1);
    let ik = hex::encode(recipient.curve25519_key().to_bytes());
    let otk = hex::encode(
        recipient
            .one_time_keys()
            .values()
            .next()
            .unwrap()
            .to_bytes(),
    );

    // Start conversation
    let resp = app
        .clone()
        .oneshot(post_json(
            "/conversations",
            &serde_json::json!({
                "recipient_did": "did:variance:bob",
                "text": "Hello Bob!",
                "recipient_identity_key": ik,
                "recipient_one_time_key": otk,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(json["conversation_id"].as_str().is_some());
    assert!(json["message_id"].as_str().is_some());

    // List conversations
    let resp = app.clone().oneshot(get("/conversations")).await.unwrap();
    let json = body_json(resp).await;
    let convos = json.as_array().unwrap();
    assert_eq!(convos.len(), 1);
    assert_eq!(convos[0]["peer_did"], "did:variance:bob");
    assert_eq!(convos[0]["has_unread"], false);

    // Fetch messages
    let resp = app
        .clone()
        .oneshot(get("/messages/direct/did:variance:bob"))
        .await
        .unwrap();
    let json = body_json(resp).await;
    let msgs = json.as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["text"], "Hello Bob!");

    // Follow-up message
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/direct",
            &serde_json::json!({
                "recipient_did": "did:variance:bob",
                "text": "Follow up",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Should have 2 messages now
    let resp = app
        .clone()
        .oneshot(get("/messages/direct/did:variance:bob"))
        .await
        .unwrap();
    let json = body_json(resp).await;
    assert_eq!(json.as_array().unwrap().len(), 2);

    // Delete conversation
    let resp = app
        .clone()
        .oneshot(delete("/conversations/did:variance:bob"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Should be empty
    let resp = app.oneshot(get("/conversations")).await.unwrap();
    let json = body_json(resp).await;
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_unread_tracking_with_inbound_message() {
    use variance_messaging::storage::MessageStorage;
    use variance_proto::messaging_proto::{DirectMessage, MessageType};

    let state = fresh_state("did:variance:alice");
    let msg = DirectMessage {
        id: "inbound-001".to_string(),
        sender_did: "did:variance:carol".to_string(),
        recipient_did: "did:variance:alice".to_string(),
        ciphertext: vec![],
        olm_message_type: 0,
        signature: vec![],
        timestamp: 10_000,
        r#type: MessageType::Text.into(),
        reply_to: None,
        sender_identity_key: None,
    };
    state.storage.store_direct(&msg).await.unwrap();
    let app = create_router(state);

    // has_unread = true
    let resp = app.clone().oneshot(get("/conversations")).await.unwrap();
    let json = body_json(resp).await;
    assert_eq!(json.as_array().unwrap()[0]["has_unread"], true);

    // Open conversation marks read
    app.clone()
        .oneshot(get("/messages/direct/did:variance:carol"))
        .await
        .unwrap();

    // has_unread = false
    let resp = app.oneshot(get("/conversations")).await.unwrap();
    let json = body_json(resp).await;
    assert_eq!(json.as_array().unwrap()[0]["has_unread"], false);
}

#[tokio::test]
async fn test_group_create_message_and_delete() {
    let app = create_router(fresh_state("did:variance:alice"));

    // Create group
    let resp = app
        .clone()
        .oneshot(post_json(
            "/mls/groups",
            &serde_json::json!({ "name": "Dev Chat" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let gid = json["group_id"].as_str().unwrap().to_string();

    // List groups
    let resp = app.clone().oneshot(get("/mls/groups")).await.unwrap();
    let json = body_json(resp).await;
    assert_eq!(json.as_array().unwrap().len(), 1);
    assert_eq!(json[0]["your_role"], "admin");

    // Members
    let resp = app
        .clone()
        .oneshot(get(&format!("/mls/groups/{gid}/members")))
        .await
        .unwrap();
    let json = body_json(resp).await;
    assert_eq!(json.as_array().unwrap().len(), 1);
    assert_eq!(json[0]["role"], "admin");

    // Send group message
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/group",
            &serde_json::json!({ "group_id": gid, "text": "Hello team!" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Fetch group messages
    let resp = app
        .clone()
        .oneshot(get(&format!("/messages/group/{gid}")))
        .await
        .unwrap();
    let json = body_json(resp).await;
    let msgs = json.as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["text"], "Hello team!");

    // Delete group
    let resp = app
        .clone()
        .oneshot(delete(&format!("/mls/groups/{gid}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Empty
    let resp = app.oneshot(get("/mls/groups")).await.unwrap();
    let json = body_json(resp).await;
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_call_lifecycle_create_accept_end() {
    let app = create_router(fresh_state("did:variance:alice"));

    // Create
    let resp = app
        .clone()
        .oneshot(post_json(
            "/calls/create",
            &serde_json::json!({ "recipient_did": "did:variance:bob", "call_type": "audio" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let cid = json["call_id"].as_str().unwrap().to_string();
    assert_eq!(json["status"], "ringing");

    // List active
    let resp = app.clone().oneshot(get("/calls/active")).await.unwrap();
    let json = body_json(resp).await;
    assert_eq!(json.as_array().unwrap().len(), 1);

    // Accept
    let resp = app
        .clone()
        .oneshot(post_json(
            &format!("/calls/{cid}/accept"),
            &serde_json::json!({}),
        ))
        .await
        .unwrap();
    let json = body_json(resp).await;
    assert_eq!(json["status"], "connecting");

    // End
    let resp = app
        .clone()
        .oneshot(post_json(
            &format!("/calls/{cid}/end"),
            &serde_json::json!({}),
        ))
        .await
        .unwrap();
    let json = body_json(resp).await;
    assert_eq!(json["status"], "ended");

    // Active empty
    let resp = app.oneshot(get("/calls/active")).await.unwrap();
    let json = body_json(resp).await;
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_call_lifecycle_create_reject() {
    let app = create_router(fresh_state("did:variance:alice"));

    let resp = app
        .clone()
        .oneshot(post_json(
            "/calls/create",
            &serde_json::json!({ "recipient_did": "did:variance:carol", "call_type": "video" }),
        ))
        .await
        .unwrap();
    let json = body_json(resp).await;
    let cid = json["call_id"].as_str().unwrap().to_string();

    let resp = app
        .clone()
        .oneshot(post_json(
            &format!("/calls/{cid}/reject"),
            &serde_json::json!({}),
        ))
        .await
        .unwrap();
    let json = body_json(resp).await;
    assert_eq!(json["status"], "ended");

    let resp = app.oneshot(get("/calls/active")).await.unwrap();
    let json = body_json(resp).await;
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_config_relay_and_retention_lifecycle() {
    let app = create_router(fresh_state("did:variance:alice"));

    // Relays start empty
    let resp = app.clone().oneshot(get("/config/relays")).await.unwrap();
    let json = body_json(resp).await;
    assert!(json.as_array().unwrap().is_empty());

    // Add two relays
    for (id, addr) in [
        ("12D3KooWRelay1", "/ip4/1.2.3.4/tcp/4001"),
        ("12D3KooWRelay2", "/ip4/5.6.7.8/tcp/4001"),
    ] {
        let resp = app
            .clone()
            .oneshot(post_json(
                "/config/relays",
                &serde_json::json!({ "peer_id": id, "multiaddr": addr }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // List = 2
    let resp = app.clone().oneshot(get("/config/relays")).await.unwrap();
    let json = body_json(resp).await;
    assert_eq!(json.as_array().unwrap().len(), 2);

    // Remove first
    let resp = app
        .clone()
        .oneshot(delete("/config/relays/12D3KooWRelay1"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // List = 1
    let resp = app.clone().oneshot(get("/config/relays")).await.unwrap();
    let json = body_json(resp).await;
    assert_eq!(json.as_array().unwrap().len(), 1);
    assert_eq!(json[0]["peer_id"], "12D3KooWRelay2");

    // Retention: get default
    let resp = app.clone().oneshot(get("/config/retention")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let default_json = body_json(resp).await;
    let default_days = default_json["group_message_max_age_days"].as_u64().unwrap();

    // Set to 90
    let resp = app
        .clone()
        .oneshot(put_json(
            "/config/retention",
            &serde_json::json!({ "group_message_max_age_days": 90 }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify 90
    let resp = app.clone().oneshot(get("/config/retention")).await.unwrap();
    let json = body_json(resp).await;
    assert_eq!(json["group_message_max_age_days"], 90);

    // Set back to default
    let resp = app
        .clone()
        .oneshot(put_json(
            "/config/retention",
            &serde_json::json!({ "group_message_max_age_days": default_days }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify back to default
    let resp = app.oneshot(get("/config/retention")).await.unwrap();
    let json = body_json(resp).await;
    assert_eq!(json["group_message_max_age_days"], default_days);
}

#[tokio::test]
async fn test_receipt_lifecycle() {
    let app = create_router(fresh_state("did:variance:alice"));

    // Delivered
    let resp = app
        .clone()
        .oneshot(post_json(
            "/receipts/delivered",
            &serde_json::json!({ "message_id": "msg-001", "sender_did": "did:variance:bob" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Read
    let resp = app
        .clone()
        .oneshot(post_json(
            "/receipts/read",
            &serde_json::json!({ "message_id": "msg-001", "sender_did": "did:variance:bob" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Query — read overwrites delivered for the same reader, so 1 entry
    let resp = app.oneshot(get("/receipts/msg-001")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(!json.as_array().unwrap().is_empty());
}
