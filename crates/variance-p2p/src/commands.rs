//! Command types for controlling the P2P node
//!
//! The Node runs in its own task and processes commands sent via channels.
//! This allows the application layer to send messages without directly
//! accessing the Swarm (which is not Send/Sync).

use libp2p::PeerId;
use tokio::sync::oneshot;
use variance_proto::identity_proto::{IdentityFound, IdentityRequest, IdentityResponse};
use variance_proto::media_proto::SignalingMessage;
use variance_proto::messaging_proto::{
    DirectMessage, GroupMessage, GroupSyncResponse, ReadReceipt, TypingIndicator,
};

use crate::error::Result;

/// Re-used type aliases (canonical definitions in `node::mod`).
type IdentityRequestOneshot = oneshot::Sender<Result<IdentityResponse>>;
type BroadcastDidResolveOneshot = oneshot::Sender<Result<IdentityFound>>;
type ProviderQueryOneshot = oneshot::Sender<Result<Vec<PeerId>>>;
/// Simple ack/error response for fire-and-confirm commands.
type CommandResponse = oneshot::Sender<Result<()>>;
type ConnectedDidsResponse = oneshot::Sender<Vec<String>>;
type GroupSyncOneshot = oneshot::Sender<Result<()>>;

/// Commands that can be sent to the P2P node
#[derive(Debug)]
pub enum NodeCommand {
    /// Send an identity request to a peer
    SendIdentityRequest {
        peer: PeerId,
        request: IdentityRequest,
        response_tx: IdentityRequestOneshot,
    },

    /// Send a WebRTC signaling message to a peer (by DID)
    SendSignalingMessage {
        peer_did: String,
        message: SignalingMessage,
        response_tx: CommandResponse,
    },

    /// Publish a group message to a GossipSub topic
    PublishGroupMessage {
        topic: String,
        message: GroupMessage,
        response_tx: CommandResponse,
    },

    /// Subscribe to a GossipSub topic (for group messaging)
    SubscribeToTopic {
        topic: String,
        response_tx: CommandResponse,
    },

    /// Unsubscribe from a GossipSub topic
    UnsubscribeFromTopic {
        topic: String,
        response_tx: CommandResponse,
    },

    /// Publish a DHT provider record announcing this node serves the given username key.
    /// Key should be constructed with `username_dht_key(username)`.
    ProvideUsername {
        key: libp2p::kad::RecordKey,
        response_tx: CommandResponse,
    },

    /// Find peers that have announced they provide the given username key.
    FindUsernameProviders {
        key: libp2p::kad::RecordKey,
        response_tx: ProviderQueryOneshot,
    },

    /// Send an encrypted direct message to a peer (by DID)
    SendDirectMessage {
        peer_did: String,
        message: DirectMessage,
        response_tx: CommandResponse,
    },

    /// Register this node's own DID and Olm keys in the identity handler.
    ///
    /// Must be called after the Olm account is initialized so the handler can
    /// respond to inbound identity requests about our own DID with Olm keys.
    SetLocalIdentity {
        did: String,
        olm_identity_key: Vec<u8>,
        one_time_keys: Vec<Vec<u8>>,
        mls_key_package: Option<Vec<u8>>,
        mailbox_token: Vec<u8>,
        /// Full DID with signing key for producing signed identity responses.
        did_struct: Box<Option<variance_identity::did::Did>>,
    },

    /// Update the one-time keys list in the identity handler.
    ///
    /// Call this after receiving a PreKey message (which consumes an OTK) to ensure
    /// we don't advertise already-used keys to other peers.
    UpdateOneTimeKeys { one_time_keys: Vec<Vec<u8>> },

    /// Set the local username and discriminator in the identity handler so they
    /// are included in responses to identity queries about ourselves.
    SetLocalUsername {
        username: String,
        discriminator: u32,
    },

    /// Resolve a peer's DID by broadcasting an identity request to all connected peers.
    ///
    /// Returns the first `IdentityFound` response received, carrying the peer's
    /// Olm identity key and one-time pre-keys needed to establish a session.
    ResolveIdentityByDid {
        did: String,
        response_tx: BroadcastDidResolveOneshot,
    },

    /// Return the list of DIDs for all currently connected peers.
    ///
    /// Used by the app layer to provide accurate initial presence state
    /// when a WebSocket client connects or to serve presence polling.
    GetConnectedDids { response_tx: ConnectedDidsResponse },

