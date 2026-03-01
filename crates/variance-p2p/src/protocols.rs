//! Custom libp2p protocols for Variance
//!
//! Protocol codecs and behaviours for custom request-response protocols

pub mod identity;
pub mod media;
pub mod messaging;

/// DHT provider key for relay node auto-discovery.
///
/// Both the relay binary and the client node use this key. Any change here
/// MUST be mirrored in `crates/variance-relay/src/main.rs`.
pub const RELAY_PROVIDER_KEY: &[u8] = b"/variance/relay/v1";
