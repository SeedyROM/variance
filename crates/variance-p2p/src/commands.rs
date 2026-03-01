//! Command types for controlling the P2P node
//!
//! The Node runs in its own task and processes commands sent via channels.
//! This allows the application layer to send messages without directly
//! accessing the Swarm (which is not Send/Sync).

use libp2p::PeerId;
use tokio::sync::oneshot;
use variance_proto::identity_proto::{IdentityFound, IdentityRequest, IdentityResponse};
use variance_proto::media_proto::SignalingMessage;
use variance_proto::messaging_proto::{DirectMessage, GroupMessage, TypingIndicator};

use crate::error::Result;

/// Re-used type aliases (canonical definitions in `node::mod`).
type IdentityRequestOneshot = oneshot::Sender<Result<IdentityResponse>>;
type BroadcastDidResolveOneshot = oneshot::Sender<Result<IdentityFound>>;
type ProviderQueryOneshot = oneshot::Sender<Result<Vec<PeerId>>>;
/// Simple ack/error response for fire-and-confirm commands.
type CommandResponse = oneshot::Sender<Result<()>>;
type ConnectedDidsResponse = oneshot::Sender<Vec<String>>;

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

    /// Broadcast our username change to all currently-connected peers (fire-and-forget).
    BroadcastUsernameChange {
        did: String,
        username: String,
        discriminator: u32,
    },
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
    pub async fn set_local_identity(
        &self,
        did: String,
        olm_identity_key: Vec<u8>,
        one_time_keys: Vec<Vec<u8>>,
        mls_key_package: Option<Vec<u8>>,
    ) -> Result<()> {
        self.command_tx
            .send(NodeCommand::SetLocalIdentity {
                did,
                olm_identity_key,
                one_time_keys,
                mls_key_package,
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

    /// Broadcast a username change to all currently-connected peers (fire-and-forget).
    ///
    /// Failures are silently dropped — rename notifications are best-effort.
    pub async fn broadcast_username_change(
        &self,
        did: String,
        username: String,
        discriminator: u32,
    ) -> Result<()> {
        self.command_tx
            .send(NodeCommand::BroadcastUsernameChange {
                did,
                username,
                discriminator,
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
}