    /// Send a typing indicator to a peer (fire-and-forget, no ack expected).
    SendTypingIndicator {
        peer_did: String,
        indicator: TypingIndicator,
    },

    /// Broadcast a typing indicator to all online members of a group.
    ///
    /// The node fans out a unicast `TypingIndicator` to each online group member,
    /// avoiding GossipSub metadata leakage. `member_dids` should be the full
    /// member list from MLS; the node filters to only currently-connected peers.
    BroadcastGroupTyping {
        member_dids: Vec<String>,
        indicator: TypingIndicator,
    },

    /// Broadcast our username change to all currently-connected peers (fire-and-forget).
    BroadcastUsernameChange {
        did: String,
        username: String,
        discriminator: u32,
        /// Ed25519 signing key bytes (32 bytes) for signing the notification.
        signing_key_bytes: Vec<u8>,
    },

    /// Request group message history from a specific peer since a given timestamp.
    ///
    /// Used for P2P epoch-based sync: after reconnecting, ask peers for any
    /// group messages we missed while offline.
    RequestGroupSync {
        peer_did: String,
        group_id: String,
        since_timestamp: i64,
        limit: u32,
        response_tx: GroupSyncOneshot,
    },

    /// Respond to an inbound group sync request with messages from storage.
    ///
    /// Called by the app layer after querying `MessageStorage::fetch_group_since`
    /// to serve the requesting peer their missed messages.
    RespondGroupSync {
        request_id: libp2p::request_response::InboundRequestId,
        response: GroupSyncResponse,
    },

    /// Send a read receipt to the original message sender (fire-and-forget).
    SendReceipt {
        peer_did: String,
        receipt: ReadReceipt,
    },

    /// Replace the advertised MLS KeyPackage after the previous one was consumed by a Welcome.
    UpdateMlsKeyPackage { key_package: Vec<u8> },
}

/// Construct the DHT record key for a username, e.g. "alice#0001".
pub fn username_dht_key(username: &str) -> libp2p::kad::RecordKey {
    libp2p::kad::RecordKey::new(&format!("/variance/username/{}", username))
}

/// Handle for sending commands to the P2P node
///
/// This is the application-facing interface for sending messages over the network.
/// It's cloneable and can be shared across the application.
#[derive(Clone)]
pub struct NodeHandle {
    command_tx: tokio::sync::mpsc::Sender<NodeCommand>,
}

impl NodeHandle {
    /// Create a new node handle with a command sender
    pub fn new(command_tx: tokio::sync::mpsc::Sender<NodeCommand>) -> Self {
        Self { command_tx }
    }

