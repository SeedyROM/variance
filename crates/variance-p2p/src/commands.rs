//! Command types for controlling the P2P node
//!
//! The Node runs in its own task and processes commands sent via channels.
//! This allows the application layer to send messages without directly
//! accessing the Swarm (which is not Send/Sync).

use libp2p::PeerId;
use tokio::sync::oneshot;
use variance_proto::identity_proto::{IdentityRequest, IdentityResponse};
use variance_proto::media_proto::SignalingMessage;
use variance_proto::messaging_proto::{DirectMessage, GroupMessage};

use crate::error::Result;

/// Commands that can be sent to the P2P node
#[derive(Debug)]
pub enum NodeCommand {
    /// Send an identity request to a peer
    SendIdentityRequest {
        peer: PeerId,
        request: IdentityRequest,
        response_tx: oneshot::Sender<Result<IdentityResponse>>,
    },

    /// Send a WebRTC signaling message to a peer (by DID)
    SendSignalingMessage {
        peer_did: String,
        message: SignalingMessage,
        response_tx: oneshot::Sender<Result<()>>,
    },

    /// Publish a group message to a GossipSub topic
    PublishGroupMessage {
        topic: String,
        message: GroupMessage,
        response_tx: oneshot::Sender<Result<()>>,
    },

    /// Subscribe to a GossipSub topic (for group messaging)
    SubscribeToTopic {
        topic: String,
        response_tx: oneshot::Sender<Result<()>>,
    },

    /// Unsubscribe from a GossipSub topic
    UnsubscribeFromTopic {
        topic: String,
        response_tx: oneshot::Sender<Result<()>>,
    },

    /// Publish a DHT provider record announcing this node serves the given username key.
    /// Key should be constructed with `username_dht_key(username)`.
    ProvideUsername {
        key: libp2p::kad::RecordKey,
        response_tx: oneshot::Sender<Result<()>>,
    },

    /// Find peers that have announced they provide the given username key.
    FindUsernameProviders {
        key: libp2p::kad::RecordKey,
        response_tx: oneshot::Sender<Result<Vec<libp2p::PeerId>>>,
    },

    /// Send an encrypted direct message to a peer (by DID)
    SendDirectMessage {
        peer_did: String,
        message: DirectMessage,
        response_tx: oneshot::Sender<Result<()>>,
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
