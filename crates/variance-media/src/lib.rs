//! WebRTC media handling for audio/video calls
//!
//! This crate implements:
//! - WebRTC signaling via libp2p
//! - STUN/TURN integration
//! - Call state management

pub mod error;
pub mod signaling;

pub use error::{Error, Result};
