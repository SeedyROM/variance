//! WebRTC media handling for audio/video calls
//!
//! This crate implements:
//! - WebRTC signaling via libp2p (Offer/Answer/ICE)
//! - STUN/TURN configuration
//! - Call state management and lifecycle

pub mod call;
pub mod config;
pub mod error;
pub mod protocol;
pub mod signaling;

pub use call::{Call, CallEvent, CallManager};
pub use config::{IceServerConfig, MediaConfig};
pub use error::{Error, Result};
pub use signaling::SignalingHandler;
