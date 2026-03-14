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

/// Direct messages with reply_to should store and return the referenced message ID.
#[tokio::test]
async fn test_direct_message_reply_to() {
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

    // Start conversation (first message)
    let resp = app
        .clone()
        .oneshot(post_json(
            "/conversations",
            &serde_json::json!({
                "recipient_did": "did:variance:bob",
                "text": "Original message",
                "recipient_identity_key": ik,
                "recipient_one_time_key": otk,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let first_msg_id = json["message_id"].as_str().unwrap().to_string();

    // Reply to the first message
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/direct",
            &serde_json::json!({
                "recipient_did": "did:variance:bob",
                "text": "This is a reply",
                "reply_to": first_msg_id,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Fetch messages — reply should have reply_to set
    let resp = app
        .oneshot(get("/messages/direct/did:variance:bob"))
        .await
        .unwrap();
    let json = body_json(resp).await;
    let msgs = json.as_array().unwrap();
    assert_eq!(msgs.len(), 2);

    // Find original and reply by text (order is non-deterministic when timestamps match)
    let original = msgs
        .iter()
        .find(|m| m["text"] == "Original message")
        .expect("Original message should exist");
    let reply = msgs
        .iter()
        .find(|m| m["text"] == "This is a reply")
        .expect("Reply message should exist");

    // Original message has no reply_to
    assert!(
        original["reply_to"].is_null(),
        "Original message should not have reply_to, but got: {:?}",
        original["reply_to"]
    );
    // Reply references the original
    assert_eq!(reply["reply_to"], first_msg_id);
}

/// Group messages with reply_to should store and return the referenced message ID.
#[tokio::test]
async fn test_group_message_reply_to() {
    let app = create_router(fresh_state("did:variance:alice"));

    // Create group
    let resp = app
        .clone()
        .oneshot(post_json(
            "/mls/groups",
            &serde_json::json!({ "name": "Reply Test" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let gid = json["group_id"].as_str().unwrap().to_string();

    // Send original message
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/group",
            &serde_json::json!({ "group_id": gid, "text": "Original group message" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let first_msg_id = json["message_id"].as_str().unwrap().to_string();

    // Send reply
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/group",
            &serde_json::json!({
                "group_id": gid,
                "text": "Replying to original",
                "reply_to": first_msg_id,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Fetch group messages
    let resp = app
        .oneshot(get(&format!("/messages/group/{gid}")))
        .await
        .unwrap();
    let json = body_json(resp).await;
    let msgs = json.as_array().unwrap();
    assert_eq!(msgs.len(), 2);
    assert!(msgs[0]["reply_to"].is_null());
    assert_eq!(msgs[1]["reply_to"], first_msg_id);
}

/// Direct message reactions: add and remove through the API.
#[tokio::test]
async fn test_direct_message_reactions() {
    let app = create_router(fresh_state("did:variance:alice"));

    // Establish Olm session via start_conversation
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

    let resp = app
        .clone()
        .oneshot(post_json(
            "/conversations",
            &serde_json::json!({
                "recipient_did": "did:variance:bob",
                "text": "Hi Bob",
                "recipient_identity_key": ik,
                "recipient_one_time_key": otk,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let msg_id = json["message_id"].as_str().unwrap().to_string();

    // Add reaction
    let resp = app
        .clone()
        .oneshot(post_json(
            &format!("/messages/direct/{msg_id}/reactions"),
            &serde_json::json!({
                "emoji": "👍",
                "recipient_did": "did:variance:bob",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(json["success"].as_bool().unwrap());

    // Remove reaction
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/messages/direct/{msg_id}/reactions/👍?recipient_did=did:variance:bob"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(json["success"].as_bool().unwrap());
}

/// Group message reactions: add and remove through the API.
#[tokio::test]
async fn test_group_message_reactions() {
    let app = create_router(fresh_state("did:variance:alice"));

    // Create group
    let resp = app
        .clone()
        .oneshot(post_json(
            "/mls/groups",
            &serde_json::json!({ "name": "Reactions Group" }),
        ))
        .await
        .unwrap();
    let json = body_json(resp).await;
    let gid = json["group_id"].as_str().unwrap().to_string();

    // Send a message to react to
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/group",
            &serde_json::json!({ "group_id": gid, "text": "React to me" }),
        ))
        .await
        .unwrap();
    let json = body_json(resp).await;
    let msg_id = json["message_id"].as_str().unwrap().to_string();

    // Add reaction
    let resp = app
        .clone()
        .oneshot(post_json(
            &format!("/messages/group/{msg_id}/reactions"),
            &serde_json::json!({
                "group_id": gid,
                "emoji": "🎉",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(json["success"].as_bool().unwrap());

    // Remove reaction
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/messages/group/{msg_id}/reactions/🎉"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({ "group_id": gid })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(json["success"].as_bool().unwrap());
}

/// Message validation: empty, whitespace-only, and too-long text are rejected.
#[tokio::test]
async fn test_message_validation() {
    let app = create_router(fresh_state("did:variance:alice"));

    // Set up Olm session for DM tests
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

    // Start a conversation so we have a session
    let resp = app
        .clone()
        .oneshot(post_json(
            "/conversations",
            &serde_json::json!({
                "recipient_did": "did:variance:bob",
                "text": "setup",
                "recipient_identity_key": ik,
                "recipient_one_time_key": otk,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Create a group for group message tests
    let resp = app
        .clone()
        .oneshot(post_json(
            "/mls/groups",
            &serde_json::json!({ "name": "Validation Test" }),
        ))
        .await
        .unwrap();
    let gid = body_json(resp).await["group_id"]
        .as_str()
        .unwrap()
        .to_string();

    // === Empty text (DM) ===
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/direct",
            &serde_json::json!({
                "recipient_did": "did:variance:bob",
                "text": "",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // === Whitespace-only text (DM) ===
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/direct",
            &serde_json::json!({
                "recipient_did": "did:variance:bob",
                "text": "   \n\t  ",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // === Too-long text (DM) — 4097 characters ===
    let long_text = "a".repeat(4097);
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/direct",
            &serde_json::json!({
                "recipient_did": "did:variance:bob",
                "text": long_text,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // === Exactly 4096 characters should succeed ===
    let max_text = "b".repeat(4096);
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/direct",
            &serde_json::json!({
                "recipient_did": "did:variance:bob",
                "text": max_text,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // === Empty text (group) ===
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/group",
            &serde_json::json!({ "group_id": gid, "text": "" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // === Whitespace-only text (group) ===
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/group",
            &serde_json::json!({ "group_id": gid, "text": "  \t\n " }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // === Too-long text (group) ===
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/group",
            &serde_json::json!({ "group_id": gid, "text": "c".repeat(4097) }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // === Empty text on start_conversation ===
    let resp = app
        .clone()
        .oneshot(post_json(
            "/conversations",
            &serde_json::json!({
                "recipient_did": "did:variance:carol",
                "text": "",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // === Reaction emoji validation: empty ===
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/direct/some-msg-id/reactions",
            &serde_json::json!({
                "emoji": "",
                "recipient_did": "did:variance:bob",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // === Reaction emoji validation: too long (9+ chars) ===
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/direct/some-msg-id/reactions",
            &serde_json::json!({
                "emoji": "abcdefghi",
                "recipient_did": "did:variance:bob",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // === Group reaction emoji validation: empty ===
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/group/some-msg-id/reactions",
            &serde_json::json!({
                "group_id": gid,
                "emoji": "",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Group name validation: empty, whitespace-only, and too-long names.
#[tokio::test]
async fn test_group_name_validation() {
    let app = create_router(fresh_state("did:variance:alice"));

    // Empty name
    let resp = app
        .clone()
        .oneshot(post_json("/mls/groups", &serde_json::json!({ "name": "" })))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Whitespace-only name
    let resp = app
        .clone()
        .oneshot(post_json(
            "/mls/groups",
            &serde_json::json!({ "name": "   \t  " }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Too-long name (101 chars)
    let resp = app
        .clone()
        .oneshot(post_json(
            "/mls/groups",
            &serde_json::json!({ "name": "x".repeat(101) }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Exactly 100 chars should succeed
    let resp = app
        .clone()
        .oneshot(post_json(
            "/mls/groups",
            &serde_json::json!({ "name": "y".repeat(100) }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// DID validation on start_conversation.
#[tokio::test]
async fn test_did_validation_on_start_conversation() {
    let app = create_router(fresh_state("did:variance:alice"));

    // Empty DID
    let resp = app
        .clone()
        .oneshot(post_json(
            "/conversations",
            &serde_json::json!({ "recipient_did": "", "text": "Hello" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // DID not starting with "did:"
    let resp = app
        .clone()
        .oneshot(post_json(
            "/conversations",
            &serde_json::json!({ "recipient_did": "not-a-did", "text": "Hello" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Group leave clears all local state: messages, metadata, group list.
#[tokio::test]
async fn test_group_leave_lifecycle() {
    let app = create_router(fresh_state("did:variance:alice"));

    // Create group
    let resp = app
        .clone()
        .oneshot(post_json(
            "/mls/groups",
            &serde_json::json!({ "name": "Leave Test" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let gid = body_json(resp).await["group_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Send a message
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/group",
            &serde_json::json!({ "group_id": gid, "text": "Before leaving" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify group exists in list
    let resp = app.clone().oneshot(get("/mls/groups")).await.unwrap();
    let json = body_json(resp).await;
    assert_eq!(json.as_array().unwrap().len(), 1);

    // Leave group
    let resp = app
        .clone()
        .oneshot(post_json(
            &format!("/mls/groups/{gid}/leave"),
            &serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(json["success"].as_bool().unwrap());

    // Group list should be empty
    let resp = app.clone().oneshot(get("/mls/groups")).await.unwrap();
    let json = body_json(resp).await;
    assert!(json.as_array().unwrap().is_empty());

    // Group messages should be empty
    let resp = app
        .oneshot(get(&format!("/messages/group/{gid}")))
        .await
        .unwrap();
    let json = body_json(resp).await;
    assert!(json.as_array().unwrap().is_empty());
}

/// Group unread tracking: has_unread is set by inbound messages and cleared
/// by opening the group messages endpoint.
#[tokio::test]
async fn test_group_unread_tracking() {
    use variance_messaging::storage::MessageStorage;

    let state = fresh_state("did:variance:alice");

    // Create group via MLS handler + metadata
    state
        .mls_groups
        .create_group("unread-group")
        .expect("create MLS group");
    let group = variance_proto::messaging_proto::Group {
        id: "unread-group".to_string(),
        name: "Unread Test".to_string(),
        admin_did: "did:variance:alice".to_string(),
        members: vec![variance_proto::messaging_proto::GroupMember {
            did: "did:variance:alice".to_string(),
            role: variance_proto::messaging_proto::GroupRole::Admin as i32,
            joined_at: 1000,
            nickname: None,
        }],
        ..Default::default()
    };
    state
        .storage
        .store_group_metadata(&group)
        .await
        .expect("store metadata");

    // Store a group message (simulating inbound) with a past timestamp.
    // The last_read_at starts at 0 so any message will be "unread".
    let msg = variance_proto::messaging_proto::GroupMessage {
        id: "grp-msg-001".to_string(),
        sender_did: "did:variance:bob".to_string(),
        group_id: "unread-group".to_string(),
        timestamp: 50_000,
        r#type: 0,
        reply_to: None,
        mls_ciphertext: vec![],
    };
    state.storage.store_group(&msg).await.expect("store msg");

    let app = create_router(state);

    // Group list should show has_unread = true
    let resp = app.clone().oneshot(get("/mls/groups")).await.unwrap();
    let json = body_json(resp).await;
    let groups = json.as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["has_unread"], true);

    // Open group messages — marks as read
    let resp = app
        .clone()
        .oneshot(get("/messages/group/unread-group"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Now has_unread should be false
    let resp = app.oneshot(get("/mls/groups")).await.unwrap();
    let json = body_json(resp).await;
    let groups = json.as_array().unwrap();
    assert_eq!(groups[0]["has_unread"], false);
}

/// Typing indicators: start, stop, and get.
#[tokio::test]
async fn test_typing_indicators() {
    let app = create_router(fresh_state("did:variance:alice"));

    // Start typing (direct)
    let resp = app
        .clone()
        .oneshot(post_json(
            "/typing/start",
            &serde_json::json!({ "recipient": "did:variance:bob", "is_group": false }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(json["success"].as_bool().unwrap());

    // Stop typing (direct)
    let resp = app
        .clone()
        .oneshot(post_json(
            "/typing/stop",
            &serde_json::json!({ "recipient": "did:variance:bob", "is_group": false }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(json["success"].as_bool().unwrap());

    // Get typing users (no one is typing from our perspective)
    let resp = app
        .clone()
        .oneshot(get("/typing/did:variance:bob"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(json["users"].as_array().unwrap().is_empty());

    // Start typing in group
    let resp = app
        .clone()
        .oneshot(post_json(
            "/typing/start",
            &serde_json::json!({ "recipient": "some-group-id", "is_group": true }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Get typing users for group
    let resp = app
        .oneshot(get("/typing/group:some-group-id"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(json["users"].as_array().unwrap().is_empty());
}

/// Invitation list and decline: seed a pending invitation, list it, decline it.
#[tokio::test]
async fn test_invitation_list_and_decline() {
    use variance_messaging::storage::MessageStorage;

    let state = fresh_state("did:variance:alice");

    // Seed a pending invitation directly into storage
    let invitation = variance_proto::messaging_proto::GroupInvitation {
        group_id: "invite-group-1".to_string(),
        group_name: "Invited Group".to_string(),
        inviter_did: "did:variance:bob".to_string(),
        invitee_did: "did:variance:alice".to_string(),
        timestamp: 50_000,
        members: vec![
            variance_proto::messaging_proto::GroupMember {
                did: "did:variance:bob".to_string(),
                role: variance_proto::messaging_proto::GroupRole::Admin as i32,
                joined_at: 1000,
                nickname: None,
            },
            variance_proto::messaging_proto::GroupMember {
                did: "did:variance:carol".to_string(),
                role: variance_proto::messaging_proto::GroupRole::Member as i32,
                joined_at: 2000,
                nickname: None,
            },
        ],
        mls_welcome: vec![],
        mls_commit: vec![],
    };
    state
        .storage
        .store_pending_invitation(&invitation)
        .await
        .expect("store invitation");

    let app = create_router(state);

    // List invitations
    let resp = app.clone().oneshot(get("/invitations")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let invitations = json.as_array().unwrap();
    assert_eq!(invitations.len(), 1);
    assert_eq!(invitations[0]["group_id"], "invite-group-1");
    assert_eq!(invitations[0]["group_name"], "Invited Group");
    assert_eq!(invitations[0]["inviter_did"], "did:variance:bob");
    assert_eq!(invitations[0]["member_count"], 2);

    // Decline the invitation
    let resp = app
        .clone()
        .oneshot(post_json(
            "/invitations/invite-group-1/decline",
            &serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(json["success"].as_bool().unwrap());

    // Invitations should be empty now
    let resp = app.oneshot(get("/invitations")).await.unwrap();
    let json = body_json(resp).await;
    assert!(json.as_array().unwrap().is_empty());
}

/// Multiple conversations: independent lifecycle for each peer.
#[tokio::test]
async fn test_multiple_conversations() {
    let app = create_router(fresh_state("did:variance:alice"));

    // Start conversation with Bob
    let mut bob = vodozemac::olm::Account::new();
    bob.generate_one_time_keys(1);
    let resp = app
        .clone()
        .oneshot(post_json(
            "/conversations",
            &serde_json::json!({
                "recipient_did": "did:variance:bob",
                "text": "Hi Bob",
                "recipient_identity_key": hex::encode(bob.curve25519_key().to_bytes()),
                "recipient_one_time_key": hex::encode(bob.one_time_keys().values().next().unwrap().to_bytes()),
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Start conversation with Carol
    let mut carol = vodozemac::olm::Account::new();
    carol.generate_one_time_keys(1);
    let resp = app
        .clone()
        .oneshot(post_json(
            "/conversations",
            &serde_json::json!({
                "recipient_did": "did:variance:carol",
                "text": "Hi Carol",
                "recipient_identity_key": hex::encode(carol.curve25519_key().to_bytes()),
                "recipient_one_time_key": hex::encode(carol.one_time_keys().values().next().unwrap().to_bytes()),
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // List = 2 conversations
    let resp = app.clone().oneshot(get("/conversations")).await.unwrap();
    let json = body_json(resp).await;
    assert_eq!(json.as_array().unwrap().len(), 2);

    // Each has independent messages
    let resp = app
        .clone()
        .oneshot(get("/messages/direct/did:variance:bob"))
        .await
        .unwrap();
    let msgs = body_json(resp).await;
    assert_eq!(msgs.as_array().unwrap().len(), 1);
    assert_eq!(msgs[0]["text"], "Hi Bob");

    let resp = app
        .clone()
        .oneshot(get("/messages/direct/did:variance:carol"))
        .await
        .unwrap();
    let msgs = body_json(resp).await;
    assert_eq!(msgs.as_array().unwrap().len(), 1);
    assert_eq!(msgs[0]["text"], "Hi Carol");

    // Delete Bob's conversation — Carol's should remain
    let resp = app
        .clone()
        .oneshot(delete("/conversations/did:variance:bob"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app.oneshot(get("/conversations")).await.unwrap();
    let json = body_json(resp).await;
    let convos = json.as_array().unwrap();
    assert_eq!(convos.len(), 1);
    assert_eq!(convos[0]["peer_did"], "did:variance:carol");
}

/// Multiple groups: independent lifecycle for each group.
#[tokio::test]
async fn test_multiple_groups() {
    let app = create_router(fresh_state("did:variance:alice"));

    // Create two groups
    let resp = app
        .clone()
        .oneshot(post_json(
            "/mls/groups",
            &serde_json::json!({ "name": "Group Alpha" }),
        ))
        .await
        .unwrap();
    let gid_a = body_json(resp).await["group_id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(post_json(
            "/mls/groups",
            &serde_json::json!({ "name": "Group Beta" }),
        ))
        .await
        .unwrap();
    let gid_b = body_json(resp).await["group_id"]
        .as_str()
        .unwrap()
        .to_string();

    // List = 2 groups
    let resp = app.clone().oneshot(get("/mls/groups")).await.unwrap();
    let json = body_json(resp).await;
    assert_eq!(json.as_array().unwrap().len(), 2);

    // Send messages to each
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/group",
            &serde_json::json!({ "group_id": gid_a, "text": "Hello Alpha" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/group",
            &serde_json::json!({ "group_id": gid_b, "text": "Hello Beta" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify independent messages
    let resp = app
        .clone()
        .oneshot(get(&format!("/messages/group/{gid_a}")))
        .await
        .unwrap();
    let msgs = body_json(resp).await;
    assert_eq!(msgs.as_array().unwrap().len(), 1);
    assert_eq!(msgs[0]["text"], "Hello Alpha");

    let resp = app
        .clone()
        .oneshot(get(&format!("/messages/group/{gid_b}")))
        .await
        .unwrap();
    let msgs = body_json(resp).await;
    assert_eq!(msgs.as_array().unwrap().len(), 1);
    assert_eq!(msgs[0]["text"], "Hello Beta");

    // Delete Alpha — Beta should remain
    let resp = app
        .clone()
        .oneshot(delete(&format!("/mls/groups/{gid_a}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app.clone().oneshot(get("/mls/groups")).await.unwrap();
    let json = body_json(resp).await;
    let groups = json.as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["name"], "Group Beta");

    // Alpha's messages should be gone
    let resp = app
        .oneshot(get(&format!("/messages/group/{gid_a}")))
        .await
        .unwrap();
    let msgs = body_json(resp).await;
    assert!(msgs.as_array().unwrap().is_empty());
}

/// Role changes should update the member's role in storage but must NOT
/// create a message in the group's chat history.  Regular messages sent
/// before and after the role change should still be visible.
#[tokio::test]
async fn test_role_change_does_not_create_message() {
    use variance_messaging::storage::MessageStorage;

    let state = fresh_state("did:variance:alice");

    // Seed a second member in storage so the role-change endpoint has a target.
    use variance_proto::messaging_proto::{Group, GroupMember, GroupRole};
    let group_id = "role-test-group";
    state
        .mls_groups
        .create_group(group_id)
        .expect("create MLS group");

    let group = Group {
        id: group_id.to_string(),
        name: "Role Test".to_string(),
        admin_did: "did:variance:alice".to_string(),
        members: vec![
            GroupMember {
                did: "did:variance:alice".to_string(),
                role: GroupRole::Admin as i32,
                joined_at: 1000,
                nickname: None,
            },
            GroupMember {
                did: "did:variance:bob".to_string(),
                role: GroupRole::Member as i32,
                joined_at: 2000,
                nickname: None,
            },
        ],
        ..Default::default()
    };
    state
        .storage
        .store_group_metadata(&group)
        .await
        .expect("store metadata");

    let state_ref = state.clone();
    let app = create_router(state);

    // 1. Send a normal message — should appear in history.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/group",
            &serde_json::json!({ "group_id": group_id, "text": "Hello before role change" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 2. Promote Bob to moderator.
    let resp = app
        .clone()
        .oneshot(put_json(
            &format!("/mls/groups/{group_id}/members/did:variance:bob/role"),
            &serde_json::json!({ "new_role": "moderator" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["new_role"], "moderator");

    // 3. Verify Bob's role was updated in storage metadata.
    //    (We can't use mls_list_members because that merges MLS crypto state
    //    with metadata; Bob is only in metadata for this test.)
    let meta = state_ref
        .storage
        .fetch_group_metadata(group_id)
        .await
        .unwrap()
        .expect("group metadata should exist");
    let bob_role = meta
        .members
        .iter()
        .find(|m| m.did == "did:variance:bob")
        .expect("Bob should be in metadata members");
    assert_eq!(bob_role.role, GroupRole::Moderator as i32);

    // 4. Send another normal message after the role change.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/messages/group",
            &serde_json::json!({ "group_id": group_id, "text": "Hello after role change" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 5. Fetch group messages — should contain exactly the two real messages,
    //    NOT the role-change control message.
    let resp = app
        .clone()
        .oneshot(get(&format!("/messages/group/{group_id}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let msgs = body_json(resp).await;
    let msgs = msgs.as_array().unwrap();
    assert_eq!(
        msgs.len(),
        2,
        "Expected exactly 2 messages but got {}: {msgs:?}",
        msgs.len()
    );
    // Both should have non-empty text (role changes had empty text).
    for m in msgs {
        assert!(
            !m["text"].as_str().unwrap().is_empty(),
            "Message should have non-empty text: {m:?}"
        );
    }

    // 6. Demote Bob back to member — should also not create a message.
    let resp = app
        .clone()
        .oneshot(put_json(
            &format!("/mls/groups/{group_id}/members/did:variance:bob/role"),
            &serde_json::json!({ "new_role": "member" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 7. Verify message count is still 2.
    let resp = app
        .oneshot(get(&format!("/messages/group/{group_id}")))
        .await
        .unwrap();
    let msgs = body_json(resp).await;
    assert_eq!(
        msgs.as_array().unwrap().len(),
        2,
        "Demote should not add a message either"
    );
}
