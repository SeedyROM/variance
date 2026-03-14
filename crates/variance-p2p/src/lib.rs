//! Core P2P networking functionality using libp2p
//!
//! This crate provides the foundation for all peer-to-peer communication,
//! including DHT, GossipSub, custom protocols, and connection management.

pub mod behaviour;
pub mod commands;
pub mod config;
pub mod error;
pub mod events;
pub mod handlers;
pub mod node;
pub mod peer_store;
pub mod protocols;
pub mod rate_limiter;

pub use commands::{NodeCommand, NodeHandle};
pub use config::{BootstrapPeer, Config};
pub use error::{Error, Result};
pub use events::{
    EventChannels, GroupSyncEvent, IdentityEvent, OfflineMessageEvent, P2pEvent, ReceiptEvent,
    RenameEvent, SignalingEvent, TypingEvent,
};
pub use node::Node;

/// Create a libp2p keypair from raw Ed25519 secret key bytes.
///
/// Pass the same bytes on every startup (e.g. from the identity file's signing key)
/// to give the node a deterministic, stable PeerId across restarts.
/// The bytes are zeroed by the underlying libp2p-identity implementation.
///
/// Returns `None` if the bytes are not a valid 32-byte Ed25519 secret key.
pub fn keypair_from_ed25519(mut bytes: Vec<u8>) -> Option<libp2p::identity::Keypair> {
    let secret = libp2p::identity::ed25519::SecretKey::try_from_bytes(&mut bytes).ok()?;
    Some(libp2p::identity::Keypair::from(
        libp2p::identity::ed25519::Keypair::from(secret),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keypair_from_valid_ed25519_bytes() {
        let secret = libp2p::identity::ed25519::SecretKey::generate();
        let bytes = secret.as_ref().to_vec();

        let keypair =
            keypair_from_ed25519(bytes).expect("valid 32-byte key should produce a keypair");
        let peer_id = keypair.public().to_peer_id();
        assert_ne!(peer_id.to_string(), "");
    }

    #[test]
    fn keypair_deterministic_across_calls() {
        let secret = libp2p::identity::ed25519::SecretKey::generate();
        let bytes = secret.as_ref().to_vec();

        let kp1 = keypair_from_ed25519(bytes.clone()).unwrap();
        let kp2 = keypair_from_ed25519(bytes).unwrap();

        assert_eq!(
            kp1.public().to_peer_id(),
            kp2.public().to_peer_id(),
            "same secret key bytes should produce the same PeerId"
        );
    }

    #[test]
    fn keypair_from_empty_bytes_returns_none() {
        assert!(keypair_from_ed25519(vec![]).is_none());
    }

    #[test]
    fn keypair_from_wrong_length_returns_none() {
        assert!(keypair_from_ed25519(vec![0u8; 16]).is_none());
        assert!(keypair_from_ed25519(vec![0u8; 64]).is_none());
    }
}
