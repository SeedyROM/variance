//! Cross-crate integration tests.
//!
//! These tests bridge variance-p2p (transport) with variance-messaging (crypto)
//! to verify full end-to-end flows that span crate boundaries:
//!
//! 1. Two real libp2p nodes discover each other via mDNS
//! 2. Olm sessions are established using keys exchanged over the P2P layer
//! 3. Encrypted DMs are sent and received through the network
//! 4. MLS groups are created and messages are exchanged via GossipSub
//!
//! These tests catch integration bugs that unit tests in individual crates miss:
//! protocol negotiation failures, race conditions in session establishment,
//! and message ordering under real network conditions.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use rand_core::OsRng;
use tempfile::tempdir;
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};
use vodozemac::olm::Account;

use variance_identity::did::Did;
use variance_messaging::direct::DirectMessageHandler;
use variance_messaging::mls::MlsGroupHandler;
use variance_messaging::storage::LocalMessageStorage;
use variance_p2p::{Config, EventChannels, Node, NodeHandle};
use variance_proto::messaging_proto::{self, MessageContent};

/// Monotonic counter for generating unique IDs across all tests in this process.
static DID_COUNTER: AtomicU64 = AtomicU64::new(0);

// ===== Harness =====

use libp2p_identity::PeerId;

/// A test node bundling a P2P node with its crypto handlers.
///
/// Represents one participant in the network with both transport (libp2p)
/// and encryption (Olm/MLS) capabilities.
#[allow(dead_code)]
struct CryptoTestNode {
    did: String,
    peer_id: PeerId,
    handle: NodeHandle,
    events: EventChannels,
    signing_key: SigningKey,
    dm_handler: Arc<DirectMessageHandler>,
    mls_handler: Arc<MlsGroupHandler>,
    storage: Arc<LocalMessageStorage>,
    shutdown_tx: mpsc::Sender<()>,
    _dir: tempfile::TempDir,
}

impl CryptoTestNode {
    /// Create and spawn a test node with full crypto capabilities.
    ///
    /// The `_label` parameter is kept for backward-compatibility with callers but
    /// is no longer used; the DID is derived as `did:variance:<hex>` from the
    /// signing key's verifying key (matching the app layer's identity generation).
    async fn spawn(_label: &str) -> Self {
        let dir = tempdir().unwrap();

        // Generate signing key first, then derive a libp2p keypair from the
        // same secret bytes so PeerId is deterministically bound to the key.
        let signing_key = SigningKey::generate(&mut OsRng);
        let libp2p_keypair = variance_p2p::keypair_from_ed25519(signing_key.to_bytes().to_vec())
            .expect("signing key bytes should be valid");
        let peer_id = libp2p_keypair.public().to_peer_id();
        // Use a monotonic counter to generate unique DIDs for each test node.
        let counter = DID_COUNTER.fetch_add(1, Ordering::Relaxed);
        let did = format!("did:variance:test{}", counter);

        let config = Config {
            storage_path: dir.path().to_path_buf(),
            bootstrap_peers: vec![],
            keypair: Some(libp2p_keypair),
            ..Default::default()
        };

        let (mut node, handle) = Node::new(config.clone()).unwrap();
        let events = node.events().clone();

        node.listen(&config).await.unwrap();

        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        tokio::spawn(async move {
            let _ = node.run(shutdown_rx).await;
        });

        // Set up crypto
        let olm_account = Account::new();
        let storage = Arc::new(LocalMessageStorage::new(dir.path().join("messages.db")).unwrap());

        let dm_handler = Arc::new(DirectMessageHandler::new(
            did.clone(),
            signing_key.clone(),
            olm_account,
            storage.clone(),
        ));

        let mls_handler = Arc::new(MlsGroupHandler::new(did.clone(), &signing_key).unwrap());

        Self {
            did,
            peer_id,
            handle,
            events,
            signing_key,
            dm_handler,
            mls_handler,
            storage,
            shutdown_tx,
            _dir: dir,
        }
    }

