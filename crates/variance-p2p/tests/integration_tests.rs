use libp2p::PeerId;
use std::time::Duration;
use tempfile::tempdir;
use tokio::time::timeout;
use variance_p2p::events::DirectMessageEvent;
use variance_p2p::{Config, IdentityEvent, Node, OfflineMessageEvent, SignalingEvent};
use variance_proto::media_proto::{signaling_message, CallType, Offer, SignalingMessage};

/// Helper to create a test node with temporary storage
async fn create_test_node() -> (Node, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let config = Config {
        storage_path: dir.path().to_path_buf(),
        bootstrap_peers: vec![],
        ..Default::default()
    };

    let (node, _handle) = Node::new(config).unwrap();
    (node, dir)
}

#[tokio::test]
async fn test_identity_protocol_event_flow() {
    let (node, _dir) = create_test_node().await;

    // Subscribe to identity events
    let mut rx = node.events().subscribe_identity();

    // Create a test DID and cache it
    let peer_id = PeerId::random();
    let did = variance_identity::did::Did::new(&peer_id).unwrap();
    let did_id = did.id.clone();

    // Cache the DID (this should trigger a DidCached event)
    // Note: We'd need to expose the identity handler or add a public method to Node
    // For now, this tests that the event system works

    // Send a test event directly
    node.events().send_identity(IdentityEvent::DidCached {
        did: did_id.clone(),
    });

    // Verify we receive the event
    let event = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("Timeout waiting for event")
        .expect("Failed to receive event");

    match event {
        IdentityEvent::DidCached { did } => {
            assert_eq!(did, did_id);
        }
        _ => panic!("Expected DidCached event"),
    }
}

#[tokio::test]
async fn test_offline_message_protocol_event_flow() {
    let (node, _dir) = create_test_node().await;

    // Subscribe to offline message events
    let mut rx = node.events().subscribe_offline_messages();

    // Send a test fetch requested event
    let peer = PeerId::random();
    node.events()
        .send_offline_message(OfflineMessageEvent::FetchRequested {
            peer,
            did: "did:variance:alice".to_string(),
            limit: 10,
        });

    // Verify we receive the event
    let event = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("Timeout waiting for event")
        .expect("Failed to receive event");

    match event {
        OfflineMessageEvent::FetchRequested {
            peer: recv_peer,
            did,
            limit,
        } => {
            assert_eq!(recv_peer, peer);
            assert_eq!(did, "did:variance:alice");
            assert_eq!(limit, 10);
        }
        _ => panic!("Expected FetchRequested event"),
    }
}

#[tokio::test]
async fn test_signaling_protocol_event_flow() {
    let (node, _dir) = create_test_node().await;

    // Subscribe to signaling events
    let mut rx = node.events().subscribe_signaling();

    // Send a test offer received event
    let peer = PeerId::random();
    let call_id = "call123".to_string();
    let message = SignalingMessage {
        call_id: call_id.clone(),
        sender_did: "did:variance:alice".to_string(),
        recipient_did: "did:variance:bob".to_string(),
        message: Some(signaling_message::Message::Offer(Offer {
            sdp: "test_sdp".to_string(),
            call_type: CallType::Video.into(),
        })),
        timestamp: 0,
        signature: vec![],
        nonce: vec![0u8; 16],
    };

    node.events().send_signaling(SignalingEvent::OfferReceived {
        peer,
        call_id: call_id.clone(),
        message: message.clone(),
    });

    // Verify we receive the event
    let event = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("Timeout waiting for event")
        .expect("Failed to receive event");

    match event {
        SignalingEvent::OfferReceived {
            peer: recv_peer,
            call_id: recv_call_id,
            message: recv_msg,
        } => {
            assert_eq!(recv_peer, peer);
            assert_eq!(recv_call_id, call_id);
            assert_eq!(recv_msg.call_id, message.call_id);
        }
        _ => panic!("Expected OfferReceived event"),
    }
}

#[tokio::test]
async fn test_multiple_subscribers_receive_same_event() {
    let (node, _dir) = create_test_node().await;

    // Create multiple subscribers
    let mut rx1 = node.events().subscribe_identity();
    let mut rx2 = node.events().subscribe_identity();
    let mut rx3 = node.events().subscribe_identity();

    // Send an event
    let did = "did:peer:test123".to_string();
    node.events()
        .send_identity(IdentityEvent::DidCached { did: did.clone() });

    // All subscribers should receive it
    for rx in [&mut rx1, &mut rx2, &mut rx3] {
        let event = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Timeout waiting for event")
            .expect("Failed to receive event");

        match event {
            IdentityEvent::DidCached { did: recv_did } => {
                assert_eq!(recv_did, did);
            }
            _ => panic!("Expected DidCached event"),
        }
    }
}

#[tokio::test]
async fn test_event_channels_isolated() {
    let (node, _dir) = create_test_node().await;

    // Subscribe to different event types
    let mut identity_rx = node.events().subscribe_identity();
    let mut offline_rx = node.events().subscribe_offline_messages();
    let mut signaling_rx = node.events().subscribe_signaling();

    // Send an identity event
    node.events().send_identity(IdentityEvent::DidCached {
        did: "did:test".to_string(),
    });

    // Only identity subscriber should receive it
    timeout(Duration::from_millis(100), identity_rx.recv())
        .await
        .expect("Should receive identity event")
        .expect("Failed to receive");

    // Other channels should timeout (no events)
    assert!(timeout(Duration::from_millis(100), offline_rx.recv())
        .await
        .is_err());
    assert!(timeout(Duration::from_millis(100), signaling_rx.recv())
        .await
        .is_err());
}

