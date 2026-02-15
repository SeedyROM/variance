//! Core P2P networking functionality using libp2p
//!
//! This crate provides the foundation for all peer-to-peer communication,
//! including DHT, GossipSub, custom protocols, and connection management.

pub mod behaviour;
pub mod config;
pub mod error;
pub mod node;
pub mod protocols;

pub use error::{Error, Result};
pub use node::Node;