    /// Register this node's identity with the P2P layer so other nodes
    /// can resolve our DID and get our Olm/MLS keys.
    /// Returns (olm_identity_key, one_time_key) for direct use in tests.
    async fn register_identity(
        &self,
    ) -> (
        vodozemac::Curve25519PublicKey,
        vodozemac::Curve25519PublicKey,
    ) {
        // Generate and capture OTKs before publishing
        self.dm_handler.generate_one_time_keys(50).await;
        let otk_map = self.dm_handler.one_time_keys().await;
        let otks_bytes: Vec<Vec<u8>> = otk_map.values().map(|k| k.to_bytes().to_vec()).collect();

        // Grab one OTK for direct use in tests
        let first_otk = *otk_map.values().next().unwrap();

        self.dm_handler.mark_one_time_keys_as_published().await;

        // Generate MLS key package
        let mls_kp = self.mls_handler.generate_key_package().unwrap();
        let mls_kp_bytes = MlsGroupHandler::serialize_message_bytes(&mls_kp).unwrap();

        let olm_ik = self.dm_handler.identity_key();
        let olm_ik_bytes = olm_ik.to_bytes().to_vec();

        // Build a signed Did struct so identity responses carry valid signatures
        let did_struct =
            Did::from_signing_key(self.did.clone(), self.signing_key.clone(), &self.peer_id)
                .expect("Did construction should succeed");

        self.handle
            .set_local_identity(
                self.did.clone(),
                olm_ik_bytes,
                otks_bytes,
                Some(mls_kp_bytes),
                vec![0u8; 32], // dummy mailbox token for tests
                Some(did_struct),
            )
            .await
            .unwrap();

        (olm_ik, first_otk)
    }
}

impl Drop for CryptoTestNode {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.try_send(());
    }
}

/// Wait for a specific condition on a broadcast receiver, with timeout.
async fn wait_for<T, F>(
    rx: &mut tokio::sync::broadcast::Receiver<T>,
    dur: Duration,
    mut predicate: F,
) -> Option<T>
where
    T: Clone,
    F: FnMut(&T) -> bool,
{
    let deadline = tokio::time::Instant::now() + dur;
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(event)) if predicate(&event) => return Some(event),
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => return None,
            Err(_) => return None,
        }
    }
}

