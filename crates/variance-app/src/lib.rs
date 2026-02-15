//! Main application logic and HTTP API
//!
//! This crate provides:
//! - HTTP API for Tauri frontend communication
//! - Application state management
//! - Event broadcasting
//! - Configuration loading

pub mod api;
pub mod config;
pub mod error;
pub mod state;

pub use error::{Error, Result};
pub use state::AppState;
