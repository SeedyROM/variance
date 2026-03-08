//! Decentralized identity system using W3C DIDs
//!
//! This crate implements:
//! - DID document creation and management
//! - IPFS/IPNS integration for identity storage
//! - Custom libp2p protocol for identity resolution
//! - Multi-layer caching
//! - Username registration and discovery

pub mod cache;
pub mod config;
pub mod did;
pub mod error;
pub mod mailbox;
pub mod protocol;
pub mod storage;
pub mod username;

pub use error::{Error, Result};
pub use mailbox::mailbox_token;