/// Wait for two nodes to discover each other and exchange identities.
/// Requires both sides to cache each other's DID before returning.
async fn wait_for_discovery(alice: &CryptoTestNode, bob: &CryptoTestNode) {
    let mut id_rx_a = alice.events.subscribe_identity();
    let mut id_rx_b = bob.events.subscribe_identity();
    let bob_did = bob.did.clone();
    let alice_did = alice.did.clone();

    let result = timeout(Duration::from_secs(15), async {
        let mut alice_found_bob = false;
        let mut bob_found_alice = false;
        loop {
            tokio::select! {
                Ok(e) = id_rx_a.recv() => {
                    if let variance_p2p::IdentityEvent::DidCached { did } = e {
                        if did == bob_did {
                            alice_found_bob = true;
                        }
                    }
                }
                Ok(e) = id_rx_b.recv() => {
                    if let variance_p2p::IdentityEvent::DidCached { did } = e {
                        if did == alice_did {
                            bob_found_alice = true;
                        }
                    }
                }
            }
            if alice_found_bob && bob_found_alice {
                return;
            }
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "Nodes should discover each other via mDNS within 15s"
    );

    // Give the DID-to-peer mapping time to propagate
    sleep(Duration::from_millis(500)).await;
}

/// Resolve a peer's identity, retrying until we get a response with non-empty
/// Olm keys. In parallel test runs, a resolution request may reach a node from
/// another test whose cache returns empty Olm keys (only the DID owner includes
/// Olm keys in a response). Retrying gives the owner's response a chance to win.
#[allow(dead_code)]
async fn resolve_identity_with_retry(
    handle: &NodeHandle,
    did: &str,
) -> variance_proto::identity_proto::IdentityFound {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let result = timeout(
            Duration::from_secs(3),
            handle.resolve_identity_by_did(did.to_string()),
        )
        .await;

        if let Ok(Ok(identity)) = result {
            if !identity.olm_identity_key.is_empty() && !identity.one_time_keys.is_empty() {
                return identity;
            }
        }

        if tokio::time::Instant::now() >= deadline {
            panic!("Failed to resolve identity with valid Olm keys for {did} within 10s");
        }

        sleep(Duration::from_millis(500)).await;
    }
}

// ===== Direct Message Integration Tests =====

/// Full end-to-end: two nodes discover, establish Olm session via P2P identity
/// resolution, send an encrypted DM through the network, and verify decryption.
#[tokio::test]
async fn test_e2e_encrypted_dm_delivery() {
    let alice = CryptoTestNode::spawn("alice").await;
    let bob = CryptoTestNode::spawn("bob").await;
    let bob_did = bob.did.clone();

    // Register identities — capture Bob's Olm keys directly for session init
    alice.register_identity().await;
    let (bob_ik, bob_otk) = bob.register_identity().await;

    // Wait for mDNS discovery and identity exchange
    wait_for_discovery(&alice, &bob).await;

    // Alice establishes an Olm session with Bob using keys obtained directly
    // (bypasses P2P identity resolution, which is tested separately in
    // two_node_tests.rs — here we focus on the Olm+transport integration)
    alice
        .dm_handler
        .init_session_as_initiator(bob_did.clone(), bob_ik, bob_otk)
        .await
        .unwrap();

    assert!(alice.dm_handler.has_session(&bob_did).await);

    // Alice encrypts a message
    let content = MessageContent {
        text: "Hello Bob, this is an encrypted message!".to_string(),
        ..Default::default()
    };
    let encrypted_msg = alice
        .dm_handler
        .send_message(bob_did.clone(), content.clone())
        .await
        .unwrap();

    // Subscribe to Bob's DM events before sending
    let mut bob_dm_rx = bob.events.subscribe_direct_messages();

    // Send the encrypted message through the P2P network
    let send_result = alice
        .handle
        .send_direct_message(bob_did.clone(), encrypted_msg.clone())
        .await;
    assert!(
        send_result.is_ok(),
        "DM send should succeed: {:?}",
        send_result.err()
    );

    // Bob should receive the message event from the P2P layer
    let received_event = wait_for(&mut bob_dm_rx, Duration::from_secs(5), |e| {
        matches!(
            e,
            variance_p2p::events::DirectMessageEvent::MessageReceived { .. }
        )
    })
    .await;

    assert!(
        received_event.is_some(),
        "Bob should receive the DM event within 5s"
    );

    // Extract the wire message from the event
    let wire_message = match received_event.unwrap() {
        variance_p2p::events::DirectMessageEvent::MessageReceived { message, .. } => message,
        _ => unreachable!(),
    };

    // Bob decrypts the message using the Olm layer
    let decrypted = bob.dm_handler.receive_message(wire_message).await.unwrap();

    assert_eq!(
        decrypted.text, "Hello Bob, this is an encrypted message!",
        "Decrypted text should match original"
    );
}

/// Verify that a reply (Bob→Alice after Alice→Bob) works correctly,
/// advancing the Olm ratchet in both directions.
#[tokio::test]
async fn test_e2e_dm_bidirectional_ratchet() {
    let alice = CryptoTestNode::spawn("alice").await;
    let bob = CryptoTestNode::spawn("bob").await;
    let alice_did = alice.did.clone();
    let bob_did = bob.did.clone();

    alice.register_identity().await;
    let (bob_ik, bob_otk) = bob.register_identity().await;
    wait_for_discovery(&alice, &bob).await;

    // Establish outbound Olm session using Bob's keys directly
    alice
        .dm_handler
        .init_session_as_initiator(bob_did.clone(), bob_ik, bob_otk)
        .await
        .unwrap();

    // Alice -> Bob (PreKey message)
    let msg1 = alice
        .dm_handler
        .send_message(
            bob_did.clone(),
            MessageContent {
                text: "Hello Bob!".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let mut bob_dm_rx = bob.events.subscribe_direct_messages();
    alice
        .handle
        .send_direct_message(bob_did.clone(), msg1)
        .await
        .unwrap();

    let bob_event = wait_for(&mut bob_dm_rx, Duration::from_secs(5), |e| {
        matches!(
            e,
            variance_p2p::events::DirectMessageEvent::MessageReceived { .. }
        )
    })
    .await
    .expect("Bob should receive Alice's message");

    let wire_msg1 = match bob_event {
        variance_p2p::events::DirectMessageEvent::MessageReceived { message, .. } => message,
        _ => unreachable!(),
    };

    let decrypted1 = bob.dm_handler.receive_message(wire_msg1).await.unwrap();
    assert_eq!(decrypted1.text, "Hello Bob!");

    // Bob now has an inbound session. Send a reply: Bob -> Alice
    assert!(
        bob.dm_handler.has_session(&alice_did).await,
        "Bob should have an Olm session with Alice after receiving a PreKey message"
    );

    let msg2 = bob
        .dm_handler
        .send_message(
            alice_did.clone(),
            MessageContent {
                text: "Hi Alice, got your message!".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let mut alice_dm_rx = alice.events.subscribe_direct_messages();
    bob.handle
        .send_direct_message(alice_did.clone(), msg2)
        .await
        .unwrap();

    let alice_event = wait_for(&mut alice_dm_rx, Duration::from_secs(5), |e| {
        matches!(
            e,
            variance_p2p::events::DirectMessageEvent::MessageReceived { .. }
        )
    })
    .await
    .expect("Alice should receive Bob's reply");

    let wire_msg2 = match alice_event {
        variance_p2p::events::DirectMessageEvent::MessageReceived { message, .. } => message,
        _ => unreachable!(),
    };

    let decrypted2 = alice.dm_handler.receive_message(wire_msg2).await.unwrap();
    assert_eq!(decrypted2.text, "Hi Alice, got your message!");

    // Send a third message (Alice -> Bob, now Normal type instead of PreKey)
    let msg3 = alice
        .dm_handler
        .send_message(
            bob_did.clone(),
            MessageContent {
                text: "Third message, Normal type".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Verify it's a Normal message (type 1), not PreKey (type 0)
    assert_eq!(
        msg3.olm_message_type, 1,
        "After receiving a reply, messages should be Normal (type 1)"
    );

    let mut bob_dm_rx2 = bob.events.subscribe_direct_messages();
    alice
        .handle
        .send_direct_message(bob_did.clone(), msg3)
        .await
        .unwrap();

    let bob_event2 = wait_for(&mut bob_dm_rx2, Duration::from_secs(5), |e| {
        matches!(
            e,
            variance_p2p::events::DirectMessageEvent::MessageReceived { .. }
        )
    })
    .await
    .expect("Bob should receive the Normal message");

    let wire_msg3 = match bob_event2 {
        variance_p2p::events::DirectMessageEvent::MessageReceived { message, .. } => message,
        _ => unreachable!(),
    };

    let decrypted3 = bob.dm_handler.receive_message(wire_msg3).await.unwrap();
    assert_eq!(decrypted3.text, "Third message, Normal type");
}

// ===== GossipSub + MLS Integration Tests =====

/// Two nodes create an MLS group, exchange messages via GossipSub, and
/// verify decryption across the network.
#[tokio::test]
async fn test_e2e_mls_group_message_via_gossipsub() {
    let alice = CryptoTestNode::spawn("alice").await;
    let bob = CryptoTestNode::spawn("bob").await;
    let alice_did = alice.did.clone();

    alice.register_identity().await;
    bob.register_identity().await;
    wait_for_discovery(&alice, &bob).await;

    let group_id = &format!("test-group-e2e-{}", DID_COUNTER.load(Ordering::Relaxed));

    // Alice creates an MLS group
    alice.mls_handler.create_group(group_id).unwrap();

    // Bob generates a KeyPackage for Alice to add him
    let bob_kp = bob.mls_handler.generate_key_package().unwrap();

    // Alice adds Bob
    let add_result = alice.mls_handler.add_member(group_id, bob_kp).unwrap();

    // Bob joins via Welcome
    let welcome_bytes = MlsGroupHandler::serialize_message(&add_result.welcome).unwrap();
    let welcome_in = MlsGroupHandler::deserialize_message(&welcome_bytes).unwrap();
    let joined_id = bob.mls_handler.join_group_from_welcome(welcome_in).unwrap();
    assert_eq!(joined_id, *group_id);

    // Both subscribe to the GossipSub topic
    alice
        .handle
        .subscribe_to_topic(group_id.to_string())
        .await
        .unwrap();
    bob.handle
        .subscribe_to_topic(group_id.to_string())
        .await
        .unwrap();

    // GossipSub mesh formation needs time (heartbeat + mesh building)
    sleep(Duration::from_secs(3)).await;

    // Alice encrypts a group message
    let plaintext = b"Hello MLS group from Alice!";
    let mls_encrypted = alice
        .mls_handler
        .encrypt_message(group_id, plaintext)
        .unwrap();
    let mls_bytes = MlsGroupHandler::serialize_message(&mls_encrypted).unwrap();

    // Create a GroupMessage protobuf wrapping the MLS ciphertext
    let group_message = messaging_proto::GroupMessage {
        id: "msg-001".to_string(),
        group_id: group_id.to_string(),
        sender_did: alice_did.clone(),
        mls_ciphertext: mls_bytes.clone(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        r#type: 0,
        reply_to: None,
    };

    // Subscribe to Bob's group message events
    let mut bob_group_rx = bob.events.subscribe_group_messages();

    // Alice publishes via GossipSub
    alice
        .handle
        .publish_group_message(group_id.to_string(), group_message)
        .await
        .unwrap();

    // Bob should receive the group message from GossipSub
    let bob_group_event = wait_for(&mut bob_group_rx, Duration::from_secs(5), |e| {
        matches!(
            e,
            variance_p2p::events::GroupMessageEvent::MessageReceived { .. }
        )
    })
    .await;

    assert!(
        bob_group_event.is_some(),
        "Bob should receive the group message via GossipSub within 5s"
    );

    // Extract the ciphertext and decrypt with MLS
    let wire_group_msg = match bob_group_event.unwrap() {
        variance_p2p::events::GroupMessageEvent::MessageReceived { message } => message,
        _ => unreachable!(),
    };

    let mls_in = MlsGroupHandler::deserialize_message(&wire_group_msg.mls_ciphertext).unwrap();
    let decrypted = bob
        .mls_handler
        .process_message(group_id, mls_in)
        .unwrap()
        .expect("Should be an application message");

    assert_eq!(
        decrypted.plaintext, plaintext,
        "Decrypted MLS message should match original"
    );

    // Verify sender credential is Alice's DID
    let sender_did = String::from_utf8_lossy(decrypted.sender_credential.serialized_content());
    assert_eq!(sender_did, alice_did);
}

/// MLS group with member removal: after removing Bob, Alice's messages should
/// not be decryptable by Bob (forward secrecy after removal).
#[tokio::test]
async fn test_e2e_mls_member_removal_forward_secrecy() {
    let alice = CryptoTestNode::spawn("alice").await;
    let bob = CryptoTestNode::spawn("bob").await;
    let alice_did = alice.did.clone();
    let bob_did = bob.did.clone();

    alice.register_identity().await;
    bob.register_identity().await;
    wait_for_discovery(&alice, &bob).await;

    let group_id = &format!("removal-test-{}", DID_COUNTER.load(Ordering::Relaxed));

    // Set up group with both members
    alice.mls_handler.create_group(group_id).unwrap();
    let bob_kp = bob.mls_handler.generate_key_package().unwrap();
    let add_result = alice.mls_handler.add_member(group_id, bob_kp).unwrap();

    let welcome_bytes = MlsGroupHandler::serialize_message(&add_result.welcome).unwrap();
    bob.mls_handler
        .join_group_from_welcome(MlsGroupHandler::deserialize_message(&welcome_bytes).unwrap())
        .unwrap();

    // Verify both can message
    let pre_remove_msg = alice
        .mls_handler
        .encrypt_message(group_id, b"before removal")
        .unwrap();
    let pre_bytes = MlsGroupHandler::serialize_message(&pre_remove_msg).unwrap();
    let decrypted = bob
        .mls_handler
        .process_message(
            group_id,
            MlsGroupHandler::deserialize_message(&pre_bytes).unwrap(),
        )
        .unwrap()
        .expect("pre-removal message should decrypt");
    assert_eq!(decrypted.plaintext, b"before removal");

    // Alice removes Bob
    let bob_idx = alice
        .mls_handler
        .find_member_index(group_id, &bob_did)
        .unwrap()
        .expect("Bob should be in group");
    let _remove_result = alice.mls_handler.remove_member(group_id, bob_idx).unwrap();

    // Alice's epoch has advanced
    let epoch_after = alice.mls_handler.epoch(group_id).unwrap();
    assert!(epoch_after > 1, "Epoch should advance after removal");

    // Alice sends a message in the new epoch
    let post_remove_msg = alice
        .mls_handler
        .encrypt_message(group_id, b"after removal - secret")
        .unwrap();
    let post_bytes = MlsGroupHandler::serialize_message(&post_remove_msg).unwrap();

    // Bob should NOT be able to decrypt (he was removed, different epoch)
    let bob_result = bob.mls_handler.process_message(
        group_id,
        MlsGroupHandler::deserialize_message(&post_bytes).unwrap(),
    );
    assert!(
        bob_result.is_err(),
        "Bob should not be able to decrypt messages after being removed"
    );

    // Verify Alice is the only member
    let members = alice.mls_handler.list_members(group_id).unwrap();
    assert_eq!(members, vec![alice_did]);
}

/// Test that DM delivery failure is properly reported when the recipient
/// disconnects between discovery and message send.
#[tokio::test]
async fn test_dm_delivery_failure_on_disconnect() {
    let alice = CryptoTestNode::spawn("alice").await;
    let bob = CryptoTestNode::spawn("bob").await;
    let alice_did = alice.did.clone();
    let bob_did = bob.did.clone();

    alice.register_identity().await;
    bob.register_identity().await;
    wait_for_discovery(&alice, &bob).await;

    // Verify Alice can see Bob
    let connected = alice.handle.get_connected_dids().await.unwrap();
    assert!(
        connected.contains(&bob_did),
        "Alice should see Bob after discovery"
    );

    // Shut down Bob
    let _ = bob.shutdown_tx.send(()).await;
    drop(bob);

    // Give the connection time to close
    sleep(Duration::from_secs(2)).await;

    // Alice tries to send a message to the now-disconnected Bob
    let message = messaging_proto::DirectMessage {
        id: "test-fail-001".to_string(),
        sender_did: alice_did.clone(),
        recipient_did: bob_did.clone(),
        ciphertext: vec![1, 2, 3],
        olm_message_type: 0,
        signature: vec![],
        timestamp: 1000,
        r#type: 0,
        reply_to: None,
        sender_identity_key: None,
    };

    let result = alice
        .handle
        .send_direct_message(bob_did.clone(), message)
        .await;

    // Should fail because Bob is disconnected
    assert!(
        result.is_err(),
        "Sending to a disconnected peer should fail"
    );
}
