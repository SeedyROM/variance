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
pub mod protocols;

pub use commands::{NodeCommand, NodeHandle};
pub use config::{BootstrapPeer, Config};
pub use error::{Error, Result};
pub use events::{EventChannels, IdentityEvent, OfflineMessageEvent, P2pEvent, SignalingEvent};
pub use node::Node;
