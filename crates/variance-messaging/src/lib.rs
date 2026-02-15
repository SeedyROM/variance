//! Messaging system for direct and group chats
//!
//! This crate implements:
//! - Direct messages (Double Ratchet encryption)
//! - Group messages (GossipSub + symmetric encryption)
//! - Offline message handling via relay nodes
//! - Read receipts and typing indicators
//! - Message persistence and caching

pub mod direct;
pub mod error;
pub mod group;
pub mod offline;
pub mod receipts;
pub mod storage;
pub mod typing;

pub use error::{Error, Result};