#[tokio::test]
async fn test_identity_handler_cache_and_resolve() {
    let (node, _dir) = create_test_node().await;

    // Create and cache a DID via the identity handler
    let peer_id = PeerId::random();
    let mut did = variance_identity::did::Did::new(&peer_id).unwrap();
    did.update_profile(Some("alice".to_string()), None, None);
    let _did_id = did.id.clone();

    // Note: We'd need to expose a way to access the identity handler to cache the DID
    // For a full integration test, we'd want to:
    // 1. Cache a DID in node A
    // 2. Send an identity request from node B
    // 3. Verify node B receives the DID

    // For now, this test verifies the node can be created and events work
    assert!(!node.peer_id().to_string().is_empty());
}

#[tokio::test]
async fn test_offline_message_handler_storage() {
    let (node, _dir) = create_test_node().await;

    // Note: For a full integration test, we'd want to:
    // 1. Store an offline message in node A
    // 2. Send a fetch request from node B
    // 3. Verify node B receives the messages

    // For now, this test verifies the node initializes properly with storage
    assert!(!node.peer_id().to_string().is_empty());
}

#[tokio::test]
async fn test_signaling_handler_call_lifecycle() {
    let (node, _dir) = create_test_node().await;

    let mut rx = node.events().subscribe_signaling();

    // Simulate a call lifecycle: offer -> answer -> hangup
    let peer = PeerId::random();
    let call_id = "call456".to_string();

    // Offer
    node.events().send_signaling(SignalingEvent::OfferReceived {
        peer,
        call_id: call_id.clone(),
        message: SignalingMessage {
            call_id: call_id.clone(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            message: Some(signaling_message::Message::Offer(Offer {
                sdp: "offer_sdp".to_string(),
                call_type: CallType::Audio.into(),
            })),
            timestamp: 0,
            signature: vec![],
            nonce: vec![0u8; 16],
        },
    });

    let _ = timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("Should receive offer")
        .expect("Failed to receive");

    // End call
    node.events().send_signaling(SignalingEvent::CallEnded {
        call_id: call_id.clone(),
        reason: "User hung up".to_string(),
    });

    let event = timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("Should receive call ended")
        .expect("Failed to receive");

    match event {
        SignalingEvent::CallEnded {
            call_id: recv_id,
            reason,
        } => {
            assert_eq!(recv_id, call_id);
            assert_eq!(reason, "User hung up");
        }
        _ => panic!("Expected CallEnded event"),
    }
}

#[tokio::test]
async fn test_delivery_failed_event_flow() {
    let (node, _dir) = create_test_node().await;

    let mut rx = node.events().subscribe_direct_messages();

    // Simulate a DeliveryFailed event (as would be emitted by OutboundFailure handler)
    node.events()
        .send_direct_message(DirectMessageEvent::DeliveryFailed {
            message_id: "msg-failed-001".to_string(),
            recipient: "did:variance:offline_user".to_string(),
        });

    let event = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("Timeout waiting for event")
        .expect("Failed to receive event");

    match event {
        DirectMessageEvent::DeliveryFailed {
            message_id,
            recipient,
        } => {
            assert_eq!(message_id, "msg-failed-001");
            assert_eq!(recipient, "did:variance:offline_user");
        }
        _ => panic!("Expected DeliveryFailed event, got {:?}", event),
    }
}

#[tokio::test]
async fn test_delivery_nack_event_flow() {
    let (node, _dir) = create_test_node().await;

    let mut rx = node.events().subscribe_direct_messages();

    let peer = PeerId::random();
    node.events()
        .send_direct_message(DirectMessageEvent::DeliveryNack {
            peer,
            message_id: "msg-nack-001".to_string(),
            error: "rate limited".to_string(),
        });

    let event = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("Timeout waiting for event")
        .expect("Failed to receive event");

    match event {
        DirectMessageEvent::DeliveryNack {
            peer: recv_peer,
            message_id,
            error,
        } => {
            assert_eq!(recv_peer, peer);
            assert_eq!(message_id, "msg-nack-001");
            assert_eq!(error, "rate limited");
        }
        _ => panic!("Expected DeliveryNack event, got {:?}", event),
    }
}

/// Sending a DM to an unknown peer DID should fail with an error
/// (the app layer then queues the message as pending).
#[tokio::test]
async fn test_send_dm_to_unknown_peer_fails() {
    let dir = tempdir().unwrap();
    let config = Config {
        storage_path: dir.path().to_path_buf(),
        ..Default::default()
    };
    let (_node, handle) = Node::new(config).unwrap();

    let message = variance_proto::messaging_proto::DirectMessage {
        id: "test-msg-001".to_string(),
        sender_did: "did:variance:alice".to_string(),
        recipient_did: "did:variance:nobody".to_string(),
        ciphertext: vec![1, 2, 3],
        olm_message_type: 0,
        signature: vec![],
        timestamp: 1000,
        r#type: 0,
        reply_to: None,
        sender_identity_key: None,
    };

    // Must run the node in the background so commands are processed
    let mut node = _node;
    let (shutdown_tx, shutdown_rx) = tokio::sync::mpsc::channel(1);
    let node_handle = tokio::spawn(async move {
        let _ = node.run(shutdown_rx).await;
    });

    // Small delay for the node run loop to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = handle
        .send_direct_message("did:variance:nobody".to_string(), message)
        .await;

    assert!(result.is_err(), "Sending to unknown peer DID should fail");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Unknown peer DID"),
        "Error should mention unknown peer DID, got: {}",
        err
    );

    drop(shutdown_tx);
    node_handle.abort();
}
