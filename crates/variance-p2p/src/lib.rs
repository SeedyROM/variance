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
    EventChannels, GroupSyncEvent, IdentityEvent, OfflineMessageEvent, P2pEvent, RenameEvent,
    SignalingEvent, TypingEvent,
};
pub use node::Node;