    /// Send an identity request to a peer
    pub async fn send_identity_request(
        &self,
        peer: PeerId,
        request: IdentityRequest,
    ) -> Result<IdentityResponse> {
        let (response_tx, response_rx) = oneshot::channel();

        self.command_tx
            .send(NodeCommand::SendIdentityRequest {
                peer,
                request,
                response_tx,
            })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })?;

        response_rx
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to receive response from node".to_string(),
            })?
    }

    /// Send a WebRTC signaling message to a peer (by DID)
    pub async fn send_signaling_message(
        &self,
        peer_did: String,
        message: SignalingMessage,
    ) -> Result<()> {
        let (response_tx, response_rx) = oneshot::channel();

        self.command_tx
            .send(NodeCommand::SendSignalingMessage {
                peer_did,
                message,
                response_tx,
            })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })?;

        response_rx
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to receive response from node".to_string(),
            })?
    }

    /// Publish a group message to a GossipSub topic
    pub async fn publish_group_message(&self, topic: String, message: GroupMessage) -> Result<()> {
        let (response_tx, response_rx) = oneshot::channel();

        self.command_tx
            .send(NodeCommand::PublishGroupMessage {
                topic,
                message,
                response_tx,
            })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })?;

        response_rx
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to receive response from node".to_string(),
            })?
    }

    /// Subscribe to a GossipSub topic for group messaging
    pub async fn subscribe_to_topic(&self, topic: String) -> Result<()> {
        let (response_tx, response_rx) = oneshot::channel();

        self.command_tx
            .send(NodeCommand::SubscribeToTopic { topic, response_tx })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })?;

        response_rx
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to receive response from node".to_string(),
            })?
    }

    /// Unsubscribe from a GossipSub topic
    pub async fn unsubscribe_from_topic(&self, topic: String) -> Result<()> {
        let (response_tx, response_rx) = oneshot::channel();

        self.command_tx
            .send(NodeCommand::UnsubscribeFromTopic { topic, response_tx })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })?;

        response_rx
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to receive response from node".to_string(),
            })?
    }

    /// Publish a DHT provider record for the given username
    pub async fn provide_username(&self, username: &str) -> Result<()> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(NodeCommand::ProvideUsername {
                key: username_dht_key(username),
                response_tx,
            })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })?;
        response_rx
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to receive response from node".to_string(),
            })?
    }

    /// Send an encrypted direct message to a peer (by DID)
    ///
    /// The peer's DID must be known (i.e., a prior message was received from them,
    /// or they were discovered via the identity protocol) so the DID→PeerId mapping
    /// is populated in the node's routing table.
    pub async fn send_direct_message(
        &self,
        peer_did: String,
        message: DirectMessage,
    ) -> Result<()> {
        let (response_tx, response_rx) = oneshot::channel();

        self.command_tx
            .send(NodeCommand::SendDirectMessage {
                peer_did,
                message,
                response_tx,
            })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })?;

        response_rx
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to receive response from node".to_string(),
            })?
    }

    /// Register this node's own DID and Olm keys with the P2P identity handler.
    ///
    /// After this call, inbound identity requests for our own DID will be answered
    /// with our Olm keys so peers can establish sessions with us.
    /// `did_struct` should be the full `Did` with signing key for signed responses.
    pub async fn set_local_identity(
        &self,
        did: String,
        olm_identity_key: Vec<u8>,
        one_time_keys: Vec<Vec<u8>>,
        mls_key_package: Option<Vec<u8>>,
        mailbox_token: Vec<u8>,
        did_struct: Option<variance_identity::did::Did>,
    ) -> Result<()> {
        self.command_tx
            .send(NodeCommand::SetLocalIdentity {
                did,
                olm_identity_key,
                one_time_keys,
                mls_key_package,
                mailbox_token,
                did_struct: Box::new(did_struct),
            })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })
    }

    /// Update the one-time keys advertised to peers (call after OTK consumption).
    ///
    /// When a PreKey message consumes an OTK, call this to refresh the advertised
    /// list so other peers don't try to use already-consumed keys.
    pub async fn update_one_time_keys(&self, one_time_keys: Vec<Vec<u8>>) -> Result<()> {
        self.command_tx
            .send(NodeCommand::UpdateOneTimeKeys { one_time_keys })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })
    }

    /// Set the local username and discriminator in the identity handler.
    ///
    /// After this call, identity responses for our own DID will include the
    /// discriminator so remote peers get the real value instead of a placeholder.
    pub async fn set_local_username(&self, username: String, discriminator: u32) -> Result<()> {
        self.command_tx
            .send(NodeCommand::SetLocalUsername {
                username,
                discriminator,
            })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })
    }

    /// Resolve a peer's DID via the P2P identity protocol.
    ///
    /// Broadcasts an identity request to all currently connected peers and returns
    /// the first `IdentityFound` response. Fails if no peers are connected or if
    /// none of the connected peers know about the requested DID.
    pub async fn resolve_identity_by_did(&self, did: String) -> Result<IdentityFound> {
        let (response_tx, response_rx) = oneshot::channel();

        self.command_tx
            .send(NodeCommand::ResolveIdentityByDid { did, response_tx })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })?;

        response_rx
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to receive response from node".to_string(),
            })?
    }

    /// Get the list of DIDs for all currently connected peers.
    ///
    /// Returns the DIDs tracked in the node's `did_to_peer` map.
    pub async fn get_connected_dids(&self) -> Result<Vec<String>> {
        let (response_tx, response_rx) = oneshot::channel();

        self.command_tx
            .send(NodeCommand::GetConnectedDids { response_tx })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })?;

        response_rx
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to receive response from node".to_string(),
            })
    }

    /// Send a typing indicator to a peer (fire-and-forget).
    ///
    /// Failures are silently dropped — typing indicators are ephemeral and
    /// it is not worth surfacing errors for a best-effort signal.
    pub async fn send_typing_indicator(
        &self,
        peer_did: String,
        indicator: TypingIndicator,
    ) -> Result<()> {
        self.command_tx
            .send(NodeCommand::SendTypingIndicator {
                peer_did,
                indicator,
            })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })
    }

    /// Broadcast a typing indicator to all online members of a group.
    ///
    /// Fans out unicast messages to each online group member, avoiding GossipSub
    /// metadata leakage. Failures are silently dropped — best-effort.
    pub async fn broadcast_group_typing(
        &self,
        member_dids: Vec<String>,
        indicator: TypingIndicator,
    ) -> Result<()> {
        self.command_tx
            .send(NodeCommand::BroadcastGroupTyping {
                member_dids,
                indicator,
            })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })
    }

    /// Broadcast a username change to all currently-connected peers (fire-and-forget).
    ///
    /// Failures are silently dropped — rename notifications are best-effort.
    pub async fn broadcast_username_change(
        &self,
        did: String,
        username: String,
        discriminator: u32,
        signing_key_bytes: Vec<u8>,
    ) -> Result<()> {
        self.command_tx
            .send(NodeCommand::BroadcastUsernameChange {
                did,
                username,
                discriminator,
                signing_key_bytes,
            })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })
    }

    /// Find peers that provide the given username
    pub async fn find_username_providers(&self, username: &str) -> Result<Vec<libp2p::PeerId>> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(NodeCommand::FindUsernameProviders {
                key: username_dht_key(username),
                response_tx,
            })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })?;
        response_rx
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to receive response from node".to_string(),
            })?
    }

    /// Request group message history from a peer since a given timestamp.
    ///
    /// The peer will respond with any group messages they have stored after
    /// `since_timestamp`. Results arrive as `GroupSyncEvent::SyncReceived` events.
    pub async fn request_group_sync(
        &self,
        peer_did: String,
        group_id: String,
        since_timestamp: i64,
        limit: u32,
    ) -> Result<()> {
        let (response_tx, response_rx) = oneshot::channel();

        self.command_tx
            .send(NodeCommand::RequestGroupSync {
                peer_did,
                group_id,
                since_timestamp,
                limit,
                response_tx,
            })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })?;

        response_rx
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to receive response from node".to_string(),
            })?
    }

    /// Replace the advertised MLS KeyPackage after the previous one was consumed.
    ///
    /// Call this after accepting a group invitation so the identity handler
    /// advertises a fresh KeyPackage and future invites succeed.
    pub async fn update_mls_key_package(&self, key_package: Vec<u8>) -> Result<()> {
        self.command_tx
            .send(NodeCommand::UpdateMlsKeyPackage { key_package })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })
    }

    /// Send a read receipt to the original message sender (fire-and-forget).
    ///
    /// Failures are silently dropped — receipt delivery is best-effort.
    pub async fn send_receipt(&self, peer_did: String, receipt: ReadReceipt) -> Result<()> {
        self.command_tx
            .send(NodeCommand::SendReceipt { peer_did, receipt })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })
    }

    /// Respond to an inbound group sync request with messages from storage.
    ///
    /// The app layer calls this after receiving a `GroupSyncEvent::SyncRequested`
    /// event and querying storage for the requested messages.
    pub async fn respond_group_sync(
        &self,
        request_id: libp2p::request_response::InboundRequestId,
        response: GroupSyncResponse,
    ) -> Result<()> {
        self.command_tx
            .send(NodeCommand::RespondGroupSync {
                request_id,
                response,
            })
            .await
            .map_err(|_| crate::error::Error::Protocol {
                message: "Failed to send command to node".to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_node_handle() {
        let (command_tx, _command_rx) = tokio::sync::mpsc::channel(100);
        let handle = NodeHandle::new(command_tx);
        // If this compiles, cloning works
        let _cloned = handle.clone();
    }

    #[test]
    fn username_dht_key_format() {
        let key = username_dht_key("alice#0042");
        // RecordKey doesn't expose its inner bytes directly, but Debug output
        // will contain the key. Verify by round-tripping through the expected format.
        let expected = libp2p::kad::RecordKey::new(&"/variance/username/alice#0042");
        assert_eq!(
            format!("{:?}", key),
            format!("{:?}", expected),
            "username_dht_key should produce /variance/username/<username>"
        );
    }

    #[test]
    fn username_dht_key_different_usernames_produce_different_keys() {
        let k1 = username_dht_key("alice#0001");
        let k2 = username_dht_key("bob#0002");
        assert_ne!(format!("{:?}", k1), format!("{:?}", k2),);
    }

    #[tokio::test]
    async fn send_identity_request_errors_when_receiver_dropped() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let handle = NodeHandle::new(tx);
        drop(rx);

        let request = IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::Did(
                    "did:variance:test".to_string(),
                ),
            ),
            requester_did: None,
            timestamp: 0,
        };

        let err = handle
            .send_identity_request(PeerId::random(), request)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("Failed to send command to node"),
            "Expected channel-closed error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn send_direct_message_errors_when_receiver_dropped() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let handle = NodeHandle::new(tx);
        drop(rx);

        let message = DirectMessage {
            id: "msg-1".to_string(),
            sender_did: "did:variance:alice".to_string(),
            recipient_did: "did:variance:bob".to_string(),
            ciphertext: vec![1, 2, 3],
            ..Default::default()
        };

        let err = handle
            .send_direct_message("did:variance:bob".to_string(), message)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Failed to send command to node"));
    }

    #[tokio::test]
    async fn publish_group_message_errors_when_receiver_dropped() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let handle = NodeHandle::new(tx);
        drop(rx);

        let message = GroupMessage::default();
        let err = handle
            .publish_group_message("topic".to_string(), message)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Failed to send command to node"));
    }

    #[tokio::test]
    async fn subscribe_to_topic_errors_when_receiver_dropped() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let handle = NodeHandle::new(tx);
        drop(rx);

        let err = handle
            .subscribe_to_topic("topic".to_string())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Failed to send command to node"));
    }

    #[tokio::test]
    async fn set_local_identity_errors_when_receiver_dropped() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let handle = NodeHandle::new(tx);
        drop(rx);

        let err = handle
            .set_local_identity(
                "did:variance:me".to_string(),
                vec![1, 2, 3],
                vec![vec![4, 5]],
                None,
                vec![6, 7, 8],
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Failed to send command to node"));
    }

    #[tokio::test]
    async fn resolve_identity_errors_when_receiver_dropped() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let handle = NodeHandle::new(tx);
        drop(rx);

        let err = handle
            .resolve_identity_by_did("did:variance:someone".to_string())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Failed to send command to node"));
    }

    #[tokio::test]
    async fn get_connected_dids_errors_when_receiver_dropped() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let handle = NodeHandle::new(tx);
        drop(rx);

        let err = handle.get_connected_dids().await.unwrap_err();
        assert!(err.to_string().contains("Failed to send command to node"));
    }

    #[tokio::test]
    async fn send_identity_request_errors_when_oneshot_dropped() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<NodeCommand>(1);
        let handle = NodeHandle::new(tx);

        // Spawn a task that receives the command but drops the oneshot sender
        let join = tokio::spawn(async move {
            if let Some(NodeCommand::SendIdentityRequest { response_tx, .. }) = rx.recv().await {
                drop(response_tx);
            }
        });

        let request = IdentityRequest {
            query: Some(
                variance_proto::identity_proto::identity_request::Query::Did(
                    "did:variance:test".to_string(),
                ),
            ),
            requester_did: None,
            timestamp: 0,
        };

        let err = handle
            .send_identity_request(PeerId::random(), request)
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("Failed to receive response from node"),
            "Expected oneshot-closed error, got: {}",
            err
        );
        join.await.unwrap();
    }

    #[tokio::test]
    async fn fire_and_forget_commands_send_successfully() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<NodeCommand>(10);
        let handle = NodeHandle::new(tx);

        // These are fire-and-forget: they succeed if the command was enqueued
        handle
            .send_typing_indicator("did:variance:bob".to_string(), TypingIndicator::default())
            .await
            .unwrap();

        handle
            .broadcast_username_change(
                "did:variance:me".to_string(),
                "alice".to_string(),
                42,
                vec![0u8; 32],
            )
            .await
            .unwrap();

        handle
            .update_one_time_keys(vec![vec![1, 2, 3]])
            .await
            .unwrap();

        handle
            .update_mls_key_package(vec![10, 20, 30])
            .await
            .unwrap();

        // Verify all four commands arrived
        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 4);
    }
}
